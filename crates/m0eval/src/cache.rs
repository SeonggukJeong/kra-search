//! 임베딩 캐시: target/m0cache/<모델>/items.jsonl — 텍스트 해시 → 768차원 원본 벡터, 한 줄에 하나.
//! 키는 임베딩에 들어간 텍스트(프롬프트·제목 포함) 자체라서 추출·청크 설정이 바뀌면 바뀐 항목만 다시 계산한다(INC-014).
//! 문서를 하나 더해도 그 문서의 텍스트만 계산한다(DEC-032). 같은 텍스트는 종류(card/chunk/query)와 상관없이 같은 벡터.
//! 캐시 형식을 다시 바꾸면 옛 캐시를 옮기는 경로를 함께 만든다(전체 재계산은 수 시간).

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

pub type Vectors = Vec<(String, Vec<f32>)>;
pub type Inputs = Vec<(String, String)>;

/// FNV-1a 64 — 러스트 버전이 바뀌어도 같은 값을 낸다(DefaultHasher 는 보장하지 않음).
fn fnv(bytes: impl Iterator<Item = u8>) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn text_key(text: &str) -> u64 {
    fnv(text.bytes())
}

/// 예전 형식(<종류>-<입력 전체 해시>.json) 파일 이름의 해시. 옛 캐시를 한 번 옮겨 오는 데만 쓴다.
fn inputs_hash(inputs: &Inputs) -> u64 {
    fnv(inputs.iter().flat_map(|(id, text)| id.bytes().chain([0u8]).chain(text.bytes()).chain([0xffu8])))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Line {
    k: String,
    v: Vec<f32>,
}

pub struct Store {
    dir: PathBuf,
    map: HashMap<u64, Vec<f32>>,
}

impl Store {
    pub fn open(dir: &Path) -> Store {
        let mut map = HashMap::new();
        if let Ok(f) = std::fs::File::open(dir.join("items.jsonl")) {
            for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
                // 쓰다가 끊긴 마지막 줄은 버린다(다시 계산됨).
                if let Ok(l) = serde_json::from_str::<Line>(&line) {
                    if let Ok(k) = u64::from_str_radix(&l.k, 16) {
                        map.insert(k, l.v);
                    }
                }
            }
        }
        Store { dir: dir.to_path_buf(), map }
    }

    fn append(&mut self, new: Vec<(u64, Vec<f32>)>) {
        if new.is_empty() {
            return;
        }
        std::fs::create_dir_all(&self.dir).unwrap();
        let f = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join("items.jsonl")).unwrap();
        let mut w = std::io::BufWriter::new(f);
        for (k, v) in new {
            writeln!(w, "{}", serde_json::to_string(&Line { k: format!("{k:016x}"), v: v.clone() }).unwrap()).unwrap();
            self.map.insert(k, v);
        }
        w.flush().unwrap();
    }

    /// 예전 형식 파일이 있으면 그 벡터를 항목별로 옮긴다(입력 순서와 같은 순서로 저장돼 있다).
    fn import_legacy(&mut self, kind: &str, inputs: &Inputs) {
        let p = self.dir.join(format!("{kind}-{:016x}.json", inputs_hash(inputs)));
        let Ok(s) = std::fs::read_to_string(&p) else { return };
        let Ok(old) = serde_json::from_str::<Vectors>(&s) else { return };
        if old.len() != inputs.len() {
            return;
        }
        let new = inputs
            .iter()
            .zip(old)
            .map(|((_, t), (_, v))| (text_key(t), v))
            .filter(|(k, _)| !self.map.contains_key(k))
            .collect();
        self.append(new);
    }

    /// 입력 순서대로 (id, 벡터)를 돌려준다. 캐시에 없는 텍스트만(중복 제거해) `compute` 에 넘긴다.
    pub fn get_or_compute(&mut self, kind: &str, inputs: Inputs, compute: impl FnOnce(Inputs) -> Vectors) -> Vectors {
        if inputs.iter().any(|(_, t)| !self.map.contains_key(&text_key(t))) {
            self.import_legacy(kind, &inputs);
        }
        let mut seen = std::collections::HashSet::new();
        let missing: Inputs = inputs
            .iter()
            .filter(|(_, t)| {
                let k = text_key(t);
                !self.map.contains_key(&k) && seen.insert(k)
            })
            .cloned()
            .collect();
        if !missing.is_empty() {
            eprintln!("  {kind}: {}/{} 항목 계산", missing.len(), inputs.len());
            let keys: Vec<u64> = missing.iter().map(|(_, t)| text_key(t)).collect();
            let vs = compute(missing);
            self.append(keys.into_iter().zip(vs.into_iter().map(|(_, v)| v)).collect());
        }
        inputs.into_iter().map(|(id, t)| { let v = self.map[&text_key(&t)].clone(); (id, v) }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("m0cache-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn items(v: &[(&str, &str)]) -> Inputs {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    /// 텍스트 길이를 벡터 값으로 쓰는 가짜 임베딩. 호출된 텍스트를 기록한다.
    fn fake<'a>(log: &'a RefCell<Vec<String>>) -> impl FnOnce(Inputs) -> Vectors + 'a {
        move |i| {
            i.into_iter()
                .map(|(id, t)| {
                    log.borrow_mut().push(t.clone());
                    (id, vec![t.chars().count() as f32])
                })
                .collect()
        }
    }

    #[test]
    fn adding_a_document_embeds_only_the_new_text() {
        let d = dir("add");
        let log = RefCell::new(Vec::new());
        Store::open(&d).get_or_compute("chunk300", items(&[("P1", "가"), ("P2", "나다")]), fake(&log));
        log.borrow_mut().clear();
        let v = Store::open(&d).get_or_compute("chunk300", items(&[("P1", "가"), ("K1", "라마바"), ("P2", "나다")]), fake(&log));
        assert_eq!(*log.borrow(), vec!["라마바".to_string()]);
        assert_eq!(v, vec![("P1".into(), vec![1.0]), ("K1".into(), vec![3.0]), ("P2".into(), vec![2.0])]);
    }

    #[test]
    fn changed_text_is_embedded_again() {
        let d = dir("change");
        let log = RefCell::new(Vec::new());
        Store::open(&d).get_or_compute("chunk300", items(&[("P1", "제1조 목적")]), fake(&log));
        log.borrow_mut().clear();
        Store::open(&d).get_or_compute("chunk300", items(&[("P1", "제1조 목적 ① 항")]), fake(&log));
        assert_eq!(*log.borrow(), vec!["제1조 목적 ① 항".to_string()]);
    }

    #[test]
    fn duplicate_texts_are_embedded_once() {
        let d = dir("dup");
        let log = RefCell::new(Vec::new());
        let v = Store::open(&d).get_or_compute("chunk300", items(&[("P1", "같음"), ("P2", "같음")]), fake(&log));
        assert_eq!(log.borrow().len(), 1);
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn legacy_whole_set_file_is_imported_without_embedding() {
        let d = dir("legacy");
        let inputs = items(&[("P1", "가"), ("P2", "나다")]);
        std::fs::create_dir_all(&d).unwrap();
        let legacy: Vectors = vec![("P1".into(), vec![7.0]), ("P2".into(), vec![8.0])];
        std::fs::write(d.join(format!("chunk300-{:016x}.json", inputs_hash(&inputs))), serde_json::to_string(&legacy).unwrap()).unwrap();
        let log = RefCell::new(Vec::new());
        let v = Store::open(&d).get_or_compute("chunk300", inputs, fake(&log));
        assert!(log.borrow().is_empty());
        assert_eq!(v, legacy);
    }

    #[test]
    fn id_text_boundary_is_part_of_the_legacy_key() {
        let a = vec![("P1".to_string(), "0가".to_string())];
        let b = vec![("P10".to_string(), "가".to_string())];
        assert_ne!(inputs_hash(&a), inputs_hash(&b));
    }
}
