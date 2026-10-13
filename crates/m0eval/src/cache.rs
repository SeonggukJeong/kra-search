//! 임베딩 캐시: target/m0cache/<모델>/<종류>-<입력 해시>.json — (id, 768차원 원본 벡터) 목록.
//! 입력(추출 결과·청크 설정·프롬프트)이 바뀌면 해시가 달라져 옛 캐시를 쓰지 않는다.

use std::path::PathBuf;

pub type Vectors = Vec<(String, Vec<f32>)>;
pub type Inputs = Vec<(String, String)>;

fn path(model: &str, kind: &str, inputs: &Inputs) -> PathBuf {
    PathBuf::from("target/m0cache").join(model).join(format!("{kind}-{:016x}.json", inputs_hash(inputs)))
}

/// FNV-1a 64 — 러스트 버전이 바뀌어도 같은 값을 낸다(DefaultHasher 는 보장하지 않음).
fn inputs_hash(inputs: &Inputs) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for (id, text) in inputs {
        for b in id.bytes().chain([0u8]).chain(text.bytes()).chain([0xffu8]) {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    h
}

pub fn get_or_compute(model: &str, kind: &str, inputs: Inputs, compute: impl FnOnce(Inputs) -> Vectors) -> Vectors {
    let p = path(model, kind, &inputs);
    if let Ok(s) = std::fs::read_to_string(&p) {
        if let Ok(v) = serde_json::from_str(&s) {
            return v;
        }
    }
    let v = compute(inputs);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, serde_json::to_string(&v).unwrap()).unwrap();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(t: &str) -> Inputs {
        vec![("P0001".into(), t.into())]
    }

    #[test]
    fn changed_input_text_changes_cache_path() {
        assert_ne!(path("m", "chunk300", &inputs("제1조 목적")), path("m", "chunk300", &inputs("제1조 목적 ① 항")));
    }

    #[test]
    fn same_input_reuses_cache_path() {
        assert_eq!(path("m", "card", &inputs("가")), path("m", "card", &inputs("가")));
    }

    #[test]
    fn id_text_boundary_is_part_of_the_key() {
        let a = vec![("P1".to_string(), "0가".to_string())];
        let b = vec![("P10".to_string(), "가".to_string())];
        assert_ne!(inputs_hash(&a), inputs_hash(&b));
    }
}
