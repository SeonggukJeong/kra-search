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
