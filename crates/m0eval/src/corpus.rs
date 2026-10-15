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

/// queries.tsv 읽기: qid · 검색어 · 유형 · 정답(쉼표) 네 열만 쓴다. 다섯째(date 질의의 기간)·여섯째(recurring) 열은
/// 사람이 읽는 표시라 점수에 쓰지 않는다. 정답의 빈 id 는 버리고, 정답이 하나도 없는 질의는 모든 지표에서 빼며
/// 표준 오류에 경고를 남긴다(정답 없는 질의는 nDCG 가 늘 0 이라 평균을 깎기만 한다). 제품 `kra-search eval`(Task 17)도 같은 규칙.
pub fn load_queries(eval_dir: &Path) -> Vec<Query> {
    std::fs::read_to_string(eval_dir.join("queries.tsv"))
        .expect("queries.tsv")
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            let q = Query {
                qid: c[0].trim().into(),
                text: c[1].into(),
                kind: c[2].trim().into(),
                relevant: c.get(3).copied().unwrap_or("").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
            };
            if q.relevant.is_empty() {
                eprintln!("경고: 정답이 없는 질의 {} 는 평가에서 뺍니다({})", q.qid, eval_dir.display());
                return None;
            }
            Some(q)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 넷째 열까지만 쓰고, 빈 id 는 버리고, 정답 없는 질의는 뺀다(Task 17 과 같은 규칙).
    #[test]
    fn queries_use_four_columns_and_skip_unanswered() {
        let d = std::env::temp_dir().join(format!("m0eval-queries-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("queries.tsv"),
            "qid\tquery\ttype\trelevant\nT001\t작년 계획\tdate\tK0014, \t2025-01-01..2025-12-31\nT003\t연말 계획\tdate\tK0009,K0010\t2025-11-01..2025-12-31\trecurring\nZ001\t정답 없음\tkeyword\t , \nZ002\t열 없음\tkeyword\n\nQ001\t돌봄\tsemantic\tP0001\n",
        )
        .unwrap();
        let q = load_queries(&d);
        std::fs::remove_dir_all(&d).unwrap();
        let ids: Vec<&str> = q.iter().map(|x| x.qid.as_str()).collect();
        assert_eq!(ids, vec!["T001", "T003", "Q001"]);
        assert_eq!(q[0].relevant, std::collections::HashSet::from(["K0014".to_string()]));
        assert_eq!(q[1].relevant.len(), 2);
        assert_eq!(q[1].kind, "date");
    }
}
