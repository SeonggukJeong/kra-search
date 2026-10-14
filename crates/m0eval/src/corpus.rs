//! 평가 말뭉치와 질의 읽기.

use doc::Document;
use std::path::Path;

pub struct EvalDoc {
    pub id: String,
    pub doc: Document,
}

pub struct Query {
    pub qid: String,
    pub text: String,
    pub kind: String,
    pub relevant: std::collections::HashSet<String>,
}

#[derive(serde::Deserialize)]
struct Manifest {
    doc: Vec<Entry>,
}
#[derive(serde::Deserialize)]
struct Entry {
    id: String,
    title: String,
    ext: String,
}

/// 반환: (추출 성공 문서, 실패 (id, 사유)).
/// 문서 제목이 추출되지 않으면 매니페스트 제목(=게시 제목, 실제 PC의 파일명 역할)을 쓴다.
pub fn load_docs(eval_dir: &Path) -> (Vec<EvalDoc>, Vec<(String, String)>) {
    let m: Manifest = toml::from_str(&std::fs::read_to_string(eval_dir.join("manifest.toml")).expect("manifest.toml")).expect("manifest");
    let (mut ok, mut failed) = (Vec::new(), Vec::new());
    for e in m.doc {
        let path = eval_dir.join("docs").join(format!("{}.{}", e.id, e.ext));
        match extract::cli::extract_one(&path) {
            extract::runner::Outcome::Ok { mut doc, .. } => {
                if doc.title.as_deref().map_or(true, |t| t.trim().is_empty()) {
                    doc.title = Some(e.title.clone());
                }
                ok.push(EvalDoc { id: e.id, doc });
            }
            other => failed.push((e.id, format!("{other:?}"))),
        }
    }
    (ok, failed)
}

pub fn load_queries(eval_dir: &Path) -> Vec<Query> {
    std::fs::read_to_string(eval_dir.join("queries.tsv"))
        .expect("queries.tsv")
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            Query {
                qid: c[0].into(),
                text: c[1].into(),
                kind: c[2].into(),
                relevant: c[3].split(',').map(|s| s.trim().to_string()).collect(),
            }
        })
        .collect()
}
