//! 검색어 해석(스펙 §8.1): 날짜 표현 → 기간(칩), 검색어 유형. 기준 시점(오늘)은 바깥에서 받는다 — 평가·시험이 해가 바뀌어도 같게.
pub mod calendar;
pub mod dates;
pub mod kind;

pub use calendar::{Day, Range};
pub use dates::{title_years, DateSpan, DateTable, Precision, Relativity};
pub use kind::{classify, classify_with, QueryKind, KEYWORD_MAX_CHARS};

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedQuery {
    pub original: String,
    pub dates: Vec<DateSpan>,
    /// 날짜 표현을 모두 뺀 글로 정한 유형
    pub kind: QueryKind,
}

impl ParsedQuery {
    /// `drop` 이 참인 날짜 표현(바로 붙은 조사 포함)을 뺀 검색어. 빼고 나면 비면 원래 검색어(빈 검색 방지).
    pub fn text_without(&self, drop: impl Fn(&DateSpan) -> bool) -> String {
        let q = &self.original;
        let mut out = String::new();
        let mut at = 0;
        for s in &self.dates {
            if drop(s) {
                out.push_str(&q[at..s.start]);
                out.push(' ');
                at = s.strip_end;
            }
        }
        out.push_str(&q[at..]);
        let t = out.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.is_empty() {
            q.split_whitespace().collect::<Vec<_>>().join(" ")
        } else {
            t
        }
    }

    /// 문턱 `max_chars` 로 다시 정한 유형. 0 이면 나누지 않는다(모두 의미형) — M2 평가가 "나누지 않음"과 견줄 수 있게.
    /// 그 밖에는 `parse` 와 같은 글(날짜 표현을 모두 뺀 글)에 `classify_with`.
    pub fn kind_with(&self, max_chars: usize) -> QueryKind {
        if max_chars == 0 {
            QueryKind::Semantic
        } else {
            classify_with(&self.text_without(|_| true), max_chars)
        }
    }

    /// 날짜 칩(가산할 기간). 미래 표현은 칩이 아니다.
    pub fn chips(&self) -> impl Iterator<Item = &DateSpan> {
        self.dates.iter().filter(|d| d.range.is_some())
    }
}

pub fn parse(q: &str, today: Day, table: &DateTable) -> ParsedQuery {
    let dates = table.find(q, today);
    let mut pq = ParsedQuery { original: q.to_string(), dates, kind: QueryKind::Keyword };
    pq.kind = classify(&pq.text_without(|_| true));
    pq
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::parse_ymd;

    fn p(q: &str) -> ParsedQuery {
        parse(q, parse_ymd("2026-10-15").unwrap(), &DateTable::builtin())
    }

    #[test]
    fn kind_is_decided_without_date_words() {
        let q = p("작년 더비온");
        assert_eq!(q.kind, QueryKind::Keyword, "날짜를 빼면 '더비온' 한 낱말");
        assert_eq!(q.text_without(|_| true), "더비온");
        assert_eq!(q.text_without(|_| false), "작년 더비온");
    }

    #[test]
    fn particles_go_with_the_date_and_spaces_collapse() {
        assert_eq!(p("작년에 만든  출장 계획").text_without(|_| true), "만든 출장 계획");
    }

    /// Review Focus 2: 날짜만 있는 검색어는 빼고 나면 비므로 원래 검색어를 쓴다.
    #[test]
    fn date_only_query_keeps_text() {
        let q = p("작년");
        assert_eq!(q.text_without(|_| true), "작년");
        assert_eq!(q.chips().count(), 1);
    }

    #[test]
    fn future_words_are_not_chips_and_stay_in_text() {
        let q = p("내년 사업계획");
        assert_eq!(q.chips().count(), 0);
        assert_eq!(q.text_without(|s| s.range.is_some()), "내년 사업계획");
    }

    /// 유형 나누기는 평가의 훑기 축이다 — 0 이면 나누지 않음(모두 의미형), 그 밖에는 `parse` 와 같은 글로 `classify_with`.
    #[test]
    fn kind_with_sweeps_the_threshold_on_the_same_text() {
        let q = p("작년 국정감사 지적사항");
        assert_eq!(q.kind_with(0), QueryKind::Semantic, "0 = 나누지 않음");
        assert_eq!(q.kind_with(6), QueryKind::Semantic);
        assert_eq!(q.kind_with(8), QueryKind::Keyword, "날짜를 뺀 '국정감사 지적사항' 8자");
        assert_eq!(q.kind_with(KEYWORD_MAX_CHARS), q.kind, "기본 문턱은 parse 와 같다");
        assert_eq!(p("더비온").kind_with(0), QueryKind::Semantic);
        assert_eq!(p("더비온").kind_with(10), QueryKind::Keyword);
    }
}
