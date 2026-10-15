//! 키워드 검색 단독 평가(M1). 같은 질의·정답으로 M0 의미 검색(eval-dev.md)과 비교한다.

use crate::corpus::{EvalDoc, Query};
use crate::metrics::{first_relevant_rank, ndcg_at_k, recall_at_k};
use index::keyword::{KeywordDoc, KeywordIndex};
use index::tokenize::Analyzers;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::time::Instant;

pub fn report(docs: &[EvalDoc], queries: &[Query]) -> String {
    let a = Analyzers::load().expect("형태소 사전");
    let mut kw = KeywordIndex::in_ram(&a).expect("색인");
    let t = Instant::now();
    for (i, d) in docs.iter().enumerate() {
        kw.upsert(&KeywordDoc::new(i as u64, Some(&d.doc), std::slice::from_ref(&d.name))).expect("색인 쓰기");
    }
    kw.commit().expect("커밋");
    let index_secs = t.elapsed().as_secs_f64();

    let mut kinds: BTreeMap<String, (usize, f64, f64)> = BTreeMap::new();
    let mut lat = Vec::new();
    let mut ranks = Vec::new();
    for q in queries {
        let t = Instant::now();
        let hits = kw.search(&q.text, 100).expect("검색");
        lat.push(t.elapsed().as_secs_f64() * 1000.0);
        let ranked: Vec<String> = hits.iter().map(|(id, _)| docs[*id as usize].id.clone()).collect();
        let (n, r) = (ndcg_at_k(&ranked, &q.relevant, 10), recall_at_k(&ranked, &q.relevant, 20));
        for k in [q.kind.as_str(), "전체"] {
            let e = kinds.entry(k.to_string()).or_default();
            e.0 += 1;
            e.1 += n;
            e.2 += r;
        }
        ranks.push((q, first_relevant_rank(&ranked, &q.relevant)));
    }
    lat.sort_by(f64::total_cmp);
    let pct = |p: f64| lat.get(((lat.len().max(1) - 1) as f64 * p).round() as usize).copied().unwrap_or(0.0);

    let mut s = String::new();
    writeln!(s, "# M1 키워드 검색 평가 (자동 생성: `cargo run -p m0eval --release -- --eval eval/public --eval eval/kra --keyword`)\n").unwrap();
    writeln!(s, "- 문서 {}건 · 질의 {}개 · 색인 {:.1}초 · 검색 p50 {:.1}ms / p95 {:.1}ms(이 PC)\n", docs.len(), queries.len(), index_secs, pct(0.5), pct(0.95)).unwrap();
    writeln!(s, "## 1. 질의 유형별\n\n| 유형 | 질의 수 | nDCG@10 | Recall@20 |\n|---|---|---|---|").unwrap();
    for (k, (n, nd, rc)) in &kinds {
        writeln!(s, "| {k} | {n} | {:.4} | {:.4} |", nd / *n as f64, rc / *n as f64).unwrap();
    }
    writeln!(s, "\n## 2. 마사회 질의별 첫 정답 순위 (100위 밖은 -, 10위 밖은 굵게)\n\n| 질의 | 유형 | 검색어 | 순위 |\n|---|---|---|---|").unwrap();
    for (q, r) in ranks.iter().filter(|(q, _)| q.kind.starts_with("kra")) {
        let rank = match r {
            Some(r) if *r > 10 => format!("**{r}**"),
            Some(r) => r.to_string(),
            None => "-".into(),
        };
        writeln!(s, "| {} | {} | {} | {} |", q.qid, q.kind, q.text, rank).unwrap();
    }
    s
}
