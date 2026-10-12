//! rhwp 문서 모델 → `doc::Document` 어댑터. rhwp 타입은 이 파일 밖으로 내보내지 않는다.

use crate::magic::{is_known_drm, magic_hex, sniff, Container};
use doc::{Block, BlockKind, Document};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum FailKind {
    Encrypted,
    Drm,
    Corrupt,
    NotContainer,
    Unsupported,
}

pub struct Extracted {
    pub doc: Document,
    pub distribution: bool,
}

pub fn extract_hwp(bytes: &[u8]) -> Result<Extracted, (FailKind, String)> {
    if is_known_drm(bytes) {
        return Err((FailKind::Drm, magic_hex(bytes)));
    }
    if let Container::Other(_) = sniff(bytes) {
        return Err((FailKind::NotContainer, magic_hex(bytes)));
    }
    let parsed = rhwp::parser::parse_document(bytes).map_err(|e| (classify(&e), magic_hex(bytes)))?;
    // 제목 계층과 표 선형화는 rhwp RAG 청커를 그대로 쓴다(절 단위로 한 덩어리가 되도록 큰 예산).
    let opts = rhwp::rag::ChunkOptions { max_tokens: usize::MAX / 4, ..Default::default() };
    let mut blocks = Vec::new();
    let mut last_heading: Option<String> = None;
    for c in rhwp::rag::build_chunks(&parsed, &opts) {
        let heading = c.heading_path.last().map(|h| h.trim().to_string()).filter(|h| !h.is_empty());
        if heading.is_some() && heading != last_heading {
            blocks.push(Block { kind: BlockKind::Heading, text: heading.clone().unwrap() });
            last_heading = heading.clone();
        }
        let mut body = text_to_blocks(&c.text);
        if let (Some(h), Some(first)) = (&heading, body.first()) {
            if first.kind == BlockKind::Paragraph && first.text == *h {
                body.remove(0);
            }
        }
        blocks.extend(body);
    }
    let title = blocks.iter().find(|b| b.kind == BlockKind::Heading).map(|b| b.text.clone());
    Ok(Extracted { doc: Document { title, blocks }, distribution: parsed.header.distribution })
}

fn classify(e: &rhwp::parser::ParseError) -> FailKind {
    use rhwp::parser::ParseError as P;
    match e {
        P::EncryptedDocument | P::CryptoError(_) => FailKind::Encrypted,
        P::UnsupportedFormat { code, .. } if code.contains("DRM") => FailKind::Drm,
        P::UnsupportedFormat { .. } | P::Hwp3Error(_) | P::HmlError(_) => FailKind::Unsupported,
        _ => FailKind::Corrupt,
    }
}

fn is_private_use(c: char) -> bool {
    matches!(c as u32, 0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD)
}

/// rhwp 표 선형화의 병합 표시 `[병합 R×C]` 를 지운다.
fn strip_merge_marks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("[병합 ") {
        let tail = &rest[i + "[병합 ".len()..];
        match tail.find(']') {
            Some(j) if tail[..j].chars().all(|c| c.is_ascii_digit() || c == '×') => {
                out.push_str(&rest[..i]);
                rest = &tail[j + 1..];
            }
            _ => {
                out.push_str(&rest[..i + "[병합 ".len()]);
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 한컴 전용 글자(PUA)·병합 표시 제거, 연속 공백 정리.
fn clean(s: &str) -> String {
    let no_pua: String = s.chars().map(|c| if is_private_use(c) { ' ' } else { c }).collect();
    strip_merge_marks(&no_pua).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn table_cells(line: &str) -> Option<Vec<String>> {
    let t = line.trim();
    if t.len() >= 2 && t.starts_with('|') && t.ends_with('|') {
        Some(t[1..t.len() - 1].split('|').map(clean).collect())
    } else {
        None
    }
}

fn is_separator(cells: &[String]) -> bool {
    !cells.is_empty() && cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':'))
}

/// 청크 텍스트(문단 줄 + 마크다운 표)를 블록으로 바꾼다.
/// 표: 첫 줄 → TableHeader("열1 | 열2"), 구분선 생략, 이후 줄 → TableRow("열1: 값 | 열2: 값", 빈 칸 생략).
pub(crate) fn text_to_blocks(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut header: Option<Vec<String>> = None;
    for line in text.lines() {
        match table_cells(line) {
            Some(cells) => {
                if is_separator(&cells) {
                    continue;
                }
                match &header {
                    None => {
                        let text = cells.iter().filter(|c| !c.is_empty()).cloned().collect::<Vec<_>>().join(" | ");
                        if !text.is_empty() {
                            out.push(Block { kind: BlockKind::TableHeader, text });
                        }
                        header = Some(cells);
                    }
                    Some(h) => {
                        let row: Vec<String> = h
                            .iter()
                            .zip(&cells)
                            .filter(|(_, v)| !v.is_empty())
                            .map(|(k, v)| if k.is_empty() { v.clone() } else { format!("{k}: {v}") })
                            .collect();
                        if !row.is_empty() {
                            out.push(Block { kind: BlockKind::TableRow, text: row.join(" | ") });
                        }
                    }
                }
            }
            None => {
                header = None;
                let t = clean(line);
                if !t.is_empty() {
                    out.push(Block { kind: BlockKind::Paragraph, text: t });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::BlockKind::*;

    #[test]
    fn markdown_table_lines_become_header_and_rows() {
        let blocks = text_to_blocks("본문 문단\n| 구분 | 금액 |\n| --- | --- |\n| 인건비 | 100 |\n| 경비 |  |\n다음 문단");
        let kinds: Vec<_> = blocks.iter().map(|b| b.kind.clone()).collect();
        assert_eq!(kinds, vec![Paragraph, TableHeader, TableRow, TableRow, Paragraph]);
        assert_eq!(blocks[1].text, "구분 | 금액");
        assert_eq!(blocks[2].text, "구분: 인건비 | 금액: 100");
        assert_eq!(blocks[3].text, "구분: 경비");
    }

    #[test]
    fn private_use_glyphs_and_merge_marks_are_removed() {
        let blocks = text_to_blocks("본인\u{F081C}\u{F081C}\u{E000}  서명\n| 복학원서 [병합 1×4] |  |  |\n| 학번 [병합 3×1] | 2025 |");
        assert_eq!(blocks[0].text, "본인 서명");
        assert_eq!(blocks[1].text, "복학원서");
        assert_eq!(blocks[2].text, "복학원서: 학번 | 2025");
    }

    #[test]
    fn pua_only_line_is_dropped() {
        assert!(text_to_blocks("\u{F081C}\u{F081C}\u{F081C}").is_empty());
    }

    #[test]
    fn blank_lines_are_dropped() {
        assert!(text_to_blocks("\n  \n").is_empty());
    }
}
