//! `kra-search eval`: 평가 세트를 제품 경로 그대로(--set, DEC-052) 또는 사용자 질의로(--queries, 폐쇄망 확인 — Task 18).

use crate::evalset::{first_page_hit, id_of, ndcg_at_k, recall_at_k, tune_split, EvalQuery};
use crate::search::Searcher;
use crate::store::Store;
use query::Day;
use std::collections::BTreeMap;
use std::fmt::Write;

/// 질의별 평가 문서 id 순위(상위 100). 경로 파일 이름의 "(id)" 로 문서를 알아본다.
pub fn rank_set(store: &Store, s: &mut Searcher, qs: &[EvalQuery], today: Day) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut out = BTreeMap::new();
    for q in qs {
        let (_, hits) = s.search(store, &q.text, today, 100)?;
        let mut ids: Vec<String> = Vec::new();
        for h in hits {
            for p in &h.hit.paths {
                if let Some(id) = id_of(p) {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
        }
        out.insert(q.qid.clone(), ids);
    }
    Ok(out)
}

pub fn set_report(qs: &[EvalQuery], ranks: &BTreeMap<String, Vec<String>>) -> String {
    let tune = tune_split(qs.iter().map(|q| (q.qid.as_str(), q.kind.as_str())));
    let mut by: BTreeMap<&str, Vec<(bool, f64, f64)>> = BTreeMap::new();
    for q in qs {
        let r = &ranks[&q.qid];
        by.entry(q.kind.as_str()).or_default().push((tune.contains(&q.qid), ndcg_at_k(r, &q.relevant, 10), recall_at_k(r, &q.relevant, 20)));
    }
    let mean = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
    let mut s = String::new();
    writeln!(s, "# kra-search eval --set (제품 경로)\n\n| 유형 | 질의 수 | nDCG@10 | Recall@20 |\n|---|---|---|---|").unwrap();
    let (mut mt, mut mh) = (Vec::new(), Vec::new());
    for (k, v) in &by {
        let n: Vec<f64> = v.iter().map(|x| x.1).collect();
        let r: Vec<f64> = v.iter().map(|x| x.2).collect();
        writeln!(s, "| {k} | {} | {:.4} | {:.4} |", v.len(), mean(&n), mean(&r)).unwrap();
        mt.push(mean(&v.iter().filter(|x| x.0).map(|x| x.1).collect::<Vec<_>>()));
        mh.push(mean(&v.iter().filter(|x| !x.0).map(|x| x.1).collect::<Vec<_>>()));
    }
    writeln!(s, "\n- 매크로 nDCG@10: 조정용 {:.4} · 검증용 {:.4}", mean(&mt), mean(&mh)).unwrap();
    // 일치 검사(docs/m2/parity.md)의 "nDCG@10" 은 질의 평균이라 같은 값을 함께 적는다.
    let all: Vec<f64> = by.values().flatten().map(|x| x.1).collect();
    let fp = qs.iter().filter(|q| first_page_hit(&ranks[&q.qid], &q.relevant)).count();
    writeln!(s, "- 질의 평균 nDCG@10: {:.4} ({}개) · 첫 페이지 적중 {fp}/{}", mean(&all), all.len(), qs.len()).unwrap();
    s
}
