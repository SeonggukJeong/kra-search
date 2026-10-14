//! zip + quick-xml 0.42 공용 도우미(DOCX·PPTX·ODF·XLSX 제목).
//! quick-xml 0.42 는 `&amp;` 같은 참조를 Text 와 별도의 `Event::GeneralRef` 로 준다 — 모든 읽기 경로에서 처리해야 글자가 빠지지 않는다.

use crate::FailKind;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use std::io::{Cursor, Read};

pub(crate) type Zip<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

/// XML 파트 하나의 최대 크기(압축 해제 후) — 압축 폭탄 방어.
pub(crate) const MAX_PART: u64 = 64 * 1024 * 1024;

pub(crate) fn open_zip(bytes: &[u8]) -> Result<Zip<'_>, FailKind> {
    zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| FailKind::Corrupt)
}

/// zip 안의 파트를 UTF-8 문자열로 읽는다. 없으면 None, MAX_PART 를 넘으면 TooLarge.
pub(crate) fn read_part(zip: &mut Zip, name: &str) -> Result<Option<String>, FailKind> {
    read_part_capped(zip, name, MAX_PART)
}

pub(crate) fn read_part_capped(zip: &mut Zip, name: &str, cap: u64) -> Result<Option<String>, FailKind> {
    let file = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(_) => return Err(FailKind::Corrupt),
    };
    let mut buf = Vec::new();
    file.take(cap + 1).read_to_end(&mut buf).map_err(|_| FailKind::Corrupt)?;
    if buf.len() as u64 > cap {
        return Err(FailKind::TooLarge);
    }
    let s = String::from_utf8(buf).map_err(|_| FailKind::Corrupt)?;
    Ok(Some(s.strip_prefix('\u{FEFF}').map(str::to_owned).unwrap_or(s)))
}

/// `&amp;` `&#xAC00;` 같은 참조를 글자로 바꿔 붙인다.
pub(crate) fn push_ref(buf: &mut String, r: &BytesRef) {
    match r.resolve_char_ref() {
        Ok(Some(c)) => buf.push(c),
        Ok(None) => {
            if let Some(s) = resolve_predefined_entity(r) {
                buf.push_str(s);
            }
        }
        Err(_) => {}
    }
}

/// 접두사를 무시하고 지역 이름이 `local` 인 속성 값(참조 해제).
pub(crate) fn attr(e: &BytesStart, local: &str) -> Option<String> {
    e.attributes().with_checks(false).flatten().find_map(|a| {
        if a.key.local_name().as_ref() == local {
            a.normalized_value(XmlVersion::Implicit1_0).ok().map(|v| v.into_owned())
        } else {
            None
        }
    })
}

/// 지역 이름이 `local` 인 첫 요소의 글자(core.xml·meta.xml 의 dc:title 용).
pub(crate) fn first_element_text(xml: &str, local: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    let mut inside = false;
    let mut out = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.local_name().as_ref() == local => inside = true,
            Ok(Event::End(e)) if inside && e.local_name().as_ref() == local => break,
            Ok(Event::Text(t)) if inside => out.push_str(&t),
            Ok(Event::GeneralRef(r)) if inside => push_ref(&mut out, &r),
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let t = crate::norm::clean(&out);
    (!t.is_empty()).then_some(t)
}

/// ODF 암호 문서인가. 암호를 건 ODF 도 ZIP 이라 겉으로는 정상이고, content.xml 이 암호문이다.
/// manifest 의 `<manifest:encryption-data>` 로 알아본다(없으면 content.xml 의 UTF-8 변환이 실패해 Corrupt 로 잘못 분류됨).
#[allow(dead_code)] // ODF 추출기(Task 5·6)가 쓴다
pub(crate) fn odf_encrypted(zip: &mut Zip) -> Result<bool, FailKind> {
    Ok(read_part(zip, "META-INF/manifest.xml")?.is_some_and(|m| m.contains("encryption-data")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn oversized_xml_part_is_capped() {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        w.start_file("big.xml", opts).unwrap();
        w.write_all(&vec![b'a'; 2048]).unwrap();
        let bytes = w.finish().unwrap().into_inner();
        let mut z = open_zip(&bytes).unwrap();
        assert_eq!(read_part_capped(&mut z, "big.xml", 1024), Err(FailKind::TooLarge));
        assert_eq!(read_part_capped(&mut z, "big.xml", 4096).unwrap().map(|s| s.len()), Some(2048));
        assert_eq!(read_part_capped(&mut z, "없음.xml", 4096), Ok(None));
    }

    #[test]
    fn entities_inside_title_are_resolved() {
        let xml = r#"<cp:coreProperties xmlns:cp="x" xmlns:dc="y"><dc:title>A &amp; &#xAC00;</dc:title></cp:coreProperties>"#;
        assert_eq!(first_element_text(xml, "title").as_deref(), Some("A & 가"));
    }
}
