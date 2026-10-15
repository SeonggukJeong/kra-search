//! `kra-search eval`: 평가 세트를 제품 경로 그대로(--set, DEC-052) 또는 사용자 질의로(--queries, 폐쇄망 확인 — Task 18).

use crate::evalset::{first_page_hit, first_relevant_rank, id_of, ndcg_at_k, recall_at_k, tune_split, EvalQuery, FIRST_PAGE};
use crate::search::Searcher;
use crate::store::Store;
use query::Day;
use index::meta::EmbedCounts;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;
use std::path::Path;
use std::time::Instant;

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

/// 사용자 질의 한 줄(줄 번호는 보고서에서 질의를 가리키는 유일한 단서 — 질의 글은 남기지 않는다).
#[derive(Debug, Clone, PartialEq)]
pub struct UserQuery {
    pub line: usize,
    pub text: String,
    pub answers: Vec<String>,
}

/// `질의<탭>정답 파일 이름[,정답…]`. BOM·빈 줄·첫 줄 머리("질의"/"query")는 건너뛴다.
pub fn read_user_queries(text: &str) -> Vec<UserQuery> {
    let text = text.trim_start_matches('\u{feff}');
    let mut out = Vec::new();
    for (i, l) in text.lines().enumerate() {
        let Some((q, a)) = l.split_once('\t') else { continue };
        let q = q.trim();
        if q.is_empty() || (i == 0 && (q == "질의" || q.eq_ignore_ascii_case("query"))) {
            continue;
        }
        let answers: Vec<String> = a.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
        if !answers.is_empty() {
            out.push(UserQuery { line: i + 1, text: q.to_string(), answers });
        }
    }
    out
}

/// 질의 하나의 한 설정 결과: (nDCG@10, Recall@20, 첫 정답 순위).
pub type Score = (f64, f64, Option<usize>);

/// 비교용 "검색어 유형 나누기" 문턱 — M1 까지 쓰던 값(DEC-058 이전). 제품 기본은 나누지 않음(0).
pub const SPLIT_COMPARE_CHARS: usize = 8;

#[derive(Debug, Clone)]
pub struct UserResult {
    pub line: usize,
    /// 날짜 표현이 있는 질의(칩이 생김)
    pub has_date: bool,
    /// 정답 파일 이름을 색인에서 찾았는가
    pub found: bool,
    /// 제품 설정(날짜 가산 켬, 유형 나누지 않음)
    pub on: Score,
    /// 날짜 가산 끔(나머지는 제품 설정)
    pub off: Score,
    /// 유형 나누기 8글자(날짜 가산 켬) — 사람마다 질의 습관이 달라 실제 질의로 견준다
    pub split: Score,
    /// 제품 설정 검색 한 번(해석 + 뜻 계산 + 검색) 밀리초 — 비교용 추가 검색은 넣지 않는다
    pub ms: f64,
}

fn file_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// 첫 페이지(상위 10) 안에 정답이 있는가.
fn on_first_page(rank: Option<usize>) -> bool {
    rank.is_some_and(|r| r <= FIRST_PAGE)
}

pub fn run_user_queries(store: &Store, s: &mut Searcher, qs: &[UserQuery], today: Day) -> Result<Vec<UserResult>, String> {
    // 파일 이름(소문자) → 내용 id. 확장자 없이 쓴 정답은 이름 앞부분으로도 찾는다.
    let mut by_name: HashMap<String, HashSet<i64>> = HashMap::new();
    for f in store.meta.all_files().map_err(crate::store::user_error)? {
        let Some(id) = f.content_id else { continue };
        let name = Path::new(&f.path).file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let stem = Path::new(&name).file_stem().and_then(|n| n.to_str()).unwrap_or("").to_string();
        by_name.entry(file_key(&name)).or_default().insert(id);
        by_name.entry(file_key(&stem)).or_default().insert(id);
    }
    let base = s.params.clone();
    let mut out = Vec::new();
    for q in qs {
        let relevant: HashSet<String> = q.answers.iter().filter_map(|a| by_name.get(&file_key(a))).flatten().map(|id| id.to_string()).collect();
        let ids = |hits: &[crate::search::HybridHit]| hits.iter().map(|h| h.hit.content_id.to_string()).collect::<Vec<_>>();
        let score = |r: &[String]| -> Score { (ndcg_at_k(r, &relevant, 10), recall_at_k(r, &relevant, 20), first_relevant_rank(r, &relevant)) };
        // 제품 설정(지연은 이 검색만 잰다).
        let t = Instant::now();
        let (pq, hits) = s.search(store, &q.text, today, 100)?;
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let on = score(&ids(&hits));
        // 날짜 가산 끔
        s.params = index::hybrid::HybridParams { date_exact: 0.0, date_vague: 0.0, ..base.clone() };
        let off = s.search(store, &q.text, today, 100).map(|(_, h)| score(&ids(&h)));
        // 유형 나누기 8글자
        s.params = index::hybrid::HybridParams { keyword_max_chars: SPLIT_COMPARE_CHARS, ..base.clone() };
        let split = s.search(store, &q.text, today, 100).map(|(_, h)| score(&ids(&h)));
        s.params = base.clone();
        out.push(UserResult { line: q.line, has_date: pq.chips().count() > 0, found: !relevant.is_empty(), on, off: off?, split: split?, ms });
    }
    Ok(out)
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
    /// kv stat_* (마지막 index 기준)
    pub stats: Vec<(String, String)>,
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
pub fn user_report(rs: &[UserResult], ix: &IndexInfo, pc: &PcInfo, model_ms: f64) -> String {
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
    let nd = |v: &[&UserResult], f: fn(&UserResult) -> Score| mean(&v.iter().map(|r| f(r).0).collect::<Vec<_>>());
    let rc = |v: &[&UserResult], f: fn(&UserResult) -> Score| mean(&v.iter().map(|r| f(r).1).collect::<Vec<_>>());
    let fp = |v: &[&UserResult], f: fn(&UserResult) -> Score| v.iter().filter(|r| on_first_page(f(r).2)).count();
    let on = |r: &UserResult| r.on;
    let off = |r: &UserResult| r.off;
    let split = |r: &UserResult| r.split;
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
    // 가산이 첫 페이지 적중을 바꾼 질의(KI-049: "작년 매출액" 처럼 날짜가 내용 기간인 질의가 가산으로 밀려날 수 있다).
    let dropped: Vec<&UserResult> = found.iter().copied().filter(|r| !on_first_page(r.on.2) && on_first_page(r.off.2)).collect();
    let gained: Vec<&UserResult> = found.iter().copied().filter(|r| on_first_page(r.on.2) && !on_first_page(r.off.2)).collect();
    let mut ms: Vec<f64> = rs.iter().map(|r| r.ms).collect();
    let n = |v: &[&UserResult]| v.len();
    let mut s = String::new();
    writeln!(s, "KRA 문서검색 — 폐쇄망 품질 확인 보고서 (M2)").unwrap();
    writeln!(s, "이 파일에는 검색어·파일 이름·경로·문서 내용이 들어 있지 않습니다. 줄 번호는 질의 파일의 줄입니다.\n").unwrap();
    writeln!(s, "[요약] (정답 파일을 찾은 질의 {}개 기준, 제품 설정 = 날짜 가산 켬 · 검색어 유형 나누지 않음)", found.len()).unwrap();
    writeln!(s, "질의 {}개 · 정답 파일을 색인에서 찾지 못한 질의 {}개(줄: {missing})", rs.len(), rs.len() - found.len()).unwrap();
    writeln!(s, "nDCG@10 {:.4} · Recall@20 {:.4}", nd(&found, on), rc(&found, on)).unwrap();
    let all = &found;
    writeln!(s, "첫 페이지(10위 안)에 정답 {}/{} · 그중 1위 {top1}", fp(all, on), n(all)).unwrap();
    writeln!(s, "첫 페이지 밖 질의(줄): {}\n", lines(&fails)).unwrap();
    writeln!(s, "[날짜 가산 켬/끔]").unwrap();
    writeln!(
        s,
        "날짜 표현 있는 질의 {}개: nDCG@10 켬 {:.4} · 끔 {:.4} (승 {w} 무 {t} 패 {l}) · Recall@20 켬 {:.4} · 끔 {:.4} · 첫 페이지 켬 {}/{} · 끔 {}/{}",
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
        "날짜 표현 없는 질의 {}개: nDCG@10 켬 {:.4} · 끔 {:.4} · Recall@20 켬 {:.4} · 끔 {:.4} · 첫 페이지 켬 {}/{} · 끔 {}/{}",
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
    writeln!(s, "전체: 첫 페이지 켬 {}/{} · 끔 {}/{}", fp(all, on), n(all), fp(all, off), n(all)).unwrap();
    writeln!(s, "가산 때문에 첫 페이지 밖으로 밀린 질의(줄): {}", lines(&dropped)).unwrap();
    writeln!(s, "가산 덕에 첫 페이지로 들어온 질의(줄): {}\n", lines(&gained)).unwrap();
    writeln!(s, "[검색어 유형 나누기 비교] (날짜 가산 켬)").unwrap();
    writeln!(
        s,
        "나누지 않음(제품): nDCG@10 {:.4} · 첫 페이지 {}/{}",
        nd(all, on),
        fp(all, on),
        n(all)
    )
    .unwrap();
    writeln!(
        s,
        "{SPLIT_COMPARE_CHARS}글자 이하를 키워드형으로 나눔: nDCG@10 {:.4} · 첫 페이지 {}/{}\n",
        nd(all, split),
        fp(all, split),
        n(all)
    )
    .unwrap();
    writeln!(s, "[색인]").unwrap();
    writeln!(s, "파일 {} · 내용 {} · 내용을 못 읽은 것 {}", ix.files, ix.contents, ix.failed).unwrap();
    let e = &ix.embed;
    writeln!(s, "청크 수 {} (의미 검색에 넣은 것 {})", e.main_total + e.tail_total, e.main_done + e.tail_done).unwrap();
    writeln!(s, "빠른 의미 검색 {}/{} · 정밀 의미 검색 {}/{} · 데이터 파일 뒷부분 {}/{}", e.cards_done, e.cards_total, e.main_done, e.main_total, e.tail_done, e.tail_total).unwrap();
    for (k, v) in &ix.stats {
        writeln!(s, "{k}: {v}").unwrap();
    }
    writeln!(s, "\n[속도]").unwrap();
    writeln!(s, "검색 p50 {:.0}ms · p95 {:.0}ms (검색어 해석 + 뜻 계산 + 검색, 모델을 불러온 뒤, 제품 설정 {}회) · 모델 불러오기 {model_ms:.0}ms", pct(&mut ms, 0.5), pct(&mut ms, 0.95), rs.len()).unwrap();
    writeln!(s, "\n[PC]").unwrap();
    let opt = |x: Option<usize>| x.map_or("알 수 없음".to_string(), |v| v.to_string());
    writeln!(s, "성능 코어 {} · 효율 코어 {} · 논리 코어 {} · 의미 검색 스레드 {}", opt(pc.perf), opt(pc.eff), pc.logical, pc.threads).unwrap();
    writeln!(s, "\n[질의별] 줄 · 날짜 표현 · 정답 순위(가산 켬) · 정답 순위(가산 끔) · 정답 순위({SPLIT_COMPARE_CHARS}글자 나눔)").unwrap();
    for r in rs {
        let rank = |x: Option<usize>| x.map_or("-".to_string(), |v| v.to_string());
        let shown = if r.found { format!("{} · {} · {}", rank(r.on.2), rank(r.off.2), rank(r.split.2)) } else { "정답 파일 없음".into() };
        writeln!(s, "{} · {} · {shown}", r.line, if r.has_date { "있음" } else { "없음" }).unwrap();
    }
    s
}
