//! 검색: 키워드 색인 → 내용 id → meta 의 제목·경로.

use crate::store::{user_error, Store};
use std::path::Path;

#[derive(Debug)]
pub struct Hit {
    pub content_id: i64,
    pub score: f32,
    pub title: String,
    /// 같은 내용의 경로들(최근 수정 먼저)
    pub paths: Vec<String>,
    pub fail: Option<String>,
}

/// 실패 코드(`FailKind::code`·"Timeout"·"Crashed") → 화면에 보일 쉬운 말.
/// 용어는 UI D7 표를 따른다: DRM → "보안 문서", 추출 실패 → "열 수 없는 파일"(스펙 §3.3).
pub fn fail_label(fail: &str) -> &'static str {
    match fail {
        "Drm" => "보안 문서 — 파일 이름으로만 찾을 수 있음",
        "Encrypted" => "암호가 걸린 문서 — 파일 이름으로만 찾을 수 있음",
        "TooLarge" => "100MB 넘는 파일 — 파일 이름으로만 찾을 수 있음",
        "TooComplex" => "내용이 너무 많은 문서 — 파일 이름으로만 찾을 수 있음",
        "NoText" => "글자가 없는 문서(스캔 등) — 파일 이름으로만 찾을 수 있음",
        "Timeout" => "읽는 데 너무 오래 걸린 파일 — 파일 이름으로만 찾을 수 있음",
        _ => "열 수 없는 파일 — 파일 이름으로만 찾을 수 있음",
    }
}

pub fn search(store: &Store, query: &str, limit: usize) -> Result<Vec<Hit>, String> {
    let hits = store.keyword.search(query, limit).map_err(|e| format!("검색 색인 오류: {e}"))?;
    let mut out = Vec::new();
    for (id, score) in hits {
        let id = id as i64;
        let Some(c) = store.meta.content(id).map_err(user_error)? else { continue };
        let paths: Vec<String> = store.meta.files_of(id).map_err(user_error)?.into_iter().map(|f| f.path).collect();
        if paths.is_empty() {
            continue; // 아직 반영 안 된 삭제
        }
        let title = c.title.clone().unwrap_or_else(|| {
            Path::new(&paths[0]).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string()
        });
        out.push(Hit { content_id: id, score, title, paths, fail: c.fail });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::fail_label;

    /// 100MB 상한(TooLarge)과 내부 상한(TooComplex)은 다른 안내 — 3MB ODS 가 "100MB 넘는 파일"로 보이면 안 된다.
    #[test]
    fn too_complex_is_not_shown_as_too_large() {
        assert!(fail_label("TooLarge").starts_with("100MB 넘는 파일"));
        assert_eq!(fail_label("TooComplex"), "내용이 너무 많은 문서 — 파일 이름으로만 찾을 수 있음");
        assert!(fail_label("Corrupt").starts_with("열 수 없는 파일"));
    }
}
