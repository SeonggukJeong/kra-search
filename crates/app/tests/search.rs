//! 하이브리드 검색(가짜 임베더). 실제 모델 품질은 m0eval·eval 이 잰다.

use app::embedding::{run_embedding, EmbedOptions};
use app::search::Searcher;
use app::{open_store, run_index};
use embed::fake::FakeEmbedder;
use embed::profile::ModelProfile;
use index::hybrid::Combine;
use extract::worker::Worker;
use index::tokenize::Analyzers;
use platform::paths::Home;
use query::calendar::{noon_kst_unix_secs, parse_ymd};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../extract/tests/fixtures")
}

fn set_date(p: &Path, ymd: &str) {
    let secs = noon_kst_unix_secs(parse_ymd(ymd).unwrap());
    let f = std::fs::File::options().write(true).open(p).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::from_secs(secs as u64)).unwrap();
}

fn setup(names_dates: &[(&str, &str, &str)]) -> (tempfile::TempDir, tempfile::TempDir, Home, app::Store) {
    setup_with(|docs| {
        for (src, dst, date) in names_dates {
            let p = docs.join(dst);
            std::fs::copy(fixtures().join(src), &p).unwrap();
            set_date(&p, date);
        }
    })
}

/// `fill` 이 문서 폴더를 채운 뒤 색인(키워드 + 가짜 임베더 의미 검색 색인)까지 한다.
fn setup_with(fill: impl FnOnce(&Path)) -> (tempfile::TempDir, tempfile::TempDir, Home, app::Store) {
    let (docs, home_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    fill(docs.path());
    let home = Home::new(home_dir.path());
    let a = Analyzers::load().unwrap();
    let mut store = open_store(&home, &a).unwrap();
    let mut w = Worker::new(|| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kra-search"));
        c.arg("worker");
        c
    });
    run_index(&mut store, &mut w, &[docs.path().to_path_buf()], Duration::from_secs(30), true).unwrap();
    run_embedding(&mut store, &home, &mut FakeEmbedder::default(), "fp", &ModelProfile::embeddinggemma2(), &EmbedOptions { data_tail: true, stop_after_append: None }, &mut |_| {}).unwrap();
    (docs, home_dir, home, store)
}

fn today() -> query::Day {
    parse_ymd("2026-10-15").unwrap()
}

#[test]
fn semantic_paths_take_part_when_vectors_match() {
    let (_d, _h, home, store) = setup(&[("office/sample.docx", "운영계획.docx", "2026-09-01"), ("schedule.hwp", "추진일정.hwp", "2026-09-02")]);
    let mut s = Searcher::with_embedder(&store, &home, Some(Box::new(FakeEmbedder::default())), Some("fp")).unwrap();
    assert!(s.notice.is_none(), "{:?}", s.notice);
    // 기본값(Task 16 이 정함)에 기대지 않는다: 하한 없이 청크 우선 결합.
    s.params.floor = 0.0;
    s.params.combine = Combine::ChunkElseCard;
    let (_, hits) = s.search(&store, "렛츠런파크 운영", today(), 10).unwrap();
    assert!(!hits.is_empty());
    assert!(hits.iter().any(|h| h.why.card.is_some() || h.why.chunk.is_some()), "의미 경로가 결합에 들어간다");
}

/// Review Focus 3: 색인 뒤 모델이 바뀌면 의미 검색을 쓰지 않고 알린다(키워드 결과는 그대로).
#[test]
fn model_change_disables_semantic_until_reindex() {
    let (_d, _h, home, store) = setup(&[("office/sample.docx", "운영계획.docx", "2026-09-01")]);
    let mut s = Searcher::with_embedder(&store, &home, Some(Box::new(FakeEmbedder::default())), Some("다른모델")).unwrap();
    assert!(s.notice.as_deref().unwrap().contains("index"), "{:?}", s.notice);
    let (_, hits) = s.search(&store, "렛츠런파크", today(), 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].why.card.is_none() && hits[0].why.chunk.is_none());
}

#[test]
fn date_chip_moves_last_years_copy_up() {
    let (_d, _h, home, store) = setup(&[("office/sample.docx", "운영계획 올해.docx", "2026-09-01"), ("office/utf8.txt", "운영계획 작년.txt", "2025-05-01")]);
    let mut s = Searcher::with_embedder(&store, &home, None, None).unwrap();
    // 기본값(Task 16 이 정함)에 기대지 않는다
    s.params.date_exact = 1.0;
    s.params.date_title_conflict = 0.0;
    s.params.recency = 0.0;
    let (pq, hits) = s.search(&store, "작년 렛츠런파크", today(), 10).unwrap();
    assert_eq!(pq.chips().count(), 1);
    assert!(hits[0].hit.paths[0].contains("작년"), "{:?}", hits.iter().map(|h| &h.hit.paths).collect::<Vec<_>>());
    assert!(hits[0].why.date_boost > 0.0);
}

/// 사용자 요구(2026-10-15): 2026년 가을에 "작년 경마시행계획" 은 2025년 계획이다.
/// 2026년 계획 파일은 2025-12-21 에 고쳐 써서 파일 날짜만 보면 "작년" 칩 안이다 — 제목 연도(2026)가 이를 이겨야 한다.
/// 2025년 계획 파일은 날짜(2025-03-01)도 칩 안이다. 본문이 거의 같아 키워드 점수는 비슷하므로(키워드 전용 검색),
/// date_exact 를 크게 잡아 날짜 가산이 순위를 가르게 한다.
#[test]
fn last_year_query_prefers_title_year_over_file_date() {
    let (_d, _h, home, store) = setup_with(|dir| {
        for (name, extra, date) in [
            ("2025년 경마시행계획.txt", "다음 해 사업 목표를 정리했습니다.", "2025-03-01"),
            ("2026년 경마시행계획.txt", "다음 해 사업 일정을 정리했습니다.", "2025-12-21"),
        ] {
            let p = dir.join(name);
            std::fs::write(&p, format!("한국마사회 경마시행계획 문서입니다. {extra}")).unwrap();
            set_date(&p, date);
        }
    });
    let mut s = Searcher::with_embedder(&store, &home, None, None).unwrap();
    s.params.date_exact = 5.0;
    s.params.date_vague = 2.5;
    s.params.date_title_conflict = 0.0;
    let (pq, hits) = s.search(&store, "작년 경마시행계획", today(), 10).unwrap();
    assert_eq!(pq.chips().count(), 1);
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert!(hits[0].hit.paths[0].contains("2025년"), "{:?}", hits.iter().map(|h| &h.hit.paths).collect::<Vec<_>>());
    assert!(hits[0].why.date_boost > hits[1].why.date_boost);
}
