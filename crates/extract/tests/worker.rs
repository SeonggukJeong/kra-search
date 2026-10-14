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
    assert!(matches!(w.extract(&p, Duration::from_secs(5)), Outcome::Failed { .. }));
    assert_eq!(w.restarts(), 0);
}

#[test]
fn non_utf8_stdout_noise_is_not_a_crash() {
    let mut w = worker();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("__binnoise__");
    std::fs::write(&p, b"x").unwrap();
    // UTF-8 이 아닌 잡음 줄이 섞여도 읽기 스레드가 끝나지 않아야 한다(Crashed 로 오인하면 안 됨).
    assert!(matches!(w.extract(&p, Duration::from_secs(5)), Outcome::Failed { .. }));
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
