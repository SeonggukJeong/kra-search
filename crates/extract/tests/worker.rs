use extract::runner::Outcome;
use extract::worker::Worker;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

fn fx(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn worker() -> Worker {
    Worker::new(|| Command::new(env!("CARGO_BIN_EXE_extract-worker")))
}

fn is_ok(o: &Outcome) -> bool {
    matches!(o, Outcome::Ok { .. })
}

#[test]
fn worker_handles_many_files_in_one_process() {
    let mut w = worker();
    for _ in 0..3 {
        assert!(is_ok(&w.extract(&fx("schedule.hwp"), Duration::from_secs(30))));
        assert!(is_ok(&w.extract(&fx("office/sample.docx"), Duration::from_secs(30))));
    }
    assert_eq!(w.restarts(), 0);
}

#[test]
fn worker_survives_hang_and_crash() {
    let mut w = worker();
    let t = std::time::Instant::now();
    assert!(matches!(w.extract(Path::new("/x/__hang__"), Duration::from_secs(2)), Outcome::Timeout));
    assert!(t.elapsed() < Duration::from_secs(10), "시간 초과가 바로 처리돼야 함: {:?}", t.elapsed());
    assert!(is_ok(&w.extract(&fx("schedule.hwp"), Duration::from_secs(30))), "멈춘 뒤 다음 파일 정상");
    assert!(matches!(w.extract(Path::new("/x/__crash__"), Duration::from_secs(30)), Outcome::Crashed { .. }));
    assert!(is_ok(&w.extract(&fx("schedule.hwp"), Duration::from_secs(30))), "죽은 뒤 다음 파일 정상");
    assert_eq!(w.restarts(), 2);
}

#[test]
fn parser_stdout_noise_does_not_break_protocol() {
    let mut w = worker();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("__noise__");
    std::fs::write(&p, b"x").unwrap();
    // 확장자가 없어 Unsupported 로 실패하지만, 응답 자체는 제대로 받아야 한다(시간 초과 아님).
    // 시간 제한에는 워커 기동이 포함된다(Windows CI 에서 Defender 가 새 exe 를 몇 초 검사할 수 있음). "시간 초과 아님"을 확인하는 시험이라 넉넉히 둔다.
    assert!(matches!(w.extract(&p, Duration::from_secs(30)), Outcome::Failed { .. }));
    assert_eq!(w.restarts(), 0);
}

#[test]
fn non_utf8_stdout_noise_is_not_a_crash() {
    let mut w = worker();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("__binnoise__");
    std::fs::write(&p, b"x").unwrap();
    // UTF-8 이 아닌 잡음 줄이 섞여도 읽기 스레드가 끝나지 않아야 한다(Crashed 로 오인하면 안 됨).
    // 시간 제한에는 워커 기동이 포함된다(Windows CI 에서 Defender 가 새 exe 를 몇 초 검사할 수 있음). "시간 초과 아님"을 확인하는 시험이라 넉넉히 둔다.
    assert!(matches!(w.extract(&p, Duration::from_secs(30)), Outcome::Failed { .. }));
    assert_eq!(w.restarts(), 0);
    assert!(is_ok(&w.extract(&fx("schedule.hwp"), Duration::from_secs(30))), "같은 워커로 다음 파일 정상");
    assert_eq!(w.restarts(), 0);
}

#[cfg(unix)]
#[test]
fn non_utf8_path_is_unreadable_not_panic() {
    use extract::FailKind;
    use std::os::unix::ffi::OsStrExt;
    let mut w = worker();
    let p = Path::new(std::ffi::OsStr::from_bytes(b"/x/\xff\xfe.hwp"));
    assert!(matches!(w.extract(p, Duration::from_secs(5)), Outcome::Failed { kind: FailKind::Unreadable, .. }));
    assert_eq!(w.restarts(), 0, "워커에 보내지도 않는다");
}

/// 상주 워커는 파일을 처리할수록 메모리가 쌓인다(공개 세트 60~490MB) — 정한 개수마다 새 프로세스로 바꾼다.
/// 재활용은 비정상 종료 재시작(restarts)과 따로 센다.
#[test]
fn worker_is_recycled_after_configured_file_count() {
    let mut w = worker().recycle_after(2, u64::MAX, usize::MAX);
    let mut pids = Vec::new();
    for _ in 0..5 {
        assert!(is_ok(&w.extract(&fx("office/sample.docx"), Duration::from_secs(30))));
        pids.push(w.pid());
    }
    // 2개 처리 뒤 바로 내려놓고(pid 없음), 다음 요청에서 새로 띄운다.
    assert_eq!((pids[0].is_some(), pids[1], pids[2].is_some(), pids[3], pids[4].is_some()), (true, None, true, None, true), "{pids:?}");
    assert_ne!(pids[0], pids[2]);
    assert_ne!(pids[2], pids[4]);
    assert_eq!((w.recycles(), w.restarts()), (2, 0));
}

/// 큰 응답(본문이 큰 문서) 뒤에도 바로 새 프로세스로 — 큰 문서를 읽느라 늘어난 메모리를 돌려준다.
#[test]
fn worker_is_recycled_after_large_response() {
    let mut w = worker().recycle_after(1_000, u64::MAX, 2_000);
    let dir = tempfile::tempdir().unwrap();
    let (big, small) = (dir.path().join("긺.txt"), dir.path().join("짧음.txt"));
    std::fs::write(&big, "가".repeat(1_000)).unwrap(); // 응답 3,000바이트 이상
    std::fs::write(&small, "짧").unwrap();
    assert!(is_ok(&w.extract(&small, Duration::from_secs(30))));
    assert!(w.pid().is_some(), "작은 응답 뒤에는 그대로");
    assert!(is_ok(&w.extract(&big, Duration::from_secs(30))));
    assert_eq!((w.pid(), w.recycles()), (None, 1), "2,000바이트 넘는 응답 뒤 내려놓는다");
    assert!(is_ok(&w.extract(&small, Duration::from_secs(30))));
    assert_eq!((w.recycles(), w.restarts()), (1, 0));
}

/// 큰 입력 파일을 읽은 뒤에도 — 워커 메모리를 늘리는 것은 응답보다 입력 크기(그림 많은 HWPX 등)다.
#[test]
fn worker_is_recycled_after_input_bytes() {
    let docx = fx("office/sample.docx");
    let size = std::fs::metadata(&docx).unwrap().len();
    let mut w = worker().recycle_after(1_000, size * 2, usize::MAX);
    for _ in 0..2 {
        assert!(is_ok(&w.extract(&docx, Duration::from_secs(30))));
        assert!(w.pid().is_some(), "합이 기준 이하면 그대로");
    }
    assert!(is_ok(&w.extract(&docx, Duration::from_secs(30))));
    assert_eq!((w.pid(), w.recycles(), w.restarts()), (None, 1, 0), "합이 기준을 넘으면 내려놓는다");
}
