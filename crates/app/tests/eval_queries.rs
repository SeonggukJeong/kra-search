//! 폐쇄망 확인 보고서: 지표는 나오고, 질의·파일 이름·경로는 나오지 않는다(INC-010 원칙).

use app::eval::{user_report, IndexInfo, PcInfo, UserResult};
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
    for secret in ["홍길동", "운영계획", "비밀", "렛츠런파크", "없는파일", "업무문서", "내질의", d.path().to_str().unwrap()] {
        assert!(!report.contains(secret), "보고서에 '{secret}' 가 있으면 안 된다:\n{report}");
        // 화면에는 보고서 저장 경로(`보고서: …`, 이 PC 화면에만)가 나온다 — 그 줄만 뺀다.
        let screen: String = out.lines().filter(|l| !l.starts_with("보고서: ")).collect::<Vec<_>>().join("\n");
        assert!(!screen.contains(secret), "화면 출력에도 질의·파일 이름은 없다: {secret}\n{out}");
    }
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
    UserResult { line, has_date, found: true, on: sc(on), off: sc(off), split: sc(split), ms: 10.0 * line as f64 }
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
    let ix = IndexInfo { files: 4, contents: 4, failed: 0, embed: Default::default(), stats: vec![] };
    let pc = PcInfo { perf: Some(4), eff: Some(4), logical: 8, threads: 4 };
    let r = user_report(&rs, &ix, &pc, 1234.0);
    assert!(r.contains("가산 때문에 첫 페이지 밖으로 밀린 질의(줄): 2\n"), "{r}");
    assert!(r.contains("가산 덕에 첫 페이지로 들어온 질의(줄): 3\n"), "{r}");
    assert!(r.contains("날짜 표현 있는 질의 2개") && r.contains("첫 페이지 켬 1/2 · 끔 1/2"), "{r}");
    assert!(r.contains("나누지 않음(제품): nDCG@10") && r.contains("첫 페이지 2/4"), "{r}");
    assert!(r.contains("8글자 이하를 키워드형으로 나눔") && r.contains("첫 페이지 2/4"), "{r}");
    assert!(r.contains("5 · 없음 · - · - · -"), "{r}");
}
