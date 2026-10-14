use app::{flush, open_store, run_index, search};
use extract::worker::Worker;
use index::tokenize::Analyzers;
use platform::paths::Home;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn real_worker() -> Worker {
    Worker::new(|| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kra-search"));
        c.arg("worker");
        c
    })
}

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../extract/tests/fixtures")
}

#[test]
fn resume_after_interrupted_flush_has_no_duplicates() {
    let docs = tempfile::tempdir().unwrap();
    std::fs::copy(fixtures().join("office/sample.docx"), docs.path().join("a.docx")).unwrap();
    std::fs::copy(fixtures().join("office/sample.odt"), docs.path().join("b.odt")).unwrap();
    let home_dir = tempfile::tempdir().unwrap();
    let home = Home::new(home_dir.path());
    let a = Analyzers::load().unwrap();
    let mut store = open_store(&home, &a).unwrap();
    let mut w = real_worker();
    run_index(&mut store, &mut w, &[docs.path().to_path_buf()], Duration::from_secs(30)).unwrap();
    assert_eq!(store.keyword.num_docs(), 2);

    // 키워드 커밋 뒤 meta 갱신 전에 죽은 상황: meta 는 모두 Pending 으로 남아 있다.
    store.meta.mark_all_pending().unwrap();
    drop(store);
    let mut store = open_store(&home, &a).unwrap();
    flush(&mut store).unwrap();
    assert_eq!(store.keyword.num_docs(), 2, "다시 써도 문서가 늘지 않는다");
    let hits = search(&store, "렛츠런파크", 10).unwrap();
    assert_eq!(hits.len(), 1, "{:?}", hits.iter().map(|h| &h.paths).collect::<Vec<_>>());
    assert_eq!(store.meta.counts().unwrap().pending, 0);
}

/// 스펙 §6.9 "1회 재시도": 비정상 종료로 끝난 파일은 바뀌지 않아도 다음 index 에서 한 번 다시 읽는다.
#[test]
fn crashed_file_is_retried_once_on_next_run() {
    let docs = tempfile::tempdir().unwrap();
    std::fs::copy(fixtures().join("office/sample.docx"), docs.path().join("a.docx")).unwrap();
    let home_dir = tempfile::tempdir().unwrap();
    let home = Home::new(home_dir.path());
    let a = Analyzers::load().unwrap();
    let mut store = open_store(&home, &a).unwrap();
    let roots = [docs.path().to_path_buf()];
    // 요청을 받자마자 끝나는 프로세스 = 파서가 죽은 워커 흉내(`--version` 은 출력 후 바로 끝난다).
    let mut broken = Worker::new(|| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kra-search"));
        c.arg("--version");
        c
    });
    let s = run_index(&mut store, &mut broken, &roots, Duration::from_secs(30)).unwrap();
    assert_eq!((s.extracted, s.failed), (1, 1));
    assert!(search(&store, "렛츠런파크", 10).unwrap().is_empty());

    let mut w = real_worker();
    let s = run_index(&mut store, &mut w, &roots, Duration::from_secs(30)).unwrap();
    assert_eq!((s.changed, s.retried, s.failed), (0, 1, 0), "{s:?}");
    assert_eq!(search(&store, "렛츠런파크", 10).unwrap().len(), 1);

    let s = run_index(&mut store, &mut w, &roots, Duration::from_secs(30)).unwrap();
    assert_eq!(s.retried, 0, "재시도는 한 번뿐");
}

/// 스펙 §6.4 "내용당 한 번 추출": 폴더째 옮겨도(FLUSH_EVERY 를 넘는 개수) 하나도 다시 읽지 않는다.
/// 옛 경로를 먼저 지우면 내용이 고아가 되어 중간 flush 가 지워 버린다 — 그 뒤 파일은 모두 다시 추출됐었다.
#[test]
fn moving_many_files_reuses_all_contents() {
    let docs = tempfile::tempdir().unwrap();
    let n = 230;
    std::fs::create_dir(docs.path().join("old")).unwrap();
    for i in 0..n {
        std::fs::write(docs.path().join(format!("old/문서{i}.txt")), format!("이동시험 본문 {i}")).unwrap();
    }
    let home_dir = tempfile::tempdir().unwrap();
    let home = Home::new(home_dir.path());
    let a = Analyzers::load().unwrap();
    let mut store = open_store(&home, &a).unwrap();
    let mut w = real_worker();
    let roots = [docs.path().to_path_buf()];
    let s = run_index(&mut store, &mut w, &roots, Duration::from_secs(30)).unwrap();
    assert_eq!(s.extracted, n);

    std::fs::rename(docs.path().join("old"), docs.path().join("new")).unwrap();
    let s = run_index(&mut store, &mut w, &roots, Duration::from_secs(30)).unwrap();
    assert_eq!((s.changed, s.removed, s.reused, s.extracted), (n, n, n, 0), "{s:?}");
    assert_eq!(store.keyword.num_docs(), n as u64);
    assert_eq!(store.meta.counts().unwrap().contents, n as u64);
}

/// meta 는 새로 만들어졌는데 옛 키워드 색인이 남은 경우(복구 중 tantivy 를 못 지움): 키워드 쪽이 앞서 있어도 새로 만든다
/// — 남겨 두면 옛 문서가 새 내용 id 와 겹쳐 엉뚱한 결과가 나온다.
#[test]
fn keyword_index_ahead_of_meta_is_rebuilt() {
    let docs = tempfile::tempdir().unwrap();
    std::fs::copy(fixtures().join("office/sample.docx"), docs.path().join("a.docx")).unwrap();
    let home_dir = tempfile::tempdir().unwrap();
    let home = Home::new(home_dir.path());
    let a = Analyzers::load().unwrap();
    let mut store = open_store(&home, &a).unwrap();
    run_index(&mut store, &mut real_worker(), &[docs.path().to_path_buf()], Duration::from_secs(30)).unwrap();
    assert_eq!(store.keyword.num_docs(), 1);
    drop(store);
    for f in ["data/meta.db", "data/meta.db-wal", "data/meta.db-shm", "keys/meta.key"] {
        let _ = std::fs::remove_file(home_dir.path().join(f));
    }
    let store = open_store(&home, &a).unwrap();
    assert_eq!(store.keyword.num_docs(), 0, "옛 키워드 색인이 남으면 안 된다");
}

/// 바뀌었지만 해시를 못 한(읽기 권한 없음) 파일은 meta 에 옛 내용 id 로 남는다. 재시도가 그 파일의 새 바이트를
/// 옛 내용(옛 해시)에 넣으면 안 된다 — 기록된 크기·수정 시각과 다르면 재시도하지 않는다.
#[cfg(unix)]
#[test]
fn retry_skips_file_changed_since_recorded() {
    use std::os::unix::fs::PermissionsExt;
    let docs = tempfile::tempdir().unwrap();
    let p = docs.path().join("a.docx");
    std::fs::copy(fixtures().join("office/sample.docx"), &p).unwrap();
    let home_dir = tempfile::tempdir().unwrap();
    let home = Home::new(home_dir.path());
    let a = Analyzers::load().unwrap();
    let mut store = open_store(&home, &a).unwrap();
    let roots = [docs.path().to_path_buf()];
    let mut broken = Worker::new(|| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kra-search"));
        c.arg("--version");
        c
    });
    run_index(&mut store, &mut broken, &roots, Duration::from_secs(30)).unwrap();
    // 내용을 바꾸고(크기가 달라짐) 읽지 못하게 한다.
    std::fs::copy(fixtures().join("office/sample.odt"), &p).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
    let s = run_index(&mut store, &mut real_worker(), &roots, Duration::from_secs(30));
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    let s = s.unwrap();
    assert_eq!((s.changed, s.unreadable, s.retried), (1, 1, 0), "{s:?}");
}
