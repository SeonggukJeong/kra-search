//! 의미 검색 색인 단계(가짜 임베더 — CI 에 모델이 없어도 파이프라인 전체를 돈다).

use app::embedding::{run_embedding, EmbedOptions};
use app::{open_store, reset_store, run_index};
use embed::fake::FakeEmbedder;
use embed::profile::ModelProfile;
use extract::worker::Worker;
use index::meta::VecKind;
use index::tokenize::Analyzers;
use platform::paths::Home;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../extract/tests/fixtures")
}

fn worker() -> Worker {
    Worker::new(|| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kra-search"));
        c.arg("worker");
        c
    })
}

struct T {
    _dirs: (tempfile::TempDir, tempfile::TempDir),
    docs: PathBuf,
    home: Home,
    a: Analyzers,
}

impl T {
    fn new() -> T {
        let (docs, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for (src, dst) in [("office/sample.docx", "운영계획.docx"), ("schedule.hwp", "추진일정.hwp"), ("office/cp949.csv", "예산.csv"), ("office/utf8.txt", "메모.txt")] {
            std::fs::copy(fixtures().join(src), docs.path().join(dst)).unwrap();
        }
        // 데이터 형식 뒷부분(③)이 생기도록 긴 CSV(청크 20개 넘게)
        let mut csv = String::from("번호,사업장,내용\n");
        for i in 0..600 {
            csv += &format!("{i},렛츠런파크 서울,관람대 좌석 {i}번 점검 결과 이상 없음 확인 완료\n");
        }
        std::fs::write(docs.path().join("점검기록.csv"), csv).unwrap();
        T { docs: docs.path().to_path_buf(), home: Home::new(home.path()), a: Analyzers::load().unwrap(), _dirs: (docs, home) }
    }

    fn index(&self) -> app::Store {
        let mut s = open_store(&self.home, &self.a).unwrap();
        run_index(&mut s, &mut worker(), std::slice::from_ref(&self.docs), Duration::from_secs(30), true).unwrap();
        s
    }

    fn embed(&self, s: &mut app::Store, e: &mut FakeEmbedder, fp: &str, opt: EmbedOptions) -> app::embedding::EmbedSummary {
        run_embedding(s, &self.home, e, fp, &ModelProfile::embeddinggemma2(), &opt, &mut |_| {}).unwrap()
    }
}

fn on() -> EmbedOptions {
    EmbedOptions { data_tail: true, stop_after_append: None }
}

#[test]
fn cards_then_body_documents_first() {
    let t = T::new();
    let mut s = t.index();
    let r = t.embed(&mut s, &mut FakeEmbedder::default(), "fp", on());
    assert!(r.cards == 5 && r.cards_empty == 0 && r.chunks > 20 && r.failed == 0, "표뿐인 CSV 도 카드가 있다(DEC-057): {r:?}");
    let c = s.meta.embed_counts(embed::plan::DATA_HEAD).unwrap();
    assert_eq!((c.cards_done, c.main_done, c.tail_done), (c.cards_total, c.main_total, c.tail_total), "{c:?}");
    assert!(c.tail_total > 0, "{c:?}");
    // 첫 청크 슬롯은 문서 형식 것이다(본문 ①이 먼저).
    let first = s.meta.live_slots(VecKind::Chunks).unwrap().into_iter().find(|x| x.0 == 0).unwrap().1;
    let path = &s.meta.files_of(first).unwrap()[0].path;
    assert!(path.ends_with(".docx") || path.ends_with(".hwp"), "{path}");
}

#[test]
fn rename_does_not_reembed() {
    let t = T::new();
    let mut s = t.index();
    t.embed(&mut s, &mut FakeEmbedder::default(), "fp", on());
    drop(s);
    std::fs::rename(t.docs.join("운영계획.docx"), t.docs.join("운영계획(최종).docx")).unwrap();
    let mut s = t.index();
    let mut e = FakeEmbedder::default();
    let r = t.embed(&mut s, &mut e, "fp", on());
    assert_eq!((e.calls, r.cards, r.chunks), (0, 0, 0), "{r:?}");
}

/// Review Focus 4: 파일에 쓴 뒤 meta 기록 전에 꺼져도 다음 실행이 이어서 하고 중복·엇갈림이 없다.
#[test]
fn crash_between_append_and_meta_resumes_cleanly() {
    let t = T::new();
    let mut s = t.index();
    let r = t.embed(&mut s, &mut FakeEmbedder::default(), "fp", EmbedOptions { data_tail: true, stop_after_append: Some(1) });
    assert!(r.stopped);
    drop(s);
    let mut s = open_store(&t.home, &t.a).unwrap();
    t.embed(&mut s, &mut FakeEmbedder::default(), "fp", on());
    for k in [VecKind::Cards, VecKind::Chunks] {
        let live = s.meta.live_slots(k).unwrap();
        let (_, len, _) = s.meta.vector_snapshot(k).unwrap();
        let mut slots: Vec<u64> = live.iter().map(|x| x.0).collect();
        slots.dedup();
        assert_eq!(slots.len(), live.len(), "슬롯 중복 없음");
        assert!(slots.iter().all(|&x| x < len), "모든 슬롯이 기록된 길이 안");
    }
    let c = s.meta.embed_counts(embed::plan::DATA_HEAD).unwrap();
    assert_eq!((c.cards_done, c.main_done, c.tail_done), (c.cards_total, c.main_total, c.tail_total), "{c:?}");
}

/// Review Focus 4(본문 쪽): 청크 커밋에서 파일에 쓴 뒤 meta 기록 전에 꺼져도 같은 위치가 두 번 들어가지 않는다.
#[test]
fn crash_during_body_resumes_cleanly() {
    let t = T::new();
    let mut s = t.index();
    // 1번째 커밋 = 카드(5개), 2번째 = 첫 청크 묶음
    let r = t.embed(&mut s, &mut FakeEmbedder::default(), "fp", EmbedOptions { data_tail: true, stop_after_append: Some(2) });
    assert!(r.stopped && r.cards == 5, "{r:?}");
    drop(s);
    let mut s = open_store(&t.home, &t.a).unwrap();
    let r2 = t.embed(&mut s, &mut FakeEmbedder::default(), "fp", on());
    assert_eq!(r2.cards, 0, "카드는 기록됐으니 다시 하지 않는다");
    let (_, len, live) = s.meta.vector_snapshot(VecKind::Chunks).unwrap();
    assert_eq!(len as usize, live.len(), "기록 안 된 꼬리는 잘리고 새로 쓴다(죽은 슬롯 없음)");
    let c = s.meta.embed_counts(embed::plan::DATA_HEAD).unwrap();
    assert_eq!((c.main_done + c.tail_done) as usize, live.len(), "위치마다 슬롯 하나: {c:?}");
    assert_eq!((c.main_done, c.tail_done), (c.main_total, c.tail_total), "{c:?}");
}

#[test]
fn fingerprint_change_rebuilds_and_data_tail_toggles() {
    let t = T::new();
    let mut s = t.index();
    let r = t.embed(&mut s, &mut FakeEmbedder::default(), "fp1", EmbedOptions { data_tail: false, stop_after_append: None });
    let c = s.meta.embed_counts(embed::plan::DATA_HEAD).unwrap();
    assert!(c.tail_total > 0 && c.tail_done == 0, "끄면 뒷부분은 하지 않는다: {c:?}");
    let r2 = t.embed(&mut s, &mut FakeEmbedder::default(), "fp1", on());
    assert_eq!(r2.cards, 0);
    assert_eq!(r2.chunks as u64, c.tail_total, "켜면 뒷부분만 더 한다");
    let r3 = t.embed(&mut s, &mut FakeEmbedder::default(), "fp2", on());
    assert_eq!(r3.cards, r.cards, "지문이 바뀌면 처음부터");
}

#[test]
fn failed_input_is_recorded_and_not_retried() {
    let t = T::new();
    let mut s = t.index();
    let mut e = FakeEmbedder { fail_on: Some("렛츠런파크".into()), ..Default::default() }; // 운영계획.docx·메모.txt 본문에 있다
    let r = t.embed(&mut s, &mut e, "fp", on());
    assert!(r.failed >= 1, "{r:?}");
    let mut e2 = FakeEmbedder::default();
    let r2 = t.embed(&mut s, &mut e2, "fp", on());
    assert_eq!((r2.cards, r2.chunks), (0, 0), "실패한 단위는 '없음'으로 기록하고 다시 하지 않는다");
}

#[test]
fn reset_store_removes_vectors() {
    let t = T::new();
    let mut s = t.index();
    t.embed(&mut s, &mut FakeEmbedder::default(), "fp", on());
    drop(s);
    assert!(t.home.vectors_dir().exists());
    reset_store(&t.home).unwrap();
    assert!(!t.home.vectors_dir().exists());
}
