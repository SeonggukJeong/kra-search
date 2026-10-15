//! 날짜 표현 → 기간(스펙 §8.1, DEC-053). 낱말은 데이터 표(`dates.tsv`, 바깥 파일 덧붙이기 가능), 숫자 꼴은 코드에서 읽는다.
//! 규칙: 연도 없는 달·분기·반기·계절은 "기준 시점에 이미 시작한 가장 최근 것". 미래 표현은 칩이 아니다(range = None).
//! 경계: 표현 앞은 글자가 아니어야 하고(앞 표현에 바로 붙은 경우는 허용), 뒤는 글자가 아니거나 조사·이어지는 시기여야 한다
//! — "연말정산"·"지난 주민설명회"·"올림픽"을 날짜로 읽지 않으려고.

use crate::calendar::{add_months, days_in_month, from_ymd, to_ymd, weekday, Day, Range};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    Exact,
    Vague,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relativity {
    /// 기준 시점에 따라 바뀌는 표현(작년·지난달·9월)
    Relative,
    /// 숫자로 쓴 해가 든 표현(2025년·'25.5.30.)
    Absolute,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DateSpan {
    /// 원래 검색어 안 표현의 바이트 위치 [start, end).
    pub start: usize,
    pub end: usize,
    /// 검색어에서 뺄 때의 끝 — 바로 붙은 조사("작년에")까지.
    pub strip_end: usize,
    /// None = 미래 표현(내년·다음 달): 칩도 아니고 검색어에서 빼지도 않는다.
    pub range: Option<Range>,
    pub precision: Precision,
    pub relativity: Relativity,
    /// 연도를 추정한 표현인가 — 해도 상대어도 없이 시기만 쓴 것(연말·9월·3분기·상반기·가을). 매해 되풀이되는 시기라
    /// 칩(range)은 가장 최근 것이지만, 이전 해의 같은 시기도 맞을 수 있다(Task 8 이 약하게 가산). 묶음이면 모든 부분이
    /// 해·상대어를 갖지 않을 때만 true. 미래 표현은 false.
    pub yearless: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Part {
    Early,
    Mid,
    Late,
    Whole,
    Turn,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

impl Season {
    /// 계절이 시작하는 달(겨울은 12월 — 해를 넘는다).
    fn start_month(self) -> u32 {
        match self {
            Season::Spring => 3,
            Season::Summer => 6,
            Season::Autumn => 9,
            Season::Winter => 12,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Word {
    Year(i32),
    YearBound,
    MonthRel(i32),
    WeekRel(i32),
    DayRel(i32),
    QuarterRel(i32),
    Quarter(u32),
    Half(u32),
    YearPart(Part),
    Season(Season),
    MonthPart(Part),
    ThisMonthPart(Part),
    Window(i32, i32),
    Future,
}

struct Entry {
    /// 공백을 뺀 표현
    key: Vec<char>,
    word: Word,
    precision: Precision,
}

pub struct DateTable {
    /// 긴 표현부터(같은 자리에서 가장 긴 것을 고른다)
    entries: Vec<Entry>,
}

const BUILTIN: &str = include_str!("dates.tsv");

/// 표현 바로 뒤에 붙어도 되는 조사(검색어에서 뺄 때 함께 뺀다). 긴 것부터.
const PARTICLES: &[&str] = &[
    "에는", "에도", "에서", "부터", "까지", "동안", "이후", "이전", "에", "의", "도", "는", "은", "엔", "쯤", "경", "간", "중", "께", "분", "내",
];

fn parse_word(kind: &str, v: &str) -> Option<Word> {
    let int = || v.trim().parse::<i32>().ok();
    let part = || match v {
        "early" => Some(Part::Early),
        "mid" => Some(Part::Mid),
        "late" => Some(Part::Late),
        "whole" => Some(Part::Whole),
        "turn" => Some(Part::Turn),
        _ => None,
    };
    Some(match kind {
        "year" => Word::Year(int()?),
        "year_bound" => Word::YearBound,
        "month_rel" => Word::MonthRel(int()?),
        "week_rel" => Word::WeekRel(int()?),
        "day_rel" => Word::DayRel(int()?),
        "quarter_rel" => Word::QuarterRel(int()?),
        "quarter" => match int()? {
            q @ 1..=4 => Word::Quarter(q as u32),
            _ => return None,
        },
        "half" => match int()? {
            h @ 1..=2 => Word::Half(h as u32),
            _ => return None,
        },
        "year_part" => Word::YearPart(part()?),
        "season" => Word::Season(match v {
            "spring" => Season::Spring,
            "summer" => Season::Summer,
            "autumn" => Season::Autumn,
            "winter" => Season::Winter,
            _ => return None,
        }),
        "month_part" => Word::MonthPart(part()?),
        "this_month_part" => Word::ThisMonthPart(part()?),
        "window" => {
            let (a, b) = v.split_once(',')?;
            Word::Window(a.trim().parse().ok()?, b.trim().parse().ok()?)
        }
        "future" => Word::Future,
        _ => return None,
    })
}

/// 뒤에 붙어 하나의 기간을 이루는 낱말(앞 표현의 경계 검사에서 허용).
fn composable(w: Word) -> bool {
    matches!(w, Word::Half(_) | Word::Quarter(_) | Word::YearPart(_) | Word::Season(_) | Word::MonthPart(_))
}

impl DateTable {
    pub fn builtin() -> DateTable {
        let mut t = DateTable { entries: Vec::new() };
        t.extend_tsv(BUILTIN).expect("내장 날짜 표");
        t
    }

    /// 형식: `표현<TAB>종류<TAB>값<TAB>정밀도`. `#` 로 시작하는 줄과 빈 줄은 건너뛴다. 오류는 줄 번호와 함께.
    pub fn extend_tsv(&mut self, text: &str) -> Result<(), String> {
        let mut add = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let c: Vec<&str> = line.split('\t').collect();
            if c.len() != 4 {
                return Err(format!("{}번째 줄: 칸이 4개가 아님(탭으로 구분)", i + 1));
            }
            let word = parse_word(c[1].trim(), c[2].trim())
                .ok_or_else(|| format!("{}번째 줄: 알 수 없는 종류·값 '{}' '{}'", i + 1, c[1], c[2]))?;
            let precision = match c[3].trim() {
                "exact" => Precision::Exact,
                "vague" => Precision::Vague,
                x => return Err(format!("{}번째 줄: 정밀도는 exact 또는 vague('{x}')", i + 1)),
            };
            let key: Vec<char> = c[0].chars().filter(|ch| !ch.is_whitespace()).collect();
            if key.is_empty() {
                return Err(format!("{}번째 줄: 표현이 비었음", i + 1));
            }
            add.push(Entry { key, word, precision });
        }
        self.entries.extend(add);
        self.entries.sort_by_key(|e| std::cmp::Reverse(e.key.len()));
        Ok(())
    }

    /// 검색어 안의 날짜 표현(왼쪽부터, 겹치지 않음).
    pub fn find(&self, q: &str, today: Day) -> Vec<DateSpan> {
        let cs: Vec<(usize, char)> = q.char_indices().collect();
        let mut hits: Vec<Hit> = Vec::new();
        let mut i = 0;
        while i < cs.len() {
            let glued = hits.last().is_some_and(|h| h.j == i);
            let boundary = i == 0 || glued || !cs[i - 1].1.is_alphanumeric();
            if boundary && !cs[i].1.is_whitespace() {
                let prev = hits.last().filter(|h| only_space(&cs, h.j, i)).map(|h| h.atom);
                if let Some(h) = numeric(&cs, i, today).or_else(|| self.word(&cs, i, today, prev)) {
                    if self.after_ok(&cs, h.j) {
                        i = h.j;
                        hits.push(h);
                        continue;
                    }
                }
            }
            i += 1;
        }
        group(q, &cs, &hits, today)
    }

    fn word(&self, cs: &[(usize, char)], i: usize, today: Day, prev: Option<Atom>) -> Option<Hit> {
        for e in &self.entries {
            let Some(j) = match_key(cs, i, &e.key) else { continue };
            let atom = match e.word {
                // "초·말·중순"은 달이나 해 바로 뒤에서만 날짜다("말 복지", "초과" 방지).
                Word::MonthPart(p) => match prev {
                    Some(Atom::Month { .. } | Atom::Year { .. } | Atom::YearBound) => Atom::MonthPart(p),
                    _ => continue,
                },
                w => word_atom(w, today),
            };
            return Some(Hit { i, j, atom, precision: e.precision });
        }
        None
    }

    /// 표현 뒤 경계: 끝·글자 아님·숫자(“2025년3월”)·조사·이어지는 시기 낱말.
    fn after_ok(&self, cs: &[(usize, char)], j: usize) -> bool {
        let Some(&(_, c)) = cs.get(j) else { return true };
        if !c.is_alphanumeric() || c.is_ascii_digit() {
            return true;
        }
        PARTICLES.iter().any(|p| particle_at(cs, j, p))
            || self.entries.iter().any(|e| composable(e.word) && match_key(cs, j, &e.key).is_some())
    }
}

/// 문서 제목 안의 "숫자로 쓴 해"(2025년·2025년도·25년·'25·2025.9·2025년 9월 …)의 목록(오름차순, 중복 없음).
/// 문서 제목의 연도 = 문서가 다루는 해라서 날짜 가산에서 파일 날짜보다 우선한다(사용자 요구 2026-10-15:
/// 2026년에 "작년 경마시행계획"을 물으면 2025년 경마시행계획). 작년·올해 같은 상대어와 달·일만 쓴 표현은 해를 알 수 없어 뺀다.
pub fn title_years(text: &str) -> Vec<i32> {
    // 숫자로 쓴 해는 기준 시점과 무관하게 같은 해로 읽힌다 — 아무 고정 날짜를 쓴다.
    let today = from_ymd(2026, 10, 15);
    let mut ys: Vec<i32> = DateTable::builtin()
        .find(text, today)
        .iter()
        .filter(|s| s.relativity == Relativity::Absolute)
        .filter_map(|s| s.range.map(|r| to_ymd(r.start).0))
        .collect();
    ys.sort_unstable();
    ys.dedup();
    ys
}

/// 조사 p 가 j 에서 시작하고, 그 뒤가 끝이거나 글자가 아니다.
fn particle_at(cs: &[(usize, char)], j: usize, p: &str) -> bool {
    let n = p.chars().count();
    j + n <= cs.len()
        && cs[j..j + n].iter().map(|x| x.1).eq(p.chars())
        && cs.get(j + n).is_none_or(|x| !x.1.is_alphanumeric())
}

/// 공백을 건너뛰며 key 를 맞춘다(첫 글자는 공백이 아니어야). 다음 위치를 돌려준다.
fn match_key(cs: &[(usize, char)], i: usize, key: &[char]) -> Option<usize> {
    let (mut j, mut k) = (i, 0);
    while k < key.len() {
        let c = cs.get(j)?.1;
        if c.is_whitespace() {
            if k == 0 {
                return None;
            }
            j += 1;
            continue;
        }
        if c != key[k] {
            return None;
        }
        j += 1;
        k += 1;
    }
    Some(j)
}

fn only_space(cs: &[(usize, char)], from: usize, to: usize) -> bool {
    from <= to && cs[from..to].iter().all(|c| c.1.is_whitespace())
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Atom {
    Year { y: i32, abs: bool },
    YearBound,
    Month { y: Option<i32>, m: u32, abs: bool },
    MonthDay { y: Option<i32>, m: u32, d: u32, abs: bool },
    Quarter(u32),
    Half(u32),
    YearPart(Part),
    Season(Season),
    MonthPart(Part),
    Range(Range, bool),
    Future,
}

#[derive(Debug, Clone, Copy)]
struct Hit {
    /// 글자 위치 [i, j)
    i: usize,
    j: usize,
    atom: Atom,
    precision: Precision,
}

fn word_atom(w: Word, today: Day) -> Atom {
    let (ty, tm, _) = to_ymd(today);
    match w {
        Word::Year(k) => Atom::Year { y: ty + k, abs: false },
        Word::YearBound => Atom::YearBound,
        Word::MonthRel(k) => {
            let (y, m) = add_months(ty, tm, k);
            Atom::Month { y: Some(y), m, abs: false }
        }
        Word::WeekRel(k) => {
            let monday = today - weekday(today) as i32 + 7 * k;
            Atom::Range(Range { start: monday, end: monday + 6 }, false)
        }
        Word::DayRel(k) => Atom::Range(Range::day(today + k), false),
        Word::QuarterRel(k) => {
            let t = ty * 4 + (tm as i32 - 1) / 3 + k;
            let (y, q) = (t.div_euclid(4), t.rem_euclid(4) as u32 + 1);
            Atom::Range(Range::months(y, 3 * q - 2, 3), false)
        }
        Word::Quarter(q) => Atom::Quarter(q),
        Word::Half(h) => Atom::Half(h),
        Word::YearPart(p) => Atom::YearPart(p),
        Word::Season(s) => Atom::Season(s),
        Word::MonthPart(p) => Atom::MonthPart(p),
        Word::ThisMonthPart(p) => Atom::Range(month_part(ty, tm, p), false),
        Word::Window(from, to) => Atom::Range(Range { start: today - from, end: today - to }, false),
        Word::Future => Atom::Future,
    }
}

// ---- 숫자 꼴 ----

/// i 에서 시작하는 숫자 묶음(1~4자리). 5자리 이상이면 날짜가 아니다(전화번호 등).
fn num(cs: &[(usize, char)], i: usize) -> Option<(u32, usize)> {
    let n = cs[i.min(cs.len())..].iter().take_while(|c| c.1.is_ascii_digit()).count();
    if n == 0 || n > 4 {
        return None;
    }
    Some((cs[i..i + n].iter().fold(0u32, |v, c| v * 10 + c.1.to_digit(10).unwrap()), n))
}

fn skip_sp(cs: &[(usize, char)], mut j: usize) -> usize {
    while cs.get(j).is_some_and(|c| c.1.is_whitespace()) {
        j += 1;
    }
    j
}

fn lit(cs: &[(usize, char)], j: usize, s: &str) -> Option<usize> {
    let n = s.chars().count();
    (j + n <= cs.len() && cs[j..j + n].iter().map(|c| c.1).eq(s.chars())).then_some(j + n)
}

fn lit_sp(cs: &[(usize, char)], j: usize, s: &str) -> Option<usize> {
    lit(cs, skip_sp(cs, j), s)
}

#[derive(Clone, Copy)]
enum Unit {
    Day,
    Week,
    Month,
    Year,
}

/// 개수: 숫자(1~3자리) 또는 한·두·세·네.
fn count_word(cs: &[(usize, char)], i: usize) -> Option<(i32, usize)> {
    if cs.get(i)?.1.is_ascii_digit() {
        let (v, n) = num(cs, i)?;
        return (n <= 3).then_some((v as i32, i + n));
    }
    for (w, v) in [("한", 1), ("두", 2), ("세", 3), ("네", 4)] {
        if let Some(e) = lit(cs, i, w) {
            return Some((v, e));
        }
    }
    None
}

fn unit_at(cs: &[(usize, char)], j: usize) -> Option<(Unit, usize)> {
    for (w, u) in [("주일", Unit::Week), ("개월", Unit::Month), ("일", Unit::Day), ("주", Unit::Week), ("달", Unit::Month), ("년", Unit::Year)] {
        if let Some(e) = lit(cs, j, w) {
            return Some((u, e));
        }
    }
    None
}

fn shift_back(today: Day, n: i32, u: Unit) -> Day {
    let (y, m, d) = to_ymd(today);
    let months = |k: i32| {
        let (yy, mm) = add_months(y, m, -k);
        from_ymd(yy, mm, d.min(days_in_month(yy, mm)))
    };
    match u {
        Unit::Day => today - n,
        Unit::Week => today - 7 * n,
        Unit::Month => months(n),
        Unit::Year => months(12 * n),
    }
}

fn ago(today: Day, n: i32, u: Unit) -> Range {
    let (y, m, _) = to_ymd(today);
    match u {
        Unit::Day => Range::day(today - n),
        Unit::Week => {
            let d = today - 7 * n;
            let monday = d - weekday(d) as i32;
            Range { start: monday, end: monday + 6 }
        }
        Unit::Month => {
            let (yy, mm) = add_months(y, m, -n);
            Range::month(yy, mm)
        }
        Unit::Year => Range::year(y - n),
    }
}

/// 기준 시점 이전에 이미 끝난 가장 최근의 그 계절("지난겨울"). 겨울은 해를 넘으므로 시작한 해를 거슬러 찾는다.
fn last_completed_season(s: Season, today: Day) -> Range {
    let ty = to_ymd(today).0;
    for y in [ty, ty - 1, ty - 2] {
        let r = Range::months(y, s.start_month(), 3);
        if r.end < today {
            return r;
        }
    }
    Range::months(ty - 3, s.start_month(), 3)
}

/// "." 또는 "-" 로 이은 달(.일). 반환: (달, 일?, 끝 위치).
fn dotted_md(cs: &[(usize, char)], j: usize) -> Option<(u32, Option<u32>, usize)> {
    let sep = cs.get(j)?.1;
    if sep != '.' && sep != '-' {
        return None;
    }
    let (m, mn) = num(cs, j + 1)?;
    if mn > 2 || !(1..=12).contains(&m) {
        return None;
    }
    let e = j + 1 + mn;
    if let Some(x) = lit(cs, e, &sep.to_string()) {
        if let Some((d, dn)) = num(cs, x) {
            if dn <= 2 {
                let end = x + dn;
                let end = lit(cs, end, ".").unwrap_or(end);
                return Some((m, Some(d), end));
            }
        }
    }
    Some((m, None, e))
}

fn md_hit(i: usize, j: usize, y: i32, m: u32, d: Option<u32>) -> Hit {
    let atom = match d {
        Some(d) => Atom::MonthDay { y: Some(y), m, d, abs: true },
        None => Atom::Month { y: Some(y), m, abs: true },
    };
    Hit { i, j, atom, precision: Precision::Exact }
}

fn exact(i: usize, j: usize, atom: Atom) -> Hit {
    Hit { i, j, atom, precision: Precision::Exact }
}

fn numeric(cs: &[(usize, char)], i: usize, today: Day) -> Option<Hit> {
    let c = cs[i].1;
    // '25 · '25.5 · '25.5.30.
    if matches!(c, '\'' | '’' | '‘') {
        let (yy, n) = num(cs, i + 1)?;
        if n != 2 {
            return None;
        }
        let y = 2000 + yy as i32;
        let j = i + 1 + n;
        if let Some((m, d, e)) = dotted_md(cs, j) {
            return Some(md_hit(i, e, y, m, d));
        }
        let e = lit_sp(cs, j, "년").map(|x| lit(cs, x, "도").unwrap_or(x)).unwrap_or(j);
        return Some(exact(i, e, Atom::Year { y, abs: true }));
    }
    // 최근/지난 N 단위 → [오늘 - N 단위, 오늘]
    for head in ["최근", "지난"] {
        if let Some(e) = lit(cs, i, head) {
            if let Some((n, e2)) = count_word(cs, skip_sp(cs, e)) {
                if let Some((u, e3)) = unit_at(cs, skip_sp(cs, e2)) {
                    return Some(exact(i, e3, Atom::Range(Range { start: shift_back(today, n, u), end: today }, false)));
                }
            }
        }
    }
    // 지난봄·지난 겨울 … → 이미 끝난 가장 최근의 그 계절(정확)
    if let Some(e) = lit(cs, i, "지난") {
        let x = skip_sp(cs, e);
        for (w, s) in [("봄", Season::Spring), ("여름", Season::Summer), ("가을", Season::Autumn), ("겨울", Season::Winter)] {
            if let Some(e2) = lit(cs, x, w) {
                return Some(exact(i, e2, Atom::Range(last_completed_season(s, today), false)));
            }
        }
    }
    // N 단위 전 → 그 시점이 든 날·주·달·해(막연)
    if let Some((n, e)) = count_word(cs, i) {
        if let Some((u, e2)) = unit_at(cs, skip_sp(cs, e)) {
            if let Some(e3) = lit_sp(cs, e2, "전") {
                return Some(Hit { i, j: e3, atom: Atom::Range(ago(today, n, u), false), precision: Precision::Vague });
            }
        }
    }
    if !c.is_ascii_digit() {
        return None;
    }
    let (v, n) = num(cs, i)?;
    let j = i + n;
    if n == 4 {
        if !(1900..=2099).contains(&v) {
            return None;
        }
        let y = v as i32;
        if let Some((m, d, e)) = dotted_md(cs, j) {
            return Some(md_hit(i, e, y, m, d));
        }
        let e = lit_sp(cs, j, "년").map(|x| lit(cs, x, "도").unwrap_or(x)).unwrap_or(j);
        return Some(exact(i, e, Atom::Year { y, abs: true }));
    }
    if n > 2 {
        return None;
    }
    if n == 2 {
        // 25.9월
        if let Some(x) = lit(cs, j, ".") {
            if let Some((m, mn)) = num(cs, x) {
                if mn <= 2 && (1..=12).contains(&m) {
                    if let Some(e) = lit_sp(cs, x + mn, "월") {
                        return Some(exact(i, e, Atom::Month { y: Some(2000 + v as i32), m, abs: true }));
                    }
                }
            }
        }
        // 25년(도)
        if let Some(x) = lit_sp(cs, j, "년") {
            let e = lit(cs, x, "도").unwrap_or(x);
            return Some(exact(i, e, Atom::Year { y: 2000 + v as i32, abs: true }));
        }
    }
    // 9월 (15일)
    if (1..=12).contains(&v) {
        if let Some(e) = lit_sp(cs, j, "월") {
            let x = skip_sp(cs, e);
            if let Some((d, dn)) = num(cs, x) {
                if dn <= 2 {
                    if let Some(e2) = lit_sp(cs, x + dn, "일") {
                        return Some(exact(i, e2, Atom::MonthDay { y: None, m: v, d, abs: false }));
                    }
                }
            }
            return Some(exact(i, e, Atom::Month { y: None, m: v, abs: false }));
        }
    }
    // 3분기 · 3/4분기 · 3사분기 · 3Q
    if (1..=4).contains(&v) {
        for tail in ["/4분기", "사분기", "분기"] {
            if let Some(e) = lit_sp(cs, j, tail) {
                return Some(exact(i, e, Atom::Quarter(v)));
            }
        }
        if let Some(e) = lit(cs, j, "Q").or_else(|| lit(cs, j, "q")) {
            if cs.get(e).is_none_or(|c| !c.1.is_ascii_alphabetic()) {
                return Some(exact(i, e, Atom::Quarter(v)));
            }
        }
    }
    None
}

// ---- 묶기·해석 ----

/// 앞 표현에 이어 붙을 수 있는가(해 + 시기, 달 + 초·말).
fn can_follow(last: Atom, next: Atom) -> bool {
    matches!(
        (last, next),
        (
            Atom::Year { .. } | Atom::YearBound,
            Atom::Half(_) | Atom::Quarter(_) | Atom::Season(_) | Atom::YearPart(_) | Atom::Month { y: None, .. } | Atom::MonthDay { y: None, .. } | Atom::MonthPart(_),
        ) | (Atom::Month { .. }, Atom::MonthPart(_))
    )
}

fn group(q: &str, cs: &[(usize, char)], hits: &[Hit], today: Day) -> Vec<DateSpan> {
    let byte = |i: usize| cs.get(i).map(|c| c.0).unwrap_or(q.len());
    let mut out = Vec::new();
    let mut k = 0;
    while k < hits.len() {
        let mut n = 1;
        while k + n < hits.len() && only_space(cs, hits[k + n - 1].j, hits[k + n].i) && can_follow(hits[k + n - 1].atom, hits[k + n].atom) {
            n += 1;
        }
        let g = &hits[k..k + n];
        k += n;
        let Some((range, relativity, precision)) = resolve(g, today) else { continue };
        let j = g[n - 1].j;
        let strip = PARTICLES.iter().find(|p| particle_at(cs, j, p)).map(|p| j + p.chars().count()).unwrap_or(j);
        // 연도 추정: 해·상대어(연도 있는 달, 기간)가 하나도 없는 묶음 — 시기만 쓴 되풀이 표현.
        let yearless = range.is_some()
            && g.iter().all(|h| {
                matches!(
                    h.atom,
                    Atom::Month { y: None, .. }
                        | Atom::MonthDay { y: None, .. }
                        | Atom::Quarter(_)
                        | Atom::Half(_)
                        | Atom::YearPart(_)
                        | Atom::Season(_)
                        | Atom::MonthPart(_)
                )
            });
        out.push(DateSpan { start: byte(g[0].i), end: byte(j), strip_end: byte(strip), range, precision, relativity, yearless });
    }
    out
}

/// 연도가 없으면 기준 시점에 이미 시작한 가장 최근 것.
fn anchored(anchor: Option<i32>, today: Day, f: impl Fn(i32) -> Range) -> Range {
    match anchor {
        Some(y) => f(y),
        None => {
            let ty = to_ymd(today).0;
            let r = f(ty);
            if r.start <= today {
                r
            } else {
                f(ty - 1)
            }
        }
    }
}

fn year_part(y: i32, p: Part) -> Range {
    match p {
        Part::Early => Range::months(y, 1, 2),
        Part::Mid => Range::months(y, 5, 4),
        Part::Late => Range::months(y, 11, 2),
        Part::Whole => Range::year(y),
        Part::Turn => Range::months(y, 12, 2),
    }
}

fn month_part(y: i32, m: u32, p: Part) -> Range {
    let last = days_in_month(y, m);
    let (a, b) = match p {
        Part::Early => (1, 10),
        Part::Mid => (11, 20),
        Part::Late => (21, last),
        Part::Whole | Part::Turn => (1, last),
    };
    Range { start: from_ymd(y, m, a), end: from_ymd(y, m, b) }
}

type Resolved = (Option<Range>, Relativity, Precision);

fn resolve(g: &[Hit], today: Day) -> Option<Resolved> {
    let vague = g.iter().any(|h| h.precision == Precision::Vague);
    let (mut abs, mut anchor, mut range, mut month) = (false, None::<i32>, None::<Range>, None::<(i32, u32)>);
    let latest_month = |m: u32| -> (i32, u32) {
        let (ty, tm, _) = to_ymd(today);
        if m <= tm {
            (ty, m)
        } else {
            (ty - 1, m)
        }
    };
    for h in g {
        match h.atom {
            Atom::Future => return Some((None, Relativity::Relative, Precision::Exact)),
            Atom::Year { y, abs: a } => {
                abs |= a;
                anchor = Some(y);
                range = Some(Range::year(y));
            }
            Atom::YearBound => anchor = Some(to_ymd(today).0),
            Atom::Month { y, m, abs: a } => {
                abs |= a;
                let (yy, mm) = match y.or(anchor) {
                    Some(y) => (y, m),
                    None => latest_month(m),
                };
                month = Some((yy, mm));
                range = Some(Range::month(yy, mm));
            }
            Atom::MonthDay { y, m, d, abs: a } => {
                abs |= a;
                let yy = y.or(anchor).unwrap_or_else(|| latest_month(m).0);
                if d == 0 || d > days_in_month(yy, m) {
                    return None;
                }
                range = Some(Range::day(from_ymd(yy, m, d)));
            }
            Atom::Quarter(q) => range = Some(anchored(anchor, today, |y| Range::months(y, 3 * q - 2, 3))),
            Atom::Half(hh) => range = Some(anchored(anchor, today, |y| Range::months(y, 6 * hh - 5, 6))),
            Atom::Season(s) => range = Some(anchored(anchor, today, |y| Range::months(y, s.start_month(), 3))),
            Atom::YearPart(p) => range = Some(anchored(anchor, today, |y| year_part(y, p))),
            Atom::MonthPart(p) => match (month, anchor) {
                (Some((y, m)), _) => range = Some(month_part(y, m, p)),
                (None, Some(y)) => range = Some(year_part(y, p)),
                (None, None) => return None,
            },
            Atom::Range(r, a) => {
                abs |= a;
                range = Some(r);
            }
        }
    }
    let range = range?;
    Some((Some(range), if abs { Relativity::Absolute } else { Relativity::Relative }, if vague { Precision::Vague } else { Precision::Exact }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::{format_ymd, parse_ymd};

    fn day(s: &str) -> Day {
        parse_ymd(s).unwrap()
    }

    /// 검색어 → (표현, 시작, 끝). 표현이 하나라고 가정한다.
    fn one(q: &str, today: &str) -> (String, String, String) {
        let t = DateTable::builtin();
        let v = t.find(q, day(today));
        assert_eq!(v.len(), 1, "{q}: {v:?}");
        let s = &v[0];
        let r = s.range.expect("미래 표현 아님");
        (q[s.start..s.end].to_string(), format_ymd(r.start), format_ymd(r.end))
    }

    fn none(q: &str) {
        let v = DateTable::builtin().find(q, day("2026-10-15"));
        assert!(v.is_empty(), "{q}: {v:?}");
    }

    #[test]
    fn relative_words_on_reference_day() {
        let t = "2026-10-15";
        for (q, expr, a, b) in [
            ("작년 경마시행계획", "작년", "2025-01-01", "2025-12-31"),
            ("지난해 출장 계획", "지난해", "2025-01-01", "2025-12-31"),
            ("전년도 실적", "전년도", "2025-01-01", "2025-12-31"),
            ("재작년 예산", "재작년", "2024-01-01", "2024-12-31"),
            ("금년 사업", "금년", "2026-01-01", "2026-12-31"),
            ("지난달 규정 개정", "지난달", "2026-09-01", "2026-09-30"),
            ("저번 달 회의", "저번 달", "2026-09-01", "2026-09-30"),
            ("이달 행사", "이달", "2026-10-01", "2026-10-31"),
            ("지난주 보도자료", "지난주", "2026-10-05", "2026-10-11"),
            ("이번 주 일정", "이번 주", "2026-10-12", "2026-10-18"),
            ("어제 받은 공문", "어제", "2026-10-14", "2026-10-14"),
            ("지난 분기 실적", "지난 분기", "2026-07-01", "2026-09-30"),
            ("요즘 민원", "요즘", "2026-07-17", "2026-10-15"),
        ] {
            assert_eq!(one(q, t), (expr.to_string(), a.to_string(), b.to_string()), "{q}");
        }
    }

    #[test]
    fn composed_periods() {
        let t = "2026-10-15";
        for (q, expr, a, b) in [
            ("올해 상반기 적극행정 계획", "올해 상반기", "2026-01-01", "2026-06-30"),
            ("올 초 인식조사", "올 초", "2026-01-01", "2026-02-28"),
            ("올봄 행사", "올봄", "2026-03-01", "2026-05-31"),
            ("작년 말 실적", "작년 말", "2025-11-01", "2025-12-31"),
            ("작년 겨울 폭설", "작년 겨울", "2025-12-01", "2026-02-28"),
            ("2025년 9월 회의", "2025년 9월", "2025-09-01", "2025-09-30"),
            ("작년 9월 말 보고", "작년 9월 말", "2025-09-21", "2025-09-30"),
            ("2025년 3분기 매출", "2025년 3분기", "2025-07-01", "2025-09-30"),
            ("지난달 말 공지", "지난달 말", "2026-09-21", "2026-09-30"),
        ] {
            assert_eq!(one(q, t), (expr.to_string(), a.to_string(), b.to_string()), "{q}");
        }
    }

    #[test]
    fn numeric_forms() {
        let t = "2026-10-15";
        for (q, expr, a, b) in [
            ("2025년 사업계획", "2025년", "2025-01-01", "2025-12-31"),
            ("2025년도 예산", "2025년도", "2025-01-01", "2025-12-31"),
            ("2025 사업계획", "2025", "2025-01-01", "2025-12-31"),
            ("25년 결산", "25년", "2025-01-01", "2025-12-31"),
            ("'25 결산", "'25", "2025-01-01", "2025-12-31"),
            ("’25.5.30. 공문", "’25.5.30.", "2025-05-30", "2025-05-30"),
            ("25.9월 회의", "25.9월", "2025-09-01", "2025-09-30"),
            ("2025.9 보고", "2025.9", "2025-09-01", "2025-09-30"),
            ("2025-09-16 공문", "2025-09-16", "2025-09-16", "2025-09-16"),
            ("9월 회의록", "9월", "2026-09-01", "2026-09-30"),
            ("12월 송년회", "12월", "2025-12-01", "2025-12-31"),
            ("9월 15일 공문", "9월 15일", "2026-09-15", "2026-09-15"),
            ("9월 말 보고", "9월 말", "2026-09-21", "2026-09-30"),
            ("10월초 행사", "10월초", "2026-10-01", "2026-10-10"),
            ("3분기 실적", "3분기", "2026-07-01", "2026-09-30"),
            ("4분기 계획", "4분기", "2026-10-01", "2026-12-31"),
            ("3/4분기 실적", "3/4분기", "2026-07-01", "2026-09-30"),
            ("3사분기 실적", "3사분기", "2026-07-01", "2026-09-30"),
            ("삼사분기 실적", "삼사분기", "2026-07-01", "2026-09-30"),
            ("2Q 실적", "2Q", "2026-04-01", "2026-06-30"),
            ("최근 3개월 민원", "최근 3개월", "2026-07-15", "2026-10-15"),
            ("최근 한 달 공지", "최근 한 달", "2026-09-15", "2026-10-15"),
            ("지난 6개월 실적", "지난 6개월", "2026-04-15", "2026-10-15"),
            ("최근 2주 보도", "최근 2주", "2026-10-01", "2026-10-15"),
            ("2년 전 감사", "2년 전", "2024-01-01", "2024-12-31"),
            ("3개월 전 회의", "3개월 전", "2026-07-01", "2026-07-31"),
            ("한 달 전 공지", "한 달 전", "2026-09-01", "2026-09-30"),
        ] {
            assert_eq!(one(q, t), (expr.to_string(), a.to_string(), b.to_string()), "{q}");
        }
    }

    #[test]
    fn boundaries_new_year_leap_and_winter() {
        assert_eq!(one("지난달 회의", "2026-01-03").1, "2025-12-01");
        assert_eq!(one("작년 실적", "2026-01-01").1, "2025-01-01");
        assert_eq!(one("지난주 공지", "2026-10-12"), ("지난주".into(), "2026-10-05".into(), "2026-10-11".into()));
        assert_eq!(one("이번 주 공지", "2026-10-18"), ("이번 주".into(), "2026-10-12".into(), "2026-10-18".into()));
        assert_eq!(one("어제 공문", "2028-03-01").1, "2028-02-29");
        assert_eq!(one("2월 말 보고", "2028-03-10"), ("2월 말".into(), "2028-02-21".into(), "2028-02-29".into()));
        assert_eq!(one("겨울 폭설", "2026-01-10"), ("겨울".into(), "2025-12-01".into(), "2026-02-28".into()));
        assert_eq!(one("겨울 폭설", "2026-12-05"), ("겨울".into(), "2026-12-01".into(), "2027-02-28".into()));
        assert_eq!(one("연말연시 근무", "2026-10-15"), ("연말연시".into(), "2025-12-01".into(), "2026-01-31".into()));
        assert_eq!(one("하반기 계획", "2026-03-02").1, "2025-07-01", "아직 오지 않은 하반기는 작년 것");
    }

    #[test]
    fn future_words_are_spans_without_range() {
        let t = DateTable::builtin();
        for q in ["내년 사업계획", "다음 달 행사", "명년도 예산", "다음 주 회의"] {
            let v = t.find(q, day("2026-10-15"));
            assert_eq!(v.len(), 1, "{q}: {v:?}");
            assert!(v[0].range.is_none(), "{q}");
        }
    }

    /// Review Focus 1: 일상어와 겹치는 낱말은 날짜가 아니다.
    /// (전달·전주·당일·전일 은 표에 넣지 않기로 정했다 — 통제관 결정 2026-10-15.)
    #[test]
    fn ordinary_words_are_not_dates() {
        for q in [
            "전달 사항 정리", "전주 출장 보고", "당일 출장", "전일 근무", "연말정산 서류", "지난 주민설명회 자료",
            "올림픽 지원", "봄철 산불 대책", "가을운동회", "말 복지 규정", "경주마 초과 출전", "이사회 의결",
            "10억 원 예산", "전화번호 010-1234-5678", "1.5배 증가", "지난 봄철 점검", "지난가을운동회",
        ] {
            none(q);
        }
    }

    #[test]
    fn particles_are_stripped_with_expression() {
        let t = DateTable::builtin();
        let q = "작년에 만든 출장 계획";
        let v = t.find(q, day("2026-10-15"));
        assert_eq!(&q[v[0].start..v[0].end], "작년");
        assert_eq!(&q[v[0].start..v[0].strip_end], "작년에");
        let q = "2025년도부터 바뀐 규정";
        let v = t.find(q, day("2026-10-15"));
        assert_eq!(&q[v[0].start..v[0].strip_end], "2025년도부터");
    }

    #[test]
    fn precision_and_relativity() {
        let t = DateTable::builtin();
        let s = &t.find("작년 계획", day("2026-10-15"))[0];
        assert_eq!((s.precision, s.relativity), (Precision::Exact, Relativity::Relative));
        let s = &t.find("2025년 계획", day("2026-10-15"))[0];
        assert_eq!(s.relativity, Relativity::Absolute);
        let s = &t.find("요즘 민원", day("2026-10-15"))[0];
        assert_eq!(s.precision, Precision::Vague);
        let s = &t.find("작년 가을 행사", day("2026-10-15"))[0];
        assert_eq!(s.precision, Precision::Vague, "막연한 낱말이 하나라도 섞이면 막연");
    }

    /// 표의 모든 줄이 실제로 해석된다(맥락이 필요한 year_bound·month_part 는 맥락과 함께).
    #[test]
    fn every_table_row_is_recognized() {
        let t = DateTable::builtin();
        for line in BUILTIN.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            let c: Vec<&str> = line.split('\t').collect();
            let q = match c[1] {
                "year_bound" => format!("{} 초 계획", c[0]),
                "month_part" => format!("9월 {} 보고", c[0]),
                _ => format!("{} 계획", c[0]),
            };
            let v = t.find(&q, day("2026-10-15"));
            assert_eq!(v.len(), 1, "{q}: {v:?}");
            assert_eq!(v[0].range.is_none(), c[1] == "future", "{q}");
        }
    }

    #[test]
    fn external_rows_extend_the_table() {
        let mut t = DateTable::builtin();
        assert!(t.find("올 시즌 경주", day("2026-10-15")).is_empty());
        t.extend_tsv("시즌\tyear\t0\tvague\n").unwrap();
        let v = t.find("시즌 경주", day("2026-10-15"));
        assert_eq!(v.len(), 1);
        assert!(t.extend_tsv("잘못된 줄\n").unwrap_err().contains("1번째 줄"));
        assert!(t.extend_tsv("x\tyear\t하나\texact\n").unwrap_err().contains("알 수 없는"));
    }

    /// "지난 + 계절" = 이미 끝난 가장 최근의 그 계절(통제관 추가).
    #[test]
    fn last_season_is_the_most_recent_completed_one() {
        let t = "2026-10-15";
        for (q, expr, a, b) in [
            ("지난겨울 폭설", "지난겨울", "2025-12-01", "2026-02-28"),
            ("지난 겨울 폭설", "지난 겨울", "2025-12-01", "2026-02-28"),
            ("지난가을 행사", "지난가을", "2025-09-01", "2025-11-30"),
            ("지난 가을 행사", "지난 가을", "2025-09-01", "2025-11-30"),
            ("지난여름 휴가", "지난여름", "2026-06-01", "2026-08-31"),
            ("지난 여름 휴가", "지난 여름", "2026-06-01", "2026-08-31"),
            ("지난봄 행사", "지난봄", "2026-03-01", "2026-05-31"),
            ("지난 봄 행사", "지난 봄", "2026-03-01", "2026-05-31"),
        ] {
            assert_eq!(one(q, t), (expr.to_string(), a.to_string(), b.to_string()), "{q}");
        }
        // 1월: 겨울(2026-12~2027-02)이 진행 중이라 끝난 겨울은 그 전 해 것.
        assert_eq!(one("지난겨울 폭설", "2027-01-10"), ("지난겨울".into(), "2025-12-01".into(), "2026-02-28".into()));
        // 3월 초: 겨울은 막 끝났다.
        assert_eq!(one("지난겨울 폭설", "2026-03-02"), ("지난겨울".into(), "2025-12-01".into(), "2026-02-28".into()));
        let v = DateTable::builtin().find("지난겨울 폭설", day("2026-10-15"));
        assert_eq!((v[0].precision, v[0].relativity), (Precision::Exact, Relativity::Relative));
        // 기존 규칙 그대로: "지난" 단독은 날짜가 아니고, "지난 6개월" 은 기간.
        none("지난 보고서");
        none("지난 자료 모음");
        assert_eq!(one("지난 6개월 실적", "2026-10-15").1, "2026-04-15");
    }

    /// 연도를 추정한 표현(되풀이 시기)만 yearless.
    #[test]
    fn yearless_marks_recurring_periods_only() {
        let t = DateTable::builtin();
        let today = day("2026-10-15");
        for q in ["연말 경마시행계획", "연초 계획", "9월 회의", "9월 말 보고", "10월 초 행사", "3분기 실적", "3/4분기 실적", "상반기 계획", "하반기 계획", "가을 행사", "겨울 폭설", "9월 15일 공문"] {
            let v = t.find(q, today);
            assert_eq!(v.len(), 1, "{q}: {v:?}");
            assert!(v[0].yearless, "{q} 는 yearless");
        }
        for q in [
            "작년 연말 계획", "올해 상반기 계획", "2025년 9월 회의", "25년 9월 회의", "지난해 가을 행사", "이번 달 행사", "지난달 회의",
            "지난주 공지", "오늘 공문", "요즘 민원", "최근 3개월 민원", "지난 6개월 실적", "지난겨울 폭설", "올봄 행사", "이번 분기 실적",
            "지난 분기 실적", "금년 사업", "2025년 사업", "내년 사업계획", "다음 달 행사",
        ] {
            let v = t.find(q, today);
            assert_eq!(v.len(), 1, "{q}: {v:?}");
            assert!(!v[0].yearless, "{q} 는 yearless 아님");
        }
    }

    #[test]
    fn title_years_reads_absolute_years_only() {
        assert_eq!(title_years("(본문) 2025년 경마시행계획"), vec![2025]);
        assert_eq!(title_years("2026년 경마시행계획(안) (K0009)"), vec![2026]);
        assert_eq!(title_years("'25년 경마시행계획(본문)_최종"), vec![2025]);
        assert_eq!(title_years("2025년도 예산"), vec![2025]);
        assert_eq!(title_years("25년 결산"), vec![2025]);
        assert_eq!(title_years("2025.9 보고"), vec![2025]);
        assert_eq!(title_years("2025년 9월 회의"), vec![2025]);
        assert_eq!(title_years("25.9월 회의"), vec![2025]);
        assert!(title_years("작년 업무보고").is_empty());
        assert!(title_years("올해 상반기 계획").is_empty());
        assert!(title_years("9월 행사 안내").is_empty());
        assert!(title_years("12월 15일 공문").is_empty());
        // 두 해가 모두 나오면 둘 다(오름차순): "2024~2025년" → 2024 는 '년' 없는 4자리 해로 읽힌다.
        assert_eq!(title_years("2024~2025년 성과"), vec![2024, 2025]);
        assert_eq!(title_years("2025년 개정 (2024년 판)"), vec![2024, 2025]);
        assert_eq!(title_years("2025년 2025년 계획"), vec![2025], "중복 제거");
        // 문서 번호의 "제2025-3호" 는 앞이 글자라 날짜가 아니다 → 빈 목록.
        assert!(title_years("제2025-3호 공고").is_empty());
    }
}
