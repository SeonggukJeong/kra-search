//! 임베딩 캐시: target/m0cache/<모델>/<종류>.json — (id, 768차원 원본 벡터) 목록.

use std::path::PathBuf;

pub type Vectors = Vec<(String, Vec<f32>)>;

fn path(model: &str, kind: &str) -> PathBuf {
    PathBuf::from("target/m0cache").join(model).join(format!("{kind}.json"))
}

pub fn get_or_compute(model: &str, kind: &str, compute: impl FnOnce() -> Vectors) -> Vectors {
    let p = path(model, kind);
    if let Ok(s) = std::fs::read_to_string(&p) {
        if let Ok(v) = serde_json::from_str(&s) {
            return v;
        }
    }
    let v = compute();
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, serde_json::to_string(&v).unwrap()).unwrap();
    v
}
