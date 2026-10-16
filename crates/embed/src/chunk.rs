//! 문서 카드(빠른 의미 검색용)와 구조 기반 본문 청크(정밀 의미 검색용) 생성.
//! 토큰 수는 호출자가 주는 `count` 함수로 센다(실제: 모델 토크나이저, 테스트: 글자 수).

use doc::{BlockKind, Document};

pub struct ChunkParams {
    pub target_tokens: usize,
    pub max_tokens: usize,
    pub max_chunks: usize,
}

/// 본문 청크 설정 — M0 평가로 정한 크기 300(docs/m0/results.md "본문 청크 크기", 평가와 같이 최대 +100).
/// 진단(kra-diag)과 앱(M2)이 같은 값을 쓴다 — 크기를 바꾸면 진단 수치도 함께 바뀐다(DEC-045).
pub const BODY_CHUNK: ChunkParams = ChunkParams { target_tokens: 300, max_tokens: 400, max_chunks: 2000 };

/// `text`를 토큰 한도 이하가 되도록 글자 경계에서 자른다. 앞에서부터 창(한도 × 4글자, 모자라면 두 배씩)만 세고
/// 그 안에서 이진 탐색한다 — 15만 자 한 문단에서 매번 글 전체를 토큰화하던 문제(KI-001).
fn cut_to(text: &str, max_tokens: usize, count: &dyn Fn(&str) -> usize) -> String {
    let idx: Vec<usize> = text.char_indices().map(|(i, _)| i).chain([text.len()]).collect();
    let n = idx.len() - 1;
    let mut w = (max_tokens.max(1) * 4).min(n);
    loop {
        if count(&text[..idx[w]]) > max_tokens {
            break;
        }
        if w == n {
            return text.to_string();
        }
        w = (w * 2).min(n);
    }
    let (mut lo, mut hi) = (0usize, w);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if count(&text[..idx[mid]]) <= max_tokens {
            lo = mid
        } else {
            hi = mid - 1
        }
    }
    text[..idx[lo]].to_string()
}

/// 제목 + 소제목들 + 첫 문단(없으면 첫 표 머리글·첫 행), 토큰 ≤ max_tokens. 내용이 없으면 None.
pub fn build_card(doc: &Document, max_tokens: usize, count: &dyn Fn(&str) -> usize) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(t) = doc.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        parts.push(t.to_string());
    }
    for b in &doc.blocks {
        if b.kind == BlockKind::Heading {
            let t = b.text.trim();
            if !t.is_empty() && parts.first().map(String::as_str) != Some(t) {
                parts.push(t.to_string());
            }
        }
    }
    match doc.blocks.iter().find(|b| b.kind == BlockKind::Paragraph && !b.text.trim().is_empty()) {
        Some(p) => parts.push(p.text.trim().to_string()),
        // 문단이 없으면(표·시트) 첫 표 머리글과 첫 행으로 대신한다(DEC-057).
        None => {
            for kind in [BlockKind::TableHeader, BlockKind::TableRow] {
                if let Some(b) = doc.blocks.iter().find(|b| b.kind == kind && !b.text.trim().is_empty()) {
                    parts.push(b.text.trim().to_string());
                }
            }
        }
    }
    if parts.is_empty() {
        return None;
    }
    let card = cut_to(&parts.join("\n"), max_tokens, count);
    (!card.trim().is_empty()).then_some(card)
}

/// 문장 경계(. / 줄바꿈) 우선으로 한도 이하 조각으로 나눈다.
fn split_long(text: &str, max: usize, count: &dyn Fn(&str) -> usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let head = cut_to(rest, max, count);
        if head.len() == rest.len() {
            out.push(rest.to_string());
            break;
        }
        let mut cut = head.rfind(['.', '\n']).map(|i| i + 1).filter(|i| *i > head.len() / 2).unwrap_or(head.len());
        if cut == 0 {
            // 한도가 글자 하나보다 작아도 반드시 앞으로 나아간다(무한 반복 방지).
            cut = rest.chars().next().map_or(rest.len(), char::len_utf8);
        }
        out.push(rest[..cut].trim().to_string());
        rest = rest[cut..].trim_start();
    }
    out
}

/// 본문 청크와 "소제목으로 시작하는가" 표시(목차·소제목 청크를 먼저 임베딩, 스펙 §6.6).
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub text: String,
    pub heading: bool,
}

/// 구조 기반 청크. 표가 이어지는 청크에는 표 머리글을 다시 붙인다. 최대 max_chunks개.
pub fn build_chunks(doc: &Document, p: &ChunkParams, count: &dyn Fn(&str) -> usize) -> Vec<String> {
    build_chunks_marked(doc, p, count).into_iter().map(|c| c.text).collect()
}

pub fn build_chunks_marked(doc: &Document, p: &ChunkParams, count: &dyn Fn(&str) -> usize) -> Vec<Chunk> {
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut cur_heading = false;
    let mut table_header: Option<String> = None;
    let flush = |cur: &mut Vec<String>, cur_heading: &mut bool, chunks: &mut Vec<Chunk>| {
        if cur.iter().any(|s| !s.trim().is_empty()) {
            chunks.push(Chunk { text: cur.join("\n"), heading: *cur_heading });
        }
        cur.clear();
        *cur_heading = false;
    };
    for b in &doc.blocks {
        let text = b.text.trim();
        if text.is_empty() {
            continue;
        }
        match b.kind {
            BlockKind::Heading => {
                flush(&mut cur, &mut cur_heading, &mut chunks);
                table_header = None;
                cur.push(text.to_string());
                cur_heading = true;
            }
            BlockKind::TableHeader => {
                flush(&mut cur, &mut cur_heading, &mut chunks);
                // 반복해서 붙일 머리글은 한도의 1/3 이내로 줄인다. 원래 머리글이 길면 그 전문은 따로 청크로 남긴다.
                let short = cut_to(text, p.max_tokens / 3, count);
                if short.len() < text.len() {
                    chunks.extend(split_long(text, p.max_tokens, count).into_iter().map(|t| Chunk { text: t, heading: false }));
                }
                if short.trim().is_empty() {
                    table_header = None;
                } else {
                    cur.push(short.clone());
                    table_header = Some(short);
                }
            }
            BlockKind::Paragraph | BlockKind::TableRow => {
                if b.kind == BlockKind::Paragraph {
                    table_header = None;
                }
                let reserve = table_header.as_deref().map(count).unwrap_or(0) + 1;
                for piece in split_long(text, p.max_tokens.saturating_sub(reserve).max(1), count) {
                    let candidate = if cur.is_empty() { piece.clone() } else { format!("{}\n{}", cur.join("\n"), piece) };
                    if !cur.is_empty() && count(&candidate) > p.target_tokens {
                        flush(&mut cur, &mut cur_heading, &mut chunks);
                        if let (BlockKind::TableRow, Some(h)) = (&b.kind, &table_header) {
                            cur.push(h.clone());
                        }
                    }
                    cur.push(piece);
                }
            }
        }
        if chunks.len() >= p.max_chunks {
            break;
        }
    }
    flush(&mut cur, &mut cur_heading, &mut chunks);
    chunks.truncate(p.max_chunks);
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::{Block, BlockKind::*, Document};
    fn b(k: doc::BlockKind, t: &str) -> Block {
        Block { kind: k, text: t.into() }
    }
    fn chars(s: &str) -> usize {
        s.chars().count()
    }

    #[test]
    fn card_has_title_headings_and_first_paragraph() {
        let d = Document {
            title: Some("마권 발매 보고".into()),
            blocks: vec![b(Heading, "Ⅰ. 개요"), b(Paragraph, "첫 문단."), b(Heading, "Ⅱ. 원인"), b(Paragraph, "둘째 문단.")],
        };
        assert_eq!(build_card(&d, 1000, &chars).unwrap(), "마권 발매 보고\nⅠ. 개요\nⅡ. 원인\n첫 문단.");
    }
    #[test]
    fn card_respects_token_limit() {
        let d = Document { title: Some("제목".into()), blocks: vec![b(Paragraph, &"가".repeat(500))] };
        assert!(chars(&build_card(&d, 50, &chars).unwrap()) <= 50);
    }
    #[test]
    fn card_none_for_empty_document() {
        assert!(build_card(&Document::default(), 256, &chars).is_none());
        let d = Document { title: Some("  ".into()), blocks: vec![b(Paragraph, "   ")] };
        assert!(build_card(&d, 256, &chars).is_none());
    }
    #[test]
    fn paragraphs_pack_to_target() {
        let d = Document { title: None, blocks: (0..10).map(|_| b(Paragraph, &"가".repeat(30))).collect() };
        let c = build_chunks(&d, &ChunkParams { target_tokens: 100, max_tokens: 120, max_chunks: 100 }, &chars);
        assert!(c.len() >= 3);
        assert!(c.iter().all(|x| chars(x) <= 120));
    }
    #[test]
    fn heading_starts_new_chunk() {
        let d = Document { title: None, blocks: vec![b(Paragraph, "앞"), b(Heading, "Ⅱ. 원인"), b(Paragraph, "뒤")] };
        let c = build_chunks(&d, &ChunkParams { target_tokens: 100, max_tokens: 120, max_chunks: 10 }, &chars);
        assert_eq!(c, vec!["앞".to_string(), "Ⅱ. 원인\n뒤".to_string()]);
    }
    #[test]
    fn table_header_repeats_in_each_chunk() {
        let mut blocks = vec![b(TableHeader, "구분 | 금액")];
        for i in 0..20 {
            blocks.push(b(TableRow, &format!("구분: 항목{i:02} | 금액: {i}00")));
        }
        let d = Document { title: None, blocks };
        let c = build_chunks(&d, &ChunkParams { target_tokens: 80, max_tokens: 100, max_chunks: 100 }, &chars);
        assert!(c.len() > 1);
        assert!(c.iter().all(|x| x.starts_with("구분 | 금액\n")));
    }
    #[test]
    fn splits_oversized_block() {
        let d = Document { title: None, blocks: vec![b(Paragraph, &"가나다라마. ".repeat(200))] };
        let c = build_chunks(&d, &ChunkParams { target_tokens: 100, max_tokens: 120, max_chunks: 100 }, &chars);
        assert!(c.len() > 5 && c.iter().all(|x| chars(x) <= 120));
    }
    /// 실제 토크나이저처럼 빈 문자열도 특수 토큰 2개로 센다.
    fn chars_plus_special(s: &str) -> usize {
        s.chars().count() + 2
    }

    #[test]
    fn long_table_header_terminates_and_respects_max() {
        let header = "열".repeat(600);
        let mut blocks = vec![b(TableHeader, &header)];
        for i in 0..3 {
            blocks.push(b(TableRow, &format!("행{i}: 값")));
        }
        let d = Document { title: None, blocks };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let c = build_chunks(&d, &ChunkParams { target_tokens: 80, max_tokens: 100, max_chunks: 2000 }, &chars_plus_special);
            let _ = tx.send(c);
        });
        let c = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("build_chunks did not finish");
        assert!(c.len() < 50, "{} chunks", c.len());
        assert!(c.iter().all(|x| chars_plus_special(x) <= 100), "oversized chunk");
    }

    #[test]
    fn split_long_always_makes_progress() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(split_long("가나다라마바사", 2, &chars_plus_special));
        });
        let parts = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("split_long did not finish");
        assert_eq!(parts.concat(), "가나다라마바사");
    }

    #[test]
    fn max_chunks_caps_output() {
        let d = Document { title: None, blocks: (0..50).map(|_| b(Paragraph, &"가".repeat(100))).collect() };
        assert_eq!(build_chunks(&d, &ChunkParams { target_tokens: 100, max_tokens: 120, max_chunks: 7 }, &chars).len(), 7);
    }

    #[test]
    fn marked_chunks_know_heading_starts() {
        let d = Document { title: None, blocks: vec![b(Paragraph, "앞"), b(Heading, "Ⅱ. 원인"), b(Paragraph, "뒤")] };
        let c = build_chunks_marked(&d, &ChunkParams { target_tokens: 100, max_tokens: 120, max_chunks: 10 }, &chars);
        assert_eq!(c, vec![Chunk { text: "앞".into(), heading: false }, Chunk { text: "Ⅱ. 원인\n뒤".into(), heading: true }]);
        assert_eq!(build_chunks(&d, &ChunkParams { target_tokens: 100, max_tokens: 120, max_chunks: 10 }, &chars), vec!["앞", "Ⅱ. 원인\n뒤"]);
    }

    /// KI-001: 15만 자 한 문단을 자를 때 매번 글 전체를 세지 않는다(창 안에서만 센다).
    #[test]
    fn huge_paragraph_is_cut_inside_a_window() {
        let text = "가나다라마바사아자차 ".repeat(15_000);
        let d = Document { title: None, blocks: vec![b(Paragraph, &text)] };
        let longest = std::cell::Cell::new(0usize);
        let count = |s: &str| {
            longest.set(longest.get().max(s.len()));
            s.chars().count() + 2
        };
        let t = std::time::Instant::now();
        let c = build_chunks(&d, &BODY_CHUNK, &count);
        assert!(c.len() > 300, "{}", c.len());
        assert!(longest.get() < 20_000, "센 글이 너무 김: {}바이트", longest.get());
        assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
    }

    #[test]
    fn card_uses_table_header_and_first_row_without_paragraphs() {
        let d = Document { title: None, blocks: vec![b(TableHeader, "번호 | 사업장"), b(TableRow, "번호: 1 | 사업장: 서울"), b(TableRow, "번호: 2 | 사업장: 제주")] };
        assert_eq!(build_card(&d, 1000, &chars).unwrap(), "번호 | 사업장\n번호: 1 | 사업장: 서울");
    }
}
