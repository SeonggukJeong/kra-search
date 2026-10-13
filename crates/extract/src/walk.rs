//! rhwp 문서 모델을 문서 순서대로 직접 순회해 블록을 만든다.
//!
//! rhwp `rag::build_chunks` 는 본문이 없는 제목(규정의 항·호처럼 한 줄짜리 조문)과
//! 글상자·각주·중첩 표 글자를 버리므로 쓰지 않는다(docs/engineering/incidents.md INC-011).
//! 제목 판정은 rhwp `build_structure`, 표 격자는 rhwp `extract_tables` 를 그대로 쓴다.

use doc::{Block, BlockKind};
use rhwp::document_core::queries::structure::{build_structure, StructureMode, StructureNode};
use rhwp::model::control::Control;
use rhwp::model::document::Document;
use rhwp::model::paragraph::Paragraph;
use rhwp::model::shape::ShapeObject;
use rhwp::model::table::Table;
use std::collections::HashSet;

/// 글상자·표 셀 중첩 깊이 상한 — 손상·악의적 문서의 병적 중첩에서 스택을 지킨다.
const MAX_DEPTH: usize = 8;
/// 표 하나를 조밀 격자로 펼칠 때 허용하는 최대 칸 수(넘으면 셀을 문단으로 나열).
const DENSE_CELL_CAP: usize = 200_000;
/// 세로 병합 셀을 아래 행에 되풀이할 최대 길이 — "구분" 같은 짧은 이름표만 되풀이한다.
const REPEAT_LABEL_MAX_CHARS: usize = 40;

pub(crate) fn document_blocks(doc: &Document) -> Vec<Block> {
    let mut headings = HashSet::new();
    collect_headings(&build_structure(doc, StructureMode::Auto).roots, &mut headings);
    let mut out = Vec::new();
    for (s, section) in doc.sections.iter().enumerate() {
        for (p, para) in section.paragraphs.iter().enumerate() {
            let kind = if headings.contains(&(s, p)) { BlockKind::Heading } else { BlockKind::Paragraph };
            push_text(&mut out, kind, &para.text);
            for c in &para.controls {
                control_blocks(c, &mut out, 0);
            }
        }
    }
    out
}

fn collect_headings(nodes: &[StructureNode], out: &mut HashSet<(usize, usize)>) {
    for n in nodes {
        out.insert((n.section, n.paragraph));
        collect_headings(&n.children, out);
    }
}

fn is_private_use(c: char) -> bool {
    matches!(c as u32, 0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD)
}

/// 한컴 전용 글자(PUA) 제거, 연속 공백 정리.
pub(crate) fn clean(s: &str) -> String {
    s.chars().map(|c| if is_private_use(c) { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn push_text(out: &mut Vec<Block>, kind: BlockKind, text: &str) {
    let t = clean(text);
    if !t.is_empty() {
        out.push(Block { kind, text: t });
    }
}

/// 본문 밖 문단(글상자·각주·셀 안): 글자는 문단으로, 안에 든 개체는 재귀.
fn inner_paragraph(p: &Paragraph, out: &mut Vec<Block>, depth: usize) {
    push_text(out, BlockKind::Paragraph, &p.text);
    for c in &p.controls {
        control_blocks(c, out, depth + 1);
    }
}

fn control_blocks(c: &Control, out: &mut Vec<Block>, depth: usize) {
    if depth >= MAX_DEPTH {
        return;
    }
    match c {
        Control::Table(t) => table_blocks(t, out, depth),
        Control::Shape(s) => shape_blocks(s, out, depth),
        Control::Footnote(f) => f.paragraphs.iter().for_each(|p| inner_paragraph(p, out, depth)),
        Control::Endnote(e) => e.paragraphs.iter().for_each(|p| inner_paragraph(p, out, depth)),
        // 머리말·꼬리말은 쪽마다 되풀이되는 장식이라 색인하지 않는다.
        _ => {}
    }
}

fn shape_blocks(s: &ShapeObject, out: &mut Vec<Block>, depth: usize) {
    if depth >= MAX_DEPTH {
        return;
    }
    match s {
        ShapeObject::Group(g) => g.children.iter().for_each(|c| shape_blocks(c, out, depth + 1)),
        other => {
            if let Some(tb) = other.drawing().and_then(|d| d.text_box.as_ref()) {
                tb.paragraphs.iter().for_each(|p| inner_paragraph(p, out, depth));
            }
        }
    }
}

/// 표 → TableHeader("열1 | 열2") + TableRow("열1: 값 | 열2: 값", 빈 칸 생략).
/// 1행·1열 표(공문서의 글상자 대용 표)와 병적으로 큰 표는 셀 문단을 그대로 문단으로 낸다.
/// 셀 안의 표·글상자는 표 뒤에 이어서 낸다.
fn table_blocks(t: &Table, out: &mut Vec<Block>, depth: usize) {
    if let Some(cap) = &t.caption {
        cap.paragraphs.iter().for_each(|p| push_text(out, BlockKind::Paragraph, &p.text));
    }
    let (rows, cols) = (t.row_count as usize, t.col_count as usize);
    let mut cells: Vec<_> = t.cells.iter().filter(|c| (c.row as usize) < rows && (c.col as usize) < cols).collect();
    cells.sort_by_key(|c| (c.row, c.col));
    if rows <= 1 || cols <= 1 || rows.saturating_mul(cols) > DENSE_CELL_CAP {
        for c in &cells {
            c.paragraphs.iter().for_each(|p| inner_paragraph(p, out, depth));
        }
        return;
    }

    let mut header_flags = vec![false; rows];
    for c in &cells {
        if c.is_header {
            let end = (c.row as usize + c.row_span.max(1) as usize).min(rows);
            header_flags[c.row as usize..end].iter_mut().for_each(|f| *f = true);
        }
    }
    let header_rows = header_flags.iter().take_while(|f| **f).count().max(1);

    let mut dense = vec![vec![String::new(); cols]; rows];
    for c in &cells {
        let text = clean(&c.paragraphs.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join(" "));
        let (r0, c0) = (c.row as usize, c.col as usize);
        let r1 = (r0 + c.row_span.max(1) as usize).min(rows);
        let c1 = (c0 + c.col_span.max(1) as usize).min(cols);
        let short = text.chars().count() <= REPEAT_LABEL_MAX_CHARS;
        for (r, row) in dense.iter_mut().enumerate().take(r1).skip(r0) {
            let header = r < header_rows;
            for (col, slot) in row.iter_mut().enumerate().take(c1).skip(c0) {
                // 머리 행은 병합 범위 전체에, 본문은 짧은 이름표만 세로로 되풀이한다.
                let fill = (r == r0 && col == c0) || header || (col == c0 && short);
                if fill {
                    slot.clone_from(&text);
                }
            }
        }
    }

    let labels: Vec<String> = (0..cols)
        .map(|col| {
            let mut parts: Vec<&str> = Vec::new();
            for row in &dense[..header_rows] {
                let v = row[col].as_str();
                if !v.is_empty() && !parts.contains(&v) {
                    parts.push(v);
                }
            }
            parts.join(" ")
        })
        .collect();
    let mut header: Vec<&str> = Vec::new();
    for l in labels.iter().filter(|l| !l.is_empty()) {
        if header.last() != Some(&l.as_str()) {
            header.push(l);
        }
    }
    if !header.is_empty() {
        out.push(Block { kind: BlockKind::TableHeader, text: header.join(" | ") });
    }
    for row in &dense[header_rows..] {
        let parts: Vec<String> = row
            .iter()
            .zip(&labels)
            .filter(|(v, _)| !v.is_empty())
            .map(|(v, l)| if l.is_empty() || l == v { v.clone() } else { format!("{l}: {v}") })
            .collect();
        if !parts.is_empty() {
            out.push(Block { kind: BlockKind::TableRow, text: parts.join(" | ") });
        }
    }
    for c in &cells {
        for p in &c.paragraphs {
            p.controls.iter().for_each(|ctrl| control_blocks(ctrl, out, depth + 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::BlockKind::{self, *};
    use rhwp::model::control::Control;
    use rhwp::model::document::Section;
    use rhwp::model::footnote::Footnote;
    use rhwp::model::paragraph::Paragraph;
    use rhwp::model::shape::{DrawingObjAttr, GroupShape, RectangleShape, ShapeObject, TextBox};
    use rhwp::model::style::ParaShape;
    use rhwp::model::table::{Cell, Table};

    fn para(text: &str) -> Paragraph {
        Paragraph { text: text.to_string(), ..Paragraph::new_empty() }
    }

    fn with_control(text: &str, c: Control) -> Paragraph {
        let mut p = para(text);
        p.controls.push(c);
        p
    }

    fn doc_of(paras: Vec<Paragraph>) -> Document {
        let mut d = Document::default();
        d.doc_info.para_shapes.push(ParaShape::default());
        d.sections.push(Section { paragraphs: paras, ..Section::default() });
        d
    }

    fn cell(row: u16, col: u16, text: &str, is_header: bool) -> Cell {
        Cell { row, col, row_span: 1, col_span: 1, is_header, paragraphs: vec![para(text)], ..Cell::default() }
    }

    fn table(rows: u16, cols: u16, cells: Vec<Cell>) -> Control {
        Control::Table(Box::new(Table { row_count: rows, col_count: cols, cells, ..Table::default() }))
    }

    fn textbox(lines: &[&str]) -> ShapeObject {
        let tb = TextBox { paragraphs: lines.iter().map(|l| para(l)).collect(), ..TextBox::default() };
        ShapeObject::Rectangle(RectangleShape {
            drawing: DrawingObjAttr { text_box: Some(tb), ..DrawingObjAttr::default() },
            ..RectangleShape::default()
        })
    }

    fn all_text(blocks: &[Block]) -> String {
        blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n")
    }

    fn kinds(blocks: &[Block]) -> Vec<BlockKind> {
        blocks.iter().map(|b| b.kind.clone()).collect()
    }

    #[test]
    fn bodiless_clause_lines_are_kept() {
        // 규정 조문: 조 제목 아래 항·호만 있고 "본문" 문단이 없다.
        let d = doc_of(vec![
            para("제1장 총칙"),
            para("제1조(목적) 이 규정은 한국마사회의 내부통제에 관한 사항을 정한다."),
            para("제2조(정의) 이 규정에서 사용하는 용어의 뜻은 다음과 같다."),
            para("1. \"내부통제\"란 조직의 목표 달성을 위한 과정을 말한다."),
            para("2. \"리스크\"란 목표 달성을 저해하는 사건을 말한다."),
            para("제3조(적용범위) ① 이 규정은 모든 조직 구성원에게 적용한다."),
            para("② 다른 규정에 특별한 정함이 있는 경우에는 그에 따른다."),
        ]);
        let text = all_text(&document_blocks(&d));
        for needle in ["내부통제에 관한 사항", "\"내부통제\"란", "\"리스크\"란", "모든 조직 구성원", "특별한 정함"] {
            assert!(text.contains(needle), "{needle} 누락:\n{text}");
        }
    }

    #[test]
    fn textbox_and_grouped_textbox_text_is_kept() {
        let group = ShapeObject::Group(GroupShape { children: vec![textbox(&["묶음 안 글상자 문장"])], ..GroupShape::default() });
        let d = doc_of(vec![
            with_control("", Control::Shape(Box::new(textbox(&["보도 일시 2026. 10. 13.", "담당 부서 윤리경영부"])))),
            with_control("", Control::Shape(Box::new(group))),
        ]);
        let blocks = document_blocks(&d);
        let text = all_text(&blocks);
        assert!(text.contains("보도 일시 2026. 10. 13."), "{text}");
        assert!(text.contains("담당 부서 윤리경영부"), "{text}");
        assert!(text.contains("묶음 안 글상자 문장"), "{text}");
        assert!(blocks.iter().all(|b| b.kind == Paragraph));
    }

    #[test]
    fn footnote_text_is_kept() {
        let note = Footnote { paragraphs: vec![para("각주: 2025년 기준 통계")], ..Footnote::default() };
        let d = doc_of(vec![with_control("본문 문장", Control::Footnote(Box::new(note)))]);
        let text = all_text(&document_blocks(&d));
        assert!(text.contains("본문 문장") && text.contains("2025년 기준 통계"), "{text}");
    }

    #[test]
    fn table_becomes_header_and_labelled_rows_after_its_paragraph() {
        let t = table(3, 2, vec![
            cell(0, 0, "구분", true), cell(0, 1, "금액", true),
            cell(1, 0, "인건비", false), cell(1, 1, "100", false),
            cell(2, 0, "경비", false), cell(2, 1, "", false),
        ]);
        let blocks = document_blocks(&doc_of(vec![with_control("예산 현황", t)]));
        assert_eq!(kinds(&blocks), vec![Paragraph, TableHeader, TableRow, TableRow]);
        assert_eq!(blocks[0].text, "예산 현황");
        assert_eq!(blocks[1].text, "구분 | 금액");
        assert_eq!(blocks[2].text, "구분: 인건비 | 금액: 100");
        assert_eq!(blocks[3].text, "구분: 경비");
    }

    #[test]
    fn first_row_is_header_when_no_cell_is_marked() {
        let t = table(2, 2, vec![cell(0, 0, "현행", false), cell(0, 1, "개정안", false), cell(1, 0, "연 2회", false), cell(1, 1, "연 1회", false)]);
        let blocks = document_blocks(&doc_of(vec![with_control("", t)]));
        assert_eq!(blocks[0].text, "현행 | 개정안");
        assert_eq!(blocks[1].text, "현행: 연 2회 | 개정안: 연 1회");
    }

    #[test]
    fn vertically_merged_label_repeats_on_each_row() {
        let mut cat = cell(1, 0, "인건비", false);
        cat.row_span = 2;
        let t = table(3, 3, vec![
            cell(0, 0, "구분", true), cell(0, 1, "항목", true), cell(0, 2, "금액", true),
            cat, cell(1, 1, "기본급", false), cell(1, 2, "70", false),
            cell(2, 1, "수당", false), cell(2, 2, "30", false),
        ]);
        let blocks = document_blocks(&doc_of(vec![with_control("", t)]));
        assert_eq!(blocks[1].text, "구분: 인건비 | 항목: 기본급 | 금액: 70");
        assert_eq!(blocks[2].text, "구분: 인건비 | 항목: 수당 | 금액: 30");
    }

    #[test]
    fn horizontally_merged_header_labels_every_spanned_column() {
        let mut h = cell(0, 1, "예산", true);
        h.col_span = 2;
        let t = table(2, 3, vec![
            cell(0, 0, "부서", true), h,
            cell(1, 0, "윤리경영부", false), cell(1, 1, "10", false), cell(1, 2, "20", false),
        ]);
        let blocks = document_blocks(&doc_of(vec![with_control("", t)]));
        assert_eq!(blocks[0].text, "부서 | 예산");
        assert_eq!(blocks[1].text, "부서: 윤리경영부 | 예산: 10 | 예산: 20");
    }

    #[test]
    fn single_column_or_single_cell_box_is_plain_paragraphs() {
        // 공문서의 "글상자 대용 표"(1×1)·한 열 표는 표가 아니라 본문이다.
        let mut c = cell(0, 0, "", false);
        c.paragraphs = vec![para("□ 추진 배경"), para("○ 내부통제 역할·책임 명확화")];
        let blocks = document_blocks(&doc_of(vec![with_control("", table(1, 1, vec![c]))]));
        assert_eq!(kinds(&blocks), vec![Paragraph, Paragraph]);
        assert_eq!(blocks[0].text, "□ 추진 배경");
        assert_eq!(blocks[1].text, "○ 내부통제 역할·책임 명확화");
    }

    #[test]
    fn nested_table_text_is_kept() {
        let inner = table(2, 2, vec![cell(0, 0, "회차", true), cell(0, 1, "일자", true), cell(1, 0, "제8차", false), cell(1, 1, "9.12.", false)]);
        let mut outer_cell = cell(1, 1, "", false);
        outer_cell.paragraphs = vec![with_control("", inner)];
        let outer = table(2, 2, vec![cell(0, 0, "구분", true), cell(0, 1, "내용", true), cell(1, 0, "심의", false), outer_cell]);
        let text = all_text(&document_blocks(&doc_of(vec![with_control("", outer)])));
        assert!(text.contains("제8차") && text.contains("9.12."), "{text}");
    }

    #[test]
    fn private_use_glyphs_are_cleaned_and_empty_paragraphs_dropped() {
        let blocks = document_blocks(&doc_of(vec![para("\u{F081C} 본인  서명"), para("   "), para("")]));
        assert_eq!(all_text(&blocks), "본인 서명");
    }
}
