//! 일반 표(행 × 칸 글자) → TableHeader("열1 | 열2") + TableRow("열1: 값 | 열2: 값").
//! HWP 표는 병합 정보를 쓰는 walk.rs 가 따로 처리한다.

use crate::norm::{clean, push};
use doc::{Block, BlockKind};

/// 머리글 행 = 채워진 칸이 min(2, 최대 채움 수) 이상인 첫 행. 그 앞 행(표 위 제목 줄 등)은 문단으로 낸다.
/// 머리글 칸이 비면 값만 쓰고, 값이 빈 칸은 생략하며, 값이 모두 빈 행은 버린다.
pub(crate) fn push_table(out: &mut Vec<Block>, rows: &[Vec<String>]) {
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| r.iter().map(|c| clean(c)).collect::<Vec<_>>())
        .filter(|r| r.iter().any(|c| !c.is_empty()))
        .collect();
    let filled = |r: &Vec<String>| r.iter().filter(|c| !c.is_empty()).count();
    let Some(max_filled) = rows.iter().map(filled).max() else {
        return;
    };
    let h = rows.iter().position(|r| filled(r) >= max_filled.min(2)).unwrap_or(0);
    for pre in &rows[..h] {
        push(out, BlockKind::Paragraph, &pre.join(" "));
    }
    let header = &rows[h];
    // 머리글 끝쪽 빈 칸은 잘라낸다(시트의 넓은 사용 범위 대비).
    let width = header.iter().rposition(|c| !c.is_empty()).map_or(0, |i| i + 1);
    push(out, BlockKind::TableHeader, &header[..width].join(" | "));
    for row in &rows[h + 1..] {
        let parts: Vec<String> = row
            .iter()
            .enumerate()
            .filter(|(_, v)| !v.is_empty())
            .map(|(i, v)| match header.get(i).filter(|h| !h.is_empty()) {
                Some(h) => format!("{h}: {v}"),
                None => v.clone(),
            })
            .collect();
        push(out, BlockKind::TableRow, &parts.join(" | "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(v: &[&[&str]]) -> Vec<Vec<String>> {
        v.iter().map(|r| r.iter().map(|s| s.to_string()).collect()).collect()
    }

    #[test]
    fn title_row_above_table_is_paragraph() {
        let mut out = Vec::new();
        push_table(&mut out, &rows(&[&["2026년 1분기 실적", "", ""], &["경마장", "매출", "비고"], &["서울", "100", ""]]));
        let got: Vec<(BlockKind, &str)> = out.iter().map(|b| (b.kind.clone(), b.text.as_str())).collect();
        assert_eq!(got, vec![
            (BlockKind::Paragraph, "2026년 1분기 실적"),
            (BlockKind::TableHeader, "경마장 | 매출 | 비고"),
            (BlockKind::TableRow, "경마장: 서울 | 매출: 100"),
        ]);
    }

    #[test]
    fn empty_table_emits_nothing() {
        let mut out = Vec::new();
        push_table(&mut out, &rows(&[&["", ""], &[" ", ""]]));
        assert!(out.is_empty());
    }
}
