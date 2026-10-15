//! M2 하이브리드 평가(M2 설계 §6, DEC-051·052). 벡터는 항목별 캐시(DEC-032), 검색은 제품 코어 `index::hybrid::search` 그대로.
//! 벡터를 넣는 순서도 제품의 슬롯 순서(카드 순서·본문 3단 일정)와 같게 한다 — 이진 후보에서 해밍 거리가 같으면 슬롯 번호로 가르기 때문.
//! 문서 번호도 제품처럼 매긴다(수정 시각 최근 먼저, 같으면 경로 오름차순 — Task 17 의 수집 순서) — 점수가 같으면 번호로 가르기 때문.
//! 조정용 질의로 매개변수를 고르고(좌표 하강, 0.005 이내면 바꾸지 않음), 검증용으로 판정하고, 결정 규칙을 숫자로 적용한다.

use crate::cache::Store as Cache;
use crate::corpus::{EvalDoc, FailedDoc, Query};
use app::embedding::title_of;
use app::evalset::{first_relevant_rank, ndcg_at_k, recall_at_k, tune_split, FIRST_PAGE};
use app::vectors::LAYOUT;
use doc::Document;
use embed::chunk::{build_card, build_chunks_marked};
use embed::engine::{Embedder, EngineOptions};
use embed::plan::{self, BodyDoc, CardDoc};
use embed::profile::ModelProfile;
use index::hybrid::{self, Combine, HybridParams, MapMeta, VectorSet};
use index::keyword::{KeywordDoc, KeywordIndex};
use index::tokenize::Analyzers;
use query::calendar::{day_of_unix_ns, format_ymd, noon_kst_unix_secs};
use query::{DateTable, Day, QueryKind};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use vector::record::Record;

const CAPS: [usize; 2] = [256, 128];
/// 결정 규칙의 "차이 없음" 폭(DEC-051)
const EPS: f64 = 0.005;
/// 좌표 하강 회차 상한(P19) — 한 회차를 다 돌아도 바뀐 것이 없으면 그 전에 멈춘다.
const MAX_PASSES: usize = 10;
/// 유형 나누기 문턱 후보(0 = 나누지 않음, 사용자 결정 2026-10-15)
const SPLITS: [usize; 4] = [0, 6, 8, 10];

pub struct HArgs {
    pub model: PathBuf,
    pub threads: usize,
    pub now: Day,
}

struct HDoc {
    id: String,
    /// 평가 세트 폴더 이름 — 경로 순서용
    set: String,
    file: String,
    ext: String,
    date: Option<Day>,
    doc: Option<Document>,
}

pub struct World {
    docs: Vec<HDoc>,
    kw: KeywordIndex,
    cards: HashMap<usize, VectorSet>,
    chunks_head: VectorSet,
    chunks_all: VectorSet,
    meta_head: MapMeta,
    meta_all: MapMeta,
    qvec: HashMap<String, Vec<f32>>,
    /// 문서별 자르기 전 카드 토큰 수(결정 #27 판정)
    card_tokens: Vec<usize>,
    now: Day,
    table: DateTable,
}

#[derive(Clone)]
pub struct Cfg {
    pub params: HybridParams,
    pub cap: usize,
    pub tail: bool,
    /// 검색어 벡터를 넘기는가 — 끄면 제품의 "모델 없음" 경로(키워드 단독)
    pub semantic: bool,
}

fn embed_all(m: &mut Embedder, items: Vec<(String, String)>) -> Vec<(String, Vec<f32>)> {
    let mut out = Vec::with_capacity(items.len());
    for b in items.chunks(8) {
        let t: Vec<String> = b.iter().map(|x| x.1.clone()).collect();
        let es = m.embed(&t).expect("임베딩");
        out.extend(b.iter().map(|x| x.0.clone()).zip(es.into_iter().map(|e| e.vector)));
    }
    out
}

/// 평가 복사본의 수정 시각(나노초) — Task 17 `materialize` 처럼 매니페스트 날짜의 한국 정오. 날짜가 없으면 0.
fn mtime_of(d: &HDoc) -> i64 {
    d.date.map(|x| noon_kst_unix_secs(x) * 1_000_000_000).unwrap_or(0)
}

/// 제품 수집 순서(P2): 수정 시각 최근 먼저, 같으면 상대 경로 `"{세트}/{파일}"` 오름차순. 이 순서의 번호가 내용 id 역할을 한다.
fn order_docs(hd: &mut [HDoc]) {
    hd.sort_by_cached_key(|d| (std::cmp::Reverse(mtime_of(d)), format!("{}/{}", d.set, d.file)));
}

/// 제목 연도를 찾을 글 — 제품 `Meta::title_texts` 와 같게: 제목이 있으면 "{제목}\n{파일 이름}", 없으면 파일 이름.
fn title_text(title: Option<&str>, file: &str) -> String {
    match title.map(str::trim) {
        Some(t) if !t.is_empty() => format!("{t}\n{file}"),
        _ => file.to_string(),
    }
}

pub fn build(docs: &[EvalDoc], failed: &[FailedDoc], queries: &[Query], a: &HArgs) -> World {
    let mut hd: Vec<HDoc> = docs
        .iter()
        .map(|d| {
            let mut doc = d.doc.clone();
            if d.title_filled {
                doc.title = None; // 제품처럼: 제목이 없으면 파일 이름(title_of)
            }
            HDoc { id: d.id.clone(), set: d.set.clone(), file: d.file.clone(), ext: d.ext.clone(), date: d.date, doc: Some(doc) }
        })
        .collect();
    hd.extend(failed.iter().map(|f| HDoc { id: f.id.clone(), set: f.set.clone(), file: f.file.clone(), ext: f.ext.clone(), date: f.date, doc: None }));
    order_docs(&mut hd);

    let an = Analyzers::load().expect("형태소 사전");
    let mut kw = KeywordIndex::in_ram(&an).expect("색인");
    for (i, d) in hd.iter().enumerate() {
        kw.upsert(&KeywordDoc::new(i as u64, d.doc.as_ref(), std::slice::from_ref(&d.file))).expect("색인 쓰기");
    }
    kw.commit().expect("커밋");

    let name = a.model.file_name().expect("모델 파일").to_string_lossy().to_string();
    let mut m = Embedder::load(&a.model, &EngineOptions { threads: a.threads, max_tokens: 1024, backend_dir: None }).expect("모델 불러오기");
    let mut cache = Cache::open(&Path::new("target/m0cache").join(&name));
    let p = ModelProfile::embeddinggemma2();
    let table = DateTable::builtin();

    // 카드: 제품의 카드 순서(문서 형식·최근 수정 먼저)대로 넣는다.
    let mut order: Vec<CardDoc> = hd
        .iter()
        .enumerate()
        .filter(|(_, d)| d.doc.is_some())
        .map(|(i, d)| CardDoc { id: i as i64, class: plan::class_of_ext(&d.ext), mtime_ns: mtime_of(d) })
        .collect();
    plan::card_order(&mut order);
    let mut cards = HashMap::new();
    let mut card_tokens = vec![0usize; hd.len()];
    for cap in CAPS {
        let mut inputs = Vec::new();
        {
            let count = |s: &str| m.token_count(s).unwrap_or(s.chars().count());
            for o in &order {
                let d = &hd[o.id as usize];
                let doc = d.doc.as_ref().expect("본문");
                if cap == CAPS[0] {
                    card_tokens[o.id as usize] = build_card(doc, 100_000, &count).map(|c| count(&c)).unwrap_or(0);
                }
                if let Some(c) = build_card(doc, cap, &count) {
                    inputs.push((o.id.to_string(), p.render_doc(Some(&title_of(doc, &d.file)), &c)));
                }
            }
        }
        let vs = cache.get_or_compute(&format!("card{cap}"), inputs, |x| embed_all(&mut m, x));
        let mut set = VectorSet::new(LAYOUT);
        for (id, v) in vs {
            if let Some(r) = Record::from_full(&v, LAYOUT) {
                set.push(id.parse().expect("id"), &r);
            }
        }
        cards.insert(cap, set);
    }

    // 본문: 청크를 만들고 제품의 3단 일정 순서로 넣는다(③ 끔 = 일정에서 ③을 뺀 것).
    let mut texts: HashMap<(i64, usize), String> = HashMap::new();
    let mut bodies = Vec::new();
    {
        let count = |s: &str| m.token_count(s).unwrap_or(s.chars().count());
        for (i, d) in hd.iter().enumerate() {
            let Some(doc) = &d.doc else { continue };
            let chunks = build_chunks_marked(doc, &plan::chunk_params(&d.ext), &count);
            let order = plan::priority_order(&chunks);
            let title = title_of(doc, &d.file);
            for (pos, &ci) in order.iter().enumerate() {
                texts.insert((i as i64, pos), p.render_doc(Some(&title), &chunks[ci].text));
            }
            bodies.push(BodyDoc { id: i as i64, class: plan::class_of_ext(&d.ext), mtime_ns: mtime_of(d), total: chunks.len() });
        }
    }
    let sched_all: Vec<(i64, usize)> = plan::schedule(&bodies, true).into_iter().flat_map(|(id, r)| r.map(move |p| (id, p))).collect();
    let inputs: Vec<(String, String)> = sched_all.iter().map(|(id, p)| (format!("{id}:{p}"), texts[&(*id, *p)].clone())).collect();
    let vs: HashMap<String, Vec<f32>> = cache.get_or_compute("chunk", inputs, |x| embed_all(&mut m, x)).into_iter().collect();
    let fill = |tail: bool| {
        let mut set = VectorSet::new(LAYOUT);
        // 제품 `chunked_contents` 처럼: 청크 벡터(슬롯)가 하나라도 있는 문서
        let mut chunked = HashSet::new();
        for (id, r) in plan::schedule(&bodies, tail) {
            for pos in r {
                if let Some(rec) = Record::from_full(&vs[&format!("{id}:{pos}")], LAYOUT) {
                    set.push(id, &rec);
                    chunked.insert(id);
                }
            }
        }
        (set, chunked)
    };
    let (chunks_head, chunked_head) = fill(false);
    let (chunks_all, chunked_all) = fill(true);
    // 문서 날짜: 제품처럼 수정 시각(나노초) → 한국 날짜(day_of_unix_ns).
    let dates: HashMap<i64, Day> = hd.iter().enumerate().filter(|(_, d)| d.date.is_some()).map(|(i, d)| (i as i64, day_of_unix_ns(mtime_of(d)))).collect();
    // 제목 연도: 제목(제목 채움을 되돌린 뒤) + 파일 이름(확장자 포함), 실패 문서는 파일 이름만. 빈 목록은 넣지 않는다(제품과 같게).
    let title_years: HashMap<i64, Vec<i32>> = hd
        .iter()
        .enumerate()
        .filter_map(|(i, d)| {
            let ys = query::title_years(&title_text(d.doc.as_ref().and_then(|x| x.title.as_deref()), &d.file));
            (!ys.is_empty()).then_some((i as i64, ys))
        })
        .collect();

    // 검색어: 제품처럼 query_texts(parse(…)).semantic 에 검색어 접두어를 붙여 임베딩한다. 벡터가 달라지는 것은 날짜 빼기
    // 두 값뿐이라 4가지 조합을 모두 만들어 두면 훑기가 낼 수 있는 모든 글이 들어 있다.
    let mut qtexts: Vec<(String, String)> = Vec::new();
    for q in queries {
        let pq = query::parse(&q.text, a.now, &table);
        for (sr, sa) in [(true, true), (true, false), (false, true), (false, false)] {
            let t = hybrid::query_texts(&pq, &HybridParams { strip_relative: sr, strip_absolute: sa, ..Default::default() }).semantic;
            qtexts.push((t.clone(), p.render_query(&t)));
        }
    }
    qtexts.sort();
    qtexts.dedup();
    let qvec: HashMap<String, Vec<f32>> = cache.get_or_compute("query", qtexts, |x| embed_all(&mut m, x)).into_iter().collect();

    World {
        docs: hd,
        kw,
        cards,
        chunks_head,
        chunks_all,
        meta_head: MapMeta { dates: dates.clone(), chunked: chunked_head, title_years: title_years.clone() },
        meta_all: MapMeta { dates, chunked: chunked_all, title_years },
        qvec,
        card_tokens,
        now: a.now,
        table,
    }
}

impl World {
    /// 질의 하나의 문서 id 순위(상위 100) — 제품 검색 코어 그대로.
    pub fn rank(&self, cfg: &Cfg, q: &Query) -> Vec<String> {
        let pq = query::parse(&q.text, self.now, &self.table);
        let t = hybrid::query_texts(&pq, &cfg.params);
        let qv = cfg.semantic.then(|| self.qvec.get(&t.semantic).map(|v| v.as_slice()).expect("검색어 벡터(날짜 빼기 4가지 조합에 없는 글)"));
        let (chunks, meta) = if cfg.tail { (&self.chunks_all, &self.meta_all) } else { (&self.chunks_head, &self.meta_head) };
        hybrid::search(&self.kw, &self.cards[&cfg.cap], chunks, &pq, qv, &cfg.params, self.now, meta, 100)
            .expect("검색")
            .iter()
            .map(|h| self.docs[h.id as usize].id.clone())
            .collect()
    }
}

/// 질의별 (nDCG@10, Recall@20, 첫 정답 순위)
type Per = HashMap<String, (f64, f64, Option<usize>)>;

fn evaluate(w: &World, cfg: &Cfg, qs: &[&Query]) -> Per {
    qs.iter()
        .map(|q| {
            let r = w.rank(cfg, q);
            (q.qid.clone(), (ndcg_at_k(&r, &q.relevant, 10), recall_at_k(&r, &q.relevant, 20), first_relevant_rank(&r, &q.relevant)))
        })
        .collect()
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
fn macro_ndcg(per: &Per, qs: &[&Query]) -> f64 {
    let mut by: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for q in qs {
        by.entry(q.kind.as_str()).or_default().push(per[&q.qid].0);
    }
    mean(by.values().map(|v| mean(v.iter().copied())))
}

fn kind_mean(per: &Per, qs: &[&Query], kind: &str) -> Option<f64> {
    let v: Vec<f64> = qs.iter().filter(|q| q.kind == kind).map(|q| per[&q.qid].0).collect();
    (!v.is_empty()).then(|| mean(v.into_iter()))
}

fn all_mean(per: &Per, qs: &[&Query]) -> f64 {
    mean(qs.iter().map(|q| per[&q.qid].0))
}

/// 첫 페이지 적중 질의 수: 정답이 하나라도 상위 10 안(사용자 원칙 2026-10-15).
fn fp_hits(per: &Per, qs: &[&Query]) -> usize {
    qs.iter().filter(|q| per[&q.qid].2.is_some_and(|r| r <= FIRST_PAGE)).count()
}

/// 첫 페이지 적중률(질의 비율)
fn fp_rate(per: &Per, qs: &[&Query]) -> f64 {
    fp_hits(per, qs) as f64 / qs.len().max(1) as f64
}

fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

/// 좌표 하강의 채택 규칙: 조정용 매크로 nDCG@10 이 EPS 넘게 오르고, 첫 페이지 적중 수가 줄지 않을 때만 바꾼다.
fn accepts(s: f64, hits: usize, best_s: f64, best_hits: usize) -> bool {
    s > best_s + EPS && hits >= best_hits
}

type Setter = Box<dyn Fn(&mut HybridParams)>;

/// 좌표 하강 축: (이름, [(값 표시, 바꾸기)])
fn axes() -> Vec<(&'static str, Vec<(String, Setter)>)> {
    fn ax<T: std::fmt::Debug + Copy + 'static>(name: &'static str, vals: &[T], set: fn(&mut HybridParams, T)) -> (&'static str, Vec<(String, Setter)>) {
        (name, vals.iter().map(|&x| (format!("{x:?}"), Box::new(move |p: &mut HybridParams| set(p, x)) as Setter)).collect())
    }
    vec![
        ax("rrf_k", &[10.0, 20.0, 30.0, 45.0, 60.0], |p, x| p.rrf_k = x),
        ax("keyword_max_chars", &SPLITS, |p, x| p.keyword_max_chars = x),
        ax("w_semantic[키워드형]", &[0.25, 0.5, 1.0, 1.5, 2.0], |p, x| p.w_semantic[0] = x),
        ax("w_semantic[의미형]", &[0.5, 1.0, 1.5, 2.0, 3.0], |p, x| p.w_semantic[1] = x),
        ax("combine", &[Combine::ChunkElseCard, Combine::Max, Combine::Separate, Combine::CardOnly], |p, x| p.combine = x),
        ax("floor", &[0.0f32, 0.2, 0.25, 0.3, 0.35, 0.4], |p, x| p.floor = x),
        ax("keyword.title", &[2.0f32, 3.0, 4.0], |p, x| p.keyword.title = x),
        ax("keyword.filename", &[1.5f32, 2.5, 3.5], |p, x| p.keyword.filename = x),
        ax("keyword.heading", &[1.0f32, 2.0, 3.0], |p, x| p.keyword.heading = x),
        ax("keyword.bigram", &[0.25f32, 0.5, 0.75, 1.0], |p, x| p.keyword.bigram = x),
        ax("date_exact", &[0.0, 0.5, 1.0, 2.0, 4.0], |p, x| p.date_exact = x),
        ax("date_vague", &[0.0, 0.1, 0.25, 0.5, 1.0], |p, x| p.date_vague = x),
        ax("date_title_conflict", &[0.0, 0.5, 1.0], |p, x| p.date_title_conflict = x),
        ax("date_recurring", &[0.0, 0.5], |p, x| p.date_recurring = x),
        ax("recency", &[0.0, 0.05, 0.1, 0.2], |p, x| p.recency = x),
        ax("phrase", &[0.0, 0.25, 0.5, 1.0], |p, x| p.phrase = x),
    ]
}

/// 조정용 매크로 nDCG 를 올리는 쪽으로 한 축씩(`accepts`). 한 회차를 다 돌아도 바뀐 것이 없을 때까지, 최대 MAX_PASSES 회차.
/// 반환: (고른 값, 돈 회차 수, 수렴했는가)
fn descend(w: &World, base: &Cfg, tune: &[&Query], trace: &mut Vec<String>) -> (HybridParams, usize, bool) {
    let score = |p: &HybridParams| {
        let per = evaluate(w, &Cfg { params: p.clone(), ..base.clone() }, tune);
        (macro_ndcg(&per, tune), fp_hits(&per, tune))
    };
    let mut best = base.params.clone();
    let (mut best_s, mut best_h) = score(&best);
    trace.push(format!("시작(기본값) nDCG {best_s:.4} · 첫 페이지 {best_h}/{}", tune.len()));
    let (mut passes, mut converged) = (0, false);
    for pass in 1..=MAX_PASSES {
        passes = pass;
        let mut changed = false;
        for (name, vals) in axes() {
            let mut cand: Option<(f64, usize, String, HybridParams)> = None;
            for (label, set) in &vals {
                let mut p = best.clone();
                set(&mut p);
                if p == best {
                    continue;
                }
                let (s, h) = score(&p);
                if accepts(s, h, best_s, best_h) && cand.as_ref().is_none_or(|c| s > c.0) {
                    cand = Some((s, h, label.clone(), p));
                }
            }
            if let Some((s, h, label, p)) = cand {
                trace.push(format!("{pass}회차: {name} = {label} → nDCG {s:.4} · 첫 페이지 {h}/{}", tune.len()));
                best = p;
                best_s = s;
                best_h = h;
                changed = true;
            }
        }
        if !changed {
            converged = true;
            break;
        }
    }
    (best, passes, converged)
}

/// 승·무·패(질의별 nDCG@10, 차이 1e-9 이하는 무)
fn wtl(a: &Per, b: &Per, qs: &[&Query]) -> (usize, usize, usize) {
    qs.iter().fold((0, 0, 0), |(w, t, l), q| {
        let d = a[&q.qid].0 - b[&q.qid].0;
        if d > 1e-9 {
            (w + 1, t, l)
        } else if d < -1e-9 {
            (w, t, l + 1)
        } else {
            (w, t + 1, l)
        }
    })
}

/// 검색어 유형 분류의 정답(스펙 §6.3, P7): keyword·abbrev 유형 = 키워드형, 나머지 = 의미형. 질의 유형은 질의를 만든 의도라
/// 분류기의 정답과 꼭 같지는 않다 — 보고서에서 참고용으로만 쓴다.
fn kind_truth(kind: &str) -> QueryKind {
    match kind {
        "keyword" | "abbrev" => QueryKind::Keyword,
        _ => QueryKind::Semantic,
    }
}

pub struct Outcome {
    pub report: String,
    pub params: HybridParams,
    pub cap: usize,
    pub ranks: BTreeMap<String, Vec<String>>,
}

pub fn run(w: &World, queries: &[Query], fixed: Option<HybridParams>) -> Outcome {
    let tune_ids = tune_split(queries.iter().map(|q| (q.qid.as_str(), q.kind.as_str())));
    let all: Vec<&Query> = queries.iter().collect();
    let tune: Vec<&Query> = queries.iter().filter(|q| tune_ids.contains(&q.qid)).collect();
    let hold: Vec<&Query> = queries.iter().filter(|q| !tune_ids.contains(&q.qid)).collect();
    let base = Cfg { params: HybridParams::default(), cap: plan::CARD_TOKENS, tail: true, semantic: true };
    let is_fixed = fixed.is_some();
    let mut trace = Vec::new();
    let (mut params, passes) = match fixed {
        Some(p) => {
            trace.push("고정 매개변수(--params) — 고르지 않고 결정 규칙도 적용하지 않음(표만 보인다)".into());
            (p, None)
        }
        None => {
            let (p, n, conv) = descend(w, &base, &tune, &mut trace);
            (p, Some((n, conv)))
        }
    };
    let mut s = String::new();

    // 날짜 빼기 규칙(DEC-051): 그 종류를 빼면 temporal 조정용 nDCG 가 떨어지면 빼지 않는다.
    let temporal: Vec<&Query> = tune.iter().copied().filter(|q| q.kind == "temporal").collect();
    let mut strip_notes = Vec::new();
    for (label, rel) in [("상대 표현", true), ("숫자 해", false)] {
        let with = |on: bool| {
            let mut p = params.clone();
            if rel {
                p.strip_relative = on;
            } else {
                p.strip_absolute = on;
            }
            let per = evaluate(w, &Cfg { params: p, ..base.clone() }, &temporal);
            (mean(per.values().map(|x| x.0)), fp_rate(&per, &temporal))
        };
        let ((on, on_fp), (off, off_fp)) = (with(true), with(false));
        let cur = if rel { params.strip_relative } else { params.strip_absolute };
        let (keep, why) = if is_fixed {
            (cur, "고정")
        } else if temporal.is_empty() {
            (cur, "판단 불가(조정용 temporal 질의 없음) — 그대로")
        } else if on + 1e-9 >= off {
            (true, "뺀다")
        } else {
            (false, "빼지 않는다(회귀)")
        };
        if rel {
            params.strip_relative = keep;
        } else {
            params.strip_absolute = keep;
        }
        strip_notes.push(format!("| {label} | {on:.4} · {} | {off:.4} · {} | {why} |", pct(on_fp), pct(off_fp)));
    }

    // 유형 나누기 규칙(사용자 결정 2026-10-15): 나누기(6·8·10자)는 나누지 않음(0)보다 조정용 매크로 nDCG 가 EPS 넘게 높고
    // 첫 페이지 적중이 줄지 않을 때만 남는다. 좌표 하강은 기본값(8)에서 출발한다 — 가중치가 키워드형·의미형 같으면 나누기는
    // 아무 차이가 없어 0 에서 출발하면 한 번도 움직이지 못하기 때문. 그래서 하강이 끝난 뒤 0 과 다시 견준다.
    let split_rows: Vec<(usize, Per)> = SPLITS
        .iter()
        .map(|&n| (n, evaluate(w, &Cfg { params: HybridParams { keyword_max_chars: n, ..params.clone() }, ..base.clone() }, &all)))
        .collect();
    let split_why = {
        let none = &split_rows[0].1;
        let cur = params.keyword_max_chars;
        let cur_per = &split_rows.iter().find(|(n, _)| *n == cur).map(|x| &x.1);
        if is_fixed {
            "고정".to_string()
        } else if cur == 0 {
            "좌표 하강이 0(나누지 않음)을 골랐다".to_string()
        } else if let Some(cp) = cur_per {
            if accepts(macro_ndcg(cp, &tune), fp_hits(cp, &tune), macro_ndcg(none, &tune), fp_hits(none, &tune)) {
                format!("{cur}자 나누기가 나누지 않음보다 뚜렷이 낫다 — 유지")
            } else {
                params.keyword_max_chars = 0;
                format!("{cur}자 나누기가 나누지 않음보다 뚜렷이 낫지 않다 — 0(나누지 않음)으로")
            }
        } else {
            // 후보 밖의 값(고정 값이 아닌데 SPLITS 밖은 생기지 않는다)
            format!("{cur}자 — 후보 밖, 그대로")
        }
    };
    let chosen = Cfg { params: params.clone(), cap: plan::CARD_TOKENS, tail: true, semantic: true };

    // 기준선과 판정
    let keyword_only = Cfg {
        params: HybridParams { date_exact: 0.0, date_vague: 0.0, recency: 0.0, phrase: 0.0, strip_relative: false, strip_absolute: false, ..HybridParams::default() },
        semantic: false,
        ..chosen.clone()
    };
    let semantic_only = |c: Combine| Cfg { params: HybridParams { w_keyword: [0.0, 0.0], combine: c, date_exact: 0.0, date_vague: 0.0, recency: 0.0, phrase: 0.0, ..params.clone() }, ..chosen.clone() };
    let no_date = Cfg { params: HybridParams { date_exact: 0.0, date_vague: 0.0, ..params.clone() }, ..chosen.clone() };
    let runs: Vec<(&str, Cfg)> = vec![
        ("키워드 단독(M2 경로)", keyword_only.clone()),
        ("카드 단독", semantic_only(Combine::CardOnly)),
        ("청크 단독(없으면 카드)", semantic_only(Combine::ChunkElseCard)),
        ("하이브리드(고른 값)", chosen.clone()),
        ("하이브리드 − 날짜 가산", no_date.clone()),
        ("하이브리드 − 하한", Cfg { params: HybridParams { floor: 0.0, ..params.clone() }, ..chosen.clone() }),
        ("하이브리드 − 데이터 뒷부분", Cfg { tail: false, ..chosen.clone() }),
        ("하이브리드 − 날짜 빼기", Cfg { params: HybridParams { strip_relative: false, strip_absolute: false, ..params.clone() }, ..chosen.clone() }),
    ];
    let per: Vec<Per> = runs.iter().map(|(_, c)| evaluate(w, c, &all)).collect();
    let kinds: Vec<String> = {
        let mut k: Vec<String> = queries.iter().map(|q| q.kind.clone()).collect();
        k.sort();
        k.dedup();
        k
    };

    writeln!(s, "# M2 하이브리드 평가 (자동 생성: `cargo run -p m0eval --release -- --hybrid --eval eval/public --eval eval/kra --eval eval/data`)\n").unwrap();
    writeln!(s, "- 문서 {}건 · 질의 {}개(조정용 {} · 검증용 {}) · 기준 시점 {} · 카드 상한 {} · 실험과 제품이 같은 검색 코어(index::hybrid)", w.docs.len(), queries.len(), tune.len(), hold.len(), format_ymd(w.now), plan::CARD_TOKENS).unwrap();
    writeln!(s, "- 첫 페이지 적중 = 정답 문서가 하나라도 상위 {FIRST_PAGE} 안(사용자 원칙: \"항상 최상위가 아니어도 첫 페이지에 정확한 결과를\"). 적중률 = 질의 비율.").unwrap();
    writeln!(s, "- 일치 검사(실험 = 제품): `docs/m2/parity.md` (`--parity <앱 덤프>` 로 만든다, DEC-052)\n").unwrap();

    writeln!(s, "## 결정 규칙(숫자로만, DEC-051)\n").unwrap();
    writeln!(s, "- 조정용(qid 해시, 유형마다 60%)으로만 고르고 검증용(40%)으로 판정한다. 목표 = 조정용 유형별 매크로 nDCG@10.").unwrap();
    writeln!(s, "- 좌표 하강: 한 축씩 후보를 견줘, 조정용 매크로 nDCG@10 이 지금보다 {EPS} 넘게 높고 **조정용 첫 페이지 적중 수가 줄지 않는** 후보 중 가장 높은 것으로 바꾼다. 한 회차를 다 돌아 바뀐 것이 없으면 멈춘다(상한 {MAX_PASSES}회차).").unwrap();
    writeln!(s, "- 유형 나누기(keyword_max_chars): 하강이 고른 문턱이 0(나누지 않음)보다 같은 규칙(+{EPS}, 첫 페이지 적중 줄지 않음)으로 낫지 않으면 0.").unwrap();
    writeln!(s, "- 날짜 표현 빼기: 종류마다, 빼면 조정용 temporal nDCG 가 떨어지면 빼지 않는다.").unwrap();
    writeln!(s, "- 카드 상한 128(결정 #27): 128에서 잘리는 카드가 25% 미만이면 판단 불가(256), 전체 nDCG −0.003 이하 또는 마사회 유형 −0.01 이하 하락이면 256, 아니면 128.").unwrap();
    writeln!(s, "- 데이터 뒷부분(본문 ③): 켰을 때 데이터 아닌 질의 매크로가 0.3%p 이상 떨어지면 기본 끔 안을 사용자에게 올린다.\n").unwrap();

    writeln!(s, "## 1. 합격 판정\n").unwrap();
    let (k_h, h_h) = (macro_ndcg(&per[0], &hold), macro_ndcg(&per[3], &hold));
    let (wn, tn, ln) = wtl(&per[3], &per[0], &hold);
    writeln!(s, "| 기준 | 값 | 판정 |\n|---|---|---|").unwrap();
    writeln!(
        s,
        "| 하이브리드 > 키워드 단독(검증용 매크로 nDCG@10) | {h_h:.4} vs {k_h:.4} · 첫 페이지 {} vs {} · 승 {wn} 무 {tn} 패 {ln} | {} |",
        pct(fp_rate(&per[3], &hold)),
        pct(fp_rate(&per[0], &hold)),
        if h_h > k_h { "통과" } else { "**실패**" }
    )
    .unwrap();
    let dates: Vec<&Query> = all.iter().copied().filter(|q| q.kind == "date").collect();
    let dates_h: Vec<&Query> = hold.iter().copied().filter(|q| q.kind == "date").collect();
    let (d_on, d_off) = (mean(dates.iter().map(|q| per[3][&q.qid].0)), mean(dates.iter().map(|q| per[4][&q.qid].0)));
    let (dh_on, dh_off) = (mean(dates_h.iter().map(|q| per[3][&q.qid].0)), mean(dates_h.iter().map(|q| per[4][&q.qid].0)));
    let (dw, dt, dl) = wtl(&per[3], &per[4], &dates);
    writeln!(
        s,
        "| 날짜 가산 켬 > 끔(date 전체 {} · 검증용 {}) | 전체 {d_on:.4} vs {d_off:.4} · 검증용 {dh_on:.4} vs {dh_off:.4} · 첫 페이지 {} vs {} · 승 {dw} 무 {dt} 패 {dl} | {} |",
        dates.len(),
        dates_h.len(),
        pct(fp_rate(&per[3], &dates)),
        pct(fp_rate(&per[4], &dates)),
        if d_on > d_off && dh_on >= dh_off { "통과" } else { "**실패**" }
    )
    .unwrap();
    let non_date: Vec<&Query> = all.iter().copied().filter(|q| q.kind != "date").collect();
    let (nd_on, nd_off) = (macro_ndcg(&per[3], &non_date), macro_ndcg(&per[4], &non_date));
    // 허용 폭 0.003 은 숫자 규칙이다(P20) — 나중에 DEC 로 기록한다.
    writeln!(
        s,
        "| 날짜 없는 질의가 떨어지지 않음(매크로, 허용 0.003) | {nd_on:.4} vs {nd_off:.4} · 첫 페이지 {} vs {} | {} |",
        pct(fp_rate(&per[3], &non_date)),
        pct(fp_rate(&per[4], &non_date)),
        if nd_on + 0.003 >= nd_off { "통과" } else { "**실패**" }
    )
    .unwrap();
    writeln!(s).unwrap();

    writeln!(s, "## 2. 유형별 nDCG@10 (전체 질의)\n").unwrap();
    writeln!(s, "| 설정 | 매크로(조정) | 매크로(검증) | 첫 페이지(조정) | 첫 페이지(검증) | {} |", kinds.join(" | ")).unwrap();
    writeln!(s, "|---|---|---|---|---|{}", "---|".repeat(kinds.len())).unwrap();
    for (i, (name, _)) in runs.iter().enumerate() {
        let cells: Vec<String> = kinds.iter().map(|k| kind_mean(&per[i], &all, k).map_or("-".into(), |x| format!("{x:.4}"))).collect();
        writeln!(s, "| {name} | {:.4} | {:.4} | {} | {} | {} |", macro_ndcg(&per[i], &tune), macro_ndcg(&per[i], &hold), pct(fp_rate(&per[i], &tune)), pct(fp_rate(&per[i], &hold)), cells.join(" | ")).unwrap();
    }
    writeln!(s, "\n- \"키워드 단독(M2 경로)\"은 제품에서 모델이 없을 때의 경로(검색어 벡터 없음, 날짜 보정 끔)다. `docs/m1/eval-keyword.md` 와 값이 다른 까닭: 파일 이름에 `(id)` 가 붙은 평가 복사본 이름을 쓰고, 추출 제목이 없어 매니페스트 제목으로 채웠던 문서의 제목을 비워 제품처럼 파일 이름을 쓴다.").unwrap();
    writeln!(s).unwrap();

    writeln!(s, "## 3. 고른 매개변수\n\n```json\n{}\n```\n", serde_json::to_string_pretty(&params).expect("json")).unwrap();
    match passes {
        Some((n, true)) => writeln!(s, "좌표 하강 기록({n}회차 — 마지막 회차는 바뀐 것 없음):\n").unwrap(),
        Some((n, false)) => writeln!(s, "좌표 하강 기록({n}회차 — **상한에 닿아 멈춤, 수렴 안 함**):\n").unwrap(),
        None => writeln!(s, "좌표 하강 기록:\n").unwrap(),
    }
    for t in &trace {
        writeln!(s, "- {t}").unwrap();
    }
    writeln!(s, "\n날짜 표현 빼기(조정용 temporal {}개 nDCG@10 · 첫 페이지 적중률, DEC-051):\n\n| 종류 | 뺌 | 안 뺌 | 결정 |\n|---|---|---|---|", temporal.len()).unwrap();
    for n in &strip_notes {
        writeln!(s, "{n}").unwrap();
    }
    writeln!(s, "\n유형 나누기 — 나누지 않음(0) vs 6·8·10자(다른 값은 고른 값):\n\n| 문턱 | 매크로 nDCG@10(조정) | 첫 페이지(조정) | 매크로 nDCG@10(검증) | 첫 페이지(검증) |\n|---|---|---|---|---|").unwrap();
    for (n, p) in &split_rows {
        let label = if *n == 0 { "0(나누지 않음)".to_string() } else { format!("{n}자") };
        writeln!(s, "| {label} | {:.4} | {} ({}/{}) | {:.4} | {} |", macro_ndcg(p, &tune), pct(fp_rate(p, &tune)), fp_hits(p, &tune), tune.len(), macro_ndcg(p, &hold), pct(fp_rate(p, &hold))).unwrap();
    }
    writeln!(s, "\n- **결정: keyword_max_chars = {}** ({split_why})", params.keyword_max_chars).unwrap();

    // 결정 #27: 카드 상한 128
    let with_cap = |cap: usize| evaluate(w, &Cfg { cap, ..chosen.clone() }, &all);
    let (c256, c128) = (with_cap(256), with_cap(128));
    let cards_n = w.card_tokens.iter().filter(|&&t| t > 0).count();
    let cut = w.card_tokens.iter().filter(|&&t| t > 128).count();
    let share = cut as f64 / cards_n.max(1) as f64;
    let overall = all_mean(&c128, &all) - all_mean(&c256, &all);
    let kra_drop: Vec<(String, f64)> = ["kra", "kra-deep", "kra-plain"]
        .iter()
        .filter_map(|k| Some((k.to_string(), kind_mean(&c128, &all, k)? - kind_mean(&c256, &all, k)?)))
        .collect();
    let long_q: Vec<&Query> = all.iter().copied().filter(|q| q.relevant.iter().any(|id| w.docs.iter().position(|d| &d.id == id).is_some_and(|i| w.card_tokens[i] > 128))).collect();
    let (cap, why) = if share < 0.25 {
        (256, format!("판단 불가 — 128에서 잘리는 카드 {:.1}% < 25%", share * 100.0))
    } else if overall <= -0.003 || kra_drop.iter().any(|(_, d)| *d <= -0.01) {
        (256, "하락 — 256 유지".to_string())
    } else {
        (128, "하락 없음 — 128 로 바꾼다".to_string())
    };
    writeln!(s, "\n## 4. 카드 상한 128 판정(결정 #27)\n").unwrap();
    writeln!(
        s,
        "- 잘리는 카드 {cut}/{cards_n} ({:.1}%) · 전체 nDCG 차이(128−256) {overall:+.4} · 첫 페이지 256 {} · 128 {} · 마사회 유형 {}",
        share * 100.0,
        pct(fp_rate(&c256, &all)),
        pct(fp_rate(&c128, &all)),
        kra_drop.iter().map(|(k, d)| format!("{k} {d:+.4}")).collect::<Vec<_>>().join(", ")
    )
    .unwrap();
    writeln!(
        s,
        "- 정답 문서 카드가 128토큰을 넘는 질의 {}개: nDCG@10 256 {:.4} · 128 {:.4} · 첫 페이지 256 {} · 128 {}",
        long_q.len(),
        mean(long_q.iter().map(|q| c256[&q.qid].0)),
        mean(long_q.iter().map(|q| c128[&q.qid].0)),
        pct(fp_rate(&c256, &long_q)),
        pct(fp_rate(&c128, &long_q))
    )
    .unwrap();
    writeln!(s, "- **결정: {cap}** ({why})").unwrap();

    // 데이터 형식 뒷부분(DEC-051)
    let non_data: Vec<&Query> = all.iter().copied().filter(|q| !q.kind.starts_with("data-")).collect();
    let nd_tail = macro_ndcg(&per[3], &non_data) - macro_ndcg(&per[6], &non_data);
    writeln!(s, "\n## 5. 데이터 형식 뒷부분(본문 ③)\n").unwrap();
    writeln!(s, "| 유형 | 켬 nDCG@10 | 끔 nDCG@10 | 켬 첫 페이지 | 끔 첫 페이지 |\n|---|---|---|---|---|").unwrap();
    for k in ["data-deep", "data-head"] {
        let qk: Vec<&Query> = all.iter().copied().filter(|q| q.kind == k).collect();
        let fp = |p: &Per| if qk.is_empty() { "-".to_string() } else { pct(fp_rate(p, &qk)) };
        writeln!(
            s,
            "| {k} | {} | {} | {} | {} |",
            kind_mean(&per[3], &all, k).map_or("-".into(), |x| format!("{x:.4}")),
            kind_mean(&per[6], &all, k).map_or("-".into(), |x| format!("{x:.4}")),
            fp(&per[3]),
            fp(&per[6])
        )
        .unwrap();
    }
    writeln!(
        s,
        "\n- 데이터 아닌 질의 매크로 차이(켬−끔) {nd_tail:+.4} · 첫 페이지 켬 {} · 끔 {} → {}",
        pct(fp_rate(&per[3], &non_data)),
        pct(fp_rate(&per[6], &non_data)),
        if nd_tail <= -0.003 { "**−0.3%p 이상 떨어짐 — 기본 끔 안을 사용자에게 올린다**" } else { "기본 켬 유지" }
    )
    .unwrap();

    writeln!(s, "\n## 6. 날짜 질의(첫 정답 순위, 10위 밖은 굵게)\n\n| 질의 | 검색어 | 해석 | 켬 | 끔 |\n|---|---|---|---|---|").unwrap();
    for q in &dates {
        let pq = query::parse(&q.text, w.now, &w.table);
        let chips: Vec<String> = pq
            .chips()
            .map(|c| {
                let r = c.range.expect("칩");
                format!("{}~{}{}", format_ymd(r.start), format_ymd(r.end), if c.yearless { "(해마다)" } else { "" })
            })
            .collect();
        let rk = |p: &Per| p[&q.qid].2.map_or("-".into(), |r| if r > FIRST_PAGE { format!("**{r}**") } else { r.to_string() });
        writeln!(s, "| {} | {} | {} | {} | {} |", q.qid, q.text, if chips.is_empty() { "(미래·없음)".into() } else { chips.join(", ") }, rk(&per[3]), rk(&per[4])).unwrap();
    }

    writeln!(s, "\n## 7. 검색어 유형 분류 정확도(참고용)\n").unwrap();
    writeln!(s, "정답: keyword·abbrev 유형 = 키워드형, 나머지 = 의미형(스펙 §6.3). 질의 유형은 만든 의도라 분류기의 정답과 꼭 같지 않다 — 결정에 쓰지 않는다.\n\n| 문턱 | 맞음 |\n|---|---|").unwrap();
    for n in SPLITS.iter().filter(|&&n| n > 0) {
        let ok = all.iter().filter(|q| query::parse(&q.text, w.now, &w.table).kind_with(*n) == kind_truth(&q.kind)).count();
        writeln!(s, "| {n}자 | {ok}/{} ({:.1}%) |", all.len(), ok as f64 * 100.0 / all.len().max(1) as f64).unwrap();
    }

    writeln!(s, "\n## 8. 키워드 단독보다 나빠진 질의(검증용)\n\n| 질의 | 유형 | 키워드 순위 | 하이브리드 순위 |\n|---|---|---|---|").unwrap();
    for q in &hold {
        if per[3][&q.qid].0 + 1e-9 < per[0][&q.qid].0 {
            let r = |p: &Per| p[&q.qid].2.map_or("-".into(), |x| x.to_string());
            writeln!(s, "| {} | {} | {} | {} |", q.qid, q.kind, r(&per[0]), r(&per[3])).unwrap();
        }
    }

    let final_cfg = Cfg { cap, ..chosen };
    let ranks: BTreeMap<String, Vec<String>> = all.iter().map(|q| (q.qid.clone(), w.rank(&final_cfg, q))).collect();
    Outcome { report: s, params, cap, ranks }
}

/// 일치 검사(DEC-052): 같은 매개변수(제품 기본값)로 m0eval 과 앱 `eval --set` 의 순위를 비교한다.
pub fn parity(w: &World, queries: &[Query], app_dump: &Path) -> (String, bool) {
    let app: BTreeMap<String, Vec<String>> = serde_json::from_str(&std::fs::read_to_string(app_dump).expect("앱 덤프")).expect("덤프 JSON");
    let cfg = Cfg { params: HybridParams::default(), cap: plan::CARD_TOKENS, tail: true, semantic: true };
    let (mut same, mut n, mut da, mut dm) = (0usize, 0usize, 0.0, 0.0);
    let (mut fa, mut fm) = (0usize, 0usize);
    let mut diff_q = Vec::new();
    for q in queries {
        let Some(a) = app.get(&q.qid) else { continue };
        let m = w.rank(&cfg, q);
        n += 1;
        let top = |v: &[String]| v.iter().take(10).cloned().collect::<Vec<_>>();
        if top(a) == top(&m) {
            same += 1;
        } else {
            diff_q.push(q.qid.clone());
        }
        da += ndcg_at_k(a, &q.relevant, 10);
        dm += ndcg_at_k(&m, &q.relevant, 10);
        fa += app::evalset::first_page_hit(a, &q.relevant) as usize;
        fm += app::evalset::first_page_hit(&m, &q.relevant) as usize;
    }
    let (na, nm) = (da / n.max(1) as f64, dm / n.max(1) as f64);
    let frac = same as f64 / n.max(1) as f64;
    let pass = (na - nm).abs() <= 0.002 && frac >= 0.98 && n == queries.len();
    let mut s = String::new();
    writeln!(s, "# M2 일치 검사 (실험 = 제품, DEC-052)\n").unwrap();
    writeln!(s, "| 항목 | 값 | 기준 |\n|---|---|---|").unwrap();
    writeln!(s, "| 비교한 질의 | {n}/{} | 전부 |", queries.len()).unwrap();
    writeln!(s, "| nDCG@10 앱 · m0eval | {na:.4} · {nm:.4} (차이 {:+.4}) | ≤ 0.002 |", na - nm).unwrap();
    writeln!(s, "| 첫 페이지 적중 앱 · m0eval | {fa} · {fm} | (참고) |").unwrap();
    writeln!(s, "| 상위 10개가 순서까지 같은 질의 | {same}/{n} ({:.1}%) | ≥ 98% |", frac * 100.0).unwrap();
    writeln!(s, "| 판정 | **{}** | |", if pass { "통과" } else { "실패" }).unwrap();
    if !diff_q.is_empty() {
        writeln!(s, "\n상위 10개가 다른 질의: {}", diff_q.join(", ")).unwrap();
    }
    (s, pass)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hd(id: &str, set: &str, file: &str, date: Option<&str>) -> HDoc {
        HDoc { id: id.into(), set: set.into(), file: file.into(), ext: "hwp".into(), date: date.and_then(query::calendar::parse_ymd), doc: None }
    }

    /// P2: 제품 수집 순서 — 수정 시각 최근 먼저, 같으면 "{세트}/{파일}" 오름차순, 날짜 없는 문서는 맨 뒤.
    #[test]
    fn docs_are_numbered_in_product_crawl_order() {
        let mut v = vec![
            hd("P2", "public", "나 (P2).hwp", Some("2025-01-01")),
            hd("K1", "kra", "가 (K1).hwp", Some("2025-01-01")),
            hd("X", "public", "x (X).hwp", None),
            hd("P1", "public", "가 (P1).hwp", Some("2026-03-01")),
            hd("D1", "data", "하 (D1).csv", Some("2025-01-01")),
        ];
        order_docs(&mut v);
        assert_eq!(v.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), vec!["P1", "D1", "K1", "P2", "X"]);
    }

    /// 제품 `Meta::title_texts` 와 같은 꼴: 제목(공백 제거)이 있으면 "{제목}\n{파일 이름}", 없으면 파일 이름.
    #[test]
    fn title_text_matches_meta_title_texts() {
        assert_eq!(title_text(Some(" 2025 경마시행계획 "), "x (K1).hwp"), "2025 경마시행계획\nx (K1).hwp");
        assert_eq!(title_text(Some("  "), "2025 시행계획 (K2).hwp"), "2025 시행계획 (K2).hwp");
        assert_eq!(title_text(None, "2024년 계획 (K3).hwp"), "2024년 계획 (K3).hwp");
        assert_eq!(query::title_years(&title_text(Some("경마시행계획"), "2025년 경마시행계획 (K4).hwp")), vec![2025], "제목에 해가 없어도 파일 이름의 해");
    }

    /// 좌표 하강 채택: EPS 넘게 오르고 첫 페이지 적중이 줄지 않을 때만.
    #[test]
    fn descent_accepts_only_clear_gains_that_keep_first_page() {
        assert!(accepts(0.5060, 10, 0.5, 10));
        assert!(!accepts(0.5050, 12, 0.5, 10), "EPS 이내");
        assert!(!accepts(0.60, 9, 0.5, 10), "첫 페이지 적중이 줄면 안 받는다");
        assert!(accepts(0.60, 11, 0.5, 10));
    }

    #[test]
    fn kind_truth_follows_spec() {
        assert_eq!(kind_truth("keyword"), QueryKind::Keyword);
        assert_eq!(kind_truth("abbrev"), QueryKind::Keyword);
        for k in ["semantic", "kra", "kra-deep", "date", "temporal", "data-deep"] {
            assert_eq!(kind_truth(k), QueryKind::Semantic, "{k}");
        }
    }
}
