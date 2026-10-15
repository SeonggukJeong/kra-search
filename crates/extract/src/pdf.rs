//! PDF 추출 — pdfium(bblanchon/pdfium-binaries)을 실행 파일 옆에서 **절대 경로**로 불러온다(스펙 §7.6).
//! pdfium 은 여러 스레드에서 동시에 쓰면 문서 열기가 무작위로 실패한다(조사 2026-10-14) → 문서 하나 전체를 뮤텍스로 감싼다.
//! 워커는 어차피 한 번에 파일 하나만 처리한다.

use crate::norm::{clean_lines, usable_title};
use crate::{FailKind, Stats};
use doc::{Block, BlockKind, Document};
use pdfium_render::prelude::*;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

#[cfg(windows)]
pub const LIB_NAME: &str = "pdfium.dll";
#[cfg(target_os = "macos")]
pub const LIB_NAME: &str = "libpdfium.dylib";
#[cfg(all(unix, not(target_os = "macos")))]
pub const LIB_NAME: &str = "libpdfium.so";

static PDFIUM: OnceLock<Mutex<Pdfium>> = OnceLock::new();
/// 처음 불러오기를 한 번에 하나만. pdfium-render 는 전역 바인딩이 이미 있으면 `bind_to_library` 가
/// `PdfiumLibraryBindingsAlreadyInitialized` 를 돌려주므로(`pdfium-render-0.9.4/src/pdfium.rs:176-190`),
/// 두 스레드가 동시에 `PDFIUM` 이 비었다고 보고 둘 다 불러오면 뒤의 것이 실패한다.
static INIT: Mutex<()> = Mutex::new(());

/// pdfium 라이브러리를 **절대 경로**로 불러온다. 프로세스당 한 번(두 번째부터는 Ok, 여러 스레드가 동시에 불러도 안전).
/// 오류 문장에 경로를 넣지 않는다.
pub fn init(lib: &Path) -> Result<(), String> {
    if PDFIUM.get().is_some() {
        return Ok(());
    }
    if !lib.is_absolute() {
        return Err("pdfium 경로는 절대 경로여야 합니다".into());
    }
    let _guard = INIT.lock().unwrap_or_else(|e| e.into_inner());
    if PDFIUM.get().is_some() {
        return Ok(()); // 기다리는 동안 다른 스레드가 불러 놓음
    }
    // 오류 값(LoadLibraryError)에는 경로가 들어 있다 — 진단 보고서로 새지 않게 고정 문장만 돌려준다(INC-010).
    let bindings = Pdfium::bind_to_library(lib).map_err(|_| "pdfium 을 불러오지 못했습니다".to_string())?;
    let _ = PDFIUM.set(Mutex::new(Pdfium::new(bindings)));
    Ok(())
}

/// 쪽 글자 품질(공백 제외 글자 수 기준).
#[derive(Default)]
struct Quality {
    chars: usize,
    hangul: usize,
    garbage: usize,        // U+FFFD, 사설 영역, 제어 문자
    latin1_letters: usize, // U+00C0~U+00FF(× ÷ 제외) — ToUnicode 없는 한글 글꼴이 흔히 이렇게 깨진다
}

fn quality(s: &str) -> Quality {
    let mut q = Quality::default();
    for c in s.chars().filter(|c| !c.is_whitespace()) {
        q.chars += 1;
        let u = c as u32;
        if (0xAC00..=0xD7A3).contains(&u) || (0x1100..=0x11FF).contains(&u) || (0x3130..=0x318F).contains(&u) {
            q.hangul += 1;
        }
        if c == '\u{FFFD}' || (0xE000..=0xF8FF).contains(&u) || c.is_control() {
            q.garbage += 1;
        }
        if (0xC0..=0xFF).contains(&u) && u != 0xD7 && u != 0xF7 {
            q.latin1_letters += 1;
        }
    }
    q
}

/// 쓸 만한 쪽인가: 글자가 있고, 깨진 글자가 10% 이하이며, 한글 없이 Latin-1 글자가 30% 넘지 않을 것.
/// 가운뎃점(·)·①②·‘’ 같은 공문서 기호는 의심 글자로 세지 않는다(실제 공문 PDF 136쪽에서 오판 0 확인).
fn page_ok(q: &Quality) -> bool {
    if q.chars == 0 {
        return false;
    }
    let r = |n: usize| n as f64 / q.chars as f64;
    !(q.chars >= 10 && (r(q.garbage) > 0.10 || (q.hangul == 0 && r(q.latin1_letters) > 0.30)))
}

pub(crate) fn pdf(bytes: &[u8]) -> Result<(Document, Stats), FailKind> {
    let Some(lock) = PDFIUM.get() else { return Err(FailKind::Unsupported) };
    let pdfium = lock.lock().unwrap_or_else(|e| e.into_inner());
    let doc = pdfium.load_pdf_from_byte_slice(bytes, None).map_err(|e| match e {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => FailKind::Encrypted,
        _ => FailKind::Corrupt,
    })?;
    let title = doc.metadata().get(PdfDocumentMetadataTagType::Title).and_then(|t| usable_title(t.value()));
    let mut stats = Stats::default();
    let mut blocks: Vec<Block> = Vec::new();
    // 글자가 `MAX_DOC_CHARS` 를 채우면 남은 쪽은 읽지 않는다(extract_bytes 가 정확히 자른다). pdf_pages 는 읽은 쪽 수.
    let mut chars = 0usize;
    for page in doc.pages().iter() {
        if chars >= crate::MAX_DOC_CHARS {
            stats.truncated = true;
            break;
        }
        stats.pdf_pages += 1;
        let text = page.text().map(|t| t.all()).unwrap_or_default();
        if page_ok(&quality(&text)) {
            let t = clean_lines(&text.replace("\r\n", "\n"));
            if !t.is_empty() {
                chars += t.chars().count();
                blocks.push(Block { kind: BlockKind::Paragraph, text: t });
                continue;
            }
        }
        stats.pdf_bad_pages += 1;
    }
    if blocks.is_empty() {
        return Err(FailKind::NoText);
    }
    Ok((Document { title, blocks }, stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn korean_punctuation_is_not_garbage() {
        assert!(page_ok(&quality("Ⅲ. 추진 계획 · 세부 ① ‘가’ 항목 2026. 10.")));
        assert!(!page_ok(&quality("ÄÅÇÉ ÑÖÜá àâäãåç éèêëíìîï")));
        assert!(!page_ok(&quality("\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{e}\u{f}")));
        assert!(!page_ok(&quality("   ")));
    }
}
