//! 확장자로 문서 형식을 정한다. 내용 검사(DRM·컨테이너)는 형식별 추출기가 한다.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Format {
    Hwp,
    Hwpx,
    Pdf,
    Docx,
    Pptx,
    Xlsx,
    Odt,
    Odp,
    Ods,
    Txt,
    Csv,
}

/// 이보다 큰 파일은 내용을 추출하지 않는다(스펙 §6.5) — 파일명만 색인.
pub const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;

impl Format {
    pub const ALL: [Format; 11] = [
        Format::Hwp, Format::Hwpx, Format::Pdf, Format::Docx, Format::Pptx, Format::Xlsx,
        Format::Odt, Format::Odp, Format::Ods, Format::Txt, Format::Csv,
    ];

    pub fn ext(self) -> &'static str {
        match self {
            Format::Hwp => "hwp",
            Format::Hwpx => "hwpx",
            Format::Pdf => "pdf",
            Format::Docx => "docx",
            Format::Pptx => "pptx",
            Format::Xlsx => "xlsx",
            Format::Odt => "odt",
            Format::Odp => "odp",
            Format::Ods => "ods",
            Format::Txt => "txt",
            Format::Csv => "csv",
        }
    }

    pub fn from_path(p: &Path) -> Option<Format> {
        let ext = p.extension()?.to_str()?.to_ascii_lowercase();
        Format::ALL.into_iter().find(|f| f.ext() == ext)
    }
}
