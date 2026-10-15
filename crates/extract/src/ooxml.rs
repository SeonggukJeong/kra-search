//! DOCX / PPTX 추출 (zip + quick-xml 0.42). 머리말·꼬리말·각주·메모·발표자 메모는 읽지 않는다.

use crate::norm::{push, title_or_heading};
use crate::table::push_table;
use crate::xmlutil::{attr, first_element_text, open_zip, push_ref, read_part, Zip};
use crate::FailKind;
use doc::{Block, BlockKind, Document};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::{HashMap, HashSet};

/// 표 하나(칸 안 중첩 표는 스택으로 다룬다).
#[derive(Default)]
struct Table {
    rows: Vec<Vec<String>>,
    row: Option<Vec<String>>,
    cell: Option<String>,
}

impl Table {
    /// 문단 글자를 현재 칸에 붙인다(칸 안 문단은 공백으로 잇는다). 칸이 없으면 false.
    fn append(&mut self, text: &str) -> bool {
        match self.cell.as_mut() {
            Some(c) => {
                if !c.is_empty() && !text.trim().is_empty() {
                    c.push(' ');
                }
                c.push_str(text.trim());
                true
            }
            None => false,
        }
    }
    fn start_row(&mut self) {
        self.row = Some(Vec::new());
    }
    fn start_cell(&mut self) {
        self.cell = Some(String::new());
    }
    fn end_cell(&mut self) {
        let c = self.cell.take().unwrap_or_default();
        self.row.get_or_insert_with(Vec::new).push(c);
    }
    fn end_row(&mut self) {
        if let Some(r) = self.row.take() {
            self.rows.push(r);
        }
    }
}

/// 문단 하나가 끝났을 때: 표 칸 안이면 칸에, 아니면 블록으로.
fn finish_para(out: &mut Vec<Block>, tables: &mut [Table], text: &str, heading: bool) {
    if let Some(t) = tables.last_mut() {
        if t.append(text) {
            return;
        }
    }
    push(out, if heading { BlockKind::Heading } else { BlockKind::Paragraph }, text);
}

/// 표가 끝났을 때: 바깥 표 칸 안(중첩 표)이면 평문으로 칸에, 아니면 표 블록으로.
fn finish_table(out: &mut Vec<Block>, tables: &mut Vec<Table>) {
    let Some(t) = tables.pop() else { return };
    if let Some(outer) = tables.last_mut() {
        let flat: Vec<String> = t.rows.iter().map(|r| r.join(" ")).collect();
        if outer.append(&flat.join(" ")) {
            return;
        }
    }
    push_table(out, &t.rows);
}

// ---------------------------------------------------------------- DOCX

/// styles.xml → 제목 스타일 ID. 이름이 heading*/title/제목* 이거나 outlineLvl 0~8 이면 제목.
/// 한국어 Word 는 "제목 1" 의 styleId 가 "1" 처럼 숫자라 ID 만으로는 판단할 수 없다.
fn heading_styles(xml: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    let mut reader = Reader::from_str(xml);
    let mut cur: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.local_name().as_ref() == "style" => cur = attr(&e, "styleId"),
            Ok(Event::Empty(e)) => {
                let Some(id) = cur.as_ref() else { continue };
                match e.local_name().as_ref() {
                    "name" => {
                        let n = attr(&e, "val").unwrap_or_default().to_lowercase();
                        if n.starts_with("heading") || n == "title" || n.starts_with("제목") {
                            set.insert(id.clone());
                        }
                    }
                    "outlineLvl" if attr(&e, "val").and_then(|v| v.parse::<u8>().ok()).is_some_and(|v| v < 9) => {
                        set.insert(id.clone());
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == "style" => cur = None,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    set
}

pub(crate) fn docx(bytes: &[u8]) -> Result<Document, FailKind> {
    let mut zip = open_zip(bytes)?;
    let xml = read_part(&mut zip, "word/document.xml")?.ok_or(FailKind::Corrupt)?;
    let headings = read_part(&mut zip, "word/styles.xml")?.map(|s| heading_styles(&s)).unwrap_or_default();
    let title = read_part(&mut zip, "docProps/core.xml")?.and_then(|s| first_element_text(&s, "title"));

    struct Para {
        text: String,
        heading: bool,
    }
    let mut out = Vec::new();
    let mut paras: Vec<Para> = Vec::new(); // 글상자 안 문단은 바깥 문단 안에 중첩된다 → 스택
    let mut tables: Vec<Table> = Vec::new();
    let mut in_t = false; // <w:t> 안
    let mut in_ppr = false; // <w:pPr> 안(탭 정의 w:tab 을 글자 탭과 구분)
    let mut skip = 0usize; // 건너뛰는 요소 깊이: mc:Fallback(글상자 글 중복 제거), w:moveFrom(옮기기 전 자리 — 글이 moveTo 에 또 있음)

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
                "Fallback" | "moveFrom" => skip = 1,
                "p" => paras.push(Para { text: String::new(), heading: false }),
                "pPr" => in_ppr = true,
                "t" => in_t = true,
                "tbl" => tables.push(Table::default()),
                "tr" => {
                    if let Some(t) = tables.last_mut() {
                        t.start_row();
                    }
                }
                "tc" => {
                    if let Some(t) = tables.last_mut() {
                        t.start_cell();
                    }
                }
                _ => {}
            },
            Event::Empty(e) => {
                let Some(p) = paras.last_mut() else { continue };
                match e.local_name().as_ref() {
                    "pStyle" if in_ppr => {
                        if attr(&e, "val").is_some_and(|v| headings.contains(&v)) {
                            p.heading = true;
                        }
                    }
                    "outlineLvl" if in_ppr => {
                        if attr(&e, "val").and_then(|v| v.parse::<u8>().ok()).is_some_and(|v| v < 9) {
                            p.heading = true;
                        }
                    }
                    "tab" if !in_ppr => p.text.push('\t'),
                    "br" | "cr" => p.text.push('\n'),
                    "noBreakHyphen" => p.text.push('-'),
                    _ => {}
                }
            }
            Event::Text(t) if in_t => {
                if let Some(p) = paras.last_mut() {
                    p.text.push_str(&t);
                }
            }
            Event::GeneralRef(r) if in_t => {
                if let Some(p) = paras.last_mut() {
                    push_ref(&mut p.text, &r);
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_t = false,
                "pPr" => in_ppr = false,
                "p" => {
                    if let Some(p) = paras.pop() {
                        finish_para(&mut out, &mut tables, &p.text, p.heading);
                    }
                }
                "tc" => {
                    if let Some(t) = tables.last_mut() {
                        t.end_cell();
                    }
                }
                "tr" => {
                    if let Some(t) = tables.last_mut() {
                        t.end_row();
                    }
                }
                "tbl" => finish_table(&mut out, &mut tables),
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    let title = title_or_heading(title, &out);
    Ok(Document { title, blocks: out })
}

// ---------------------------------------------------------------- PPTX

/// 발표 순서대로 슬라이드 파트 경로: presentation.xml 의 sldIdLst(r:id) → rels 의 Target.
/// 실패하면 ppt/slides/slideN.xml 을 N 숫자 순으로.
fn slide_paths(zip: &mut Zip) -> Result<Vec<String>, FailKind> {
    let mut ordered = Vec::new();
    let pres = read_part(zip, "ppt/presentation.xml")?;
    let rels = read_part(zip, "ppt/_rels/presentation.xml.rels")?;
    if let (Some(pres), Some(rels)) = (pres, rels) {
        let mut targets = HashMap::new();
        let mut r = Reader::from_str(&rels);
        loop {
            match r.read_event() {
                Ok(Event::Empty(e)) | Ok(Event::Start(e)) if e.local_name().as_ref() == "Relationship" => {
                    if let (Some(id), Some(t)) = (attr(&e, "Id"), attr(&e, "Target")) {
                        let path = match t.strip_prefix('/') {
                            Some(abs) => abs.to_string(),
                            None => format!("ppt/{t}"),
                        };
                        targets.insert(id, path);
                    }
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
        let mut r = Reader::from_str(&pres);
        loop {
            match r.read_event() {
                Ok(Event::Empty(e)) | Ok(Event::Start(e)) if e.local_name().as_ref() == "sldId" => {
                    // 속성 "id"(숫자)와 "r:id"(관계)는 지역 이름이 같다 → 접두사 있는 쪽이 관계 ID
                    let rid = e.attributes().with_checks(false).flatten().find_map(|a| {
                        if a.key.local_name().as_ref() == "id" && a.key.prefix().is_some() {
                            Some(a.value.to_string())
                        } else {
                            None
                        }
                    });
                    if let Some(p) = rid.and_then(|id| targets.get(&id).cloned()) {
                        ordered.push(p);
                    }
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
    }
    if ordered.is_empty() {
        let mut numbered: Vec<(u32, String)> = zip
            .file_names()
            .filter_map(|n| {
                let num = n.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?.parse().ok()?;
                Some((num, n.to_string()))
            })
            .collect();
        numbered.sort();
        ordered = numbered.into_iter().map(|(_, n)| n).collect();
    }
    Ok(ordered)
}

/// 슬라이드 하나: 제목 자리표시자(title/ctrTitle) 문단 → Heading, 나머지 → Paragraph, a:tbl → 표.
fn pptx_slide(xml: &str, out: &mut Vec<Block>) -> Result<(), FailKind> {
    let mut title_shape = false;
    let mut para: Option<String> = None;
    let mut in_t = false;
    let mut tables: Vec<Table> = Vec::new();
    let mut title = String::new();
    let mut body = Vec::new();

    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event().map_err(|_| FailKind::Corrupt)? {
            Event::Start(e) => match e.local_name().as_ref() {
                "sp" => title_shape = false,
                "p" => para = Some(String::new()),
                "t" => in_t = true,
                // PowerPoint 는 줄바꿈을 <a:br><a:rPr …/></a:br>(시작 요소)로 쓴다 — 빈 요소 <a:br/> 와 같이 처리
                "br" => {
                    if let Some(p) = para.as_mut() {
                        p.push('\n');
                    }
                }
                "tbl" => tables.push(Table::default()),
                "tr" => {
                    if let Some(t) = tables.last_mut() {
                        t.start_row();
                    }
                }
                "tc" => {
                    if let Some(t) = tables.last_mut() {
                        t.start_cell();
                    }
                }
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                "ph" => title_shape = matches!(attr(&e, "type").as_deref(), Some("title" | "ctrTitle")),
                "br" => {
                    if let Some(p) = para.as_mut() {
                        p.push('\n');
                    }
                }
                _ => {}
            },
            Event::Text(t) if in_t => {
                if let Some(p) = para.as_mut() {
                    p.push_str(&t);
                }
            }
            Event::GeneralRef(r) if in_t => {
                if let Some(p) = para.as_mut() {
                    push_ref(p, &r);
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_t = false,
                "p" => {
                    let Some(text) = para.take() else { continue };
                    if title_shape && tables.is_empty() {
                        if !title.is_empty() {
                            title.push(' ');
                        }
                        title.push_str(text.trim());
                    } else {
                        finish_para(&mut body, &mut tables, &text, false);
                    }
                }
                "sp" => title_shape = false,
                "tc" => {
                    if let Some(t) = tables.last_mut() {
                        t.end_cell();
                    }
                }
                "tr" => {
                    if let Some(t) = tables.last_mut() {
                        t.end_row();
                    }
                }
                "tbl" => finish_table(&mut body, &mut tables),
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    push(out, BlockKind::Heading, &title);
    out.append(&mut body);
    Ok(())
}

pub(crate) fn pptx(bytes: &[u8]) -> Result<Document, FailKind> {
    let mut zip = open_zip(bytes)?;
    let title = read_part(&mut zip, "docProps/core.xml")?.and_then(|s| first_element_text(&s, "title"));
    let mut out = Vec::new();
    for path in slide_paths(&mut zip)? {
        if let Some(xml) = read_part(&mut zip, &path)? {
            pptx_slide(&xml, &mut out)?;
        }
    }
    let title = title_or_heading(title, &out);
    Ok(Document { title, blocks: out })
}
