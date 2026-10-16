//! M0 개발 PC 평가 실행기. 결과는 마크다운 표로 저장한다.

mod cache;
mod corpus;
mod experiments;
mod hybrid;
mod keyword;
mod metrics;

use clap::Parser;
use std::fmt::Write;
use std::path::PathBuf;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "models")]
    models: PathBuf,
    /// 평가 세트 폴더(여러 번 지정 가능: `--eval eval/public --eval eval/kra`). 문서와 질의를 합친다.
    #[arg(long, default_values = ["eval/public"])]
    eval: Vec<PathBuf>,
    /// 저장 위치. 없으면 --keyword 는 docs/m1/eval-keyword.md, 그 밖에는 docs/m0/eval-dev.md
    #[arg(long)]
    out: Option<PathBuf>,
    /// 키워드 검색만 평가한다(모델 불필요).
    #[arg(long)]
    keyword: bool,
    #[arg(long, default_value_t = 8)]
    threads: usize,
    /// M2 하이브리드 평가(docs/m2/eval-hybrid.md · docs/m2/params.json)
    #[arg(long)]
    hybrid: bool,
    /// M2 실험 모델
    #[arg(long, default_value = "models/embeddinggemma-2-Q8_0.gguf")]
    model: PathBuf,
    /// 기준 시점(날짜 표현 해석)
    #[arg(long, default_value = "2026-10-15")]
    now: String,
    /// 매개변수를 고르지 않고 이 JSON 을 쓴다
    #[arg(long)]
    params: Option<PathBuf>,
    /// 질의별 순위 JSON 저장
    #[arg(long)]
    dump: Option<PathBuf>,
    /// 앱 `kra-search eval --dump` 결과와 비교(일치 검사, DEC-059)
    #[arg(long)]
    parity: Option<PathBuf>,
    /// 일치 검사가 제품 저장 벡터를 읽을 평가 색인 폴더(`kra-search --home`)
    #[arg(long, default_value = "target/eval-home")]
    eval_home: PathBuf,
}

fn main() {
    let a = Args::parse();
    let (mut docs, mut failed, mut queries) = (Vec::new(), Vec::new(), Vec::new());
    for dir in &a.eval {
        let (d, f) = corpus::load_docs(dir);
        docs.extend(d);
        failed.extend(f);
        queries.extend(corpus::load_queries(dir));
    }
    eprintln!("문서 {} (실패 {}), 질의 {}", docs.len(), failed.len(), queries.len());
    for f in &failed {
        eprintln!("추출 실패: {} {}", f.id, f.why);
    }
    if a.keyword {
        let out = a.out.clone().unwrap_or_else(|| PathBuf::from("docs/m1/eval-keyword.md"));
        std::fs::write(&out, keyword::report(&docs, &queries)).unwrap();
        eprintln!("저장: {}", out.display());
        return;
    }
    if a.hybrid || a.parity.is_some() {
        let now = query::calendar::parse_ymd(&a.now).expect("--now YYYY-MM-DD");
        // 어떤 파일보다 먼저 만든다(일치 검사 경로 포함, P17).
        std::fs::create_dir_all("docs/m2").unwrap();
        let w = hybrid::build(&docs, &failed, &queries, &hybrid::HArgs { model: a.model.clone(), threads: a.threads, now });
        if let Some(app) = &a.parity {
            let (report, pass) = hybrid::parity(&w, &queries, &hybrid::ParityArgs { app_dump: app, home: &a.eval_home, model: &a.model, threads: a.threads });
            std::fs::write("docs/m2/parity.md", &report).unwrap();
            eprintln!("{report}");
            std::process::exit(if pass { 0 } else { 1 });
        }
        // params.json = 매개변수 + card_tokens(고른 카드 상한). card_tokens 가 있으면 다시 정하지 않는다.
        let fixed: Option<hybrid::Saved> = a.params.as_ref().map(|p| serde_json::from_str(&std::fs::read_to_string(p).unwrap()).expect("params JSON"));
        let o = hybrid::run(&w, &queries, fixed);
        let out = a.out.clone().unwrap_or_else(|| PathBuf::from("docs/m2/eval-hybrid.md"));
        std::fs::write(&out, &o.report).unwrap();
        std::fs::write("docs/m2/params.json", serde_json::to_string_pretty(&hybrid::Saved { params: o.params.clone(), card_tokens: Some(o.cap) }).unwrap() + "\n").unwrap();
        if let Some(d) = &a.dump {
            std::fs::write(d, serde_json::to_string(&o.ranks).unwrap()).unwrap();
        }
        eprintln!("저장: {} · 카드 상한 결정 {}", out.display(), o.cap);
        return;
    }
    let mut models: Vec<PathBuf> = std::fs::read_dir(&a.models)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "gguf"))
        .collect();
    models.sort();
    let runs: Vec<_> = models
        .iter()
        .map(|m| {
            eprintln!("임베딩: {}", m.display());
            // 청크 크기 비교는 Q8에서만(시간 절약). Q4는 400만.
            let sizes: &[usize] = if m.to_string_lossy().contains("Q8") { &[300, 400, 500] } else { &[400] };
            experiments::run_model(m, &docs, &queries, a.threads, sizes)
        })
        .collect();

    let mut s = String::new();
    writeln!(s, "# M0 개발 PC 평가 결과 (자동 생성: `cargo run -p m0eval --release`)\n").unwrap();
    let sets = a.eval.iter().map(|p| format!("`{}`", p.display())).collect::<Vec<_>>().join(" + ");
    writeln!(s, "- 말뭉치: {sets} 문서 {}건(추출 실패 {}건: {}), 질의 {}개", docs.len(), failed.len(),
        failed.iter().map(|f| format!("{} {}", f.id, f.why)).collect::<Vec<_>>().join(", "), queries.len()).unwrap();
    writeln!(s, "- 실행: {} · 스레드 {}\n", std::env::consts::ARCH, a.threads).unwrap();
    writeln!(s, "## 1. 검색 품질 (의미 검색 단독, nDCG@10 내림차순)\n").unwrap();
    writeln!(s, "{}", experiments::quality_table(&runs, &queries)).unwrap();
    if let Some(q8) = runs.iter().find(|r| r.name.contains("Q8")) {
        writeln!(s, "## 2. 이진 후보 선별 재현율 ({})\n\n{}", q8.name, experiments::binary_recall_table(q8)).unwrap();
        writeln!(s, "## 3. 토큰 통계 ({})\n\n{}", q8.name, experiments::token_table(q8)).unwrap();
    }
    writeln!(s, "## 4. 처리 속도 (이 PC, 참고용 — 본문 청크(400) 192개 표본, 예열 후 캐시 없이 매번 측정)\n\n| 모델 | 초당 토큰 |\n|---|---|").unwrap();
    for r in &runs {
        writeln!(s, "| {} | {:.0} |", r.name, r.tokens_per_sec).unwrap();
    }
    if queries.iter().any(|q| q.kind.starts_with("kra")) {
        writeln!(s, "\n## 5. 마사회 질의별 첫 정답 순위 (512차원 · 청크 400 · 청크 점수, 10위 밖은 굵게, 섹션 = 마사회 문서만 남긴 순위)\n\n{}",
            experiments::per_query_table(&runs, &queries, "kra", 512, 400)).unwrap();
        writeln!(s, "## 6. 출처별 섹션으로 나눠 보여 주면 (512차원 · 청크 400 · 청크 점수)\n\n{}",
            experiments::section_table(&runs, &queries, "kra", 512, 400)).unwrap();
    }
    let out = a.out.clone().unwrap_or_else(|| PathBuf::from("docs/m0/eval-dev.md"));
    std::fs::write(&out, s).unwrap();
    eprintln!("저장: {}", out.display());
}
