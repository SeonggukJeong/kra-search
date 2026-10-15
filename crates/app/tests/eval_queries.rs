//! 폐쇄망 확인 보고서: 지표는 나오고, 질의·파일 이름·경로는 나오지 않는다(INC-010 원칙).

use app::eval::{parse_user_queries, user_report, IndexInfo, IndexStats, NameIndex, PcInfo, RunInfo, UserResult};
use std::path::Path;
use std::process::Command;

#[test]
fn user_query_report_has_metrics_but_no_text_names_or_paths() {
    let d = tempfile::tempdir().unwrap();
    let docs = d.path().join("업무문서");
    std::fs::create_dir_all(&docs).unwrap();
    let fx = Path::new(env!("CARGO_MANIFEST_DIR")).join("../extract/tests/fixtures/office");
    std::fs::copy(fx.join("sample.docx"), docs.join("홍길동_운영계획.docx")).unwrap();
    std::fs::copy(fx.join("utf8.txt"), docs.join("비밀메모.txt")).unwrap();
    let home = d.path().join("home");
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_kra-search"))
            .arg("--home")
            .arg(&home)
            .arg("--model")
            .arg(d.path().join("모델없음.gguf"))
            .args(args)
            .current_dir(d.path())
            .output()
            .unwrap();
        let t = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success(), "{args:?}\n{t}");
        t
    };
    run(&["index", "--root", docs.to_str().unwrap()]);
    let tsv = d.path().join("내질의.tsv");
    std::fs::write(&tsv, "질의\t정답\n홍길동 렛츠런파크 운영\t홍길동_운영계획.docx\n작년 비밀 메모\t비밀메모.txt\n없는 문서 찾기\t없는파일.hwp\n").unwrap();
    // 모델이 없는 환경이라 --keyword-only(시험용)로 평가 경로만 시험한다.
    let out = run(&["eval", "--now", "2026-10-15", "--keyword-only", "--queries", tsv.to_str().unwrap()]);
    let report = std::fs::read_to_string(d.path().join("kra-search-eval-report.txt")).unwrap();
    assert!(report.contains("nDCG@10") && report.contains("Recall@20") && report.contains("p95"), "{report}");
    assert!(report.contains("정답 파일을 색인에서 찾지 못한 질의 1개"), "{report}");
    assert!(report.contains("첫 페이지") && report.contains("검색어 유형 나누기 비교"), "{report}");
    // 질의 글 낱말·정답 확장자·문서 내용 낱말·홈 경로·사용자 이름·컴퓨터 이름은 어디에도 없어야 한다.
    let mut secrets: Vec<String> = ["홍길동", "운영계획", "비밀", "렛츠런파크", "운영", "작년", "메모", "찾기", "없는파일", "업무문서", "내질의", ".docx", ".txt", ".hwp", "사업 개요", "세부 추진", "남은 글"].map(String::from).to_vec();
    secrets.push(d.path().to_str().unwrap().to_string());
    secrets.push(home.to_str().unwrap().to_string());
    for k in ["USER", "USERNAME", "LOGNAME"] {
        if let Ok(v) = std::env::var(k) {
            if v.chars().count() >= 3 {
                secrets.push(v);
            }
        }
    }
    if let Ok(o) = Command::new("hostname").output() {
        let h = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if h.chars().count() >= 3 {
            secrets.push(h);
        }
    }
    let screen: String = out.lines().filter(|l| !l.starts_with("보고서: ")).collect::<Vec<_>>().join("\n");
    for secret in &secrets {
        assert!(!report.contains(secret.as_str()), "보고서에 '{secret}' 가 있으면 안 된다:\n{report}");
        // 화면에는 보고서 저장 경로(`보고서: …`, 이 PC 화면에만)가 나온다 — 그 줄만 뺀다.
        assert!(!screen.contains(secret.as_str()), "화면 출력에도 질의·파일 이름은 없다: {secret}\n{out}");
    }
    assert!(report.contains("프로그램 ") && report.contains("검색 설정 "), "{report}");
}

#[test]
fn model_less_run_without_keyword_only_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let tsv = d.path().join("q.tsv");
    std::fs::write(&tsv, "가\ta.txt\n").unwrap();
    let docs = d.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("a.txt"), "내용").unwrap();
    let base = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_kra-search"))
            .arg("--home")
            .arg(d.path().join("home"))
            .arg("--model")
            .arg(d.path().join("모델없음.gguf"))
            .args(args)
            .current_dir(d.path())
            .output()
            .unwrap()
    };
    assert!(base(&["index", "--root", docs.to_str().unwrap()]).status.success());
    let out = base(&["eval", "--queries", tsv.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("평가에는 의미 검색이 있어야 합니다"));
    assert!(!d.path().join("kra-search-eval-report.txt").exists());
}

#[test]
fn reads_tsv_with_header_bom_and_multiple_answers() {
    let qs = app::eval::read_user_queries("\u{feff}질의\t정답\n가 나 다\ta.hwp, b.pdf\n\n라마\tc.txt\n");
    assert_eq!(qs.len(), 2);
    assert_eq!((qs[0].line, qs[0].answers.clone()), (2, vec!["a.hwp".to_string(), "b.pdf".to_string()]));
    assert_eq!(qs[1].line, 4);
}

fn res(line: usize, has_date: bool, on: Option<usize>, off: Option<usize>, split: Option<usize>) -> UserResult {
    let sc = |r: Option<usize>| (r.map_or(0.0, |x| 1.0 / (1.0 + x as f64)), if r.is_some_and(|x| x <= 20) { 1.0 } else { 0.0 }, r);
    UserResult { line, has_date, found: true, on: sc(on), off: sc(off), split: sc(split), ms: 10.0 * line as f64, ambiguous: false, partial: false }
}

/// 첫 페이지 적중이 가산 켬/끔에서 뒤바뀐 질의 줄(KI-049)과 나누기 비교가 보고서에 나온다.
#[test]
fn report_lists_first_page_flips_and_split_comparison() {
    let rs = vec![
        res(2, true, Some(15), Some(3), Some(15)), // 가산 때문에 밀림
        res(3, true, Some(4), Some(30), Some(1)),  // 가산 덕에 들어옴
        res(4, false, Some(1), Some(1), Some(12)),
        res(5, false, None, None, None),
    ];
    let ix = IndexInfo { files: 4, contents: 4, failed: 0, embed: Default::default(), stats: IndexStats { extracted: Some(4), cards_new: Some(4), chunks_new: Some(100), body_secs: Some(10.0), cards_secs: Some(1.0), keyword_secs: Some(0.3), threads: Some(4), full: Some(true) } };
    let pc = PcInfo { perf: Some(4), eff: Some(4), logical: 8, threads: 4 };
    let r = user_report(&rs, &ix, &pc, &RunInfo { version: "0.0.1".into(), params_id: "abcd1234".into(), skipped_lines: vec![7] }, 1234.0);
    assert!(r.contains("가산 때문에 첫 페이지 밖으로 밀린 질의(줄): 2\n"), "{r}");
    assert!(r.contains("가산 덕에 첫 페이지로 들어온 질의(줄): 3\n"), "{r}");
    assert!(r.contains("날짜 표현 있는 질의 2개") && r.contains("첫 페이지 켬 1/2 · 끔 1/2"), "{r}");
    assert!(r.contains("나누지 않음(제품): nDCG@10") && r.contains("첫 페이지 2/4"), "{r}");
    assert!(r.contains("8글자 이하를 키워드형으로 나눔") && r.contains("첫 페이지 2/4"), "{r}");
    assert!(r.contains("5 · 없음 · - · - · -"), "{r}");
    assert!(r.contains("프로그램 0.0.1 · 검색 설정 abcd1234"), "{r}");
    assert!(r.contains("읽지 못한 질의 파일 줄: 7"), "{r}");
    assert!(r.contains("새로 읽음 4개 · 빠른 의미 검색 4개 · 정밀 의미 검색 100개 · 처음부터 색인 여부: 예"), "{r}");
    assert!(r.contains("정밀 의미 검색(청크) 10.0개/초"), "{r}");
}

fn names() -> NameIndex {
    NameIndex::new([
        ("/d/2025/계획.hwp", 1),
        ("/d/2024/계획.hwp", 2),
        ("/d/2025/복사/계획.hwp", 1),
        ("C:\\work\\붙임1.hwp", 3),
        ("/d/1,2분기 실적.hwp", 4),
        ("/d/1.hwp", 5),
        ("/d/2분기 실적.hwp", 6),
    ])
}

fn ids(v: &[i64]) -> std::collections::HashSet<i64> {
    v.iter().copied().collect()
}

/// (2a) 폴더가 든 정답은 저장 경로의 끝과 맞춘다(두 구분자 모두), 이름만 쓰면 이름으로 맞춘다.
#[test]
fn answer_with_folder_suffix_matches_path_end_with_either_separator() {
    let n = names();
    assert_eq!(n.resolve("계획.hwp"), ids(&[1, 2]));
    assert_eq!(n.resolve("2025/계획.hwp"), ids(&[1]));
    assert_eq!(n.resolve("2024\\계획.hwp"), ids(&[2]));
    assert_eq!(n.resolve("work/붙임1.hwp"), ids(&[3]));
    assert_eq!(n.resolve("붙임1"), ids(&[3]), "확장자 없는 이름");
    assert!(n.resolve("025/계획.hwp").is_empty(), "폴더 이름 중간부터는 맞지 않는다");
}

/// (2b) 같은 이름이 서로 다른 내용을 가리키면 집합이 둘 이상 — 보고서가 줄 번호로 알린다. 같은 내용의 복사본은 하나로 센다.
#[test]
fn same_name_in_two_folders_is_ambiguous_but_copies_are_not() {
    let n = names();
    assert_eq!(n.resolve_field("계획.hwp")[0].len(), 2);
    assert_eq!(n.resolve_field("2025/계획.hwp")[0].len(), 1);
}

/// (3) 맥(NFD)·윈도(NFC) 한글 조합 차이와 대소문자를 넘어 맞춘다.
#[test]
fn answers_match_across_unicode_normalization_and_case() {
    use unicode_normalization::UnicodeNormalization;
    let nfd: String = "붙임1.HWP".nfd().collect();
    assert_ne!(nfd, "붙임1.HWP");
    assert_eq!(names().resolve(&nfd), ids(&[3]));
    let stored_nfd: String = "/d/계획서.hwp".nfd().collect();
    let n = NameIndex::new([(stored_nfd.as_str(), 9)]);
    assert_eq!(n.resolve("계획서.hwp"), ids(&[9]));
}

/// (4) 따옴표·추가 탭 칸·쉼표 든 이름·머리 낱말, 읽지 못한 줄.
#[test]
fn answer_field_parsing() {
    let (qs, skipped) = parse_user_queries("검색어\t정답\n가\t\"a.hwp\"\tb.hwp, c.hwp\n탭 없는 줄\n\n나\t\n\t다.hwp\n");
    assert_eq!(qs.len(), 1);
    assert_eq!(qs[0].answers, vec!["a.hwp", "b.hwp", "c.hwp"]);
    assert_eq!(skipped, vec![3, 5, 6]);
    // 쉼표 든 이름은 칸 전체를 먼저 이름으로 본다 — 색인에 있으면 한 개, 없으면 쉼표로 가른다.
    let (qs, _) = parse_user_queries("가\t\"1,2분기 실적.hwp\"\n나\t1.hwp,2분기 실적.hwp\n");
    let n = names();
    assert_eq!(n.resolve_field(&qs[0].fields[0]), vec![ids(&[4])]);
    assert_eq!(n.resolve_field(&qs[1].fields[0]), vec![ids(&[5]), ids(&[6])]);
    assert_eq!(n.resolve_field("1,없는.hwp"), vec![ids(&[5]), ids(&[])]);
}

#[test]
fn index_stats_parse_from_kv() {
    let st = IndexStats::from_kv(|k| match k {
        "stat_keyword_secs" => Some("0.3".into()),
        "stat_chunks_new" => Some("48000".into()),
        "stat_full" => Some("0".into()),
        _ => None,
    });
    assert_eq!((st.keyword_secs, st.chunks_new, st.full, st.extracted), (Some(0.3), Some(48000), Some(false), None));
}
