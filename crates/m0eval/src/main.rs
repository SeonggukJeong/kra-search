//! M0 개발 PC 평가 실행기. 결과는 마크다운 표로 저장한다.

mod cache;
mod corpus;
mod experiments;
mod metrics;

use clap::Parser;
use std::fmt::Write;
use std::path::PathBuf;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "models")]
    models: PathBuf,
    #[arg(long, default_value = "eval/public")]
    eval: PathBuf,
    #[arg(long, default_value = "docs/m0/eval-dev.md")]
    out: PathBuf,
    #[arg(long, default_value_t = 8)]
    threads: usize,
}

fn main() {
    let a = Args::parse();
    let (docs, failed) = corpus::load_docs(&a.eval);
    let queries = corpus::load_queries(&a.eval);
    eprintln!("문서 {} (실패 {}), 질의 {}", docs.len(), failed.len(), queries.len());
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
    writeln!(s, "- 말뭉치: `eval/public` 문서 {}건(추출 실패 {}건: {}), 질의 {}개", docs.len(), failed.len(),
        failed.iter().map(|f| format!("{} {}", f.0, f.1)).collect::<Vec<_>>().join(", "), queries.len()).unwrap();
    writeln!(s, "- 실행: {} · 스레드 {}\n", std::env::consts::ARCH, a.threads).unwrap();
    writeln!(s, "## 1. 검색 품질 (의미 검색 단독, nDCG@10 내림차순)\n").unwrap();
    writeln!(s, "{}", experiments::quality_table(&runs, &queries)).unwrap();
    if let Some(q8) = runs.iter().find(|r| r.name.contains("Q8")) {
        writeln!(s, "## 2. 이진 후보 선별 재현율 ({})\n\n{}", q8.name, experiments::binary_recall_table(q8)).unwrap();
        writeln!(s, "## 3. 토큰 통계 ({})\n\n{}", q8.name, experiments::token_table(q8)).unwrap();
    }
    writeln!(s, "## 4. 처리 속도 (이 PC, 참고용)\n\n| 모델 | 초당 토큰 |\n|---|---|").unwrap();
    for r in &runs {
        writeln!(s, "| {} | {:.0} |", r.name, r.tokens_per_sec).unwrap();
    }
    std::fs::write(&a.out, s).unwrap();
    eprintln!("저장: {}", a.out.display());
}
