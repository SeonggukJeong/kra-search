//! 시험용 임베더 — CI 가 모델 없이 색인·검색 파이프라인 전체를 돌리려고. 글자 bigram 을 해시해 768차원에 더한다:
//! 결정적이고, 글자가 겹치는 글끼리 비슷하다. 품질 평가에는 쓰지 않는다.

use crate::api::Embed;
use crate::engine::EmbedError;

#[derive(Debug, Default)]
pub struct FakeEmbedder {
    /// embed 호출 횟수(시험이 "다시 임베딩하지 않았다"를 확인)
    pub calls: usize,
    /// 이 글자를 담은 입력이 있으면 그 묶음을 실패시킨다(실패 처리 시험)
    pub fail_on: Option<String>,
}

pub fn fake_vector(t: &str) -> Vec<f32> {
    let cs: Vec<char> = t.chars().filter(|c| !c.is_whitespace()).collect();
    let mut v = vec![0f32; 768];
    v[0] = 1e-3; // 빈 글도 0벡터가 되지 않게
    for w in cs.windows(2) {
        let mut h: u64 = 0xcbf29ce484222325;
        let mut buf = [0u8; 8];
        for c in w {
            for b in c.encode_utf8(&mut buf).bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
        }
        let sign = if (h >> 40) & 1 == 1 { -1.0 } else { 1.0 };
        v[(h % 768) as usize] += sign;
    }
    v
}

impl Embed for FakeEmbedder {
    fn embed(&mut self, inputs: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        self.calls += 1;
        if let Some(f) = &self.fail_on {
            if inputs.iter().any(|t| t.contains(f.as_str())) {
                return Err(EmbedError::Decode(-9));
            }
        }
        Ok(inputs.iter().map(|t| fake_vector(t)).collect())
    }

    fn count_tokens(&self, text: &str) -> usize {
        text.chars().count() + 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::cosine;

    #[test]
    fn deterministic_and_text_sensitive() {
        assert_eq!(fake_vector("내부통제 규정"), fake_vector("내부통제 규정"));
        let a = fake_vector("내부통제 규정 개정 이유");
        let b = fake_vector("내부통제 규정을 개정한 이유");
        let c = fake_vector("경주마 수송 차량 냉방");
        assert!(cosine(&a, &b) > cosine(&a, &c));
        assert_eq!(a.len(), 768);
    }

    #[test]
    fn fail_on_marks_inputs() {
        let mut f = FakeEmbedder { fail_on: Some("고장".into()), ..Default::default() };
        assert!(f.embed(&["정상".into()]).is_ok());
        assert!(f.embed(&["정상".into(), "고장 입력".into()]).is_err());
        assert_eq!(f.calls, 2);
        assert_eq!(f.count_tokens("가나"), 4);
    }
}
