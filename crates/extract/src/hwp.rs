//! rhwp 파싱 → `doc::Document` 어댑터(본문 순회는 walk.rs). rhwp 타입은 extract 크레이트 밖으로 내보내지 않는다.

pub use crate::FailKind;
use crate::magic::magic_hex;
use crate::{Extracted, Stats};
use doc::{BlockKind, Document};

/// HWP·HWPX·HWP 3.x·HWPML 모두 rhwp 가 형식을 감지해 읽는다.
pub fn extract_hwp(bytes: &[u8]) -> Result<Extracted, (FailKind, String)> {
    let parsed = rhwp::parser::parse_document(bytes).map_err(|e| (classify(&e), magic_hex(bytes)))?;
    let blocks = crate::walk::document_blocks(&parsed);
    let title = blocks.iter().find(|b| b.kind == BlockKind::Heading).map(|b| b.text.clone());
    let stats = Stats { article_numbered_paras: article_numbered_paras(&parsed), ..Stats::default() };
    Ok(Extracted { doc: Document { title, blocks }, distribution: parsed.header.distribution, stats })
}

fn classify(e: &rhwp::parser::ParseError) -> FailKind {
    use rhwp::parser::ParseError as P;
    match e {
        P::EncryptedDocument | P::CryptoError(_) => FailKind::Encrypted,
        P::UnsupportedFormat { code, .. } if code.contains("DRM") => FailKind::Drm,
        // 빈 파일·알 수 없는 형식 = 한글 문서가 아님
        P::UnsupportedFormat { .. } => FailKind::NotContainer,
        P::Hwp3Error(_) | P::HmlError(_) => FailKind::Unsupported,
        _ => FailKind::Corrupt,
    }
}

/// "제N조" 꼴 자동 번호를 쓰는 본문 문단 수. 번호 형식 문자열(예: "제^1조")에 '조'가 있는 개요·문단 번호를 센다.
/// KI-011(자동 번호 미추출) 을 고칠지 판단하는 진단 통계 — 내용은 남기지 않는다.
pub(crate) fn article_numbered_paras(d: &rhwp::model::document::Document) -> u32 {
    use rhwp::model::style::HeadType;
    let mut n = 0;
    for s in &d.sections {
        for p in &s.paragraphs {
            let Some(ps) = d.doc_info.para_shapes.get(p.para_shape_id as usize) else { continue };
            let id = match ps.head_type {
                HeadType::Outline if ps.numbering_id == 0 => s.section_def.outline_numbering_id,
                HeadType::Outline | HeadType::Number => ps.numbering_id,
                _ => continue,
            };
            let Some(num) = (id as usize).checked_sub(1).and_then(|i| d.doc_info.numberings.get(i)) else { continue };
            if num.level_formats.get(ps.para_level as usize).is_some_and(|f| f.contains('조')) {
                n += 1;
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use rhwp::model::document::{Document, Section};
    use rhwp::model::paragraph::Paragraph;
    use rhwp::model::style::{HeadType, Numbering, ParaShape};

    #[test]
    fn counts_article_style_auto_numbering() {
        let mut d = Document::default();
        d.doc_info.para_shapes.push(ParaShape::default()); // 0: 번호 없음
        d.doc_info.para_shapes.push(ParaShape { head_type: HeadType::Number, numbering_id: 1, para_level: 0, ..ParaShape::default() });
        d.doc_info.para_shapes.push(ParaShape { head_type: HeadType::Number, numbering_id: 2, para_level: 0, ..ParaShape::default() });
        let mut article = Numbering::default();
        article.level_formats[0] = "제^1조".into();
        let mut plain = Numbering::default();
        plain.level_formats[0] = "^1.".into();
        d.doc_info.numberings = vec![article, plain];
        let para = |shape: u16| Paragraph { text: "본문".into(), para_shape_id: shape, ..Paragraph::new_empty() };
        d.sections.push(Section { paragraphs: vec![para(0), para(1), para(1), para(2)], ..Section::default() });
        assert_eq!(super::article_numbered_paras(&d), 2);
    }
}
