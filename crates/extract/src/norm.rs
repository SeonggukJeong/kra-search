//! 추출 글자 정리(모든 형식 공통).

use doc::{Block, BlockKind};

fn is_private_use(c: char) -> bool {
    matches!(c as u32, 0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD)
}

/// 같은 줄표 글자가 이만큼 이상 이어지면 지운다 — 신구대조표의 "------"(현행과 같음) 표시 등(KI-012).
const DASH_RUN_MIN: usize = 5;

fn is_dash(c: char) -> bool {
    matches!(c, '-' | '─' | '━' | '－' | '―' | '—')
}

/// 줄표가 DASH_RUN_MIN 개 이상 이어진 구간을 공백 하나로 바꾼다.
fn drop_dash_runs(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if is_dash(chars[i]) {
            let start = i;
            while i < chars.len() && chars[i] == chars[start] {
                i += 1;
            }
            if i - start >= DASH_RUN_MIN {
                out.push(' ');
            } else {
                out.extend(&chars[start..i]);
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// 한 줄로: 한컴 전용 글자(사설 영역) 제거, 긴 줄표 제거, 연속 공백(줄바꿈 포함)을 하나로.
pub(crate) fn clean(s: &str) -> String {
    let s: String = s.chars().map(|c| if is_private_use(c) { ' ' } else { c }).collect();
    drop_dash_runs(&s).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 줄 구조를 남기고 줄마다 `clean`. 빈 줄은 버린다.
#[allow(dead_code)] // 형식별 추출기(Task 3 이후)가 쓴다
pub(crate) fn clean_lines(s: &str) -> String {
    s.lines().map(clean).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// 정리한 글자가 비어 있지 않으면 블록으로 추가한다.
#[allow(dead_code)] // 형식별 추출기(Task 3 이후)가 쓴다
pub(crate) fn push(out: &mut Vec<Block>, kind: BlockKind, text: &str) {
    let t = clean_lines(text);
    if !t.is_empty() {
        out.push(Block { kind, text: t });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dash_runs_are_dropped_short_dashes_kept() {
        assert_eq!(clean("현행 ------ 개정안"), "현행 개정안");
        assert_eq!(clean("──────────"), "");
        assert_eq!(clean("KRA-2026 — 2--3"), "KRA-2026 — 2--3");
    }

    #[test]
    fn private_use_and_spaces_are_cleaned() {
        assert_eq!(clean("\u{F081C} 본인  서명\t끝"), "본인 서명 끝");
    }

    #[test]
    fn clean_lines_keeps_line_structure() {
        assert_eq!(clean_lines(" 첫 줄 \r\n\r\n  둘째   줄 \n"), "첫 줄\n둘째 줄");
        let mut v = Vec::new();
        push(&mut v, BlockKind::Paragraph, "  \n ");
        assert!(v.is_empty());
    }
}
