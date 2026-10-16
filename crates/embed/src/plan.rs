//! 임베딩 순서와 범위(결정 #27·#28, 스펙 §6.6): 카드는 문서 형식·최근 수정 먼저,
//! 본문은 ① 문서 형식 전체(50청크 조각을 문서끼리 번갈아) → ② 데이터 형식 앞 20청크 → ③ 데이터 형식 나머지(끌 수 있음).
//! 청크 "위치(pos)" = `priority_order` 결과 안의 순번(소제목 청크가 앞). 이 값들이 바뀌면 벡터를 다시 만들어야 하므로 `tag()` 에 넣는다.

use crate::chunk::{Chunk, ChunkParams, BODY_CHUNK};
use std::ops::Range;

/// 문서 카드 상한(토큰). 결정 #27 — M2 평가(Task 16)가 128 로 바꿀지 정한다.
pub const CARD_TOKENS: usize = 256;
/// 데이터 형식 앞부분(본문 ②) 청크 수
pub const DATA_HEAD: usize = 20;
/// 공정 스케줄링 조각(스펙 §6.6)
pub const PIECE: usize = 50;
/// 표 데이터(xlsx·ods·csv) 문서당 청크 상한(스펙 §6.6)
pub const TABLE_CAP: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FormatClass {
    Document,
    Data,
}

/// 확장자 → 묶음. 모르는 확장자는 문서 형식으로 본다.
pub fn class_of_ext(ext: &str) -> FormatClass {
    match ext.to_ascii_lowercase().as_str() {
        "txt" | "csv" | "xlsx" | "ods" => FormatClass::Data,
        _ => FormatClass::Document,
    }
}

/// 형식별 본문 청크 설정(크기는 M0 결정, 상한은 표 200 · 나머지 2,000).
pub fn chunk_params(ext: &str) -> ChunkParams {
    let max_chunks = match ext.to_ascii_lowercase().as_str() {
        "xlsx" | "ods" | "csv" => TABLE_CAP,
        _ => BODY_CHUNK.max_chunks,
    };
    ChunkParams { target_tokens: BODY_CHUNK.target_tokens, max_tokens: BODY_CHUNK.max_tokens, max_chunks }
}

/// 임베딩할 순서: 소제목으로 시작하는 청크 먼저(각 묶음 안에서는 원래 순서).
pub fn priority_order(chunks: &[Chunk]) -> Vec<usize> {
    let mut v: Vec<usize> = (0..chunks.len()).collect();
    v.sort_by_key(|&i| !chunks[i].heading);
    v
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardDoc {
    pub id: i64,
    pub class: FormatClass,
    pub mtime_ns: i64,
}

/// 문서 형식 먼저, 그 안에서 최근 수정 먼저(같으면 id).
pub fn card_order(v: &mut [CardDoc]) {
    v.sort_by(|a, b| a.class.cmp(&b.class).then(b.mtime_ns.cmp(&a.mtime_ns)).then(a.id.cmp(&b.id)));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyDoc {
    pub id: i64,
    pub class: FormatClass,
    pub mtime_ns: i64,
    /// 상한을 적용한 청크 수
    pub total: usize,
}

fn rounds(out: &mut Vec<(i64, Range<usize>)>, docs: &[&BodyDoc], from: usize) {
    let mut r = 0;
    loop {
        let mut any = false;
        for d in docs {
            let a = from + r * PIECE;
            if a < d.total {
                out.push((d.id, a..(a + PIECE).min(d.total)));
                any = true;
            }
        }
        if !any {
            break;
        }
        r += 1;
    }
}

/// 본문 임베딩 순서: (내용 id, 위치 범위). 이미 한 위치는 부르는 쪽이 건너뛴다.
pub fn schedule(docs: &[BodyDoc], data_tail: bool) -> Vec<(i64, Range<usize>)> {
    let mut sorted: Vec<&BodyDoc> = docs.iter().collect();
    sorted.sort_by(|a, b| b.mtime_ns.cmp(&a.mtime_ns).then(a.id.cmp(&b.id)));
    let (doc, data): (Vec<&BodyDoc>, Vec<&BodyDoc>) = sorted.into_iter().partition(|d| d.class == FormatClass::Document);
    let mut out = Vec::new();
    rounds(&mut out, &doc, 0);
    for d in &data {
        if d.total > 0 {
            out.push((d.id, 0..d.total.min(DATA_HEAD)));
        }
    }
    if data_tail {
        rounds(&mut out, &data, DATA_HEAD);
    }
    out
}

/// 벡터 내용을 바꾸는 값들 — 임베딩 지문에 넣는다(DEC-047).
pub fn tag() -> String {
    format!(
        "card={CARD_TOKENS};card_v=2;chunk={}/{};doc_cap={};table_cap={TABLE_CAP};head={DATA_HEAD};order=heading-first",
        BODY_CHUNK.target_tokens, BODY_CHUNK.max_tokens, BODY_CHUNK.max_chunks
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_and_caps() {
        for e in ["hwp", "HWPX", "pdf", "docx", "pptx", "odt", "odp"] {
            assert_eq!(class_of_ext(e), FormatClass::Document, "{e}");
        }
        for e in ["txt", "csv", "xlsx", "ods"] {
            assert_eq!(class_of_ext(e), FormatClass::Data, "{e}");
        }
        assert_eq!(chunk_params("xlsx").max_chunks, 200);
        assert_eq!(chunk_params("csv").max_chunks, 200);
        assert_eq!(chunk_params("txt").max_chunks, 2000);
        assert_eq!(chunk_params("hwp").max_chunks, 2000);
        assert_eq!((chunk_params("hwp").target_tokens, chunk_params("hwp").max_tokens), (300, 400));
    }

    #[test]
    fn heading_chunks_first_and_stable() {
        let c = |t: &str, h: bool| Chunk { text: t.into(), heading: h };
        assert_eq!(priority_order(&[c("a", false), c("b", true), c("c", false), c("d", true)]), vec![1, 3, 0, 2]);
    }

    #[test]
    fn cards_documents_first_then_recent() {
        let mut v = vec![
            CardDoc { id: 1, class: FormatClass::Data, mtime_ns: 900 },
            CardDoc { id: 2, class: FormatClass::Document, mtime_ns: 100 },
            CardDoc { id: 3, class: FormatClass::Document, mtime_ns: 500 },
            CardDoc { id: 4, class: FormatClass::Document, mtime_ns: 500 },
        ];
        card_order(&mut v);
        assert_eq!(v.iter().map(|d| d.id).collect::<Vec<_>>(), vec![3, 4, 2, 1]);
    }

    #[test]
    fn schedule_three_tiers() {
        let docs = [
            BodyDoc { id: 1, class: FormatClass::Document, mtime_ns: 10, total: 120 },
            BodyDoc { id: 2, class: FormatClass::Document, mtime_ns: 20, total: 30 },
            BodyDoc { id: 3, class: FormatClass::Data, mtime_ns: 30, total: 75 },
            BodyDoc { id: 4, class: FormatClass::Data, mtime_ns: 5, total: 10 },
            BodyDoc { id: 5, class: FormatClass::Document, mtime_ns: 99, total: 0 },
        ];
        assert_eq!(
            schedule(&docs, true),
            vec![(2, 0..30), (1, 0..50), (1, 50..100), (1, 100..120), (3, 0..20), (4, 0..10), (3, 20..70), (3, 70..75)]
        );
        assert_eq!(schedule(&docs, false), vec![(2, 0..30), (1, 0..50), (1, 50..100), (1, 100..120), (3, 0..20), (4, 0..10)]);
    }

    #[test]
    fn tag_names_every_value_that_changes_vectors() {
        let t = tag();
        let expect = [
            format!("card={CARD_TOKENS}"),
            "card_v=2".to_string(),
            format!("chunk={}/{}", BODY_CHUNK.target_tokens, BODY_CHUNK.max_tokens),
            format!("doc_cap={}", BODY_CHUNK.max_chunks),
            format!("table_cap={TABLE_CAP}"),
            format!("head={DATA_HEAD}"),
        ];
        for x in &expect {
            assert!(t.contains(x.as_str()), "{t}");
        }
    }
}
