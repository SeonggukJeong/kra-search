//! rhwp 파싱 → `doc::Document` 어댑터(본문 순회는 walk.rs). rhwp 타입은 extract 크레이트 밖으로 내보내지 않는다.

use crate::magic::{is_known_drm, magic_hex, sniff, Container};
use doc::{BlockKind, Document};

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
    let blocks = crate::walk::document_blocks(&parsed);
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
