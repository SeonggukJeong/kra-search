//! 검색어 해석(스펙 §8.1): 날짜 표현 → 기간(칩), 검색어 유형. 기준 시점(오늘)은 바깥에서 받는다 — 평가·시험이 해가 바뀌어도 같게.
pub mod calendar;
pub mod dates;

pub use calendar::{Day, Range};
pub use dates::{title_years, DateSpan, DateTable, Precision, Relativity};
