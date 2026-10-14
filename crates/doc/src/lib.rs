//! 추출기와 임베딩이 함께 쓰는 문서 모델.

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BlockKind {
    Heading,
    Paragraph,
    TableHeader,
    TableRow,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Document {
    pub title: Option<String>,
    pub blocks: Vec<Block>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_roundtrips_json() {
        let d = Document { title: Some("제목".into()), blocks: vec![Block { kind: BlockKind::Heading, text: "Ⅰ. 개요".into() }] };
        let s = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<Document>(&s).unwrap(), d);
    }
}
