//! 의미 검색 색인(스펙 §6.2 의 2·3단계, DEC-046): 카드 → 본문 ①②③(embed::plan). 묶음마다 벡터 파일에 쓰고(fsync)
//! 그다음 meta 에 기록한다 — 사이에서 꺼져도 다음 실행이 꼬리를 자르고 이어서 한다(DEC-048).
//! 입력 하나가 실패하면 그 단위만 "없음"(슬롯 없음)으로 기록하고 계속한다(M2 설계 §3.5).

use crate::pipeline::IndexError;
use crate::store::Store;
use crate::vectors::{self, VecStore, LAYOUT};
use embed::api::Embed;
use embed::chunk::{build_card, build_chunks_marked, Chunk};
use embed::plan::{self, BodyDoc, CardDoc, FormatClass};
use embed::profile::ModelProfile;
use index::meta::{EmbedCounts, Meta, MetaError, VecKind};
use platform::paths::Home;
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;
use vector::record::Record;

/// 한 번에 임베딩하는 입력 수
const BATCH: usize = 16;
/// 이만큼 모이면 파일 → meta 로 커밋
const COMMIT: usize = 64;
/// 본문 조각 사이에 들고 있는 청크 글의 바이트 합 상한(`ChunkCache`).
const CHUNK_CACHE_BYTES: usize = 32 << 20;

#[derive(Debug, Clone, Default)]
pub struct EmbedOptions {
    /// 본문 ③(데이터 형식 뒷부분)
    pub data_tail: bool,
    /// 시험용: n 번째 커밋에서 파일에 쓴 뒤 meta 기록 전에 멈춘다(강제 종료 흉내)
    pub stop_after_append: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct EmbedSummary {
    pub cards: usize,
    pub cards_empty: usize,
    pub chunks: usize,
    pub failed: usize,
    pub secs_cards: f64,
    pub secs_body: f64,
    pub stopped: bool,
}

pub fn ext_of(path: &str) -> String {
    Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// 카드·청크 머리의 제목: 문서 제목, 없으면 파일 이름(스펙 §6.6). m0eval 도 같은 규칙.
pub fn title_of(doc: &doc::Document, path: &str) -> String {
    doc.title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .unwrap_or_else(|| Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string())
}

/// 진행 한 줄(UI D7 용어).
pub fn embed_line(c: &EmbedCounts) -> String {
    let tail = if c.tail_total > 0 { format!(" · 데이터 파일 뒷부분 {}/{}", c.tail_done, c.tail_total) } else { String::new() };
    format!("빠른 의미 검색 {}/{} · 정밀 의미 검색 {}/{}{tail}", c.cards_done, c.cards_total, c.main_done, c.main_total)
}

/// 입력이 있는 것만 묶어 임베딩. 묶음이 실패하면 하나씩 다시 해 실패한 것만 None.
fn embed_each(e: &mut dyn Embed, inputs: &[Option<String>], failed: &mut usize) -> Vec<Option<Vec<f32>>> {
    let texts: Vec<String> = inputs.iter().flatten().cloned().collect();
    if texts.is_empty() {
        // 모두 빈 카드면 엔진을 부르지 않는다.
        return vec![None; inputs.len()];
    }
    let got: Vec<Option<Vec<f32>>> = match e.embed(&texts) {
        Ok(v) if v.len() == texts.len() => v.into_iter().map(Some).collect(),
        _ => texts
            .iter()
            .map(|t| match e.embed(std::slice::from_ref(t)) {
                Ok(mut v) if v.len() == 1 => v.pop(),
                _ => {
                    *failed += 1;
                    None
                }
            })
            .collect(),
    };
    let mut it = got.into_iter();
    inputs.iter().map(|x| if x.is_some() { it.next().flatten() } else { None }).collect()
}

/// 커밋을 기다리는 단위: (열쇠, 레코드 — None 이면 빈 카드·청크 또는 실패). 열쇠는 카드면 내용 id, 청크면 (내용 id, 위치).
type Pending<K> = Vec<(K, Option<Record>)>;

/// meta 에 행과 파일 길이를 한 트랜잭션으로 기록하는 함수(`Meta::set_cards`·`record_chunks`).
type RecordFn<K> = fn(&mut Meta, &[(K, Option<u64>)], u64) -> Result<(), MetaError>;

fn record_chunks(meta: &mut Meta, rows: &[((i64, usize), Option<u64>)], len: u64) -> Result<(), MetaError> {
    let rows: Vec<(i64, usize, Option<u64>)> = rows.iter().map(|&((id, pos), slot)| (id, pos, slot)).collect();
    meta.add_chunks(&rows, len)
}

struct Committer<'a> {
    commits: usize,
    opt: &'a EmbedOptions,
}

impl Committer<'_> {
    /// 카드·청크 공통: 레코드를 파일에 쓰고(fsync) meta 에 기록한다(DEC-048 순서). 시험용 멈춤 지점이면 true(meta 기록 없이).
    fn commit<K: Copy>(&mut self, meta: &mut Meta, vs: &mut VecStore, kind: VecKind, pending: &mut Pending<K>, record: RecordFn<K>) -> Result<bool, IndexError> {
        if pending.is_empty() {
            return Ok(false);
        }
        let file = vs.file(kind);
        let recs: Vec<Record> = pending.iter().filter_map(|x| x.1.clone()).collect();
        let mut slot = file.append(&recs).map_err(|e| IndexError::Other(format!("의미 검색 색인 파일 오류: {e}")))?;
        self.commits += 1;
        if self.opt.stop_after_append == Some(self.commits) {
            return Ok(true);
        }
        let rows: Vec<(K, Option<u64>)> = pending
            .iter()
            .map(|(k, r)| {
                (*k, r.as_ref().map(|_| {
                    slot += 1;
                    slot - 1
                }))
            })
            .collect();
        record(meta, &rows, file.len())?;
        pending.clear();
        Ok(false)
    }
}

/// 본문을 나눈 결과(우선순위 순서·제목 포함).
struct Prepared {
    chunks: Vec<Chunk>,
    order: Vec<usize>,
    title: String,
    bytes: usize,
}

/// 본문 조각(50청크)마다 문서 전체를 실제 토크나이저로 다시 나누지 않도록, 조각이 남은 문서의 청크를 들고 있는다.
/// 같은 내용(content id)·같은 토크나이저면 청크가 같으므로 결과는 다시 나눈 것과 같다. 저사양 PC 를 위해 글 바이트 합에
/// 상한을 둔다 — 넘치면 담지 않고 그 문서는 다음 조각에서 다시 나눈다. 문서의 마지막 조각을 하면 버린다.
#[derive(Default)]
struct ChunkCache {
    map: HashMap<i64, Prepared>,
    bytes: usize,
}

impl ChunkCache {
    fn take(&mut self, id: i64) -> Option<Prepared> {
        let p = self.map.remove(&id)?;
        self.bytes -= p.bytes;
        Some(p)
    }

    /// `more` = 이 문서의 조각이 뒤에 더 남았는가.
    fn keep(&mut self, id: i64, p: Prepared, more: bool) {
        if more && self.bytes + p.bytes <= CHUNK_CACHE_BYTES {
            self.bytes += p.bytes;
            self.map.insert(id, p);
        }
    }
}

pub fn run_embedding(
    store: &mut Store,
    home: &Home,
    e: &mut dyn Embed,
    fp: &str,
    profile: &ModelProfile,
    opt: &EmbedOptions,
    progress: &mut dyn FnMut(&EmbedCounts),
) -> Result<EmbedSummary, IndexError> {
    let mut vs = vectors::open_for_write(home, &mut store.meta, fp)?;
    let mut s = EmbedSummary::default();
    let mut c = Committer { commits: 0, opt };

    // ---- 카드: 문서 형식 먼저, 최근 수정 먼저(결정 #27)
    let t = Instant::now();
    let rows = store.meta.card_todo()?;
    let path_of: HashMap<i64, String> = rows.iter().map(|r| (r.id, r.path.clone())).collect();
    let mut order: Vec<CardDoc> = rows.iter().map(|r| CardDoc { id: r.id, class: plan::class_of_ext(&ext_of(&r.path)), mtime_ns: r.mtime_ns }).collect();
    plan::card_order(&mut order);
    let mut pending: Pending<i64> = Vec::new();
    for group in order.chunks(BATCH) {
        let mut inputs = Vec::with_capacity(group.len());
        {
            let count = |x: &str| e.count_tokens(x);
            for d in group {
                let input = store.meta.doc(d.id)?.and_then(|doc| {
                    build_card(&doc, plan::CARD_TOKENS, &count).map(|card| profile.render_doc(Some(&title_of(&doc, &path_of[&d.id])), &card))
                });
                inputs.push(input);
            }
        }
        let vecs = embed_each(e, &inputs, &mut s.failed);
        for (d, v) in group.iter().zip(vecs) {
            let rec = v.and_then(|v| Record::from_full(&v, LAYOUT));
            if rec.is_some() {
                s.cards += 1;
            } else {
                s.cards_empty += 1;
            }
            pending.push((d.id, rec));
        }
        if pending.len() >= COMMIT {
            if c.commit(&mut store.meta, &mut vs, VecKind::Cards, &mut pending, Meta::set_cards)? {
                s.stopped = true;
                return Ok(s);
            }
            progress(&store.meta.embed_counts(plan::DATA_HEAD)?);
        }
    }
    if c.commit(&mut store.meta, &mut vs, VecKind::Cards, &mut pending, Meta::set_cards)? {
        s.stopped = true;
        return Ok(s);
    }
    s.secs_cards = t.elapsed().as_secs_f64();

    // ---- 본문: 청크 수부터 센다(진행률 분모, 토큰화만)
    let t = Instant::now();
    let todo = store.meta.body_todo()?;
    let mut plans: Vec<(i64, usize, bool)> = Vec::new();
    for r in &todo {
        let ext = ext_of(&r.path);
        let total = match store.meta.doc(r.id)? {
            Some(doc) => {
                let count = |x: &str| e.count_tokens(x);
                build_chunks_marked(&doc, &plan::chunk_params(&ext), &count).len()
            }
            None => 0,
        };
        plans.push((r.id, total, plan::class_of_ext(&ext) == FormatClass::Data));
        if plans.len() >= 200 {
            store.meta.set_bodies(&plans)?;
            plans.clear();
        }
    }
    store.meta.set_bodies(&plans)?;
    progress(&store.meta.embed_counts(plan::DATA_HEAD)?);

    // ---- 본문 ①②③
    let bodies = store.meta.bodies()?;
    let body_path: HashMap<i64, String> = bodies.iter().map(|b| (b.id, b.path.clone())).collect();
    let docs: Vec<BodyDoc> = bodies
        .iter()
        .map(|b| BodyDoc { id: b.id, class: if b.data { FormatClass::Data } else { FormatClass::Document }, mtime_ns: b.mtime_ns, total: b.total })
        .collect();
    let sched = plan::schedule(&docs, opt.data_tail);
    // 문서마다 마지막 조각의 순번 — 그 뒤로는 청크를 들고 있지 않는다.
    let last: HashMap<i64, usize> = sched.iter().enumerate().map(|(i, (id, _))| (*id, i)).collect();
    let mut cache = ChunkCache::default();
    let mut pending: Pending<(i64, usize)> = Vec::new();
    for (i, (id, range)) in sched.into_iter().enumerate() {
        let more = last[&id] > i;
        let cached = cache.take(id);
        let done = store.meta.chunk_positions(id)?;
        let todo_pos: Vec<usize> = range.filter(|p| !done.contains(p)).collect();
        if todo_pos.is_empty() {
            if let Some(p) = cached {
                cache.keep(id, p, more);
            }
            continue;
        }
        let p = match cached {
            Some(p) => p,
            None => {
                let Some(doc) = store.meta.doc(id)? else { continue };
                let path = &body_path[&id];
                let chunks = {
                    let count = |x: &str| e.count_tokens(x);
                    build_chunks_marked(&doc, &plan::chunk_params(&ext_of(path)), &count)
                };
                let order = plan::priority_order(&chunks);
                let bytes = chunks.iter().map(|c| c.text.len()).sum();
                Prepared { chunks, order, title: title_of(&doc, path), bytes }
            }
        };
        for group in todo_pos.chunks(BATCH) {
            let inputs: Vec<Option<String>> = group.iter().map(|&q| p.order.get(q).map(|&k| profile.render_doc(Some(&p.title), &p.chunks[k].text))).collect();
            let vecs = embed_each(e, &inputs, &mut s.failed);
            for (&q, v) in group.iter().zip(vecs) {
                let rec = v.and_then(|v| Record::from_full(&v, LAYOUT));
                if rec.is_some() {
                    s.chunks += 1;
                }
                pending.push(((id, q), rec));
            }
            if pending.len() >= COMMIT {
                if c.commit(&mut store.meta, &mut vs, VecKind::Chunks, &mut pending, record_chunks)? {
                    s.stopped = true;
                    return Ok(s);
                }
                progress(&store.meta.embed_counts(plan::DATA_HEAD)?);
            }
        }
        cache.keep(id, p, more);
    }
    if c.commit(&mut store.meta, &mut vs, VecKind::Chunks, &mut pending, record_chunks)? {
        s.stopped = true;
        return Ok(s);
    }
    s.secs_body = t.elapsed().as_secs_f64();
    vectors::compact_if_needed(&mut store.meta, &mut vs)?;
    progress(&store.meta.embed_counts(plan::DATA_HEAD)?);
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared(bytes: usize) -> Prepared {
        Prepared { chunks: vec![], order: vec![], title: String::new(), bytes }
    }

    #[test]
    fn chunk_cache_drops_after_last_piece_and_respects_budget() {
        let mut c = ChunkCache::default();
        c.keep(1, prepared(10), true);
        assert_eq!(c.bytes, 10);
        let p = c.take(1).expect("남은 조각이 있으면 들고 있다");
        assert_eq!(c.bytes, 0);
        c.keep(1, p, false);
        assert!(c.take(1).is_none(), "마지막 조각 뒤에는 버린다");
        c.keep(2, prepared(CHUNK_CACHE_BYTES), true);
        c.keep(3, prepared(1), true);
        assert!(c.take(3).is_none(), "상한을 넘으면 담지 않는다");
        assert_eq!(c.bytes, CHUNK_CACHE_BYTES);
    }

    #[test]
    fn title_falls_back_to_file_stem() {
        let d = |t: Option<&str>| doc::Document { title: t.map(String::from), blocks: vec![] };
        assert_eq!(title_of(&d(Some(" 계획 ")), "/a/b.hwp"), "계획");
        assert_eq!(title_of(&d(Some("  ")), "/a/점검기록.csv"), "점검기록");
        assert!(title_of(&d(None), "C:\\문서\\메모.txt").ends_with("메모"), "Windows 경로도 파일 이름으로 끝난다");
    }
}
