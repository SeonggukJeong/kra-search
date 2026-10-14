//! 검색 품질 지표(이진 정답).

use std::collections::HashSet;

pub fn ndcg_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    let dcg: f64 = ranked
        .iter()
        .take(k)
        .enumerate()
        .filter(|(_, d)| relevant.contains(*d))
        .map(|(i, _)| 1.0 / ((i + 2) as f64).log2())
        .sum();
    let idcg: f64 = (0..relevant.len().min(k)).map(|i| 1.0 / ((i + 2) as f64).log2()).sum();
    if idcg == 0.0 { 0.0 } else { dcg / idcg }
}

pub fn recall_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    if relevant.is_empty() {
        return 0.0;
    }
    ranked.iter().take(k).filter(|d| relevant.contains(*d)).count() as f64 / relevant.len() as f64
}

/// 첫 정답 문서의 순위(1부터). 정답이 순위 목록에 없으면 None.
pub fn first_relevant_rank(ranked: &[String], relevant: &HashSet<String>) -> Option<usize> {
    ranked.iter().position(|d| relevant.contains(d)).map(|i| i + 1)
}

/// 한 섹션(예: 우리 기관 문서)만 남겼을 때 첫 정답 순위. 출처별 섹션 화면을 흉내 낸다.
pub fn section_rank(ranked: &[String], relevant: &HashSet<String>, in_section: impl Fn(&str) -> bool) -> Option<usize> {
    ranked.iter().filter(|d| in_section(d)).position(|d| relevant.contains(d)).map(|i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }
    fn set(v: &[&str]) -> std::collections::HashSet<String> {
        v.iter().map(|x| x.to_string()).collect()
    }
    #[test]
    fn ndcg_single_relevant_at_rank2() {
        let n = ndcg_at_k(&s(&["a", "b", "c"]), &set(&["b"]), 10);
        assert!((n - 1.0 / 3f64.log2()).abs() < 1e-9);
    }
    #[test]
    fn ndcg_perfect_is_one() {
        assert!((ndcg_at_k(&s(&["a", "b"]), &set(&["a", "b"]), 10) - 1.0).abs() < 1e-9);
    }
    #[test]
    fn ndcg_none_is_zero() {
        assert_eq!(ndcg_at_k(&s(&["x"]), &set(&["a"]), 10), 0.0);
    }
    #[test]
    fn first_relevant_rank_is_one_based() {
        assert_eq!(first_relevant_rank(&s(&["x", "a", "b"]), &set(&["b", "a"])), Some(2));
        assert_eq!(first_relevant_rank(&s(&["x"]), &set(&["a"])), None);
    }
    #[test]
    fn section_rank_ignores_other_sections() {
        let ranked = s(&["P1", "P2", "K1", "P3", "K2"]);
        assert_eq!(section_rank(&ranked, &set(&["K2"]), |d| d.starts_with('K')), Some(2));
        assert_eq!(section_rank(&ranked, &set(&["P3"]), |d| d.starts_with('K')), None);
    }
    #[test]
    fn recall_counts_within_k() {
        assert_eq!(recall_at_k(&s(&["a", "x", "b"]), &set(&["a", "b"]), 2), 0.5);
    }
}
