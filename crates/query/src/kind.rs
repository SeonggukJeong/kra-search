//! 검색어 유형(스펙 §8.1): 짧은 명사·문서번호·파일명 꼴 → 키워드형, 문장 → 의미형. 결합 가중치에 쓴다(§8.3).
//! 사람 이름 인식은 없다(KI-044) — 이름 두세 글자는 "짧은 낱말"로 키워드형이 된다.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    Keyword,
    Semantic,
}

const EXTS: &[&str] = &[".hwp", ".hwpx", ".pdf", ".docx", ".pptx", ".xlsx", ".odt", ".odp", ".ods", ".txt", ".csv"];

/// 키워드형으로 보는 글자 수 상한(낱말 사이 공백 제외). 잠정값이다 — 사람마다 검색 습관이 달라
/// M2 평가(Task 15·16)에서 "나누지 않음" 기준과 6·8·10자를 견줘 뚜렷이 나을 때만 유지한다.
pub const KEYWORD_MAX_CHARS: usize = 8;

/// 문서번호 꼴: 글자들-숫자(예: 경마기획부-1736).
fn doc_number(w: &str) -> bool {
    let Some((a, b)) = w.split_once('-') else { return false };
    !a.is_empty() && a.chars().all(char::is_alphabetic) && b.chars().next().is_some_and(|c| c.is_ascii_digit())
}

fn file_like(w: &str) -> bool {
    let l = w.to_lowercase();
    EXTS.iter().any(|e| l.ends_with(e)) || w.contains('_')
}

pub fn classify(text: &str) -> QueryKind {
    classify_with(text, KEYWORD_MAX_CHARS)
}

/// 문턱을 바꿔 가며 평가할 수 있게 `max_chars` 를 받는다.
pub fn classify_with(text: &str, max_chars: usize) -> QueryKind {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() || words.iter().any(|w| doc_number(w) || file_like(w)) {
        return QueryKind::Keyword;
    }
    let chars: usize = words.iter().map(|w| w.chars().count()).sum();
    if words.len() <= 2 && chars <= max_chars {
        QueryKind::Keyword
    } else {
        QueryKind::Semantic
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_nouns_numbers_and_file_names_are_keyword() {
        for q in ["더비온", "과기정통부 예산", "경마기획부-1736", "내부통제규정_개정전문.hwp", "보고서.pdf", "K-water"] {
            assert_eq!(classify(q), QueryKind::Keyword, "{q}");
        }
    }

    #[test]
    fn sentences_are_semantic() {
        for q in ["일 열심히 하다 실수했을 때 징계 안 받는 경우", "새 사업 시작하기 전에 미리 확인해야 하는 것", "부경 주차장 밤에 불 꺼진 민원"] {
            assert_eq!(classify(q), QueryKind::Semantic, "{q}");
        }
    }

    /// 글자 수 문턱은 평가에서 훑어 본다 — "국정감사 지적사항"은 4+4=8자.
    #[test]
    fn threshold_is_sweepable() {
        assert_eq!(classify_with("국정감사 지적사항", 6), QueryKind::Semantic);
        assert_eq!(classify_with("국정감사 지적사항", 8), QueryKind::Keyword);
        assert_eq!(classify_with("국정감사 지적사항", 10), QueryKind::Keyword);
        assert_eq!(classify_with("국정감사 지적사항 결과", 10), QueryKind::Semantic);
        assert_eq!(classify("국정감사 지적사항"), classify_with("국정감사 지적사항", KEYWORD_MAX_CHARS));
    }
}
