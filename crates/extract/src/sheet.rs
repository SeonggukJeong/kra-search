//! XLSX / ODS 추출 (calamine). 셀 값은 수식 결과(저장된 값)를 쓴다. 옛 .xls 는 범위 밖(스펙 §16).
//! 메모리 상한(DEC-036): XML 파트 크기를 먼저 재고, XLSX 는 셀 단위로 읽으며, ODS 는 펼칠 칸 수를 미리 어림한다.

use crate::norm::{push, usable_title};
use crate::table::push_table;
use crate::xmlutil::{attr, first_element_text, odf_encrypted, open_zip, read_part, MAX_PART};
use crate::FailKind;
use calamine::{open_workbook_from_rs, Data, Ods, Reader, Xlsx, XlsxError};
use doc::{Block, BlockKind, Document};
use quick_xml::events::Event;
use std::io::{BufReader, Cursor, Read};

/// 시트 하나에서 내는 최대 행 수(비지 않은 행) — 거대한 데이터 시트가 키워드 색인을 채우지 않도록(DEC-036).
pub(crate) const MAX_ROWS_PER_SHEET: usize = 50_000;
/// XLSX 시트 하나에서 모으는 최대 셀 수(비지 않은 셀).
const MAX_CELLS_PER_SHEET: usize = 1_000_000;
/// 표로 펼친 칸 수(빈 칸 포함) 상한 — 한 시트 기준. 넘는 행부터는 버린다.
const MAX_SLOTS_PER_SHEET: usize = 2_000_000;
/// ODS: calamine 이 펼칠 칸 수 어림의 상한(문서 전체). 넘으면 TooLarge(파일명만 색인).
const ODS_MAX_CELLS: u64 = 2_000_000;
/// 셀 글자 바이트 합 상한(문서 전체). 같은 긴 글자가 여러 칸에 복제돼도(공유 문자열·ODS 반복) 메모리가 곱해지지 않게(DEC-036).
const MAX_SHEET_TEXT_BYTES: u64 = 64 * 1024 * 1024;
/// XML 파트 전체 크기 합 상한(압축 해제 후).
const MAX_XML_TOTAL: u64 = 256 * 1024 * 1024;
/// ODF·XLSX 한계(calamine 과 같은 값).
const MAX_ROWS: u64 = 1_048_576;
const MAX_COLS: u64 = 16_384;

/// 셀 값을 검색용 문자열로.
fn cell(d: &Data) -> String {
    match d {
        Data::Empty | Data::Error(_) => String::new(),
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
        Data::Float(f) => f.to_string(),
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        // chrono 기능 없이 변환(1900/1904 기준·1900 윤년 버그 처리 포함)
        Data::DateTime(dt) if dt.is_datetime() => {
            let (y, m, d, hh, mm, ss, _) = dt.to_ymd_hms_milli();
            if (hh, mm, ss) == (0, 0, 0) {
                format!("{y:04}-{m:02}-{d:02}")
            } else {
                format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}")
            }
        }
        Data::DateTime(dt) => dt.as_f64().to_string(), // 기간(duration)
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
    }
}

/// calamine 은 zip 파트를 상한 없이 읽는다 → 넘기기 전에 XML 파트(.xml·.rels)를 실제로 풀어 크기를 잰다.
/// 선언된 크기는 거짓일 수 있으므로 읽은 바이트로 센다(버리면서 읽음 — 메모리 안 씀). 그림 등 다른 파트는 calamine 이 읽지 않으므로 건너뛴다.
fn check_parts(bytes: &[u8], part_cap: u64, total_cap: u64) -> Result<(), FailKind> {
    let mut zip = open_zip(bytes)?;
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|_| FailKind::Corrupt)?;
        let name = f.name().to_ascii_lowercase();
        if !(name.ends_with(".xml") || name.ends_with(".rels")) {
            continue;
        }
        let n = std::io::copy(&mut (&mut f).take(part_cap + 1), &mut std::io::sink()).map_err(|_| FailKind::Corrupt)?;
        if n > part_cap {
            return Err(FailKind::TooLarge);
        }
        total += n;
        if total > total_cap {
            return Err(FailKind::TooLarge);
        }
    }
    Ok(())
}

/// 흩어진 (행, 열, 글자) → 표 행. 비지 않은 칸이 하나라도 있는 열만 남겨 칸 위치를 맞춘다
/// (먼 곳에 떨어진 셀 두 개가 거대한 빈 격자를 만들지 않게). 펼친 칸이 MAX_SLOTS_PER_SHEET 를 넘으면 거기서 멈춘다.
fn to_rows(mut cells: Vec<(u32, u32, String)>) -> Vec<Vec<String>> {
    cells.sort_by_key(|c| (c.0, c.1));
    let mut used: Vec<u32> = cells.iter().map(|c| c.1).collect();
    used.sort_unstable();
    used.dedup();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut cur_row = None;
    let mut slots = 0usize;
    for (r, c, v) in cells {
        if cur_row != Some(r) {
            cur_row = Some(r);
            rows.push(Vec::new());
        }
        let i = used.binary_search(&c).unwrap_or_else(|i| i);
        let Some(row) = rows.last_mut() else { continue };
        if i >= row.len() {
            let grow = i + 1 - row.len();
            if slots + grow > MAX_SLOTS_PER_SHEET {
                break;
            }
            slots += grow;
            row.resize(i + 1, String::new());
        }
        row[i] = v;
    }
    rows
}

/// XLSX: 셀을 하나씩 읽는다(빈 격자를 만들지 않음).
fn xlsx_sheets(bytes: &[u8], out: &mut Vec<Block>) -> Result<(), FailKind> {
    let mut wb: Xlsx<_> = open_workbook_from_rs(Cursor::new(bytes)).map_err(|_| FailKind::Corrupt)?;
    let mut text_bytes = 0u64;
    for name in wb.sheet_names() {
        let mut reader = match wb.worksheet_cells_reader(&name) {
            Ok(r) => r,
            Err(XlsxError::NotAWorksheet(_)) => continue, // 차트 시트
            Err(_) => return Err(FailKind::Corrupt),
        };
        let mut cells: Vec<(u32, u32, String)> = Vec::new();
        let (mut rows, mut last_row) = (0usize, None);
        while let Some(c) = reader.next_cell().map_err(|_| FailKind::Corrupt)? {
            let v = cell(&Data::from(c.get_value().clone()));
            if v.is_empty() {
                continue;
            }
            let (r, col) = c.get_position();
            if last_row != Some(r) {
                last_row = Some(r);
                rows += 1;
                if rows > MAX_ROWS_PER_SHEET {
                    break;
                }
            }
            text_bytes += v.len() as u64;
            if text_bytes > MAX_SHEET_TEXT_BYTES {
                return Err(FailKind::TooLarge);
            }
            cells.push((r, col, v));
            if cells.len() >= MAX_CELLS_PER_SHEET {
                break;
            }
        }
        if cells.is_empty() {
            continue; // 빈 시트는 이름도 내지 않는다
        }
        push(out, BlockKind::Heading, &name);
        push_table(out, &to_rows(cells));
    }
    Ok(())
}

/// ODS: calamine 이 펼칠 칸 수를 content.xml 에서 어림한다(`calamine-0.36.1/src/ods.rs` read_row·get_range 와 같은 셈).
/// - 시트마다: (첫 비지 않은 행 ~ 마지막 비지 않은 행, 반복 포함) × (가장 왼쪽 ~ 가장 오른쪽 비지 않은 열)
/// - 행마다 펼치기 전 칸: 마지막 비지 않은 칸 위치 + 1 (앞쪽 빈 칸도 채워 넣음)
///
/// 비지 않은 칸 = 병합·반복·스타일 말고 다른 속성(value-type·value·string-value·formula 등)이 있거나 안에 내용이 있는(시작 태그) 칸.
/// calamine 은 value-type 없이 office:value 만 있어도 값으로 읽으므로 넉넉하게(많게) 센다.
/// 글자 바이트도 어림한다: Σ (칸 내용 바이트 × 열 반복 × 행 반복) 이 MAX_SHEET_TEXT_BYTES 를 넘으면 TooLarge.
fn ods_cell_estimate(bytes: &[u8]) -> Result<u64, FailKind> {
    /// 반복 속성(없으면 1, 0·잘못된 값도 1).
    fn num(e: &quick_xml::events::BytesStart, a: &str) -> u64 {
        attr(e, a).and_then(|v| v.parse::<u64>().ok()).unwrap_or(1).max(1)
    }
    /// 칸 시작 태그: (비지 않은 칸인가, 값 속성 바이트).
    fn cell_attrs(e: &quick_xml::events::BytesStart) -> (bool, u64) {
        let (mut filled, mut bytes) = (false, 0u64);
        for a in e.attributes().with_checks(false).flatten() {
            match a.key.local_name().as_ref() {
                "number-columns-repeated" | "number-columns-spanned" | "number-rows-spanned" | "style-name" => {}
                "value" | "string-value" | "date-value" | "time-value" | "boolean-value" => {
                    filled = true;
                    bytes += a.value.len() as u64;
                }
                _ => filled = true,
            }
        }
        (filled, bytes)
    }
    let mut zip = open_zip(bytes)?;
    let f = match zip.by_name("content.xml") {
        Ok(f) => f,
        Err(_) => return Err(FailKind::Corrupt),
    };
    let mut reader = quick_xml::Reader::from_reader(BufReader::new(f));
    let mut buf = Vec::new();
    let (mut total, mut total_bytes) = (0u64, 0u64);
    // 칸 상태(글자 바이트)
    let (mut in_cell, mut cell_bytes, mut cell_rep, mut row_bytes) = (false, 0u64, 1u64, 0u64);
    // 시트 상태
    let (mut rows_seen, mut first_row, mut last_row_end) = (0u64, None::<u64>, 0u64);
    let (mut col_min, mut col_max) = (u64::MAX, 0u64);
    // 행 상태
    let (mut row_rep, mut col, mut row_first, mut row_last) = (1u64, 0u64, None::<u64>, 0u64);
    loop {
        buf.clear();
        let ev = reader.read_event_into(&mut buf).map_err(|_| FailKind::Corrupt)?;
        let (e, has_body) = match &ev {
            Event::Start(e) => (Some(e), true),
            Event::Empty(e) => (Some(e), false),
            Event::End(e) => {
                match e.local_name().as_ref() {
                    "table-cell" | "covered-table-cell" if in_cell => {
                        row_bytes = row_bytes.saturating_add(cell_bytes.saturating_mul(cell_rep));
                        in_cell = false;
                    }
                    "table-row" => {
                        total_bytes = total_bytes.saturating_add(row_bytes.saturating_mul(row_rep));
                        row_bytes = 0;
                        if let Some(f) = row_first {
                            first_row.get_or_insert(rows_seen);
                            last_row_end = rows_seen + row_rep;
                            col_min = col_min.min(f);
                            col_max = col_max.max(row_last);
                            total += row_last + 1;
                        }
                        rows_seen += row_rep;
                    }
                    "table" => {
                        if let Some(f) = first_row {
                            total += (last_row_end - f) * (col_max + 1 - col_min);
                        }
                        (rows_seen, first_row, last_row_end, col_min, col_max) = (0, None, 0, u64::MAX, 0);
                    }
                    _ => {}
                }
                if total > ODS_MAX_CELLS || total_bytes > MAX_SHEET_TEXT_BYTES {
                    return Err(FailKind::TooLarge);
                }
                continue;
            }
            Event::Text(t) => {
                if in_cell {
                    cell_bytes += t.len() as u64;
                }
                continue;
            }
            Event::CData(t) => {
                if in_cell {
                    cell_bytes += t.len() as u64;
                }
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        let Some(e) = e else { continue };
        match e.local_name().as_ref() {
            "table-row" => {
                row_rep = num(e, "number-rows-repeated").min(MAX_ROWS.saturating_sub(rows_seen));
                (col, row_first, row_last) = (0, None, 0);
                row_bytes = 0;
                if !has_body {
                    rows_seen += row_rep; // 칸 없는 빈 행
                }
            }
            "table-cell" | "covered-table-cell" => {
                let rep = num(e, "number-columns-repeated").min(MAX_COLS.saturating_sub(col));
                let (attr_filled, attr_bytes) = cell_attrs(e);
                let filled = has_body || attr_filled;
                if filled && rep > 0 {
                    row_first.get_or_insert(col);
                    row_last = col + rep - 1;
                }
                col += rep;
                if has_body {
                    (in_cell, cell_bytes, cell_rep) = (true, attr_bytes, rep);
                } else {
                    row_bytes = row_bytes.saturating_add(attr_bytes.saturating_mul(rep));
                }
            }
            _ => {}
        }
    }
    Ok(total)
}

pub(crate) fn sheet(bytes: &[u8], ods: bool) -> Result<Document, FailKind> {
    if ods {
        // 암호 ODS 는 content.xml 이 암호문 — calamine 에 넘기면 Corrupt 가 된다.
        if odf_encrypted(&mut open_zip(bytes)?)? {
            return Err(FailKind::Encrypted);
        }
    }
    check_parts(bytes, MAX_PART, MAX_XML_TOTAL)?;
    let mut out = Vec::new();
    if ods {
        if ods_cell_estimate(bytes)? > ODS_MAX_CELLS {
            return Err(FailKind::TooLarge);
        }
        let mut wb: Ods<_> = open_workbook_from_rs(Cursor::new(bytes)).map_err(|_| FailKind::Corrupt)?;
        for name in wb.sheet_names() {
            let range = wb.worksheet_range(&name).map_err(|_| FailKind::Corrupt)?;
            let rows: Vec<Vec<String>> = range
                .rows()
                .map(|r| r.iter().map(cell).collect::<Vec<_>>())
                .filter(|r| r.iter().any(|c| !c.is_empty()))
                .take(MAX_ROWS_PER_SHEET)
                .collect();
            if rows.is_empty() {
                continue; // 빈 시트는 이름도 내지 않는다
            }
            push(&mut out, BlockKind::Heading, &name);
            push_table(&mut out, &rows);
        }
    } else {
        xlsx_sheets(bytes, &mut out)?;
    }
    // calamine 은 문서 속성을 주지 않으므로 제목은 직접 읽는다. 시트 이름("Sheet1")은 제목으로 올리지 않는다.
    let mut zip = open_zip(bytes)?;
    let title = read_part(&mut zip, if ods { "meta.xml" } else { "docProps/core.xml" })?
        .and_then(|s| first_element_text(&s, "title"))
        .and_then(|t| usable_title(&t));
    Ok(Document { title, blocks: out })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{Duration, Instant};

    fn zip_of(parts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in parts {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    const ODS_NS: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0""#;

    fn ods_with(rows_xml: &str) -> Vec<u8> {
        let content = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {ODS_NS} office:version="1.3"><office:body><office:spreadsheet><table:table table:name="시트">{rows_xml}</table:table></office:spreadsheet></office:body></office:document-content>"#
        );
        let manifest = r#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/></manifest:manifest>"#;
        zip_of(&[
            ("mimetype", b"application/vnd.oasis.opendocument.spreadsheet"),
            ("META-INF/manifest.xml", manifest.as_bytes()),
            ("content.xml", content.as_bytes()),
        ])
    }

    #[test]
    fn non_empty_repeated_rows_are_refused_quickly() {
        // 비지 않은 행(10칸 "x")이 100만 번 반복 → calamine 이 1,000만 칸을 펼칠 파일. 펼치기 전에 TooLarge.
        let bytes = ods_with(
            r#"<table:table-row table:number-rows-repeated="1000000"><table:table-cell table:number-columns-repeated="10" office:value-type="string"><text:p>x</text:p></table:table-cell></table:table-row>"#,
        );
        let t = Instant::now();
        assert_eq!(sheet(&bytes, true).err(), Some(FailKind::TooLarge));
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    }

    #[test]
    fn small_repeated_rows_are_read() {
        // 비지 않은 반복 행이라도 상한 안이면 읽는다(3행 × 2칸).
        let bytes = ods_with(
            r#"<table:table-row><table:table-cell office:value-type="string"><text:p>구분</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>값</text:p></table:table-cell></table:table-row><table:table-row table:number-rows-repeated="3"><table:table-cell office:value-type="string"><text:p>경마</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="1"><text:p>1</text:p></table:table-cell></table:table-row>"#,
        );
        assert_eq!(ods_cell_estimate(&bytes), Ok(2 + 2 + 4 * 2));
        let d = sheet(&bytes, true).unwrap();
        assert_eq!(d.blocks.iter().filter(|b| b.kind == BlockKind::TableRow).count(), 3);
    }

    #[test]
    fn far_apart_cells_do_not_build_a_grid() {
        // A1 과 XFD1048576 두 칸만 있는 XLSX — worksheet_range 였다면 170억 칸 격자를 잡으려다 죽는다.
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
        let wb = r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="시트" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
        let wb_rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#;
        let ws = r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><dimension ref="A1:XFD1048576"/><sheetData><row r="1"><c r="A1"><v>11</v></c></row><row r="1048576"><c r="XFD1048576"><v>22</v></c></row></sheetData></worksheet>"#;
        let bytes = zip_of(&[
            ("_rels/.rels", rels.as_bytes()),
            ("xl/workbook.xml", wb.as_bytes()),
            ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
            ("xl/worksheets/sheet1.xml", ws.as_bytes()),
        ]);
        let t = Instant::now();
        let d = sheet(&bytes, false).unwrap();
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        assert_eq!(d.blocks.first().map(|b| (b.kind.clone(), b.text.as_str())), Some((BlockKind::Heading, "시트")));
        assert!(d.blocks.iter().any(|b| b.text.contains("11")), "{:?}", d.blocks);
        assert_eq!(to_rows(vec![(0, 0, "a".into()), (5, 16_383, "b".into())]), vec![vec!["a".to_string()], vec![String::new(), "b".to_string()]]);
    }

    #[test]
    fn value_without_value_type_counts_as_filled() {
        // value-type 없이 office:value 만 있는 자기 닫힘 칸도 calamine 은 값으로 읽는다 → 추정에도 들어가야 한다.
        let bytes = ods_with(
            r#"<table:table-row table:number-rows-repeated="1000"><table:table-cell office:value="1" table:number-columns-repeated="16384"/></table:table-row>"#,
        );
        let t = Instant::now();
        assert_eq!(sheet(&bytes, true).err(), Some(FailKind::TooLarge));
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    }

    #[test]
    fn long_repeated_ods_cell_is_too_large_by_bytes() {
        // 100KB 글자 × 1000열 × 1000행 = 100만 칸(칸 수 상한 안)이지만 100GB.
        let long = "가".repeat(33_000);
        let bytes = ods_with(&format!(
            r#"<table:table-row table:number-rows-repeated="1000"><table:table-cell table:number-columns-repeated="1000" office:value-type="string"><text:p>{long}</text:p></table:table-cell></table:table-row>"#
        ));
        let t = Instant::now();
        assert_eq!(sheet(&bytes, true).err(), Some(FailKind::TooLarge));
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    }

    #[test]
    fn shared_string_referenced_by_many_cells_is_too_large() {
        // 1MB 공유 문자열을 100칸이 가리킨다 → 복제하면 100MB.
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
        let wb = r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="시트" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
        let wb_rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/></Relationships>"#;
        let sst = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="1" uniqueCount="1"><si><t>{}</t></si></sst>"#,
            "a".repeat(1 << 20)
        );
        let rows: String = (1..=100).map(|i| format!(r#"<row r="{i}"><c r="A{i}" t="s"><v>0</v></c></row>"#)).collect();
        let ws = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{rows}</sheetData></worksheet>"#
        );
        let bytes = zip_of(&[
            ("_rels/.rels", rels.as_bytes()),
            ("xl/workbook.xml", wb.as_bytes()),
            ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
            ("xl/sharedStrings.xml", sst.as_bytes()),
            ("xl/worksheets/sheet1.xml", ws.as_bytes()),
        ]);
        let t = Instant::now();
        assert_eq!(sheet(&bytes, false).err(), Some(FailKind::TooLarge));
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    }

    #[test]
    fn lying_or_large_xml_part_is_too_large() {
        let bytes = zip_of(&[("xl/sharedStrings.xml", &[b'a'; 2048]), ("xl/media/image1.png", &[0u8; 4096])]);
        assert_eq!(check_parts(&bytes, 1024, 1 << 20), Err(FailKind::TooLarge));
        assert_eq!(check_parts(&bytes, 4096, 1024), Err(FailKind::TooLarge)); // 합계 상한
        assert_eq!(check_parts(&bytes, 4096, 1 << 20), Ok(())); // 그림 파트는 세지 않는다
    }

    #[test]
    fn encrypted_ods_is_encrypted() {
        let manifest = r#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"><manifest:encryption-data manifest:checksum-type="sha256-1k" manifest:checksum="AAAA"/></manifest:file-entry></manifest:manifest>"#;
        let cipher: Vec<u8> = (0..512u32).map(|i| ((i * 151 + 7) % 256) as u8).collect();
        let bytes = zip_of(&[("META-INF/manifest.xml", manifest.as_bytes()), ("content.xml", &cipher)]);
        assert_eq!(sheet(&bytes, true).err(), Some(FailKind::Encrypted));
    }
}
