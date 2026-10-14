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
