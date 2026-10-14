//! 형식 판별·실패 분류 시험. 형식별 내용 시험은 각 형식 Task 에서 이 파일에 덧붙인다.

use extract::cli::extract_one;
use extract::runner::Outcome;
use extract::FailKind;
use std::path::{Path, PathBuf};

fn fx(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// 임시 폴더에 이름·내용을 정해 파일을 만든다.
fn temp_file(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(name);
    std::fs::write(&p, bytes).unwrap();
    (dir, p)
}

fn fail_kind(o: &Outcome) -> Option<FailKind> {
    match o {
        Outcome::Failed { kind, .. } => Some(kind.clone()),
        _ => None,
    }
}

#[test]
fn format_from_extension_is_case_insensitive() {
    use extract::format::Format;
    assert_eq!(Format::from_path(Path::new("a/보고서.HWPX")), Some(Format::Hwpx));
    assert_eq!(Format::from_path(Path::new("x.Pdf")), Some(Format::Pdf));
    assert_eq!(Format::from_path(Path::new("x.doc")), None);
    assert_eq!(Format::from_path(Path::new("확장자없음")), None);
    for f in Format::ALL {
        assert_eq!(Format::from_path(Path::new(&format!("a.{}", f.ext()))), Some(f));
    }
}

#[test]
fn extension_mismatch_and_empty_are_classified() {
    let (_d, empty) = temp_file("빈.hwp", b"");
    assert_eq!(fail_kind(&extract_one(&empty)), Some(FailKind::NotContainer));
    let (_d, unknown) = temp_file("메모.doc", b"anything");
    assert_eq!(fail_kind(&extract_one(&unknown)), Some(FailKind::Unsupported));
    // 어떤 확장자든 DRM 헤더면 DRM
    let (_d, drm) = temp_file("보안.pdf", b"\x9b DRMONE  rest of file");
    assert_eq!(fail_kind(&extract_one(&drm)), Some(FailKind::Drm));
}

#[test]
fn oversized_file_is_too_large_without_reading() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("큰파일.hwp");
    let f = std::fs::File::create(&p).unwrap();
    f.set_len(extract::format::MAX_FILE_BYTES + 1).unwrap(); // 희소 파일 — 디스크를 쓰지 않는다
    assert_eq!(fail_kind(&extract_one(&p)), Some(FailKind::TooLarge));
}

#[test]
fn fail_kind_codes_are_stable() {
    // meta.db 에 저장되는 값 — 바꾸면 기존 색인의 실패 표시가 기본 문구로 떨어진다.
    let all = [
        (FailKind::Encrypted, "Encrypted"),
        (FailKind::Drm, "Drm"),
        (FailKind::Corrupt, "Corrupt"),
        (FailKind::NotContainer, "NotContainer"),
        (FailKind::Unsupported, "Unsupported"),
        (FailKind::TooLarge, "TooLarge"),
        (FailKind::NoText, "NoText"),
        (FailKind::Unreadable, "Unreadable"),
    ];
    for (k, code) in all {
        assert_eq!(k.code(), code);
    }
}

#[test]
fn missing_file_is_unreadable() {
    assert_eq!(fail_kind(&extract_one(Path::new("/없는/경로/문서.hwp"))), Some(FailKind::Unreadable));
}

#[test]
fn hwpml_saved_as_hwp_is_extracted() {
    // KI-010: HWPML(XML) 문서를 .hwp 로 저장한 경우 — 예전에는 "HWP 아님"으로 분류됐다.
    let bytes = std::fs::read(fx("formatting_table.hml")).unwrap();
    let (_d, p) = temp_file("xml문서.hwp", &bytes);
    match extract_one(&p) {
        Outcome::Ok { doc, .. } => assert!(!doc.blocks.is_empty()),
        other => panic!("{other:?}"),
    }
}

fn office(name: &str) -> PathBuf {
    fx(&format!("office/{name}"))
}

/// 성공 결과의 (제목, [(블록 종류, 글자)]).
fn blocks(o: Outcome) -> (Option<String>, Vec<(doc::BlockKind, String)>) {
    match o {
        Outcome::Ok { doc, .. } => (doc.title, doc.blocks.into_iter().map(|b| (b.kind, b.text)).collect()),
        other => panic!("추출 실패: {other:?}"),
    }
}

fn kt(k: doc::BlockKind, t: &str) -> (doc::BlockKind, String) {
    (k, t.to_string())
}

#[test]
fn docx_headings_tables_textbox_once() {
    use doc::BlockKind::*;
    let (title, b) = blocks(extract_one(&office("sample.docx")));
    assert_eq!(title.as_deref(), Some("2026 운영 계획 & 예산"));
    assert_eq!(b, vec![
        kt(Heading, "사업 개요"),
        kt(Paragraph, "본 문서는 렛츠런파크 운영 계획이다."),
        kt(Paragraph, "항목 A&B <중요> 가나다\n둘째 줄"),
        kt(Heading, "세부 추진 계획"),
        kt(Heading, "개요 수준만 지정된 제목"),
        kt(Paragraph, "남은 글 옮긴 글"),
        kt(TableHeader, "구분 | 예산(백만원) | 비고"),
        kt(TableRow, "구분: 경마 시행 | 예산(백만원): 1,200 | 비고: 전년 대비 5% 증가"),
        kt(TableRow, "구분: 시설 개선 | 예산(백만원): 300"),
        kt(Paragraph, "글상자 안 문장"),
        kt(Paragraph, "글상자 앞"),
        kt(Paragraph, "맺음말"),
    ]);
}

#[test]
fn pptx_follows_presentation_order() {
    use doc::BlockKind::*;
    let (title, b) = blocks(extract_one(&office("sample.pptx")));
    assert_eq!(title.as_deref(), Some("더비 발표"));
    assert_eq!(b, vec![
        kt(Heading, "더비 운영 계획"),
        kt(Paragraph, "2026년 3월"),
        kt(Heading, "추진 일정"),
        kt(Paragraph, "1단계: 준비\n2단계: 시행"),
        kt(Paragraph, "세부 항목 & 비고"),
        kt(TableHeader, "지역 | 입장객"),
        kt(TableRow, "지역: 서울 | 입장객: 12,000"),
        kt(Heading, "마지막 슬라이드"),
        kt(Paragraph, "감사합니다"),
    ]);
}

#[test]
fn pptx_default_title_falls_back_to_first_slide_title() {
    // 한국어 PowerPoint 기본 제목 "PowerPoint 프레젠테이션" 은 제목 필드(가중치 3.0)에 넣지 않는다.
    let (title, _) = blocks(extract_one(&office("placeholder_title.pptx")));
    assert_eq!(title.as_deref(), Some("2026 고객 만족 개선 방안"));
}

#[test]
fn docx_without_core_uses_first_heading_as_title() {
    use doc::BlockKind::*;
    let (title, b) = blocks(extract_one(&office("no_core.docx")));
    assert_eq!(title.as_deref(), Some("인사 규정 개정 검토"));
    assert_eq!(b, vec![
        kt(Paragraph, "보고서 본문 앞 문단"),
        kt(Heading, "인사 규정 개정 검토"),
        kt(Paragraph, "검토 결과를 보고한다."),
    ]);
}

#[test]
fn not_a_zip_with_office_extension_is_not_container() {
    let (_d, p) = temp_file("가짜.docx", "그냥 글자".as_bytes());
    assert_eq!(fail_kind(&extract_one(&p)), Some(FailKind::NotContainer));
}

#[test]
fn password_protected_office_is_encrypted() {
    // 암호를 건 DOCX/XLSX/PPTX 는 ZIP 이 아니라 OLE(복합 파일) 안에 EncryptedPackage 로 들어 있다.
    let mut ole = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    ole.extend_from_slice(&[0u8; 504]);
    let (_d, p) = temp_file("암호.xlsx", &ole);
    assert_eq!(fail_kind(&extract_one(&p)), Some(FailKind::Encrypted));
}

#[test]
fn odt_headings_spaces_footnote_table() {
    use doc::BlockKind::*;
    let (title, b) = blocks(extract_one(&office("sample.odt")));
    assert_eq!(title.as_deref(), Some("규정 개정안"));
    assert_eq!(b, vec![
        kt(Heading, "제1장 총칙"),
        kt(Paragraph, "목적 및 범위 탭 뒤\n다음 줄"),
        kt(Paragraph, "각주가 있는 문장 각주 본문 이어지는 글"),
        kt(Paragraph, "목록 항목 & 하나"),
        kt(TableHeader, "구분 | 수량 | 비고"),
        kt(TableRow, "구분: 말 | 수량: 12"),
        kt(TableRow, "구분: 합쳐진 칸 | 비고: 끝"),
        kt(Paragraph, "맺음"),
    ]);
}

#[test]
fn odp_titles_are_headings_and_notes_are_skipped() {
    use doc::BlockKind::*;
    let (title, b) = blocks(extract_one(&office("sample.odp")));
    assert_eq!(title.as_deref(), Some("발표 자료"));
    assert_eq!(b, vec![
        kt(Heading, "발표 제목"),
        kt(Paragraph, "부제목"),
        kt(Heading, "둘째 장"),
        kt(Paragraph, "요점 하나"),
        kt(Paragraph, "요점 둘"),
    ]);
}

#[test]
fn odt_without_title_uses_first_heading() {
    let (title, _) = blocks(extract_one(&office("untitled.odt")));
    assert_eq!(title.as_deref(), Some("제1장 총칙"));
}

#[test]
fn encrypted_odt_is_encrypted_not_corrupt() {
    // 암호 ODF 는 ZIP 이고 content.xml 이 암호문 — manifest 의 encryption-data 로 알아본다.
    assert_eq!(fail_kind(&extract_one(&office("encrypted.odt"))), Some(FailKind::Encrypted));
}
