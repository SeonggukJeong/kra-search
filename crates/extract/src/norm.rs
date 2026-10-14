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
pub(crate) fn clean_lines(s: &str) -> String {
    s.lines().map(clean).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// 정리한 글자가 비어 있지 않으면 블록으로 추가한다.
pub(crate) fn push(out: &mut Vec<Block>, kind: BlockKind, text: &str) {
    let t = clean_lines(text);
    if !t.is_empty() {
        out.push(Block { kind, text: t });
    }
}

/// 프로그램이 새 문서에 넣는 기본 제목(소문자, 공백 하나로 정리한 뒤 비교).
const PLACEHOLDER_TITLES: &[&str] = &[
    "untitled",
    "powerpoint 프레젠테이션",
    "powerpoint presentation",
    "프레젠테이션 제목",
    "슬라이드 제목",
    "제목 없음",
    "제목을 입력하십시오",
    "제목을 입력하세요",
];

/// 기본 파일 이름 꼴 "문서1"·"Document1"·"프레젠테이션1"·"Presentation12"(뒤에 숫자 1~3자리).
fn default_numbered_name(lower: &str) -> bool {
    let base = lower.trim_end_matches(|c: char| c.is_ascii_digit());
    let digits = lower.len() - base.len();
    (1..=3).contains(&digits) && ["문서", "document", "프레젠테이션", "presentation"].contains(&base.trim_end())
}

/// 메타데이터 제목이 믿을 만한가. 프로그램 기본 제목, 원본 파일 이름(…hwp), 깨진 Latin-1 은 버린다.
pub(crate) fn usable_title(raw: &str) -> Option<String> {
    if raw.contains('\0') {
        return None;
    }
    let t = clean(raw);
    if t.is_empty() {
        return None;
    }
    let lower = t.to_lowercase();
    if PLACEHOLDER_TITLES.contains(&lower.as_str()) || default_numbered_name(&lower) || lower.starts_with("microsoft word - ") {
        return None;
    }
    if [".hwp", ".hwpx", ".doc", ".docx", ".pdf", ".xls", ".xlsx", ".ppt", ".pptx"].iter().any(|e| lower.ends_with(e)) {
        return None;
    }
    let n = t.chars().count();
    let latin1 = t.chars().filter(|c| ('\u{80}'..='\u{FF}').contains(c)).count();
    let hangul = t.chars().filter(|c| ('\u{AC00}'..='\u{D7A3}').contains(c)).count();
    if hangul == 0 && latin1 * 2 > n {
        return None;
    }
    Some(t)
}

/// 메타데이터 제목(usable_title 통과) → 없으면 첫 Heading 블록 글자.
pub(crate) fn title_or_heading(meta: Option<String>, blocks: &[Block]) -> Option<String> {
    meta.as_deref()
        .and_then(usable_title)
        .or_else(|| blocks.iter().find(|b| b.kind == BlockKind::Heading).map(|b| b.text.clone()))
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

    #[test]
    fn placeholder_titles_are_dropped() {
        // PDF(Task 8 에서 옮겨 온 판정)
        assert_eq!(usable_title("Untitled"), None);
        assert_eq!(usable_title("2026년 계획(안).hwp"), None);
        assert_eq!(usable_title("Ã«Â¬Â¸Ã¬Â\u{84}Â\u{9c}"), None);
        assert_eq!(usable_title("제목\0"), None);
        // 사무 프로그램이 새 문서에 넣는 기본 제목
        for t in ["PowerPoint 프레젠테이션", "powerpoint presentation", "프레젠테이션 제목", "슬라이드 제목", "제목 없음",
                  "제목을 입력하십시오", "제목을 입력하세요", "문서1", "Document1", "프레젠테이션1", "Presentation12",
                  "Microsoft Word - 보고서", "  PowerPoint   프레젠테이션 "] {
            assert_eq!(usable_title(t), None, "{t}");
        }
        // 진짜 제목은 정리만
        assert_eq!(usable_title(" 2026년 더비온 운영계획 ").as_deref(), Some("2026년 더비온 운영계획"));
        assert_eq!(usable_title("문서 관리 규정").as_deref(), Some("문서 관리 규정"));
        assert_eq!(usable_title("프레젠테이션 기법 교육").as_deref(), Some("프레젠테이션 기법 교육"));
        assert_eq!(usable_title("2026").as_deref(), Some("2026"));
    }

    #[test]
    fn title_falls_back_to_first_heading() {
        let b = vec![
            Block { kind: BlockKind::Paragraph, text: "앞 문단".into() },
            Block { kind: BlockKind::Heading, text: "첫 제목".into() },
            Block { kind: BlockKind::Heading, text: "둘째 제목".into() },
        ];
        assert_eq!(title_or_heading(Some("제목 없음".into()), &b).as_deref(), Some("첫 제목"));
        assert_eq!(title_or_heading(None, &b).as_deref(), Some("첫 제목"));
        assert_eq!(title_or_heading(Some(" 진짜 제목 ".into()), &b).as_deref(), Some("진짜 제목"));
        assert_eq!(title_or_heading(None, &b[..1]), None);
    }
}
