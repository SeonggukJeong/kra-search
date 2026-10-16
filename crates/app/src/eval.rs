//! `kra-search eval`: 평가 세트를 제품 경로 그대로(--set, DEC-052) 또는 사용자 질의로(--queries, 폐쇄망 확인 — Task 18).

use crate::evalset::{first_page_hit, first_relevant_rank, id_of, ndcg_at_k, recall_at_k, tune_split, EvalQuery, FIRST_PAGE};
use crate::search::{OpenTimes, Searcher};
use index::hybrid::HybridParams;
use crate::store::Store;
use query::Day;
use index::meta::EmbedCounts;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;
use std::time::Instant;

/// 질의별 평가 문서 id 순위(상위 100). 경로 파일 이름의 "(id)" 로 문서를 알아본다.
pub fn rank_set(store: &Store, s: &mut Searcher, qs: &[EvalQuery], today: Day) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut out = BTreeMap::new();
    let notice_before = s.notice.clone();
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
    check_no_mid_run_notice(&notice_before, s)?;
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

/// 사용자 질의 한 줄(줄 번호는 보고서에서 질의를 가리키는 유일한 단서 — 질의 글은 남기지 않는다).
#[derive(Debug, Clone, PartialEq)]
pub struct UserQuery {
    pub line: usize,
    pub text: String,
    /// 쉼표로 가른 정답 이름(따옴표 뗌)
    pub answers: Vec<String>,
    /// 탭으로 가른 정답 칸(따옴표 뗌) — `1,2분기 실적.hwp` 처럼 이름에 쉼표가 있을 수 있어 칸 전체를 먼저 이름으로 본다.
    pub fields: Vec<String>,
}

fn is_header(q: &str) -> bool {
    q == "질의" || q == "검색어" || q.eq_ignore_ascii_case("query")
}

/// 앞뒤 공백과 감싼 큰따옴표(엑셀·메모장에서 쉼표 든 칸에 붙는다)를 뗀다.
fn unquote(x: &str) -> &str {
    x.trim().trim_matches('"').trim()
}

/// `질의<탭>정답 파일 이름[,정답…][<탭>정답…]`. BOM·빈 줄·첫 줄 머리("질의"/"검색어"/"query")는 건너뛴다.
/// 두 번째 값은 읽지 못한 줄 번호(탭이 없거나 질의·정답이 빈 줄) — 보고서에 줄 번호만 남긴다.
pub fn parse_user_queries(text: &str) -> (Vec<UserQuery>, Vec<usize>) {
    let text = text.trim_start_matches('\u{feff}');
    let (mut out, mut skipped) = (Vec::new(), Vec::new());
    for (i, l) in text.lines().enumerate() {
        if l.trim().is_empty() {
            continue;
        }
        let mut cols = l.split('\t');
        let q = cols.next().unwrap_or("").trim();
        if i == 0 && is_header(q) {
            continue;
        }
        let fields: Vec<String> = cols.map(|c| unquote(c).to_string()).filter(|c| !c.is_empty()).collect();
        if q.is_empty() || fields.is_empty() {
            skipped.push(i + 1);
            continue;
        }
        let answers: Vec<String> = fields.iter().flat_map(|f| f.split(',')).map(|x| unquote(x).to_string()).filter(|x| !x.is_empty()).collect();
        out.push(UserQuery { line: i + 1, text: q.to_string(), answers, fields });
    }
    (out, skipped)
}

pub fn read_user_queries(text: &str) -> Vec<UserQuery> {
    parse_user_queries(text).0
}

/// 이름 맞추기용 꼴: NFC(맥·윈도 한글 조합 차이) + 소문자 + 구분자 `/`.
fn norm(x: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    x.trim().nfc().collect::<String>().to_lowercase().replace('\\', "/")
}

/// 색인된 파일(정규화한 경로 → 내용 id) 위에서 정답 이름을 내용 id 로 푼다.
pub struct NameIndex {
    files: Vec<(String, i64)>,
}

impl NameIndex {
    pub fn new<'a>(files: impl IntoIterator<Item = (&'a str, i64)>) -> NameIndex {
        NameIndex { files: files.into_iter().map(|(p, id)| (norm(p), id)).collect() }
    }

    /// 정답 하나가 가리키는 내용 id 들. 폴더가 든 정답(`2025\\계획.hwp`)은 저장 경로의 끝과 맞추고(구분자 `\\`·`/` 모두),
    /// 이름만 쓴 정답은 파일 이름과 맞춘다. 확장자를 뺀 이름은 그렇게 맞는 것이 없을 때만 본다.
    pub fn resolve(&self, answer: &str) -> HashSet<i64> {
        let a = norm(unquote(answer)).trim_matches('/').to_string();
        if a.is_empty() {
            return HashSet::new();
        }
        let suffix = format!("/{a}");
        let exact = |p: &str| p == a || p.ends_with(&suffix);
        let noext = |p: &str| match p.rfind('.') {
            Some(i) if i > p.rfind('/').map_or(0, |s| s + 1) => p[..i].to_string(),
            _ => p.to_string(),
        };
        let hit: HashSet<i64> = self.files.iter().filter(|(p, _)| exact(p)).map(|x| x.1).collect();
        if !hit.is_empty() {
            return hit;
        }
        self.files.iter().filter(|(p, _)| exact(&noext(p))).map(|x| x.1).collect()
    }

    /// 한 칸의 정답들: 칸 전체가 색인된 이름이면 그대로 한 개, 아니면 쉼표로 가른다. 정답 단위마다 내용 id 집합.
    pub fn resolve_field(&self, field: &str) -> Vec<HashSet<i64>> {
        let whole = self.resolve(field);
        if !whole.is_empty() {
            return vec![whole];
        }
        field.split(',').filter(|x| !unquote(x).is_empty()).map(|x| self.resolve(x)).collect()
    }
}

/// 질의 하나의 한 설정 결과: (nDCG@10, Recall@20, 첫 정답 순위).
pub type Score = (f64, f64, Option<usize>);

/// 비교용 "검색어 유형 나누기"(DEC-069): 문턱 8글자 + 키워드형 칸([0])의 가중치. 가중치는 눈으로 고르지 않고 m0eval 의 나누는 쪽
/// 좌표 하강(문턱 8 에서 출발)이 조정용 질의로 고른 값을 그대로 옮긴다(`docs/m2/eval-hybrid.md` §3.1). 2026-10-16 다시 잰 값:
/// 고른 [0] = 의미형 [1] = 1.0 — `w_semantic[0]` 후보 0.25·0.5·1.5·2 중 조정용에서 +0.005 넘게 오른 것이 없었다.
/// [0] 이 [1] 과 같으면 유형이 점수에 영향을 주지 않아 비교가 구조적으로 같다(최종 리뷰 I-1) — 그때는 비교 대신 그 사실을 적는다.
pub const SPLIT_COMPARE_CHARS: usize = 8;
/// m0eval 나누는 쪽이 고른 `w_semantic[0]`(DEC-069)
pub const SPLIT_COMPARE_W_SEMANTIC: f64 = 1.0;
/// m0eval 나누는 쪽의 `w_keyword[0]`(축 없음 — 출발값 그대로, DEC-069)
pub const SPLIT_COMPARE_W_KEYWORD: f64 = 1.0;

/// 제품 설정 `base` 에 문턱 `chars` 와 키워드형 가중치([0])만 바꾼 비교 설정. 키워드형 가중치가 의미형([1])과 같으면 유형이 점수에
/// 영향을 주지 않으므로 비교할 것이 없다 — None.
pub fn split_params(base: &HybridParams, chars: usize, w_semantic_kw: f64, w_keyword_kw: f64) -> Option<HybridParams> {
    if w_semantic_kw == base.w_semantic[1] && w_keyword_kw == base.w_keyword[1] {
        return None;
    }
    let mut p = HybridParams { keyword_max_chars: chars, ..base.clone() };
    p.w_semantic[0] = w_semantic_kw;
    p.w_keyword[0] = w_keyword_kw;
    Some(p)
}

/// 보고서의 나누기 비교 설정(DEC-069 상수). None = 비교 의미 없음.
pub fn split_compare_params(base: &HybridParams) -> Option<HybridParams> {
    split_params(base, SPLIT_COMPARE_CHARS, SPLIT_COMPARE_W_SEMANTIC, SPLIT_COMPARE_W_KEYWORD)
}

/// "날짜 칩 해제" 설정(KI-049 의 완화책 = 검색창에서 칩의 ✕ 를 누른 상태): 날짜 가산 끔 + 뺐던 날짜 낱말을 검색어에 되살림.
/// m0eval 의 "하이브리드 − 날짜 가산"(가산만 끔, 낱말은 그대로 뺌)과는 목적이 다르다 — 그쪽은 가산 하나의 효과를 떼어 보는 실험이고,
/// 이쪽은 사용자가 실제로 고를 수 있는 상태와 견준다.
pub fn chip_off_params(base: &HybridParams) -> HybridParams {
    HybridParams { date_exact: 0.0, date_vague: 0.0, strip_relative: false, strip_absolute: false, ..base.clone() }
}

#[derive(Debug, Clone)]
pub struct UserResult {
    pub line: usize,
    /// 날짜 칩이 생긴 질의(미래 표현은 칩이 아니다)
    pub has_date: bool,
    /// 정답 파일 이름을 색인에서 찾았는가
    pub found: bool,
    /// 제품 설정(날짜 가산 켬, 유형 나누지 않음)
    pub on: Score,
    /// 날짜 칩 해제(가산 끔 + 낱말 되살림, `chip_off_params`)
    pub off: Score,
    /// 유형 나누기 비교(`split_compare_params`, 날짜 칩 켬) — 사람마다 질의 습관이 달라 실제 질의로 견준다.
    /// None = 비교 설정이 제품과 점수가 같아 검색하지 않음(DEC-069)
    pub split: Option<Score>,
    /// 제품 설정 검색 한 번(해석 + 뜻 계산 + 검색) 밀리초 — 비교용 추가 검색은 넣지 않는다
    pub ms: f64,
    /// 정답 하나가 내용 둘 이상과 맞음(같은 이름이 여러 폴더에 있음) — 정답 집합이 넓어졌을 수 있다
    pub ambiguous: bool,
    /// 정답 여러 개 중 일부만 색인에서 찾음
    pub partial: bool,
}

/// 첫 페이지(상위 10) 안에 정답이 있는가.
fn on_first_page(rank: Option<usize>) -> bool {
    rank.is_some_and(|r| r <= FIRST_PAGE)
}

pub fn run_user_queries(store: &Store, s: &mut Searcher, qs: &[UserQuery], today: Day) -> Result<Vec<UserResult>, String> {
    let all = store.meta.all_files().map_err(crate::store::user_error)?;
    let names = NameIndex::new(all.iter().filter_map(|f| f.content_id.map(|id| (f.path.as_str(), id))));
    let base = s.params.clone();
    let split_p = split_compare_params(&base);
    let notice_before = s.notice.clone();
    let mut out = Vec::new();
    for q in qs {
        let units: Vec<HashSet<i64>> = q.fields.iter().flat_map(|f| names.resolve_field(f)).collect();
        let relevant: HashSet<String> = units.iter().flatten().map(|id| id.to_string()).collect();
        let ambiguous = units.iter().any(|u| u.len() > 1);
        let partial = units.iter().any(|u| !u.is_empty()) && units.iter().any(|u| u.is_empty());
        let ids = |hits: &[crate::search::HybridHit]| hits.iter().map(|h| h.hit.content_id.to_string()).collect::<Vec<_>>();
        let score = |r: &[String]| -> Score { (ndcg_at_k(r, &relevant, 10), recall_at_k(r, &relevant, 20), first_relevant_rank(r, &relevant)) };
        // 제품 설정(지연은 이 검색만 잰다).
        let t = Instant::now();
        let (pq, hits) = s.search(store, &q.text, today, 100)?;
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let on = score(&ids(&hits));
        // 날짜 칩 해제(✕): 가산 끔 + 낱말 되살림
        s.params = chip_off_params(&base);
        let off = s.search(store, &q.text, today, 100).map(|(_, h)| score(&ids(&h)));
        // 유형 나누기 비교(DEC-069) — 제품과 점수가 같은 설정이면 검색하지 않는다.
        let split = match &split_p {
            Some(p) => {
                s.params = p.clone();
                Some(s.search(store, &q.text, today, 100).map(|(_, h)| score(&ids(&h))))
            }
            None => None,
        };
        s.params = base.clone();
        out.push(UserResult { line: q.line, has_date: pq.chips().count() > 0, found: !relevant.is_empty(), on, off: off?, split: split.transpose()?, ms, ambiguous, partial });
    }
    check_no_mid_run_notice(&notice_before, s)?;
    Ok(out)
}

/// 평가 도중 검색어의 뜻을 계산하지 못해 키워드만 쓴 검색이 섞였으면(`notice` 가 새로 생김) 보고서를 만들지 않는다.
pub fn check_no_mid_run_notice(before: &Option<String>, s: &Searcher) -> Result<(), String> {
    match &s.notice {
        Some(n) if Some(n) != before.as_ref() => Err(format!("평가 도중 의미 검색을 쓰지 못한 검색이 있어 보고서를 만들지 않습니다: {n}")),
        _ => Ok(()),
    }
}

/// 마지막 `index` 실행의 기록(kv `stat_*`). 숫자·참거짓뿐이라 문서·경로가 들어갈 수 없다.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IndexStats {
    pub keyword_secs: Option<f64>,
    pub cards_secs: Option<f64>,
    pub body_secs: Option<f64>,
    pub threads: Option<u64>,
    /// 이번 실행에서 새로 읽은(바뀐) 문서 수
    pub extracted: Option<u64>,
    pub cards_new: Option<u64>,
    pub chunks_new: Option<u64>,
    /// 실행을 시작할 때 색인이 비어 있었는가(처음부터 색인)
    pub full: Option<bool>,
}

impl IndexStats {
    pub fn from_kv(get: impl Fn(&str) -> Option<String>) -> IndexStats {
        let f = |k: &str| get(k).and_then(|v| v.trim().parse::<f64>().ok());
        let n = |k: &str| get(k).and_then(|v| v.trim().parse::<u64>().ok());
        IndexStats {
            keyword_secs: f("stat_keyword_secs"),
            cards_secs: f("stat_cards_secs"),
            body_secs: f("stat_body_secs"),
            threads: n("stat_threads"),
            extracted: n("stat_extracted"),
            cards_new: n("stat_cards_new"),
            chunks_new: n("stat_chunks_new"),
            full: get("stat_full").and_then(|v| match v.trim() {
                "1" => Some(true),
                "0" => Some(false),
                _ => None,
            }),
        }
    }
}

/// 보고서 머리에 남기는 실행 정보 — 프로그램 버전, 설정 지문(앞 8자리), 읽지 못한 질의 파일 줄 번호. 사용자 내용 없음.
pub struct RunInfo {
    pub version: String,
    pub params_id: String,
    pub skipped_lines: Vec<usize>,
}

pub struct PcInfo {
    pub perf: Option<usize>,
    pub eff: Option<usize>,
    pub logical: usize,
    pub threads: usize,
}

pub struct IndexInfo {
    pub files: u64,
    pub contents: u64,
    pub failed: u64,
    pub embed: EmbedCounts,
    pub stats: IndexStats,
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(((v.len().max(1) - 1) as f64 * p).round() as usize).copied().unwrap_or(0.0)
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

/// 반출용 보고서. 질의 글·파일 이름·경로는 받지 않으므로 들어갈 수 없다(M2 설계 §7, INC-010 원칙).
pub fn user_report(rs: &[UserResult], ix: &IndexInfo, pc: &PcInfo, run: &RunInfo, open: OpenTimes) -> String {
    let found: Vec<&UserResult> = rs.iter().filter(|r| r.found).collect();
    let lines = |v: &[&UserResult]| -> String {
        if v.is_empty() {
            "없음".to_string()
        } else {
            v.iter().map(|r| r.line.to_string()).collect::<Vec<_>>().join(", ")
        }
    };
    let missing: Vec<String> = rs.iter().filter(|r| !r.found).map(|r| r.line.to_string()).collect();
    let missing = if missing.is_empty() { "없음".to_string() } else { missing.join(", ") };
    let nums = |v: &[usize]| if v.is_empty() { "없음".to_string() } else { v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ") };
    let ambiguous: Vec<usize> = rs.iter().filter(|r| r.ambiguous).map(|r| r.line).collect();
    let partial: Vec<usize> = rs.iter().filter(|r| r.partial).map(|r| r.line).collect();
    let nd = |v: &[&UserResult], f: fn(&UserResult) -> Score| mean(&v.iter().map(|r| f(r).0).collect::<Vec<_>>());
    let rc = |v: &[&UserResult], f: fn(&UserResult) -> Score| mean(&v.iter().map(|r| f(r).1).collect::<Vec<_>>());
    let fp = |v: &[&UserResult], f: fn(&UserResult) -> Score| v.iter().filter(|r| on_first_page(f(r).2)).count();
    let on = |r: &UserResult| r.on;
    let off = |r: &UserResult| r.off;
    let split = |r: &UserResult| r.split.unwrap_or_default();
    // 비교 설정이 제품과 점수가 같으면(DEC-069) 비교 검색을 하지 않았다 — 비교 줄 대신 그 사실을 적는다.
    let split_on = rs.iter().any(|r| r.split.is_some());
    let top1 = found.iter().filter(|r| r.on.2 == Some(1)).count();
    let fails: Vec<&UserResult> = found.iter().copied().filter(|r| !on_first_page(r.on.2)).collect();
    let (dated, plain): (Vec<&UserResult>, Vec<&UserResult>) = found.iter().copied().partition(|r| r.has_date);
    let (mut w, mut t, mut l) = (0, 0, 0);
    for r in &dated {
        let d = r.on.0 - r.off.0;
        if d > 1e-9 {
            w += 1
        } else if d < -1e-9 {
            l += 1
        } else {
            t += 1
        }
    }
    // 날짜 칩(가산·낱말 빼기)이 첫 페이지 적중을 바꾼 질의(KI-049: "작년 매출액" 처럼 날짜가 내용 기간인 질의가 칩 때문에 밀려날 수 있다).
    let dropped: Vec<&UserResult> = found.iter().copied().filter(|r| !on_first_page(r.on.2) && on_first_page(r.off.2)).collect();
    let gained: Vec<&UserResult> = found.iter().copied().filter(|r| on_first_page(r.on.2) && !on_first_page(r.off.2)).collect();
    let mut ms: Vec<f64> = rs.iter().map(|r| r.ms).collect();
    let n = |v: &[&UserResult]| v.len();
    let mut s = String::new();
    writeln!(s, "KRA 문서검색 — 폐쇄망 품질 확인 보고서 (M2)").unwrap();
    writeln!(s, "이 파일에는 검색어·파일 이름·경로·문서 내용이 들어 있지 않습니다. 줄 번호는 질의 파일의 줄입니다.").unwrap();
    writeln!(s, "프로그램 {} · 검색 설정 {}\n", run.version, run.params_id).unwrap();
    writeln!(s, "[요약] (정답 파일을 찾은 질의 {}개 기준, 제품 설정 = 날짜 칩 켬 · 검색어 유형 나누지 않음)", found.len()).unwrap();
    writeln!(s, "질의 {}개 · 정답 파일을 색인에서 찾지 못한 질의 {}개(줄: {missing})", rs.len(), rs.len() - found.len()).unwrap();
    writeln!(s, "정답 이름이 여러 파일과 맞은 질의(줄): {} · 정답 일부만 찾은 질의(줄): {} · 읽지 못한 질의 파일 줄: {}", nums(&ambiguous), nums(&partial), nums(&run.skipped_lines)).unwrap();
    writeln!(s, "nDCG@10 {:.4} · Recall@20 {:.4}", nd(&found, on), rc(&found, on)).unwrap();
    let all = &found;
    writeln!(s, "첫 페이지(10위 안)에 정답 {}/{} · 그중 1위 {top1}", fp(all, on), n(all)).unwrap();
    writeln!(s, "첫 페이지 밖 질의(줄): {}\n", lines(&fails)).unwrap();
    writeln!(s, "[날짜 칩 켬/해제] 해제 = 날짜 칩 해제(가산 끔·낱말 되살림) — 검색창에서 칩의 ✕ 를 누른 상태").unwrap();
    writeln!(
        s,
        "날짜 칩이 생긴 질의(미래 표현 제외) {}개: nDCG@10 켬 {:.4} · 해제 {:.4} (승 {w} 무 {t} 패 {l}) · Recall@20 켬 {:.4} · 해제 {:.4} · 첫 페이지 켬 {}/{} · 해제 {}/{}",
        n(&dated),
        nd(&dated, on),
        nd(&dated, off),
        rc(&dated, on),
        rc(&dated, off),
        fp(&dated, on),
        n(&dated),
        fp(&dated, off),
        n(&dated)
    )
    .unwrap();
    writeln!(
        s,
        "날짜 칩이 없는 질의 {}개: nDCG@10 켬 {:.4} · 해제 {:.4} · Recall@20 켬 {:.4} · 해제 {:.4} · 첫 페이지 켬 {}/{} · 해제 {}/{}",
        n(&plain),
        nd(&plain, on),
        nd(&plain, off),
        rc(&plain, on),
        rc(&plain, off),
        fp(&plain, on),
        n(&plain),
        fp(&plain, off),
        n(&plain)
    )
    .unwrap();
    writeln!(s, "전체: 첫 페이지 켬 {}/{} · 해제 {}/{}", fp(all, on), n(all), fp(all, off), n(all)).unwrap();
    writeln!(s, "날짜 칩 때문에 첫 페이지 밖으로 밀린 질의(줄): {}", lines(&dropped)).unwrap();
    writeln!(s, "날짜 칩 덕에 첫 페이지로 들어온 질의(줄): {}\n", lines(&gained)).unwrap();
    writeln!(s, "[검색어 유형 나누기 비교] (날짜 칩 켬)").unwrap();
    writeln!(s, "나누지 않음(제품): nDCG@10 {:.4} · 첫 페이지 {}/{}", nd(all, on), fp(all, on), n(all)).unwrap();
    if split_on {
        writeln!(
            s,
            "{SPLIT_COMPARE_CHARS}글자 이하를 키워드형으로 나눔(키워드형 가중치 의미 {SPLIT_COMPARE_W_SEMANTIC} · 키워드 {SPLIT_COMPARE_W_KEYWORD}, DEC-069): nDCG@10 {:.4} · 첫 페이지 {}/{}\n",
            nd(all, split),
            fp(all, split),
            n(all)
        )
        .unwrap();
    } else {
        writeln!(s, "{SPLIT_COMPARE_CHARS}글자 나눔: 평가 세트에서 나눔이 고른 가중치가 안 나눔과 같아 비교 의미 없음(DEC-069)\n").unwrap();
    }
    writeln!(s, "[색인]").unwrap();
    writeln!(s, "파일 {} · 내용 {} · 내용을 못 읽은 것 {}", ix.files, ix.contents, ix.failed).unwrap();
    let e = &ix.embed;
    writeln!(s, "정밀 의미 검색 단위 {}개 (의미 검색에 넣은 것 {})", e.main_total + e.tail_total, e.main_done + e.tail_done).unwrap();
    writeln!(s, "빠른 의미 검색 {}/{} · 정밀 의미 검색 {}/{} · 데이터 파일 뒷부분 {}/{}", e.cards_done, e.cards_total, e.main_done, e.main_total, e.tail_done, e.tail_total).unwrap();
    let st = &ix.stats;
    let yn = |x: Option<bool>| x.map_or("기록 없음", |v| if v { "예" } else { "아니오" });
    let cnt = |x: Option<u64>| x.map_or("기록 없음".to_string(), |v| format!("{v}개"));
    let sec = |x: Option<f64>| x.map_or("기록 없음".to_string(), |v| format!("{v:.1}초"));
    writeln!(s, "마지막 index 실행: 새로 읽음 {} · 빠른 의미 검색 {} · 정밀 의미 검색 {} · 처음부터 색인 여부: {}", cnt(st.extracted), cnt(st.cards_new), cnt(st.chunks_new), yn(st.full)).unwrap();
    writeln!(s, "걸린 시간: 키워드 검색 색인 {} · 빠른 의미 검색 {} · 정밀 의미 검색 {} · 색인 스레드 {}", sec(st.keyword_secs), sec(st.cards_secs), sec(st.body_secs), st.threads.map_or("기록 없음".to_string(), |v| v.to_string())).unwrap();
    let rate = |n: Option<u64>, secs: Option<f64>| match (n, secs) {
        (Some(n), Some(t)) if n > 0 && t > 0.0 => format!("{:.1}개/초", n as f64 / t),
        _ => "계산 못 함".to_string(),
    };
    writeln!(s, "처리 속도: 빠른 의미 검색 {} · 정밀 의미 검색 단위 {}", rate(st.cards_new, st.cards_secs), rate(st.chunks_new, st.body_secs)).unwrap();
    writeln!(s, "\n[속도]").unwrap();
    writeln!(
        s,
        "검색 p50 {:.0}ms · p95 {:.0}ms (검색어 해석 + 뜻 계산 + 검색, 모델을 불러온 뒤, 제품 설정 {}회) · 의미 검색 색인 불러오기 {:.0}ms · 모델 불러오기 {:.0}ms",
        pct(&mut ms, 0.5),
        pct(&mut ms, 0.95),
        rs.len(),
        open.index_ms,
        open.model_ms
    )
    .unwrap();
    writeln!(s, "\n[PC]").unwrap();
    let opt = |x: Option<usize>| x.map_or("알 수 없음".to_string(), |v| v.to_string());
    writeln!(s, "성능 코어 {} · 효율 코어 {} · 논리 코어 {} · 의미 검색 스레드 {}", opt(pc.perf), opt(pc.eff), pc.logical, pc.threads).unwrap();
    let split_col = if split_on { format!(" · 정답 순위({SPLIT_COMPARE_CHARS}글자 나눔)") } else { String::new() };
    writeln!(s, "\n[질의별] 줄 · 날짜 칩 · 정답 순위(칩 켬) · 정답 순위(칩 해제){split_col}").unwrap();
    for r in rs {
        let rank = |x: Option<usize>| x.map_or("-".to_string(), |v| v.to_string());
        let sp = r.split.map_or(String::new(), |x| format!(" · {}", rank(x.2)));
        let shown = if r.found { format!("{} · {}{sp}", rank(r.on.2), rank(r.off.2)) } else { "정답 파일 없음".into() };
        writeln!(s, "{} · {} · {shown}", r.line, if r.has_date { "있음" } else { "없음" }).unwrap();
    }
    s
}
