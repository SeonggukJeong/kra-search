//! kra-search 비교 평가 하네스 — Docufinder(Anything) 를 kra-search 평가 세트로 잰다.
//! 원본: kra-search `tools/compare/docufinder/kra_eval.rs`. `#[cfg(test)]` 이라 앱 바이너리에 들어가지 않는다.
//!
//! 화면의 '폴더 추가'(commands/index/folder.rs add_folder)와 같은 순서로 색인한다:
//! 감시 폴더 등록 → 메타데이터 스캔 → FTS 색인 → 상태 completed → (의미 검색 켬) 벡터 색인.
//! 검색은 commands/search.rs 와 같은 서비스 함수 + 같은 기본값(max_results 50, AND, group_versions 켬 → 버전 접기).
//! 결과는 청크 단위라 파일 순서대로 한 번씩만 남긴다(화면도 파일 하나가 한 줄).
//!
//! 환경 변수:
//!   KRA_EVAL_DOCS    평가 문서 복사본 폴더(kra-search `eval --set` 이 만든 eval-docs: 수정일 = 매니페스트 날짜)
//!   KRA_EVAL_HOME    Docufinder 데이터 폴더(DB·벡터·models/kosimcse-roberta-multitask)
//!   KRA_EVAL_QUERIES queries.tsv 경로들(':' 로 구분)
//!   KRA_EVAL_OUT     순위 JSON 을 둘 폴더 — docufinder-{keyword,hybrid,smart}.json ({qid: [문서 id…]})
//!   ORT_DYLIB_PATH   libonnxruntime.dylib(앱 setup 이 하던 일)

#[cfg(test)]
mod tests {
    use crate::application::services::search_service::helpers::collapse_by_lineage;
    use crate::application::dto::search::SearchResult;
    use crate::search::KeywordMode;
    use crate::AppContainer;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    fn env(k: &str) -> String {
        std::env::var(k).unwrap_or_else(|_| panic!("{k} 가 필요합니다"))
    }

    /// kra-search app::evalset::id_of 와 같은 규칙: "{제목} ({id}).{확장자}" 에서 id.
    fn id_of(path: &str) -> Option<String> {
        let stem = Path::new(path).file_stem()?.to_str()?;
        let open = stem.rfind(" (")?;
        let id = stem[open + 2..].strip_suffix(')')?;
        (!id.is_empty()).then(|| id.to_string())
    }

    /// 청크 결과 → 파일 순서(첫 등장)대로 문서 id.
    fn doc_ids(results: &[SearchResult]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in results {
            if let Some(id) = id_of(&r.file_path) {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        out
    }

    /// (qid, 검색어) — kra-search queries.tsv 의 앞 두 열.
    fn load_queries(paths: &str) -> Vec<(String, String)> {
        let mut v = Vec::new();
        for p in paths.split(':').filter(|s| !s.is_empty()) {
            let text = std::fs::read_to_string(p).expect("queries.tsv");
            for line in text.lines().skip(1) {
                let cols: Vec<&str> = line.split('\t').collect();
                if cols.len() >= 2 && !cols[0].trim().is_empty() {
                    v.push((cols[0].trim().to_string(), cols[1].trim().to_string()));
                }
            }
        }
        v
    }

    #[test]
    #[ignore]
    fn kra_eval() {
        let docs = PathBuf::from(env("KRA_EVAL_DOCS"));
        let home = PathBuf::from(env("KRA_EVAL_HOME"));
        let out = PathBuf::from(env("KRA_EVAL_OUT"));
        let queries = load_queries(&env("KRA_EVAL_QUERIES"));
        std::fs::create_dir_all(&out).unwrap();
        assert!(crate::parsers::kordoc::is_available(), "kordoc(node·cli.js)이 실행 파일 옆 resources/ 에 있어야 합니다");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let c = AppContainer::new(&home);
        crate::db::init_database(&c.db_path).expect("DB 초기화");
        let mut s = c.get_settings();
        s.semantic_search_enabled = true;
        c.update_settings_cache(s.clone());
        c.get_embedder().expect("KoSimCSE 임베더(models/kosimcse-roberta-multitask)");

        let docs_s = docs.to_string_lossy().to_string();
        let already = crate::db::get_connection(&c.db_path).ok().is_some_and(|conn| crate::db::is_folder_watched(&conn, &docs_s).unwrap_or(false));
        if !already {
            let idx = c.index_service();
            idx.add_watched_folder(&docs_s).expect("감시 폴더 등록");
            let t = Instant::now();
            rt.block_on(idx.scan_metadata_only(&docs, s.include_subfolders, None, s.max_file_size_mb, s.exclude_dirs.clone())).expect("메타데이터 스캔");
            let r = rt.block_on(idx.index_folder_fts(&docs, s.include_subfolders, None, s.max_file_size_mb, s.exclude_dirs.clone())).expect("FTS 색인");
            let conn = crate::db::get_connection(&c.db_path).unwrap();
            let _ = crate::db::set_folder_indexing_status(&conn, &docs_s, "completed");
            eprintln!("[kra_eval] FTS {:.1}초 · indexed {} · failed {}", t.elapsed().as_secs_f64(), r.indexed_count, r.failed_count);
            for e in r.errors.iter().take(30) {
                eprintln!("[kra_eval] 실패: {e:?}");
            }
            let t = Instant::now();
            assert!(idx.start_vector_indexing(None, None).expect("벡터 색인 시작"), "벡터 워커가 이미 돎");
            loop {
                std::thread::sleep(Duration::from_secs(5));
                if !c.get_vector_worker().read().unwrap().is_running() {
                    break;
                }
            }
            eprintln!("[kra_eval] 벡터 {:.1}초 · {:?}", t.elapsed().as_secs_f64(), idx.get_vector_status().ok());
            // 다음 실행이 색인을 다시 쓰도록 벡터 저장(종료 절차와 같은 일)
            if let Ok(vi) = c.get_vector_index() {
                let _ = vi.save();
            }
        }

        let svc = c.search_service();
        assert!(c.embedder_if_ready().is_some(), "의미 검색 임베더가 붙지 않음");
        let kw = c.keyword_search_service();
        let collapse = |v: Vec<SearchResult>| if s.group_versions { collapse_by_lineage(v) } else { v };
        type Run<'a> = Box<dyn Fn(&str) -> Vec<SearchResult> + 'a>;
        let modes: Vec<(&str, Run<'_>)> = vec![
            ("keyword", Box::new(|q: &str| kw.search_keyword_with_operators(q, s.max_results, None, KeywordMode::And).map(|r| r.results).unwrap_or_default())),
            ("hybrid", Box::new(|q: &str| svc.search_hybrid_with_operators(q, s.max_results, None, KeywordMode::And).map(|r| r.results).unwrap_or_default())),
            ("smart", Box::new(|q: &str| svc.search_smart(q, s.max_results, None).map(|r| r.results).unwrap_or_default())),
        ];
        for (name, run) in &modes {
            let t = Instant::now();
            let mut ranks: BTreeMap<String, Vec<String>> = BTreeMap::new();
            let mut empty = 0;
            for (qid, q) in &queries {
                let ids = doc_ids(&collapse(run(q)));
                if ids.is_empty() {
                    empty += 1;
                }
                ranks.insert(qid.clone(), ids);
            }
            let p = out.join(format!("docufinder-{name}.json"));
            std::fs::write(&p, serde_json::to_string(&ranks).unwrap()).unwrap();
            eprintln!("[kra_eval] {name}: 질의 {} · 결과 없음 {empty} · {:.1}초 → {}", queries.len(), t.elapsed().as_secs_f64(), p.display());
        }
    }
}
