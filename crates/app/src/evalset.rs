//! 평가 세트 공용(DEC-051·052): 평가 문서 복사본의 파일 이름, 조정용/검증용 분할, 지표.
//! m0eval(실험)과 `kra-search eval`(제품)이 같은 함수를 쓴다 — 파일 이름이 다르면 제목 없는 문서의 카드 머리와
//! 키워드 파일명 필드가 달라져 일치 검사가 깨진다.

use std::collections::{HashMap, HashSet};
use std::path::Path;

/// `"{제목} ({id}).{확장자}"` — Windows 금지 문자·제어 문자는 공백, 공백은 하나로, 제목은 80자까지.
pub fn file_name(id: &str, title: &str, ext: &str) -> String {
    let t: String = title.chars().map(|c| if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { ' ' } else { c }).collect();
    let t: String = t.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect();
    format!("{} ({id}).{ext}", t.trim_end_matches(['.', ' ']))
}

/// 복사본 파일 이름(또는 경로)에서 평가 문서 id 를 꺼낸다.
pub fn id_of(file_name: &str) -> Option<String> {
    let stem = Path::new(file_name).file_stem()?.to_str()?;
    let open = stem.rfind(" (")?;
    let id = stem[open + 2..].strip_suffix(')')?;
    (!id.is_empty()).then(|| id.to_string())
}

/// FNV-1a 64 — 러스트 버전이 바뀌어도 같은 값.
fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// 조정용 qid 집합: 유형마다 qid 해시 순서로 앞 60%(올림). 나머지는 검증용(DEC-051).
pub fn tune_split<'a>(items: impl IntoIterator<Item = (&'a str, &'a str)>) -> HashSet<String> {
    let mut by: HashMap<&str, Vec<&str>> = HashMap::new();
    for (q, k) in items {
        by.entry(k).or_default().push(q);
    }
    let mut out = HashSet::new();
    for (_, mut v) in by {
        v.sort_by_key(|q| (fnv(q), *q));
        let n = (v.len() * 6).div_ceil(10);
        out.extend(v.into_iter().take(n).map(String::from));
    }
    out
}

pub fn ndcg_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    let dcg: f64 = ranked.iter().take(k).enumerate().filter(|(_, d)| relevant.contains(*d)).map(|(i, _)| 1.0 / ((i + 2) as f64).log2()).sum();
    let idcg: f64 = (0..relevant.len().min(k)).map(|i| 1.0 / ((i + 2) as f64).log2()).sum();
    if idcg == 0.0 {
        0.0
    } else {
        dcg / idcg
    }
}

pub fn recall_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    if relevant.is_empty() {
        return 0.0;
    }
    ranked.iter().take(k).filter(|d| relevant.contains(*d)).count() as f64 / relevant.len() as f64
}

/// 첫 정답 문서의 순위(1부터).
pub fn first_relevant_rank(ranked: &[String], relevant: &HashSet<String>) -> Option<usize> {
    ranked.iter().position(|d| relevant.contains(d)).map(|i| i + 1)
}

/// 첫 페이지(화면 한 쪽)에 보이는 결과 수.
pub const FIRST_PAGE: usize = 10;

/// 첫 페이지 적중: 정답 문서가 하나라도 상위 10 안에 있다(사용자 원칙 2026-10-15: "항상 최상위가 아니어도 첫 페이지에 정확한 결과를").
pub fn first_page_hit(ranked: &[String], relevant: &HashSet<String>) -> bool {
    first_relevant_rank(ranked, relevant).is_some_and(|r| r <= FIRST_PAGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_safe_and_carry_the_id() {
        assert_eq!(file_name("P0001", "근로복지공단, 장애학생 방학 중  교육·돌봄", "hwpx"), "근로복지공단, 장애학생 방학 중 교육·돌봄 (P0001).hwpx");
        assert_eq!(file_name("K0001", "「규정」 a/b:c*?\"<>|", "hwp"), "「규정」 a b c (K0001).hwp");
        assert_eq!(id_of("근로복지공단 (P0001).hwpx").as_deref(), Some("P0001"));
        assert_eq!(id_of("/x/y/2025년 계획(안) (K0011).hwp").as_deref(), Some("K0011"));
        assert_eq!(id_of("그냥 파일.hwp"), None);
        let long = "가".repeat(200);
        assert_eq!(file_name("P1", &long, "hwp").chars().count(), 80 + " (P1).hwp".chars().count());
    }

    #[test]
    fn tune_split_is_stable_and_stratified() {
        let items: Vec<(String, String)> = (0..10).map(|i| (format!("Q{i:03}"), "a".to_string())).chain((0..5).map(|i| (format!("R{i:03}"), "b".to_string()))).collect();
        let s1 = tune_split(items.iter().map(|(q, k)| (q.as_str(), k.as_str())));
        let s2 = tune_split(items.iter().rev().map(|(q, k)| (q.as_str(), k.as_str())));
        assert_eq!(s1, s2, "입력 순서와 무관");
        assert_eq!(s1.iter().filter(|q| q.starts_with('Q')).count(), 6);
        assert_eq!(s1.iter().filter(|q| q.starts_with('R')).count(), 3, "5 × 0.6 = 3");
    }

    #[test]
    fn metrics() {
        let r: Vec<String> = ["a", "b", "c"].iter().map(|x| x.to_string()).collect();
        let rel: HashSet<String> = ["b".to_string()].into_iter().collect();
        assert!((ndcg_at_k(&r, &rel, 10) - 1.0 / 3f64.log2()).abs() < 1e-9);
        assert_eq!(first_relevant_rank(&r, &rel), Some(2));
        assert_eq!(recall_at_k(&r, &rel, 1), 0.0);
        assert!(first_page_hit(&r, &rel));
        let far: Vec<String> = (0..10).map(|i| format!("x{i}")).chain(["b".to_string()]).collect();
        assert_eq!(first_relevant_rank(&far, &rel), Some(11));
        assert!(!first_page_hit(&far, &rel), "11위는 첫 페이지 밖");
        assert!(first_page_hit(&far[1..], &rel), "10위는 첫 페이지");
    }
}
