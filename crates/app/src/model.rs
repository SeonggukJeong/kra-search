//! 의미 검색 모델 찾기와 임베딩 지문 계산 — `index`(의미 검색 색인)와 `search`(검색어 임베딩)가 같은 단계를 쓴다.

use crate::vectors;
use embed::profile::ModelProfile;
use std::path::{Path, PathBuf};

/// 기본 모델 파일 이름(실행 파일 옆 `models\` 폴더에서 찾는다).
pub const MODEL_FILE: &str = "embeddinggemma-2-Q8_0.gguf";

/// meta kv 에 모델 해시 캐시("크기:수정시각:해시")를 두는 키.
pub const MODEL_HASH_KV: &str = "model_hash";

/// 실행 파일이 있는 폴더.
pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// 의미 검색 모델 경로(파일이 있을 때만). `--model` 이 있으면 그것(상대 경로는 절대 경로로), 없으면 실행 파일 옆
/// `models\embeddinggemma-2-Q8_0.gguf`.
pub fn model_path(arg: &Option<PathBuf>) -> Option<PathBuf> {
    let p = match arg {
        Some(p) => std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()),
        None => exe_dir()?.join("models").join(MODEL_FILE),
    };
    p.is_file().then_some(p)
}

/// 지금 쓰는 모델 프로필(입력 형식).
pub fn profile() -> ModelProfile {
    ModelProfile::embeddinggemma2()
}

/// 모델 파일의 임베딩 지문과 kv 에 저장할 해시 캐시 문자열. `cached` 는 kv `model_hash` 에 있던 값 —
/// 크기·수정 시각이 같으면 모델(300MB)을 다시 읽지 않는다. 캐시 문자열은 부르는 쪽이 쓸 수 있으면 kv 에 저장한다.
pub fn fingerprint_of(path: &Path, cached: Option<&str>) -> std::io::Result<(String, String)> {
    let (hash, cache) = vectors::model_hash(cached, path)?;
    Ok((vectors::fingerprint(hash, &profile()), cache))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_model_is_none_and_fingerprint_matches_vectors() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(model_path(&Some(d.path().join("없음.gguf"))), None);
        let f = d.path().join("m.gguf");
        std::fs::write(&f, b"model bytes").unwrap();
        assert_eq!(model_path(&Some(f.clone())).as_deref(), Some(f.as_path()));
        let (fp, cache) = fingerprint_of(&f, None).unwrap();
        let (h, _) = vectors::model_hash(None, &f).unwrap();
        assert_eq!(fp, vectors::fingerprint(h, &profile()));
        assert_eq!(fingerprint_of(&f, Some(&cache)).unwrap(), (fp, cache), "캐시로도 같은 지문");
    }
}
