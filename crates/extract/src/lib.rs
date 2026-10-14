//! 파일 → `doc::Document`. 형식별 추출기는 이 크레이트 밖으로 파서 타입을 내보내지 않는다.

pub mod cli;
pub mod coverage;
pub mod format;
pub mod hwp;
pub mod magic;
pub(crate) mod norm;
pub mod pdf;
pub mod runner;
pub mod worker;
pub(crate) mod table;
mod odf;
mod ooxml;
mod sheet;
mod text;
mod walk;
pub(crate) mod xmlutil;

use format::Format;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum FailKind {
    Encrypted,
    Drm,
    Corrupt,
    NotContainer,
    Unsupported,
    /// 파일이 100MB 초과(`format::MAX_FILE_BYTES`) — 내용을 읽지 않음. 이 경우에만 쓴다.
    TooLarge,
    /// 파일은 작아도 내부 상한을 넘음(압축 해제한 XML 파트 64MB·zip 합계 256MB·시트 글자 64MB·ODS 200만 칸 등) — 파일명만 색인
    TooComplex,
    /// 글자 층이 없거나 깨짐(스캔 PDF·ToUnicode 없는 PDF 등) — 파일명만 색인
    NoText,
    /// 읽기 오류(권한 없음·다른 프로그램이 잠금·사라짐)
    Unreadable,
}

impl FailKind {
    /// meta.db 에 저장하는 고정 코드. 변형을 더하면 여기도 더해야 컴파일된다(`_` 갈래 없음).
    pub fn code(&self) -> &'static str {
        match self {
            FailKind::Encrypted => "Encrypted",
            FailKind::Drm => "Drm",
            FailKind::Corrupt => "Corrupt",
            FailKind::NotContainer => "NotContainer",
            FailKind::Unsupported => "Unsupported",
            FailKind::TooLarge => "TooLarge",
            FailKind::TooComplex => "TooComplex",
            FailKind::NoText => "NoText",
            FailKind::Unreadable => "Unreadable",
        }
    }
}

/// 진단·평가용 문서 단위 통계(내용 없음, 숫자만).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Stats {
    /// PDF 쪽 수와, 글자가 비었거나 깨져 버린 쪽 수
    pub pdf_pages: u32,
    pub pdf_bad_pages: u32,
    /// HWP: "제N조" 꼴 자동 번호(개요·문단 번호)를 쓰는 문단 수 — KI-011 판단용
    pub article_numbered_paras: u32,
    /// 글자가 `MAX_DOC_CHARS` 를 넘어 뒷부분을 버렸다(스펙 §6.5 `flags.truncated`). 옛 워커 응답에는 없다.
    #[serde(default)]
    pub truncated: bool,
}

/// 문서 하나에서 워커 밖으로 내보내는 글자 수 상한(블록 글자 합). 워커 → 앱 파이프, 앱 메모리, meta.db 의 문서 JSON 크기를 묶는다.
/// `index::keyword::BODY_MAX_CHARS` 와 같은 값이어야 한다 — extract 는 index 에 의존하지 않으므로 값을 따로 두고,
/// 두 크레이트를 다 쓰는 app 이 컴파일할 때 같은지 확인한다(`app/src/lib.rs`).
pub const MAX_DOC_CHARS: usize = 5_000_000;

pub struct Extracted {
    pub doc: doc::Document,
    pub distribution: bool,
    pub stats: Stats,
}

/// 문서보안(DRM) 복호화 플러그인 자리(스펙 §6.5) — 인터페이스만 정의하고 구현은 없다(Fasoo SDK 연동은 로드맵 §16).
/// 복호화할 수 없으면 None.
pub trait Decryptor: Send + Sync {
    fn decrypt(&self, bytes: &[u8]) -> Option<Vec<u8>>;
}

/// 바이트에서 추출. 실패하면 (유형, 앞 8바이트 16진수 — 진단 통계용)을 돌려준다.
/// 결과 글자는 모든 형식 공통으로 `MAX_DOC_CHARS` 에서 자르고 `stats.truncated` 를 켠다.
pub fn extract_bytes(format: Format, bytes: &[u8]) -> Result<Extracted, (FailKind, String)> {
    let mut x = extract_uncapped(format, bytes)?;
    if norm::cap_blocks(&mut x.doc.blocks, MAX_DOC_CHARS) {
        x.stats.truncated = true;
    }
    Ok(x)
}

fn extract_uncapped(format: Format, bytes: &[u8]) -> Result<Extracted, (FailKind, String)> {
    if bytes.is_empty() {
        return Err((FailKind::NotContainer, String::new()));
    }
    // 문서보안(DRM)은 원래 형식과 상관없이 파일 전체를 감싼다.
    if magic::is_known_drm(bytes) {
        return Err((FailKind::Drm, magic::magic_hex(bytes)));
    }
    use magic::{sniff, Container};
    let zip_based = matches!(format, Format::Docx | Format::Pptx | Format::Xlsx | Format::Odt | Format::Odp | Format::Ods);
    if zip_based {
        match sniff(bytes) {
            Container::Zip => {}
            // 암호를 건 OOXML 은 OLE 복합 파일 안의 EncryptedPackage 로 저장된다.
            Container::Ole if matches!(format, Format::Docx | Format::Pptx | Format::Xlsx) => {
                return Err((FailKind::Encrypted, magic::magic_hex(bytes)))
            }
            _ => return Err((FailKind::NotContainer, magic::magic_hex(bytes))),
        }
    }
    let plain = |r: Result<doc::Document, FailKind>| {
        r.map(|doc| Extracted { doc, distribution: false, stats: Stats::default() }).map_err(|k| (k, magic::magic_hex(bytes)))
    };
    match format {
        Format::Hwp | Format::Hwpx => hwp::extract_hwp(bytes),
        Format::Docx => plain(ooxml::docx(bytes)),
        Format::Pptx => plain(ooxml::pptx(bytes)),
        Format::Odt | Format::Odp => plain(odf::odf(bytes)),
        Format::Xlsx => plain(sheet::sheet(bytes, false)),
        Format::Ods => plain(sheet::sheet(bytes, true)),
        // 큰 TXT·CSV 는 디코딩 전에 바이트를 잘라 워커 메모리(디코딩 복사본 여러 벌)도 묶는다.
        Format::Txt | Format::Csv => {
            let (head, cut) = text::cap_input(bytes);
            let r = if format == Format::Txt { Ok(text::txt(head)) } else { text::csv(head) };
            plain(r).map(|mut x| {
                x.stats.truncated |= cut;
                x
            })
        }
        Format::Pdf => pdf::pdf(bytes)
            .map(|(doc, stats)| Extracted { doc, distribution: false, stats })
            .map_err(|k| (k, magic::magic_hex(bytes))),
    }
}
