//! 외부 순위 파일 채점(`--compare 이름=순위.json`) — 다른 검색 프로그램이나 다른 설정의 질의별 순위를
//! 제품 평가와 같은 지표·같은 조정용/검증용 분할(DEC-051)로 나란히 놓는다. 순위 JSON 은 `{qid: [문서 id…]}`
//! (`kra-search eval --dump` 와 같은 꼴). 지표 함수는 `app::evalset` 하나를 쓴다.

use crate::corpus::Query;
use app::evalset::{first_relevant_rank, ndcg_at_k, recall_at_k, tune_split, FIRST_PAGE};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

pub type Ranks = BTreeMap<String, Vec<String>>;

/// 한 실행의 질의별 (nDCG@10, Recall@20, 첫 정답 순위). 순위 파일에 없는 질의는 결과 없음으로 센다.
fn per_query(ranks: &Ranks, q: &Query) -> (f64, f64, Option<usize>) {
    let empty = Vec::new();
    let r = ranks.get(&q.qid).unwrap_or(&empty);
    (ndcg_at_k(r, &q.relevant, 10), recall_at_k(r, &q.relevant, 20), first_relevant_rank(r, &q.relevant))
}

fn mean(v: impl Iterator<Item = f64>) -> f64 {
    let (s, n) = v.fold((0.0, 0usize), |(s, n), x| (s + x, n + 1));
    if n == 0 {
        0.0
    } else {
        s / n as f64
    }
}

/// 유형별 평균 nDCG 의 평균(매크로, DEC-051)
fn macro_ndcg(ranks: &Ranks, qs: &[&Query]) -> f64 {
    let mut by: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for q in qs {
        by.entry(q.kind.as_str()).or_default().push(per_query(ranks, q).0);
    }
    mean(by.values().map(|v| mean(v.iter().copied())))
}

fn fp_hits(ranks: &Ranks, qs: &[&Query]) -> usize {
    qs.iter().filter(|q| per_query(ranks, q).2.is_some_and(|r| r <= FIRST_PAGE)).count()
}

/// 실행들을 나란히 놓은 마크다운. `runs` 순서대로 열·행을 만든다.
pub fn report(queries: &[Query], runs: &[(String, Ranks)]) -> String {
    let tune_ids = tune_split(queries.iter().map(|q| (q.qid.as_str(), q.kind.as_str())));
    let all: Vec<&Query> = queries.iter().collect();
    let (tune, hold): (Vec<&Query>, Vec<&Query>) = all.iter().copied().partition(|q| tune_ids.contains(&q.qid));
    let kinds: BTreeSet<&str> = queries.iter().map(|q| q.kind.as_str()).collect();

    let mut s = String::new();
    writeln!(s, "## 전체 지표\n").unwrap();
    writeln!(s, "질의 {}개(조정용 {} · 검증용 {}). 매크로 = 유형별 평균의 평균(DEC-051). 첫 페이지 = 정답이 하나라도 상위 {FIRST_PAGE} 안.\n", all.len(), tune.len(), hold.len()).unwrap();
    writeln!(s, "| 실행 | 매크로 nDCG@10 (조정 · 검증 · 전체) | 질의 평균 nDCG@10 | 첫 페이지(전체) | 첫 페이지(검증) | Recall@20 | 결과 없음 |").unwrap();
    writeln!(s, "|---|---|---|---|---|---|---|").unwrap();
    for (name, r) in runs {
        let empty = all.iter().filter(|q| r.get(&q.qid).is_none_or(|v| v.is_empty())).count();
        writeln!(
            s,
            "| {name} | {:.4} · {:.4} · {:.4} | {:.4} | {}/{} ({:.1}%) | {}/{} ({:.1}%) | {:.4} | {empty} |",
            macro_ndcg(r, &tune),
            macro_ndcg(r, &hold),
            macro_ndcg(r, &all),
            mean(all.iter().map(|q| per_query(r, q).0)),
            fp_hits(r, &all),
            all.len(),
            100.0 * fp_hits(r, &all) as f64 / all.len().max(1) as f64,
            fp_hits(r, &hold),
            hold.len(),
            100.0 * fp_hits(r, &hold) as f64 / hold.len().max(1) as f64,
            mean(all.iter().map(|q| per_query(r, q).1)),
        )
        .unwrap();
    }

    writeln!(s, "\n## 유형별 nDCG@10 · 첫 페이지(전체 질의)\n").unwrap();
    write!(s, "| 유형 | 질의 수 |").unwrap();
    for (name, _) in runs {
        write!(s, " {name} |").unwrap();
    }
    write!(s, "\n|---|---|").unwrap();
    for _ in runs {
        write!(s, "---|").unwrap();
    }
    writeln!(s).unwrap();
    for k in &kinds {
        let qs: Vec<&Query> = all.iter().copied().filter(|q| q.kind == *k).collect();
        write!(s, "| {k} | {} |", qs.len()).unwrap();
        for (_, r) in runs {
            write!(s, " {:.4} · {}/{} |", mean(qs.iter().map(|q| per_query(r, q).0)), fp_hits(r, &qs), qs.len()).unwrap();
        }
        writeln!(s).unwrap();
    }

    writeln!(s, "\n## 질의별 첫 정답 순위\n").unwrap();
    writeln!(s, "`-` = 결과 안에 정답 없음. 순위 파일의 결과 수(실행마다 다름) 안에서 센다.\n").unwrap();
    write!(s, "| qid | 유형 |").unwrap();
    for (name, _) in runs {
        write!(s, " {name} |").unwrap();
    }
    write!(s, "\n|---|---|").unwrap();
    for _ in runs {
        write!(s, "---|").unwrap();
    }
    writeln!(s).unwrap();
    for q in &all {
        write!(s, "| {} | {} |", q.qid, q.kind).unwrap();
        for (_, r) in runs {
            match per_query(r, q).2 {
                Some(n) => write!(s, " {n} |").unwrap(),
                None => write!(s, " - |").unwrap(),
            }
        }
        writeln!(s).unwrap();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn q(qid: &str, kind: &str, rel: &[&str]) -> Query {
        Query { qid: qid.into(), text: String::new(), kind: kind.into(), relevant: rel.iter().map(|s| s.to_string()).collect::<HashSet<_>>() }
    }

    #[test]
    fn missing_query_counts_as_empty() {
        let qs = vec![q("A1", "a", &["x"])];
        let mut r = Ranks::new();
        assert_eq!(per_query(&r, &qs[0]), (0.0, 0.0, None));
        r.insert("A1".into(), vec!["y".into(), "x".into()]);
        assert_eq!(per_query(&r, &qs[0]).2, Some(2));
    }

    #[test]
    fn macro_is_mean_of_kind_means() {
        let qs = [q("A1", "a", &["x"]), q("A2", "a", &["x"]), q("B1", "b", &["x"])];
        let r: Ranks = [("A1", vec!["x"]), ("A2", vec!["z"]), ("B1", vec!["x"])].into_iter().map(|(k, v)| (k.to_string(), v.into_iter().map(String::from).collect())).collect();
        let refs: Vec<&Query> = qs.iter().collect();
        assert!((macro_ndcg(&r, &refs) - 0.75).abs() < 1e-9);
        assert_eq!(fp_hits(&r, &refs), 2);
    }
}
