//! 임베딩 모델별 입력 형식(접두어)과 차원 정보.

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelProfile {
    pub name: String,
    pub native_dim: usize,
    pub max_tokens: usize,
    pub query_template: String,
    pub doc_template: String,
}

impl ModelProfile {
    pub fn embeddinggemma2() -> Self {
        Self {
            name: "EmbeddingGemma 2".into(),
            native_dim: 768,
            max_tokens: 1024,
            query_template: "task: search result | query: {text}".into(),
            doc_template: "title: {title} | text: {text}".into(),
        }
    }

    pub fn render_query(&self, q: &str) -> String {
        self.query_template.replace("{text}", q)
    }

    pub fn render_doc(&self, title: Option<&str>, text: &str) -> String {
        let t = title.map(str::trim).filter(|t| !t.is_empty()).unwrap_or("none");
        self.doc_template.replace("{title}", t).replace("{text}", text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gemma_query_prompt() {
        assert_eq!(ModelProfile::embeddinggemma2().render_query("경마 매출"), "task: search result | query: 경마 매출");
    }
    #[test]
    fn gemma_doc_prompt_with_and_without_title() {
        let p = ModelProfile::embeddinggemma2();
        assert_eq!(p.render_doc(Some("보고서"), "본문"), "title: 보고서 | text: 본문");
        assert_eq!(p.render_doc(None, "본문"), "title: none | text: 본문");
        assert_eq!(p.render_doc(Some("  "), "본문"), "title: none | text: 본문");
    }
}
