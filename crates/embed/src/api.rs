//! 임베딩 엔진 추상화: 앱 파이프라인이 실제 모델(`Embedder`)과 시험용 가짜(`FakeEmbedder`)를 같은 길로 쓴다.

use crate::engine::{EmbedError, Embedder};

pub trait Embed {
    /// 원본 차원(정규화 전) 벡터, 입력 순서대로. 공백뿐인 입력은 넣지 않는다(부르는 쪽 책임).
    fn embed(&mut self, inputs: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;
    /// 카드·청크 상한을 재는 토큰 수(실제: 모델 토크나이저).
    fn count_tokens(&self, text: &str) -> usize;
}

impl Embed for Embedder {
    fn embed(&mut self, inputs: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(Embedder::embed(self, inputs)?.into_iter().map(|e| e.vector).collect())
    }

    fn count_tokens(&self, text: &str) -> usize {
        self.token_count(text).unwrap_or_else(|_| text.chars().count())
    }
}
