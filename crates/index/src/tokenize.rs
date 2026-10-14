//! tantivy 용 한국어 토크나이저 두 개(스펙 §4·§8.2):
//! - 형태소(lindera + mecab-ko-dic): 조사·어미·기호를 빼고 내용 형태소만. 용언은 어간("달렸고" → "달리").
//! - 한글 bigram: 한글 구간에서만 2글자씩, **실제 위치**를 기록해 구절 검색이 된다(tantivy NgramTokenizer 는 위치가 0).
//!
//! 둘 다 NFKC 정규화 + 소문자. 오프셋은 정규화한 글자 기준.
//! 토큰 규칙(KEEP_POS·RUN_GAP·MAX_TOKEN_CHARS·정규화)이나 lindera·사전 버전을 바꾸면 `keyword::KEYWORD_VERSION` 을 올린다
//! (이미 만든 색인의 토큰과 검색어 토큰이 달라져 조용히 못 찾게 되므로 — 버전이 다르면 다시 색인한다).

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use std::borrow::Cow;
use std::sync::Arc;
use tantivy::tokenizer::{Token, TokenStream, Tokenizer};
use unicode_normalization::UnicodeNormalization;

pub const MORPH: &str = "ko_morph";
pub const BIGRAM: &str = "ko_bigram";
pub const MAX_TOKEN_CHARS: usize = 40;

fn normalize(text: &str) -> String {
    text.nfkc().collect()
}

/// 미리 계산한 토큰을 흘려보내는 TokenStream.
pub struct VecTokenStream {
    tokens: Vec<Token>,
    cursor: usize,
}

impl TokenStream for VecTokenStream {
    fn advance(&mut self) -> bool {
        if self.cursor < self.tokens.len() {
            self.cursor += 1;
            true
        } else {
            false
        }
    }
    fn token(&self) -> &Token {
        &self.tokens[self.cursor - 1]
    }
    fn token_mut(&mut self) -> &mut Token {
        &mut self.tokens[self.cursor - 1]
    }
}

fn token(from: usize, to: usize, position: usize, text: String) -> Token {
    Token { offset_from: from, offset_to: to, position, text, position_length: 1 }
}

// ------------------------------------------------------------------ 형태소

/// 남길 품사(mecab-ko-dic). 나머지(조사 J*, 어미 E*, 접사 X[PS]*, 기호 S[FPSOCEY], 의존명사 NNB/NNBC,
/// 대명사 NP, 관형사 MM, 부사 MA*, 감탄사 IC, 지정사 VC*, 보조용언 VX)는 버린다. 사전에 없는 말(UNKNOWN)은 남긴다.
const KEEP_POS: &[&str] = &["NNG", "NNP", "NR", "VV", "VA", "XR", "SL", "SH", "SN", "UNKNOWN"];

fn keep(tag: &str) -> bool {
    KEEP_POS.contains(&tag)
}

#[derive(Clone)]
pub struct KoMorphTokenizer {
    segmenter: Arc<Segmenter>,
}

impl KoMorphTokenizer {
    /// 위치: 형태소마다 1씩 올린다(활용형은 분해된 형태소마다). 버린 형태소도 위치를 차지한다(구절이 조사를 건너뛰어 맞지 않게).
    pub fn analyze(&self, text: &str) -> Vec<Token> {
        let text = normalize(text);
        let mut out = Vec::new();
        let Ok(mut ltoks) = self.segmenter.segment(Cow::Borrowed(text.as_str())) else {
            return out;
        };
        let mut pos = 0usize;
        for lt in ltoks.iter_mut() {
            let (from, to) = (lt.byte_start, lt.byte_end);
            let surface = lt.surface.to_string();
            let tag = lt.get_detail(0).unwrap_or("UNKNOWN").to_string();
            let emit = |form: &str, out: &mut Vec<Token>, pos: usize| {
                if !form.is_empty() && form.chars().count() <= MAX_TOKEN_CHARS {
                    out.push(token(from, to, pos, form.to_lowercase()));
                }
            };
            if tag.contains('+') {
                // 예: "했" = XSV+EP, 8번째 필드 = "하/XSV/*+았/EP/*"
                let expr = lt.get_detail(7).unwrap_or("*").to_string();
                for part in expr.split('+') {
                    let mut it = part.split('/');
                    let (form, ptag) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
                    if keep(ptag) {
                        emit(form, &mut out, pos);
                    }
                    pos += 1;
                }
            } else {
                if keep(&tag) {
                    emit(&surface, &mut out, pos);
                }
                pos += 1;
            }
        }
        out
    }
}

impl Tokenizer for KoMorphTokenizer {
    type TokenStream<'a> = VecTokenStream;
    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        VecTokenStream { tokens: self.analyze(text), cursor: 0 }
    }
}

// ------------------------------------------------------------------ 한글 bigram

fn is_hangul(c: char) -> bool {
    matches!(c, '\u{AC00}'..='\u{D7A3}' | '\u{1100}'..='\u{11FF}' | '\u{3130}'..='\u{318F}')
}

#[derive(Clone, Copy, PartialEq)]
enum Class {
    Hangul,
    Other, // 한글이 아닌 글자·숫자(한자 포함)
    Sep,
}

fn class(c: char) -> Class {
    if is_hangul(c) {
        Class::Hangul
    } else if c.is_alphanumeric() {
        Class::Other
    } else {
        Class::Sep
    }
}

/// 구간(run) 사이 위치 간격. 다른 어절의 bigram 이 이어져 구절로 잘못 맞는 것을 막는다("가나 나다" ≠ "가나다").
pub const RUN_GAP: usize = 1;

#[derive(Clone, Default)]
pub struct HangulBigramTokenizer;

impl HangulBigramTokenizer {
    pub fn analyze(text: &str) -> Vec<Token> {
        let text = normalize(text);
        let text = text.as_str();
        let mut out = Vec::new();
        let mut pos = 0usize;
        let mut first_run = true;
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let end_of = |i: usize| chars.get(i).map(|&(b, _)| b).unwrap_or(text.len());
        let mut i = 0;
        while i < chars.len() {
            let k = class(chars[i].1);
            if k == Class::Sep {
                i += 1;
                continue;
            }
            let start = i;
            while i < chars.len() && class(chars[i].1) == k {
                i += 1;
            }
            if !first_run {
                pos += RUN_GAP;
            }
            first_run = false;
            if k == Class::Hangul && i - start >= 2 {
                for (j, &(from, _)) in chars.iter().enumerate().take(i - 1).skip(start) {
                    let to = end_of(j + 2);
                    out.push(token(from, to, pos, text[from..to].to_string()));
                    pos += 1;
                }
            } else {
                // 1글자 한글 구간, 또는 한글 아닌 글자·숫자 구간 → 구간 전체가 토큰 하나
                let (from, to) = (chars[start].0, end_of(i));
                if i - start <= MAX_TOKEN_CHARS {
                    out.push(token(from, to, pos, text[from..to].to_lowercase()));
                }
                pos += 1;
            }
        }
        out
    }
}

impl Tokenizer for HangulBigramTokenizer {
    type TokenStream<'a> = VecTokenStream;
    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        VecTokenStream { tokens: Self::analyze(text), cursor: 0 }
    }
}

// ------------------------------------------------------------------ 등록

#[derive(Clone)]
pub struct Analyzers {
    pub morph: KoMorphTokenizer,
}

impl Analyzers {
    pub fn load() -> Result<Analyzers, String> {
        let dictionary = load_dictionary("embedded://ko-dic").map_err(|e| format!("형태소 사전: {e}"))?;
        let segmenter = Arc::new(Segmenter::new(Mode::Normal, dictionary, None));
        Ok(Analyzers { morph: KoMorphTokenizer { segmenter } })
    }
}

pub fn register(index: &tantivy::Index, a: &Analyzers) {
    index.tokenizers().register(MORPH, a.morph.clone());
    index.tokenizers().register(BIGRAM, HangulBigramTokenizer);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bigrams(s: &str) -> Vec<(String, usize)> {
        HangulBigramTokenizer::analyze(s).into_iter().map(|t| (t.text, t.position)).collect()
    }

    fn morphs(s: &str) -> Vec<(String, usize)> {
        let a = Analyzers::load().unwrap();
        a.morph.analyze(s).into_iter().map(|t| (t.text, t.position)).collect()
    }

    fn v(x: &[(&str, usize)]) -> Vec<(String, usize)> {
        x.iter().map(|(t, p)| (t.to_string(), *p)).collect()
    }

    #[test]
    fn bigram_positions_and_runs() {
        assert_eq!(
            bigrams("내부통제규정 KRA-2026 가"),
            v(&[("내부", 0), ("부통", 1), ("통제", 2), ("제규", 3), ("규정", 4), ("kra", 6), ("2026", 8), ("가", 10)])
        );
        assert_eq!(bigrams("2026년에"), v(&[("2026", 0), ("년에", 2)]));
    }

    #[test]
    fn fullwidth_is_normalized_and_long_runs_dropped() {
        assert_eq!(bigrams("ＫＲＡ２０２６"), v(&[("kra2026", 0)]));
        let long = "a".repeat(MAX_TOKEN_CHARS + 1);
        assert_eq!(bigrams(&format!("{long} 규정")), v(&[("규정", 2)]));
    }

    #[test]
    fn morph_drops_particles_and_endings() {
        assert_eq!(
            morphs("한국마사회는 내부통제규정을 2026년에 개정했다. KRA-2026 문서"),
            v(&[("한국", 0), ("마사회", 1), ("내부", 3), ("통제", 4), ("규정", 5), ("2026", 7), ("개정", 10), ("kra", 15), ("2026", 17), ("문서", 18)])
        );
    }

    #[test]
    fn morph_inflected_verbs_become_stems() {
        let texts: Vec<String> = morphs("말들이 달렸고 빠른 말이 이겼습니다").into_iter().map(|x| x.0).collect();
        assert_eq!(texts, vec!["말", "달리", "빠르", "말", "이기"]);
    }
}
