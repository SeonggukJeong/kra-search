//! 하이브리드 검색 코어(스펙 §8.2·§8.3, M2 설계 §4.2): 키워드 + 문서 카드 + 본문 청크 → 문서 단위 가중 RRF + 날짜·최근·구절 보정.
//! m0eval(실험)과 앱(제품)이 **이 함수들을 그대로** 부른다 — 바깥이 다른 것은 벡터를 어디서 읽는지뿐이다(DEC-052).
//! 모든 값은 `HybridParams`. 기본값은 Task 16 실험이 정한 값이다.

use crate::keyword::{KeywordIndex, KeywordWeights};
use query::calendar::{from_ymd, to_ymd};
use query::{DateSpan, Day, ParsedQuery, Precision, QueryKind, Range, Relativity};
use std::collections::{HashMap, HashSet};
use vector::flat::FlatIndex;
use vector::record::{Layout, Record};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Combine {
    /// 최고 본문 청크, 청크가 아직 없는 문서만 카드(스펙 §8.3 기본)
    ChunkElseCard,
    /// max(카드, 청크)
    Max,
    /// 카드 목록과 청크 목록을 따로 RRF 에 넣는다
    Separate,
    /// 카드만(비교 기준)
    CardOnly,
}

/// 빠진 필드는 기본값으로 읽는다(`serde(default)`) — 필드가 생기기 전에 저장한 params JSON 도 읽히게.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct HybridParams {
    pub rrf_k: f64,
    /// [키워드형 질의, 의미형 질의]
    pub w_keyword: [f64; 2],
    pub w_semantic: [f64; 2],
    /// 검색어 유형(키워드형·의미형)을 나누는 글자 수 문턱(`query::classify_with`). 0 이면 나누지 않는다 — 모든 검색어가
    /// 의미형 가중치(`[1]`)를 쓴다. 사람마다 검색 습관이 달라 M2 평가에서 0·6·8·10 을 견줘 나누기가 뚜렷이 나을 때만 남긴다(사용자 결정 2026-10-15).
    pub keyword_max_chars: usize,
    pub combine: Combine,
    /// 코사인 하한 — 미만인 의미 결과는 결합에서 뺀다. 0 이하면 끔.
    pub floor: f32,
    pub keyword: KeywordWeights,
    /// 날짜 칩 기간 안 문서 가산(1위 RRF 기여 1/(k+1) 의 배수). 경계 근처는 절반.
    pub date_exact: f64,
    pub date_vague: f64,
    /// 문서 제목이 칩과 겹치지 않는 해를 적었을 때, 파일 날짜가 칩 안이어도 주는 가산 비율(0 = 제목이 이김).
    /// 1년 이상 기간 칩(작년·올해·2025년·지난 2년)에만 적용한다 — 제목 연도는 문서가 다룬 해이지 쓴 주·달이 아니다.
    /// 2026-10-15 사용자 요구: 올해(2026) "작년 경마시행계획" 에는 2026-12 에 쓴 2026년 계획이 아니라 2025년 계획이 떠야 한다.
    pub date_title_conflict: f64,
    /// 연도 없는 되풀이 시기(연말·9월 말·3분기)의 지난 해 같은 시기에 주는 가산 비율(0 = 가장 최근 시기만).
    /// 2026-10-15 사용자 요구: "연말에 나온 경마시행계획" 은 2026·2025·2024 계획이 모두 연말에 나온 계획이다. 연중 같은 1년 이상 기간은 제외.
    pub date_recurring: f64,
    /// 상대 표현(작년·지난달)을 키워드·의미 검색어에서 뺀다
    pub strip_relative: bool,
    /// 숫자로 쓴 해(2025년)를 뺀다
    pub strip_absolute: bool,
    /// 최근 문서 약한 가산 세기(0 = 끔): 세기 × exp(−경과일/365) × 1/(k+1)
    pub recency: f64,
    /// 정확 구절 일치 가산(0 = 끔), 1/(k+1) 의 배수
    pub phrase: f64,
    pub keyword_limit: usize,
    pub card_limit: usize,
    /// 재계산 뒤 남길 청크 수(문서로 묶기 전)
    pub chunk_top: usize,
    /// 이진 후보 수 = max(chunk_min, 청크 수 × chunk_frac)(스펙 §8.2)
    pub chunk_min: usize,
    pub chunk_frac: f64,
}

impl Default for HybridParams {
    fn default() -> Self {
        HybridParams {
            rrf_k: 60.0,
            w_keyword: [1.0, 1.0],
            w_semantic: [1.0, 1.0],
            keyword_max_chars: query::KEYWORD_MAX_CHARS,
            combine: Combine::ChunkElseCard,
            floor: 0.0,
            keyword: KeywordWeights::default(),
            date_exact: 1.0,
            date_vague: 0.5,
            date_title_conflict: 0.0,
            date_recurring: 0.0,
            strip_relative: true,
            strip_absolute: false,
            recency: 0.0,
            phrase: 0.0,
            keyword_limit: 200,
            card_limit: 200,
            chunk_top: 200,
            chunk_min: 1_000,
            chunk_frac: 0.02,
        }
    }
}

/// 슬롯 순서 그대로의 벡터와 그 주인(내용 id).
pub struct VectorSet {
    pub index: FlatIndex,
    pub owners: Vec<i64>,
}

impl VectorSet {
    pub fn new(l: Layout) -> VectorSet {
        VectorSet { index: FlatIndex::new(l.dim, l.bit_dim), owners: Vec::new() }
    }
    pub fn push(&mut self, owner: i64, r: &Record) {
        self.index.push_record(r);
        self.owners.push(owner);
    }
    pub fn len(&self) -> usize {
        self.owners.len()
    }
    pub fn is_empty(&self) -> bool {
        self.owners.is_empty()
    }
}

/// 결합에 필요한 문서 정보.
pub trait DocMeta {
    /// 문서 날짜(M2: 가장 최근 경로의 수정일, 한국 날짜)
    fn date(&self, id: i64) -> Option<Day>;
    /// 본문 청크가 하나라도 있는가(ChunkElseCard 에서 카드로 대신할지)
    fn has_chunks(&self, id: i64) -> bool;
    /// 문서 제목·파일명에 적힌 해(`query::title_years` 로 부르는 쪽이 채운다). 없으면 빈 목록. 1년 이상 기간 칩에서만 쓰인다.
    fn title_years(&self, id: i64) -> Vec<i32>;
}

#[derive(Debug, Default, Clone)]
pub struct MapMeta {
    pub dates: HashMap<i64, Day>,
    pub chunked: HashSet<i64>,
    pub title_years: HashMap<i64, Vec<i32>>,
}

impl DocMeta for MapMeta {
    fn date(&self, id: i64) -> Option<Day> {
        self.dates.get(&id).copied()
    }
    fn has_chunks(&self, id: i64) -> bool {
        self.chunked.contains(&id)
    }
    fn title_years(&self, id: i64) -> Vec<i32> {
        self.title_years.get(&id).cloned().unwrap_or_default()
    }
}

pub struct QueryTexts {
    pub keyword: String,
    /// 모델 프로필의 검색어 접두어를 붙이기 전
    pub semantic: String,
}

pub fn query_texts(pq: &ParsedQuery, p: &HybridParams) -> QueryTexts {
    let drop = |s: &DateSpan| {
        s.range.is_some()
            && match s.relativity {
                Relativity::Relative => p.strip_relative,
                Relativity::Absolute => p.strip_absolute,
            }
    };
    let t = pq.text_without(drop);
    QueryTexts { keyword: t.clone(), semantic: t }
}

#[derive(Debug, Default, Clone)]
pub struct Semantic {
    pub cards: Vec<(i64, f32)>,
    pub chunks: Vec<(i64, f32)>,
}

fn sort_desc(v: &mut [(i64, f32)]) {
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
}

/// 카드는 int8 전체 스캔, 청크는 이진 후보 → int8 재계산 → 상위 chunk_top 청크 → 문서별 최고.
pub fn semantic(cards: &VectorSet, chunks: &VectorSet, qv: &[f32], p: &HybridParams) -> Semantic {
    let card_hits: Vec<(i64, f32)> = cards.index.search_exact(qv, p.card_limit).into_iter().map(|(s, c)| (cards.owners[s as usize], c)).collect();
    let cand = p.chunk_min.max((chunks.len() as f64 * p.chunk_frac).round() as usize);
    let mut best: HashMap<i64, f32> = HashMap::new();
    for (s, c) in chunks.index.search_hamming(qv, cand, p.chunk_top) {
        let e = best.entry(chunks.owners[s as usize]).or_insert(f32::MIN);
        if c > *e {
            *e = c;
        }
    }
    let mut chunk_hits: Vec<(i64, f32)> = best.into_iter().collect();
    sort_desc(&mut chunk_hits);
    Semantic { cards: card_hits, chunks: chunk_hits }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub id: i64,
    pub score: f64,
    pub keyword_rank: Option<u32>,
    /// (순위, 코사인)
    pub card: Option<(u32, f32)>,
    pub chunk: Option<(u32, f32)>,
    pub date_boost: f64,
}

fn hit(acc: &mut HashMap<i64, Hit>, id: i64) -> &mut Hit {
    acc.entry(id).or_insert(Hit { id, score: 0.0, keyword_rank: None, card: None, chunk: None, date_boost: 0.0 })
}

/// 파일 날짜만으로 본 가산 비율: 칩 안이면 전부, 여유(기간의 1/10, 최소 7일) 안이면 절반.
fn file_value(r: Range, w: f64, file: Option<Day>) -> f64 {
    let Some(d) = file else { return 0.0 };
    let margin = (r.days() / 10).max(7);
    if r.contains(d) {
        w
    } else if d >= r.start - margin && d <= r.end + margin {
        w / 2.0
    } else {
        0.0
    }
}

/// 날짜 칩 하나에 대한 가산 비율(1위 RRF 기여의 배수).
/// - 연도가 분명한 1년 이상 칩: 제목에 해가 있으면 제목이 이긴다(겹치면 w, 아니면 파일 날짜 × `date_title_conflict`). 제목 해가 없거나 1년 미만 칩이면 파일 날짜.
/// - 연도 없는 되풀이 칩: 제목 연도는 무시하고 파일 날짜. 0 이고 기간이 1년 미만이면 지난 해 같은 시기(최대 10년 전)를 `date_recurring` 만큼.
fn chip_value(yearless: bool, r: Range, w: f64, file: Option<Day>, titles: &[i32], p: &HybridParams) -> f64 {
    let file_v = file_value(r, w, file);
    if !yearless {
        // 제목 연도는 문서가 다룬 해를 말할 뿐 쓴 주·달이 아니다 → 1년 미만 칩(지난주·2025년 9월·작년 연말)은 파일 날짜만.
        if titles.is_empty() || r.days() < 365 {
            return file_v;
        }
        let title_v = if titles.iter().any(|y| from_ymd(*y, 1, 1) <= r.end && from_ymd(*y, 12, 31) >= r.start) { w } else { 0.0 };
        return title_v.max(file_v * p.date_title_conflict);
    }
    if file_v > 0.0 || r.days() >= 365 || p.date_recurring <= 0.0 {
        return file_v;
    }
    let Some(d) = file else { return 0.0 };
    for k in 1..=10 {
        let (s, e) = (shift_years(r.start, -k), shift_years(r.end, -k));
        if d >= s && d <= e {
            return w * p.date_recurring;
        }
    }
    0.0
}

/// 같은 월·일로 `k` 해 옮긴 날(2월 29일은 28일로).
fn shift_years(d: Day, k: i32) -> Day {
    let (y, m, dd) = to_ymd(d);
    let ny = y + k;
    let dd = if m == 2 && dd == 29 && !query::calendar::is_leap(ny) { 28 } else { dd };
    from_ymd(ny, m, dd)
}

pub fn fuse(keyword: &[(i64, f32)], sem: &Semantic, pq: &ParsedQuery, p: &HybridParams, today: Day, meta: &dyn DocMeta, phrase: &HashSet<i64>) -> Vec<Hit> {
    let ki = match pq.kind_with(p.keyword_max_chars) {
        QueryKind::Keyword => 0,
        QueryKind::Semantic => 1,
    };
    let k = p.rrf_k;
    let rrf = |r: usize| 1.0 / (k + r as f64 + 1.0);
    let mut acc: HashMap<i64, Hit> = HashMap::new();
    let mut listed: HashSet<i64> = HashSet::new();
    for (r, (id, _)) in keyword.iter().enumerate() {
        let h = hit(&mut acc, *id);
        h.keyword_rank = Some(r as u32 + 1);
        h.score += p.w_keyword[ki] * rrf(r);
        listed.insert(*id);
    }
    let keep = |v: &[(i64, f32)]| -> Vec<(i64, f32)> { v.iter().copied().filter(|x| p.floor <= 0.0 || x.1 >= p.floor).collect() };
    let (cards, chunks) = (keep(&sem.cards), keep(&sem.chunks));
    for (r, (id, c)) in cards.iter().enumerate() {
        hit(&mut acc, *id).card = Some((r as u32 + 1, *c));
    }
    for (r, (id, c)) in chunks.iter().enumerate() {
        hit(&mut acc, *id).chunk = Some((r as u32 + 1, *c));
    }
    let ws = p.w_semantic[ki];
    let mut add_list = |acc: &mut HashMap<i64, Hit>, list: &[(i64, f32)]| {
        for (r, (id, _)) in list.iter().enumerate() {
            hit(acc, *id).score += ws * rrf(r);
            listed.insert(*id);
        }
    };
    match p.combine {
        Combine::Separate => {
            add_list(&mut acc, &cards);
            add_list(&mut acc, &chunks);
        }
        Combine::CardOnly => add_list(&mut acc, &cards),
        Combine::ChunkElseCard | Combine::Max => {
            let mut docs: HashMap<i64, f32> = chunks.iter().copied().collect();
            for (id, c) in &cards {
                if p.combine == Combine::Max {
                    let e = docs.entry(*id).or_insert(*c);
                    if *c > *e {
                        *e = *c;
                    }
                } else if !docs.contains_key(id) && !meta.has_chunks(*id) {
                    docs.insert(*id, *c);
                }
            }
            let mut list: Vec<(i64, f32)> = docs.into_iter().collect();
            sort_desc(&mut list);
            add_list(&mut acc, &list);
        }
    }
    // 보정은 어느 목록에든 오른 문서에만(가산만으로 새 문서가 들어오지 않게)
    let top = rrf(0);
    let chips: Vec<&DateSpan> = pq.chips().collect();
    acc.retain(|id, _| listed.contains(id));
    for h in acc.values_mut() {
        let file = meta.date(h.id);
        let titles = if chips.is_empty() { Vec::new() } else { meta.title_years(h.id) };
        let mut b: f64 = 0.0;
        for s in &chips {
            let r = s.range.expect("칩은 기간이 있다");
            let w = match s.precision {
                Precision::Exact => p.date_exact,
                Precision::Vague => p.date_vague,
            };
            b = b.max(chip_value(s.yearless, r, w, file, &titles, p));
        }
        h.date_boost = b * top;
        h.score += h.date_boost;
        if let Some(d) = file {
            if p.recency > 0.0 {
                let age = (today - d).max(0) as f64;
                h.score += p.recency * (-age / 365.0).exp() * top;
            }
        }
        if p.phrase > 0.0 && phrase.contains(&h.id) {
            h.score += p.phrase * top;
        }
    }
    let mut v: Vec<Hit> = acc.into_values().collect();
    v.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.id.cmp(&b.id)));
    v
}

/// 검색 한 번. `qv` 가 없으면(모델 없음·벡터 없음) 키워드만.
#[allow(clippy::too_many_arguments)]
pub fn search(
    kw: &KeywordIndex,
    cards: &VectorSet,
    chunks: &VectorSet,
    pq: &ParsedQuery,
    qv: Option<&[f32]>,
    p: &HybridParams,
    today: Day,
    meta: &dyn DocMeta,
    limit: usize,
) -> tantivy::Result<Vec<Hit>> {
    let t = query_texts(pq, p);
    let keyword: Vec<(i64, f32)> = kw.search_with(&t.keyword, p.keyword_limit, &p.keyword)?.into_iter().map(|(id, s)| (id as i64, s)).collect();
    let phrase: HashSet<i64> = if p.phrase > 0.0 {
        kw.phrase_ids(&t.keyword, p.keyword_limit)?.into_iter().map(|x| x as i64).collect()
    } else {
        HashSet::new()
    };
    let sem = match qv {
        Some(v) if !(cards.is_empty() && chunks.is_empty()) => semantic(cards, chunks, v, p),
        _ => Semantic::default(),
    };
    let mut hits = fuse(&keyword, &sem, pq, p, today, meta, &phrase);
    hits.truncate(limit);
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyword::{KeywordDoc, KeywordIndex};
    use crate::tokenize::Analyzers;
    use query::calendar::parse_ymd;
    use query::{parse, DateTable};

    fn today() -> Day {
        parse_ymd("2026-10-15").unwrap()
    }
    fn pq(q: &str) -> ParsedQuery {
        parse(q, today(), &DateTable::builtin())
    }
    fn meta(dates: &[(i64, &str)], chunked: &[i64]) -> MapMeta {
        MapMeta { dates: dates.iter().map(|(i, d)| (*i, parse_ymd(d).unwrap())).collect(), chunked: chunked.iter().copied().collect(), title_years: HashMap::new() }
    }
    /// 시험이 기대하는 값에 영향을 주는 모든 필드를 못박은 기준값(기본값은 Task 16 평가로 바뀔 수 있다 — P4).
    fn pinned() -> HybridParams {
        HybridParams {
            rrf_k: 60.0,
            w_keyword: [1.0, 1.0],
            w_semantic: [1.0, 1.0],
            keyword_max_chars: 8,
            combine: Combine::ChunkElseCard,
            floor: 0.0,
            keyword: KeywordWeights::default(),
            date_exact: 1.0,
            date_vague: 0.5,
            date_title_conflict: 0.0,
            date_recurring: 0.0,
            strip_relative: true,
            strip_absolute: false,
            recency: 0.0,
            phrase: 0.0,
            keyword_limit: 200,
            card_limit: 200,
            chunk_top: 200,
            chunk_min: 1_000,
            chunk_frac: 0.02,
        }
    }
    /// 제목 연도를 채운 메타.
    fn meta_t(rows: &[(i64, Option<&str>, &[i32])]) -> MapMeta {
        let mut m = meta(&[], &[]);
        for (id, d, ys) in rows {
            if let Some(d) = d {
                m.dates.insert(*id, parse_ymd(d).unwrap());
            }
            m.title_years.insert(*id, ys.to_vec());
        }
        m
    }
    fn ids(h: &[Hit]) -> Vec<i64> {
        h.iter().map(|x| x.id).collect()
    }
    fn none() -> HashSet<i64> {
        HashSet::new()
    }

    #[test]
    fn rrf_scores_follow_formula() {
        let p = HybridParams { rrf_k: 10.0, ..pinned() };
        let q = pq("부경 주차장 밤에 불 꺼진 민원"); // 의미형
        let h = fuse(&[(7, 3.0), (8, 2.0)], &Semantic::default(), &q, &p, today(), &meta(&[], &[]), &none());
        assert_eq!(ids(&h), vec![7, 8]);
        assert!((h[0].score - p.w_keyword[1] / 11.0).abs() < 1e-12);
        assert!((h[1].score - p.w_keyword[1] / 12.0).abs() < 1e-12);
        assert_eq!(h[1].keyword_rank, Some(2));
    }

    #[test]
    fn query_kind_picks_weights() {
        let p = HybridParams { w_keyword: [5.0, 1.0], ..pinned() };
        let kw = fuse(&[(1, 1.0)], &Semantic::default(), &pq("더비온"), &p, today(), &meta(&[], &[]), &none());
        let se = fuse(&[(1, 1.0)], &Semantic::default(), &pq("새 사업 시작하기 전에 확인할 것"), &p, today(), &meta(&[], &[]), &none());
        assert!((kw[0].score / se[0].score - 5.0).abs() < 1e-9);
    }

    /// 유형 나누기 문턱은 매개변수다: 0 = 나누지 않음(모두 의미형 가중치), 6 이면 "국정감사 지적사항"(8자)은 의미형.
    #[test]
    fn keyword_max_chars_decides_the_weight_slot() {
        let base = HybridParams { w_keyword: [5.0, 1.0], ..pinned() };
        let score = |q: &str, n: usize| fuse(&[(1, 1.0)], &Semantic::default(), &pq(q), &HybridParams { keyword_max_chars: n, ..base.clone() }, today(), &meta(&[], &[]), &none())[0].score;
        let (kw_slot, se_slot) = (5.0 / 61.0, 1.0 / 61.0);
        assert!((score("더비온", 8) - kw_slot).abs() < 1e-12);
        assert!((score("더비온", 0) - se_slot).abs() < 1e-12, "0 이면 짧은 낱말도 의미형");
        assert!((score("국정감사 지적사항", 8) - kw_slot).abs() < 1e-12);
        assert!((score("국정감사 지적사항", 6) - se_slot).abs() < 1e-12);
        assert!((score("국정감사 지적사항", 10) - kw_slot).abs() < 1e-12);
        assert_eq!(HybridParams::default().keyword_max_chars, query::KEYWORD_MAX_CHARS);
    }

    #[test]
    fn chunk_else_card_uses_card_only_for_documents_without_chunks() {
        let p = HybridParams { combine: Combine::ChunkElseCard, ..pinned() };
        let s = Semantic { cards: vec![(1, 0.9), (2, 0.8)], chunks: vec![(3, 0.7)] };
        // 1 은 본문 청크가 있지만 상위 청크에 없음 → 카드로 대신하지 않는다. 2 는 청크가 아직 없음 → 카드.
        let h = fuse(&[], &s, &pq("새 사업 시작하기 전에 확인할 것"), &p, today(), &meta(&[], &[1, 3]), &none());
        assert_eq!(ids(&h), vec![2, 3]);
        assert_eq!(h[0].card, Some((2, 0.8)));
    }

    #[test]
    fn max_and_separate_and_card_only() {
        let s = Semantic { cards: vec![(1, 0.9), (2, 0.5)], chunks: vec![(2, 0.95), (1, 0.6)] };
        let q = pq("새 사업 시작하기 전에 확인할 것");
        let m = meta(&[], &[1, 2]);
        let max = fuse(&[], &s, &q, &HybridParams { combine: Combine::Max, ..pinned() }, today(), &m, &none());
        assert_eq!(ids(&max), vec![2, 1], "max(0.5, 0.95) > max(0.9, 0.6)");
        let sep = fuse(&[], &s, &q, &HybridParams { combine: Combine::Separate, rrf_k: 10.0, ..pinned() }, today(), &m, &none());
        assert!((sep[0].score - sep[1].score).abs() < 1e-12, "카드 1위+청크 2위 = 카드 2위+청크 1위");
        let card = fuse(&[], &s, &q, &HybridParams { combine: Combine::CardOnly, ..pinned() }, today(), &m, &none());
        assert_eq!(ids(&card), vec![1, 2]);
    }

    #[test]
    fn floor_drops_weak_semantic_results() {
        let s = Semantic { cards: vec![], chunks: vec![(1, 0.6), (2, 0.2)] };
        let h = fuse(&[], &s, &pq("새 사업 시작하기 전에 확인할 것"), &HybridParams { floor: 0.3, ..pinned() }, today(), &meta(&[], &[1, 2]), &none());
        assert_eq!(ids(&h), vec![1]);
    }

    #[test]
    fn date_boost_inside_full_near_half_far_none() {
        let p = HybridParams { rrf_k: 10.0, date_exact: 2.0, ..pinned() };
        // 작년 = 2025-01-01..12-31, 여유 = max(365/10, 7) = 36일
        let m = meta(&[(1, "2025-06-01"), (2, "2026-01-20"), (3, "2026-09-01")], &[]);
        let h = fuse(&[(3, 1.0), (2, 1.0), (1, 1.0)], &Semantic::default(), &pq("작년 출장 계획"), &p, today(), &m, &none());
        let by: HashMap<i64, &Hit> = h.iter().map(|x| (x.id, x)).collect();
        let top = 1.0 / 11.0;
        assert!((by[&1].date_boost - 2.0 * top).abs() < 1e-12);
        assert!((by[&2].date_boost - 1.0 * top).abs() < 1e-12);
        assert_eq!(by[&3].date_boost, 0.0);
        assert_eq!(ids(&h)[0], 1);
    }

    #[test]
    fn future_and_vague_dates() {
        let p = HybridParams { date_exact: 2.0, date_vague: 0.5, ..pinned() };
        let m = meta(&[(1, "2026-10-01")], &[]);
        let h = fuse(&[(1, 1.0)], &Semantic::default(), &pq("내년 사업계획"), &p, today(), &m, &none());
        assert_eq!(h[0].date_boost, 0.0, "미래 표현은 가산하지 않는다");
        let h = fuse(&[(1, 1.0)], &Semantic::default(), &pq("요즘 민원"), &p, today(), &m, &none());
        assert!((h[0].date_boost - 0.5 / (p.rrf_k + 1.0)).abs() < 1e-12);
    }

    #[test]
    fn query_texts_strip_by_relativity() {
        let q = pq("작년 2025년 출장 계획");
        let both = HybridParams { strip_relative: true, strip_absolute: true, ..pinned() };
        assert_eq!(query_texts(&q, &both).keyword, "출장 계획");
        let rel = HybridParams { strip_relative: true, strip_absolute: false, ..pinned() };
        assert_eq!(query_texts(&q, &rel).keyword, "2025년 출장 계획");
        let off = HybridParams { strip_relative: false, strip_absolute: false, ..pinned() };
        assert_eq!(query_texts(&q, &off).semantic, "작년 2025년 출장 계획");
    }

    #[test]
    fn recency_and_phrase_boosts_only_listed_docs() {
        let p = HybridParams { recency: 1.0, phrase: 1.0, rrf_k: 10.0, ..pinned() };
        let m = meta(&[(1, "2026-10-15"), (2, "2016-10-15"), (9, "2026-10-15")], &[]);
        let phrase: HashSet<i64> = [2, 9].into_iter().collect();
        let h = fuse(&[(2, 1.0), (1, 1.0)], &Semantic::default(), &pq("내부통제 규정 개정 이유"), &p, today(), &m, &phrase);
        assert_eq!(ids(&h).len(), 2, "목록에 없는 9 는 가산만으로 들어오지 않는다");
        let by: HashMap<i64, &Hit> = h.iter().map(|x| (x.id, x)).collect();
        let top = 1.0 / 11.0;
        assert!((by[&1].score - (p.w_keyword[1] / 12.0 + top)).abs() < 1e-9, "오늘 문서 최근 가산 = 1위 기여");
        assert!(by[&2].score > p.w_keyword[1] / 11.0 + top * 0.99, "구절 가산");
    }

    fn vecs(n: usize, seed: u64) -> Vec<Vec<f32>> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                (0..768)
                    .map(|_| {
                        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                        ((s >> 33) as f32 / (1u64 << 31) as f32) - 0.5
                    })
                    .collect()
            })
            .collect()
    }

    /// 일치 검사의 단위 판: f32 → 레코드(평가) 와 f32 → 레코드 → 바이트 → 레코드(제품 디스크)가 같은 순위를 낸다.
    #[test]
    fn same_vectors_give_same_ranking_through_disk_bytes() {
        let a = Analyzers::load().unwrap();
        let mut kw = KeywordIndex::in_ram(&a).unwrap();
        let docs = ["내부통제 규정 개정", "더비온 운영 계획", "경마 시행 계획", "출장 계획"];
        for (i, t) in docs.iter().enumerate() {
            let d = doc::Document { title: Some(t.to_string()), blocks: vec![] };
            kw.upsert(&KeywordDoc::new(i as u64, Some(&d), &[format!("{t}.hwp")])).unwrap();
        }
        kw.commit().unwrap();
        let vs = vecs(12, 3);
        let (mut c1, mut k1, mut c2, mut k2) = (VectorSet::new(Layout::M0), VectorSet::new(Layout::M0), VectorSet::new(Layout::M0), VectorSet::new(Layout::M0));
        for (i, v) in vs.iter().enumerate() {
            let r = Record::from_full(v, Layout::M0).unwrap();
            let mut b = Vec::new();
            r.write_to(&mut b);
            let r2 = Record::read_from(&b, Layout::M0);
            let owner = (i % 4) as i64;
            if i < 4 {
                c1.push(owner, &r);
                c2.push(owner, &r2);
            } else {
                k1.push(owner, &r);
                k2.push(owner, &r2);
            }
        }
        let p = HybridParams::default();
        let q = pq("작년 출장 계획");
        let m = meta(&[(3, "2025-03-01")], &[0, 1, 2, 3]);
        let h1 = search(&kw, &c1, &k1, &q, Some(&vs[5]), &p, today(), &m, 10).unwrap();
        let h2 = search(&kw, &c2, &k2, &q, Some(&vs[5]), &p, today(), &m, 10).unwrap();
        assert_eq!(h1, h2);
        assert!(!h1.is_empty());
    }

    #[test]
    fn keyword_only_without_query_vector() {
        let a = Analyzers::load().unwrap();
        let mut kw = KeywordIndex::in_ram(&a).unwrap();
        let d = doc::Document { title: Some("더비온 운영 계획".into()), blocks: vec![] };
        kw.upsert(&KeywordDoc::new(4, Some(&d), &["a.hwp".into()])).unwrap();
        kw.commit().unwrap();
        let empty = VectorSet::new(Layout::M0);
        let h = search(&kw, &empty, &empty, &pq("더비온"), None, &pinned(), today(), &meta(&[], &[]), 10).unwrap();
        assert_eq!(ids(&h), vec![4]);
    }

    fn boost_of(h: &[Hit], id: i64) -> f64 {
        h.iter().find(|x| x.id == id).unwrap().date_boost
    }

    #[test]
    fn title_year_beats_file_date() {
        // 2026 년 "작년 경마시행계획" → 2025 년 계획이 위. 2026 계획은 2025-12 에 쓰였어도 가산 없음.
        let q = pq("작년 경마시행계획");
        let m = meta_t(&[(1, Some("2024-12-29"), &[2025]), (2, Some("2025-12-21"), &[2026])]);
        let base = HybridParams { rrf_k: 10.0, date_exact: 2.0, date_vague: 0.5, recency: 0.0, phrase: 0.0, ..pinned() };
        let top = 1.0 / 11.0;
        let kw = [(2, 1.0), (1, 1.0)]; // 2 가 키워드 1위
        let h = fuse(&kw, &Semantic::default(), &q, &HybridParams { date_title_conflict: 0.0, ..base.clone() }, today(), &m, &none());
        assert!((boost_of(&h, 1) - 2.0 * top).abs() < 1e-12);
        assert_eq!(boost_of(&h, 2), 0.0);
        assert_eq!(ids(&h)[0], 1, "제목 연도가 맞는 문서가 앞선다");
        let h = fuse(&kw, &Semantic::default(), &q, &HybridParams { date_title_conflict: 0.5, ..base }, today(), &m, &none());
        assert!((boost_of(&h, 2) - 0.5 * 2.0 * top).abs() < 1e-12);
        assert!((boost_of(&h, 1) - 2.0 * top).abs() < 1e-12);
    }

    #[test]
    fn title_year_without_file_date_still_boosts() {
        let q = pq("작년 경마시행계획");
        let m = meta_t(&[(1, None, &[2025])]);
        let p = HybridParams { rrf_k: 10.0, date_exact: 2.0, date_title_conflict: 0.0, ..pinned() };
        let h = fuse(&[(1, 1.0)], &Semantic::default(), &q, &p, today(), &m, &none());
        assert!((boost_of(&h, 1) - 2.0 / 11.0).abs() < 1e-12);
    }

    #[test]
    fn yearless_chip_uses_file_date_and_recurring() {
        // "연말에 나온 …": 2026·2025·2024 계획 모두 연말에 나온 계획 — 제목 연도는 무시, 지난 해 연말은 date_recurring 만큼.
        let q = pq("연말에 나온 경마시행계획");
        assert!(q.chips().next().unwrap().yearless);
        let m = meta_t(&[(1, Some("2025-12-21"), &[2026]), (2, Some("2024-12-29"), &[2025])]);
        let base = HybridParams { rrf_k: 10.0, date_exact: 2.0, date_title_conflict: 0.0, ..pinned() };
        let top = 1.0 / 11.0;
        let kw = [(1, 1.0), (2, 1.0)];
        let h = fuse(&kw, &Semantic::default(), &q, &HybridParams { date_recurring: 0.0, ..base.clone() }, today(), &m, &none());
        assert!((boost_of(&h, 1) - 2.0 * top).abs() < 1e-12);
        assert_eq!(boost_of(&h, 2), 0.0);
        let h = fuse(&kw, &Semantic::default(), &q, &HybridParams { date_recurring: 0.5, ..base }, today(), &m, &none());
        assert!((boost_of(&h, 2) - 0.5 * 2.0 * top).abs() < 1e-12);
        assert!((boost_of(&h, 1) - 2.0 * top).abs() < 1e-12);
    }

    #[test]
    fn whole_year_yearless_gets_no_recurring() {
        let q = pq("연중 행사 계획");
        let s = q.chips().next().expect("연중은 칩");
        assert!(s.yearless);
        let r = s.range.unwrap();
        assert!(r.days() >= 365);
        // 칩 기간보다 꼭 한 해 앞선 날
        let (y, mo, d) = query::calendar::to_ymd(r.start);
        let prev = query::calendar::from_ymd(y - 1, mo, d);
        let m = MapMeta { dates: [(1, prev)].into_iter().collect(), chunked: HashSet::new(), title_years: HashMap::new() };
        let p = HybridParams { date_exact: 2.0, date_vague: 2.0, date_recurring: 1.0, date_title_conflict: 1.0, ..pinned() };
        let h = fuse(&[(1, 1.0)], &Semantic::default(), &q, &p, today(), &m, &none());
        assert_eq!(boost_of(&h, 1), 0.0);
    }

    #[test]
    fn title_years_ignored_for_sub_year_chips() {
        let q = pq("지난주 회의");
        let m = meta_t(&[(1, Some("2026-03-02"), &[2026]), (2, Some("2026-10-07"), &[2025])]);
        let p = HybridParams { rrf_k: 10.0, date_exact: 2.0, date_title_conflict: 0.0, date_recurring: 0.0, ..pinned() };
        let h = fuse(&[(1, 1.0), (2, 1.0)], &Semantic::default(), &q, &p, today(), &m, &none());
        assert_eq!(boost_of(&h, 1), 0.0, "제목이 올해여도 지난주 문서가 아니다");
        assert!((boost_of(&h, 2) - 2.0 / 11.0).abs() < 1e-12, "제목이 작년이어도 지난주에 쓴 문서");
    }

    #[test]
    fn multiple_chips_take_the_max() {
        let q = pq("작년 회의 지난주 보고");
        assert_eq!(q.chips().count(), 2);
        let m = meta_t(&[(1, Some("2025-06-01"), &[]), (2, Some("2026-10-07"), &[])]);
        let p = HybridParams { rrf_k: 10.0, date_exact: 2.0, ..pinned() };
        let h = fuse(&[(1, 1.0), (2, 1.0)], &Semantic::default(), &q, &p, today(), &m, &none());
        assert!((boost_of(&h, 1) - 2.0 / 11.0).abs() < 1e-12);
        assert!((boost_of(&h, 2) - 2.0 / 11.0).abs() < 1e-12, "합이 아니라 최댓값");
    }

    #[test]
    fn old_params_json_without_new_fields_loads_with_defaults() {
        // keyword_max_chars·date_title_conflict·date_recurring 가 생기기 전의 params JSON
        let old = r#"{"rrf_k":30.0,"w_keyword":[1.0,1.0],"w_semantic":[1.0,1.5],"combine":"Max","floor":0.2,
            "keyword":{"title":3.0,"filename":2.5,"heading":2.0,"body":1.0,"bigram":0.5},"date_exact":2.0,"date_vague":0.5,
            "strip_relative":true,"strip_absolute":false,"recency":0.0,"phrase":0.0,"keyword_limit":200,"card_limit":200,
            "chunk_top":200,"chunk_min":1000,"chunk_frac":0.02}"#;
        let p: HybridParams = serde_json::from_str(old).expect("옛 JSON");
        let d = HybridParams::default();
        assert_eq!((p.rrf_k, p.combine, p.w_semantic), (30.0, Combine::Max, [1.0, 1.5]));
        assert_eq!(p.keyword_max_chars, d.keyword_max_chars);
        assert_eq!((p.date_title_conflict, p.date_recurring), (d.date_title_conflict, d.date_recurring));
        let back: HybridParams = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn shift_years_clamps_feb_29() {
        use query::calendar::from_ymd;
        assert_eq!(shift_years(from_ymd(2024, 2, 29), 1), from_ymd(2025, 2, 28));
        assert_eq!(shift_years(from_ymd(2024, 2, 29), -4), from_ymd(2020, 2, 29));
        assert_eq!(shift_years(from_ymd(2026, 12, 31), -2), from_ymd(2024, 12, 31));
    }

    #[test]
    fn shifted_ranges_have_no_half_margin() {
        // 연말(2025-12) 의 한 해 앞(2024-12) 끝에서 3일 지난 날: 여유 안이어도 되풀이 가산은 없다.
        let q = pq("연말에 나온 경마시행계획");
        let m = meta_t(&[(1, Some("2025-01-03"), &[])]);
        let p = HybridParams { date_exact: 2.0, date_recurring: 1.0, date_title_conflict: 1.0, ..pinned() };
        let h = fuse(&[(1, 1.0)], &Semantic::default(), &q, &p, today(), &m, &none());
        assert_eq!(boost_of(&h, 1), 0.0);
    }
}
