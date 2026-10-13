//! M0 실험: 품질(양자화×차원×청크 크기×점수 방식), 이진 선별 재현율, 토큰 통계, 처리 속도.

use crate::cache::{get_or_compute, Vectors};
use crate::corpus::{EvalDoc, Query};
use crate::metrics::{ndcg_at_k, recall_at_k};
use embed::chunk::{build_card, build_chunks, ChunkParams};
use embed::engine::{Embedder, EngineOptions};
use embed::profile::ModelProfile;
use embed::vector::truncate_normalize;
use index::flat::FlatIndex;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::path::Path;
use std::time::Instant;

pub const DIMS: [usize; 3] = [768, 512, 256];
const CARD_TOKENS: usize = 256;

pub struct ModelRun {
    pub name: String,
    pub cards: Vectors,
    pub chunks: HashMap<usize, Vectors>, // 청크 크기 → (문서 id, 벡터) — 같은 id 여러 개
    pub queries: Vectors,                // (qid, 벡터)
    pub tokens_per_sec: f64,
    pub token_ratio: Vec<f64>, // 문서별 1,000자당 토큰
    pub chunks_per_doc: Vec<usize>,
}

fn embed_all(m: &mut Embedder, items: Vec<(String, String)>, tokens: &mut usize, secs: &mut f64) -> Vectors {
    let t = Instant::now();
    let mut out = Vec::with_capacity(items.len());
    for batch in items.chunks(8) {
        let texts: Vec<String> = batch.iter().map(|x| x.1.clone()).collect();
        let es = m.embed(&texts).expect("embed");
        for ((id, _), e) in batch.iter().zip(es) {
            *tokens += e.n_tokens;
            out.push((id.clone(), e.vector));
        }
    }
    *secs += t.elapsed().as_secs_f64();
    out
}

pub fn run_model(model_path: &Path, docs: &[EvalDoc], queries: &[Query], threads: usize, sizes: &[usize]) -> ModelRun {
    let name = model_path.file_name().unwrap().to_string_lossy().to_string();
    let p = ModelProfile::embeddinggemma2();
    let mut m = Embedder::load(model_path, &EngineOptions { threads, max_tokens: 1024, backend_dir: None }).expect("load model");
    let (mut tokens, mut secs) = (0usize, 0f64);

    let card_inputs: Vec<(String, String)> = {
        let count = |s: &str| m.token_count(s).unwrap_or(s.chars().count());
        docs.iter()
            .filter_map(|d| build_card(&d.doc, CARD_TOKENS, &count).map(|c| (d.id.clone(), p.render_doc(d.doc.title.as_deref(), &c))))
            .collect()
    };
    let cards = get_or_compute(&name, "card", card_inputs, |i| embed_all(&mut m, i, &mut tokens, &mut secs));

    let mut chunks = HashMap::new();
    let mut chunks_per_doc = Vec::new();
    for &size in sizes {
        let inputs: Vec<(String, String)> = {
            let count = |s: &str| m.token_count(s).unwrap_or(s.chars().count());
            let mut v = Vec::new();
            for d in docs {
                let cs = build_chunks(&d.doc, &ChunkParams { target_tokens: size, max_tokens: size + 100, max_chunks: 2000 }, &count);
                if size == 400 {
                    chunks_per_doc.push(cs.len());
                }
                v.extend(cs.into_iter().map(|c| (d.id.clone(), p.render_doc(d.doc.title.as_deref(), &c))));
            }
            v
        };
        chunks.insert(size, get_or_compute(&name, &format!("chunk{size}"), inputs, |i| embed_all(&mut m, i, &mut tokens, &mut secs)));
    }
    let q_inputs: Vec<(String, String)> = queries.iter().map(|q| (q.qid.clone(), p.render_query(&q.text))).collect();
    let qv = get_or_compute(&name, "query", q_inputs, |i| embed_all(&mut m, i, &mut tokens, &mut secs));

    let token_ratio = docs
        .iter()
        .filter_map(|d| {
            let text: String = d.doc.blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
            let chars = text.chars().count();
            (chars >= 200).then(|| m.token_count(&text).unwrap_or(0) as f64 * 1000.0 / chars as f64)
        })
        .collect();

    ModelRun {
        name,
        cards,
        chunks,
        queries: qv,
        tokens_per_sec: if secs > 0.0 { tokens as f64 / secs } else { 0.0 },
        token_ratio,
        chunks_per_doc,
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Strategy {
    Card,
    Chunk,
    Combined,
}
impl Strategy {
    fn label(self) -> &'static str {
        match self {
            Strategy::Card => "카드",
            Strategy::Chunk => "청크",
            Strategy::Combined => "카드+청크",
        }
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn norm_all(v: &Vectors, dim: usize) -> Vec<(String, Vec<f32>)> {
    v.iter().filter_map(|(id, x)| truncate_normalize(x, dim).map(|n| (id.clone(), n))).collect()
}

/// 질의별 문서 순위(문서 id 목록).
fn rank(run: &ModelRun, dim: usize, chunk_size: usize, s: Strategy) -> HashMap<String, Vec<String>> {
    let cards = norm_all(&run.cards, dim);
    let chunks = norm_all(&run.chunks[&chunk_size], dim);
    let mut out = HashMap::new();
    for (qid, qv) in &run.queries {
        let q = truncate_normalize(qv, dim).unwrap();
        let mut score: HashMap<&str, (f32, f32)> = HashMap::new(); // (카드, 최고 청크)
        for (id, v) in &cards {
            score.entry(id).or_insert((f32::MIN, f32::MIN)).0 = dot(&q, v);
        }
        for (id, v) in &chunks {
            let e = score.entry(id).or_insert((f32::MIN, f32::MIN));
            e.1 = e.1.max(dot(&q, v));
        }
        let mut docs: Vec<(&str, f32)> = score
            .into_iter()
            .map(|(id, (c, k))| {
                let v = match s {
                    Strategy::Card => c,
                    Strategy::Chunk => k,
                    Strategy::Combined if c == f32::MIN => k,
                    Strategy::Combined if k == f32::MIN => c,
                    Strategy::Combined => (c + k) / 2.0,
                };
                (id, v)
            })
            .collect();
        docs.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
        out.insert(qid.clone(), docs.into_iter().map(|x| x.0.to_string()).collect());
    }
    out
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 }
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() { 0.0 } else { sorted[((sorted.len() - 1) as f64 * p).round() as usize] }
}

pub fn quality_table(runs: &[ModelRun], queries: &[Query]) -> String {
    let mut s = String::new();
    // 질의 유형 열은 질의 파일에 나온 순서대로(평가 세트를 더하면 열도 늘어난다).
    let mut kinds: Vec<&str> = Vec::new();
    for q in queries {
        if !kinds.contains(&q.kind.as_str()) {
            kinds.push(&q.kind);
        }
    }
    writeln!(s, "| 모델 | 차원 | 청크 | 점수 | nDCG@10 전체 | {} | Recall@20 |", kinds.join(" | ")).unwrap();
    writeln!(s, "|---|---|---|---|---|{}---|", "---|".repeat(kinds.len())).unwrap();
    let mut rows: Vec<(f64, String)> = Vec::new();
    for run in runs {
        let mut sizes: Vec<usize> = run.chunks.keys().copied().collect();
        sizes.sort();
        for dim in DIMS {
            for &size in &sizes {
                for st in [Strategy::Card, Strategy::Chunk, Strategy::Combined] {
                    if st == Strategy::Card && size != 400 {
                        continue; // 카드 단독은 청크 크기와 무관
                    }
                    let ranked = rank(run, dim, size, st);
                    let mut by_kind: HashMap<&str, Vec<f64>> = HashMap::new();
                    let (mut all, mut rec) = (Vec::new(), Vec::new());
                    for q in queries {
                        let r = &ranked[&q.qid];
                        let n = ndcg_at_k(r, &q.relevant, 10);
                        all.push(n);
                        rec.push(recall_at_k(r, &q.relevant, 20));
                        by_kind.entry(q.kind.as_str()).or_default().push(n);
                    }
                    let per_kind: String = kinds
                        .iter()
                        .map(|name| format!(" {:.4} |", mean(by_kind.get(name).map(Vec::as_slice).unwrap_or(&[]))))
                        .collect();
                    let size_label = if st == Strategy::Card { "-".to_string() } else { size.to_string() };
                    rows.push((
                        mean(&all),
                        format!(
                            "| {} | {} | {} | {} | **{:.4}** |{} {:.4} |",
                            run.name, dim, size_label, st.label(), mean(&all), per_kind, mean(&rec)
                        ),
                    ));
                }
            }
        }
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, r) in rows {
        writeln!(s, "{r}").unwrap();
    }
    s
}

pub fn binary_recall_table(run: &ModelRun) -> String {
    let chunks = &run.chunks[&400];
    let n = chunks.len();
    let mut s = String::new();
    writeln!(s, "본문 청크(400) {n}개, 검색어 {}개. 기준: int8 256차원 완전 스캔 상위 10.\n", run.queries.len()).unwrap();
    writeln!(s, "| 이진 비트 | 방식 | 후보 0.5% | 후보 1% | 후보 2% | 후보 5% |").unwrap();
    writeln!(s, "|---|---|---|---|---|---|").unwrap();
    for bit in [256usize, 768] {
        let mut ix = FlatIndex::new(256, bit);
        for (_, v) in chunks {
            ix.push(v);
        }
        for method in ["hamming", "asym"] {
            let mut cells = Vec::new();
            for frac in [0.005, 0.01, 0.02, 0.05] {
                let cand = ((n as f64 * frac).round() as usize).max(10);
                let mut overlaps = Vec::new();
                for (_, q) in &run.queries {
                    let exact: HashSet<u32> = ix.search_exact(q, 10).into_iter().map(|x| x.0).collect();
                    let got = if method == "hamming" { ix.search_hamming(q, cand, 10) } else { ix.search_asym(q, cand, 10) };
                    let hit = got.iter().filter(|x| exact.contains(&x.0)).count();
                    overlaps.push(hit as f64 / exact.len().max(1) as f64);
                }
                cells.push(format!("{:.3} ({cand}개)", mean(&overlaps)));
            }
            writeln!(s, "| {bit} | {method} | {} |", cells.join(" | ")).unwrap();
        }
    }
    s
}

pub fn token_table(run: &ModelRun) -> String {
    let mut r = run.token_ratio.clone();
    r.sort_by(f64::total_cmp);
    let mut c: Vec<f64> = run.chunks_per_doc.iter().map(|x| *x as f64).collect();
    c.sort_by(f64::total_cmp);
    format!(
        "| 항목 | 중앙값 | p90 | 최대 |\n|---|---|---|---|\n| 한글 1,000자당 토큰 | {:.0} | {:.0} | {:.0} |\n| 문서당 본문 청크(400) | {:.0} | {:.0} | {:.0} |\n",
        pct(&r, 0.5), pct(&r, 0.9), r.last().copied().unwrap_or(0.0), pct(&c, 0.5), pct(&c, 0.9), c.last().copied().unwrap_or(0.0)
    )
}
