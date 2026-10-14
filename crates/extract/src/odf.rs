//! ODT / ODP 추출 (content.xml + meta.xml). 발표자 메모·메모(주석)·변경 추적 삭제 글·각주 번호는 뺀다.

use crate::norm::{push, title_or_heading};
use crate::table::push_table;
use crate::xmlutil::{attr, first_element_text, odf_encrypted, open_zip, push_ref, read_part};
use crate::FailKind;
use doc::{BlockKind, Document};
use quick_xml::events::Event;
use quick_xml::Reader;

#[derive(Default)]
struct Table {
    rows: Vec<Vec<String>>,
    row: Option<Vec<String>>,
    cell: Option<String>,
}

/// 반복 칸(number-columns-repeated)을 펼치는 최대 개수 — 빈 칸 1,024개 같은 것을 막는다.
const MAX_REPEAT: usize = 64;

pub(crate) fn odf(bytes: &[u8]) -> Result<Document, FailKind> {
    let mut zip = open_zip(bytes)?;
    if odf_encrypted(&mut zip)? {
        return Err(FailKind::Encrypted);
    }
    let xml = read_part(&mut zip, "content.xml")?.ok_or(FailKind::Corrupt)?;
    let title = read_part(&mut zip, "meta.xml")?.and_then(|s| first_element_text(&s, "title"));

    struct Para {
        text: String,
        heading: bool,
    }
    let mut out = Vec::new();
    let mut paras: Vec<Para> = Vec::new(); // 각주 본문 문단은 바깥 문단 안에 중첩 → 스택
    let mut tables: Vec<Table> = Vec::new();
    let mut skip = 0usize;
    let mut title_frame = false; // ODP: presentation:class="title" 틀 안
    let mut repeat = 1usize; // 현재 칸의 number-columns-repeated

    let mut reader = Reader::from_str(&xml);
    loop {
        let ev = reader.read_event().map_err(|_| FailKind::Corrupt)?;
        if skip > 0 {
            match ev {
                Event::Start(_) => skip += 1,
                Event::End(_) => skip -= 1,
                Event::Eof => break,
                _ => {}
            }
            continue;
        }
        match ev {
            Event::Start(e) => match e.local_name().as_ref() {
                "notes" | "annotation" | "tracked-changes" | "note-citation" => skip = 1,
                "h" => paras.push(Para { text: String::new(), heading: true }),
                "p" => paras.push(Para { text: String::new(), heading: title_frame }),
                "frame" => title_frame = attr(&e, "class").as_deref() == Some("title"),
                "table" => tables.push(Table::default()),
                "table-row" => {
                    if let Some(t) = tables.last_mut() {
                        t.row = Some(Vec::new());
                    }
                }
                "table-cell" => {
                    if let Some(t) = tables.last_mut() {
                        t.cell = Some(String::new());
                        repeat = attr(&e, "number-columns-repeated").and_then(|v| v.parse().ok()).unwrap_or(1);
                    }
                }
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                "s" => {
                    if let Some(p) = paras.last_mut() {
                        let n = attr(&e, "c").and_then(|v| v.parse().ok()).unwrap_or(1usize);
                        p.text.extend(std::iter::repeat_n(' ', n.min(1000)));
                    }
                }
                "tab" => {
                    if let Some(p) = paras.last_mut() {
                        p.text.push('\t');
                    }
                }
                "line-break" => {
                    if let Some(p) = paras.last_mut() {
                        p.text.push('\n');
                    }
                }
                // 빈 칸·병합으로 가려진 칸도 열 위치를 지키려고 빈 문자열로 넣는다.
                "table-cell" | "covered-table-cell" => {
                    if let Some(row) = tables.last_mut().and_then(|t| t.row.as_mut()) {
                        let n = attr(&e, "number-columns-repeated").and_then(|v| v.parse().ok()).unwrap_or(1usize);
                        row.extend(std::iter::repeat_n(String::new(), n.min(MAX_REPEAT)));
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if let Some(p) = paras.last_mut() {
                    p.text.push_str(&t);
                }
            }
            Event::GeneralRef(r) => {
                if let Some(p) = paras.last_mut() {
                    push_ref(&mut p.text, &r);
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                "h" | "p" => {
                    let Some(p) = paras.pop() else { continue };
                    if let Some(cell) = tables.last_mut().and_then(|t| t.cell.as_mut()) {
                        if !cell.is_empty() {
                            cell.push(' ');
                        }
                        cell.push_str(p.text.trim());
                    } else if let Some(outer) = paras.last_mut() {
                        // 각주 본문 등 중첩 문단: 바깥 문단 뒤에 붙인다.
                        outer.text.push(' ');
                        outer.text.push_str(&p.text);
                    } else {
                        push(&mut out, if p.heading { BlockKind::Heading } else { BlockKind::Paragraph }, &p.text);
                    }
                }
                "frame" => title_frame = false,
                "table-cell" => {
                    if let Some(t) = tables.last_mut() {
                        let c = t.cell.take().unwrap_or_default();
                        t.row.get_or_insert_with(Vec::new).extend(std::iter::repeat_n(c, repeat.min(MAX_REPEAT)));
                        repeat = 1;
                    }
                }
                "table-row" => {
                    if let Some(t) = tables.last_mut() {
                        if let Some(r) = t.row.take() {
                            t.rows.push(r);
                        }
                    }
                }
                "table" => {
                    let Some(t) = tables.pop() else { continue };
                    let flat = t.rows.iter().map(|r| r.join(" ")).collect::<Vec<_>>().join(" ");
                    if let Some(cell) = tables.last_mut().and_then(|o| o.cell.as_mut()) {
                        cell.push(' ');
                        cell.push_str(&flat);
                    } else {
                        push_table(&mut out, &t.rows);
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    let title = title_or_heading(title, &out);
    Ok(Document { title, blocks: out })
}
