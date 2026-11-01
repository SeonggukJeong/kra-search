//! 그레고리력 날짜 계산(외부 크레이트 없이). 날짜는 1970-01-01 기준 일수(`Day`), 한국 시각(UTC+9) 기준.

use std::time::{SystemTime, UNIX_EPOCH};

pub type Day = i32;

pub const KST_OFFSET_SECS: i64 = 9 * 3_600;

/// Howard Hinnant 의 days_from_civil.
pub fn from_ymd(y: i32, m: u32, d: u32) -> Day {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let m = m as i32;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Howard Hinnant 의 civil_from_days.
pub fn to_ymd(z: Day) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
    }
}

/// 0 = 월요일 … 6 = 일요일(1970-01-01 은 목요일).
pub fn weekday(d: Day) -> u32 {
    (d + 3).rem_euclid(7) as u32
}

/// (y, m) 에서 k 개월 뒤(음수면 앞).
pub fn add_months(y: i32, m: u32, k: i32) -> (i32, u32) {
    let t = y * 12 + (m as i32 - 1) + k;
    (t.div_euclid(12), (t.rem_euclid(12) + 1) as u32)
}

/// "YYYY-MM-DD" → Day. 없는 날짜면 None.
pub fn parse_ymd(s: &str) -> Option<Day> {
    let mut it = s.trim().split('-');
    let y: i32 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    let d: u32 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
        return None;
    }
    Some(from_ymd(y, m, d))
}

pub fn format_ymd(d: Day) -> String {
    let (y, m, dd) = to_ymd(d);
    format!("{y:04}-{m:02}-{dd:02}")
}

/// 유닉스 시각(나노초) → 한국 날짜.
pub fn day_of_unix_ns(ns: i64) -> Day {
    (ns.div_euclid(1_000_000_000) + KST_OFFSET_SECS).div_euclid(86_400) as Day
}

/// 이 PC 의 오늘(한국 시각). 검색어 해석의 기준 시점 기본값 — 시험·평가는 고정 날짜를 주입한다.
pub fn today() -> Day {
    let ns = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(0);
    day_of_unix_ns(ns)
}

/// 그날 정오(한국 시각)의 유닉스 초 — 평가 문서의 수정 시각을 날짜 경계에서 떨어뜨리려고.
pub fn noon_kst_unix_secs(d: Day) -> i64 {
    d as i64 * 86_400 + 12 * 3_600 - KST_OFFSET_SECS
}

/// 날짜 기간(양 끝 포함).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub start: Day,
    pub end: Day,
}

impl Range {
    pub fn contains(&self, d: Day) -> bool {
        self.start <= d && d <= self.end
    }
    pub fn days(&self) -> i32 {
        self.end - self.start + 1
    }
    pub fn day(d: Day) -> Range {
        Range { start: d, end: d }
    }
    pub fn year(y: i32) -> Range {
        Range { start: from_ymd(y, 1, 1), end: from_ymd(y, 12, 31) }
    }
    pub fn month(y: i32, m: u32) -> Range {
        Range { start: from_ymd(y, m, 1), end: from_ymd(y, m, days_in_month(y, m)) }
    }
    /// (y, m) 부터 n 개월(해를 넘을 수 있음 — 겨울·연말연시).
    pub fn months(y: i32, m: u32, n: i32) -> Range {
        let (ey, em) = add_months(y, m, n - 1);
        Range { start: from_ymd(y, m, 1), end: from_ymd(ey, em, days_in_month(ey, em)) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_roundtrip_and_known_days() {
        assert_eq!(from_ymd(1970, 1, 1), 0);
        assert_eq!(from_ymd(2026, 10, 15), 20_741);
        for d in [-800_000, -1, 0, 1, 11_016, 20_741, 60_000] {
            let (y, m, dd) = to_ymd(d);
            assert_eq!(from_ymd(y, m, dd), d);
        }
        assert_eq!(to_ymd(from_ymd(2028, 2, 29)), (2028, 2, 29));
    }

    #[test]
    fn weekday_monday_is_zero() {
        assert_eq!(weekday(from_ymd(2026, 10, 15)), 3, "2026-10-15 은 목요일");
        assert_eq!(weekday(from_ymd(2026, 10, 12)), 0, "월요일");
    }

    #[test]
    fn months_and_leap_years() {
        assert_eq!(days_in_month(2028, 2), 29);
        assert_eq!(days_in_month(2100, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(add_months(2026, 1, -1), (2025, 12));
        assert_eq!(add_months(2026, 12, 1), (2027, 1));
        assert_eq!(add_months(2026, 10, -22), (2024, 12));
        let r = Range::months(2025, 12, 3);
        assert_eq!((format_ymd(r.start), format_ymd(r.end)), ("2025-12-01".to_string(), "2026-02-28".to_string()));
    }

    #[test]
    fn parse_and_format() {
        assert_eq!(parse_ymd("2026-10-15"), Some(20_741));
        assert_eq!(parse_ymd("2026-02-30"), None);
        assert_eq!(parse_ymd("2026-13-01"), None);
        assert_eq!(format_ymd(20_741), "2026-10-15");
    }

    #[test]
    fn unix_ns_is_read_in_korean_time() {
        // 2026-10-14 15:00:00 UTC = 2026-10-15 00:00 한국 시각
        let ns = (from_ymd(2026, 10, 14) as i64 * 86_400 + 15 * 3_600) * 1_000_000_000;
        assert_eq!(day_of_unix_ns(ns), from_ymd(2026, 10, 15));
        assert_eq!(day_of_unix_ns(ns - 1), from_ymd(2026, 10, 14));
        assert_eq!(day_of_unix_ns(noon_kst_unix_secs(20_741) * 1_000_000_000), 20_741);
    }
}
