//! 평가 말뭉치와 질의 읽기.

use doc::Document;
use std::path::Path;

pub struct EvalDoc {
    pub id: String,
    /// 평가 세트 폴더 이름(`public`·`kra`·`data`) — 제품 복사본 `out/<세트>/<파일>` 의 경로 순서를 흉내 낸다
    pub set: String,
    /// 매니페스트 제목 + 확장자 — M0·M1 보고서의 파일명 필드(그대로 둔다 — 그 결과를 다시 만들 수 있게)
    pub name: String,
    /// M2: 평가 복사본 파일 이름(app::evalset::file_name) — 제품 `eval` 과 같게
    pub file: String,
    pub ext: String,
    /// 매니페스트 date(한국 날짜)
    pub date: Option<query::Day>,
    pub doc: Document,
    /// 추출 제목이 없어 매니페스트 제목을 넣었는가 — M2 는 이때 제목을 비워 제품처럼 파일 이름을 쓴다
    pub title_filled: bool,
}

/// 추출에 실패한 문서 — 제품처럼 파일 이름으로만 찾히게 M2 실험에도 넣는다.
pub struct FailedDoc {
    pub id: String,
    pub set: String,
    pub why: String,
    pub file: String,
    pub ext: String,
    pub date: Option<query::Day>,
}

/// 질의 — 제품 `kra-search eval --set` 과 같은 타입·같은 읽기 규칙(`app::evalset::queries`, Task 17 P15).
pub type Query = app::evalset::EvalQuery;

#[derive(serde::Deserialize)]
struct Manifest {
    doc: Vec<Entry>,
}
#[derive(serde::Deserialize)]
struct Entry {
    id: String,
    title: String,
    ext: String,
    date: Option<String>,
}

/// 반환: (추출 성공 문서, 실패 문서).
/// 문서 제목이 추출되지 않으면 매니페스트 제목(=게시 제목, 실제 PC의 파일명 역할)을 쓴다(M0·M1). M2 는 `title_filled` 를 보고 되돌린다.
pub fn load_docs(eval_dir: &Path) -> (Vec<EvalDoc>, Vec<FailedDoc>) {
    let m: Manifest = toml::from_str(&std::fs::read_to_string(eval_dir.join("manifest.toml")).expect("manifest.toml")).expect("manifest");
    let set = eval_dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let (mut ok, mut failed) = (Vec::new(), Vec::new());
    for e in m.doc {
        let path = eval_dir.join("docs").join(format!("{}.{}", e.id, e.ext));
        let name = format!("{}.{}", e.title, e.ext);
        let file = app::evalset::file_name(&e.id, &e.title, &e.ext);
        let date = e.date.as_deref().and_then(query::calendar::parse_ymd);
        match extract::cli::extract_one(&path) {
            extract::runner::Outcome::Ok { mut doc, .. } => {
                let title_filled = doc.title.as_deref().is_none_or(|t| t.trim().is_empty());
                if title_filled {
                    doc.title = Some(e.title.clone());
                }
                ok.push(EvalDoc { id: e.id, set: set.clone(), name, file, ext: e.ext, date, doc, title_filled });
            }
            other => failed.push(FailedDoc { id: e.id, set: set.clone(), why: format!("{other:?}"), file, ext: e.ext, date }),
        }
    }
    (ok, failed)
}

/// queries.tsv 읽기 — 제품과 같은 함수(`app::evalset::queries`: 네 열만, 빈 id 버림, 정답 없는 질의는 경고하고 뺌).
pub fn load_queries(eval_dir: &Path) -> Vec<Query> {
    app::evalset::queries(eval_dir).unwrap_or_else(|e| panic!("{e}"))
}
