//! 검색: 키워드 색인 → 내용 id → meta 의 제목·경로.

use crate::model;
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

/// 내용 id 하나를 화면용 `Hit` 으로. 경로가 없으면(아직 반영 안 된 삭제) None.
fn build_hit(store: &Store, id: i64, score: f32) -> Result<Option<Hit>, String> {
    let Some(c) = store.meta.content(id).map_err(user_error)? else { return Ok(None) };
    let paths: Vec<String> = store.meta.files_of(id).map_err(user_error)?.into_iter().map(|f| f.path).collect();
    if paths.is_empty() {
        return Ok(None);
    }
    let title = c.title.clone().unwrap_or_else(|| Path::new(&paths[0]).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string());
    Ok(Some(Hit { content_id: id, score, title, paths, fail: c.fail }))
}

pub fn search(store: &Store, query: &str, limit: usize) -> Result<Vec<Hit>, String> {
    let hits = store.keyword.search(query, limit).map_err(|e| format!("검색 색인 오류: {e}"))?;
    let mut out = Vec::new();
    for (id, score) in hits {
        out.extend(build_hit(store, id as i64, score)?);
    }
    Ok(out)
}

use crate::vectors::{self, Sets, SetsState};
use embed::api::Embed;
use embed::engine::{Embedder, EngineOptions};
use embed::profile::ModelProfile;
use index::hybrid::{self, HybridParams, MapMeta};
use platform::paths::Home;
use query::{DateTable, Day, ParsedQuery};

/// 하이브리드 검색 결과 한 줄: 화면용 정보 + 경로별 기여(UI D3 "왜 이 결과?"의 재료).
#[derive(Debug)]
pub struct HybridHit {
    pub hit: Hit,
    pub why: hybrid::Hit,
}

pub struct Searcher {
    sets: Sets,
    docs: MapMeta,
    embedder: Option<Box<dyn Embed>>,
    profile: ModelProfile,
    table: DateTable,
    pub params: HybridParams,
    /// 사용자에게 알릴 한 줄(의미 검색을 못 쓰는 이유)
    pub notice: Option<String>,
}

impl Searcher {
    /// 제품 경로: 모델이 있으면 지문을 계산해 색인과 맞을 때만 의미 검색을 쓴다.
    pub fn open(store: &Store, home: &Home, model: Option<&Path>, threads: usize) -> Result<Searcher, String> {
        let Some(path) = model else {
            let mut s = Self::with_embedder(store, home, None, None)?;
            if s.sets.state == SetsState::Ready {
                s.notice = Some("의미 검색 모델을 찾지 못해 키워드 검색만 합니다.".into());
            }
            return Ok(s);
        };
        let cached = store.meta.kv(model::MODEL_HASH_KV).map_err(user_error)?;
        // 색인과 같은 계산(model::fingerprint_of)을 쓴다 — 따로 계산하면 지문이 어긋난다.
        let fp = match model::fingerprint_of(path, cached.as_deref()) {
            Ok((fp, _)) => fp,
            Err(e) => {
                // 잠긴·권한 없는·복사 중인 모델 파일이어도 키워드 검색은 돌려준다(스펙 §3.5).
                let mut s = Self::with_embedder(store, home, None, None)?;
                s.notice = Some(format!("모델 파일을 읽지 못해 키워드 검색만 합니다: {e}"));
                return Ok(s);
            }
        };
        // 벡터가 있을 때만 모델을 불러온다(검색어 임베딩 — 1초 안팎).
        let probe = vectors::load_sets(home, &store.meta, Some(&fp))?;
        let embedder: Option<Box<dyn Embed>> = if probe.state == SetsState::Ready {
            match Embedder::load(path, &EngineOptions { threads, max_tokens: 1024, backend_dir: model::exe_dir() }) {
                Ok(m) => Some(Box::new(m)),
                Err(e) => {
                    let mut s = Self::from_sets(store, probe, None)?;
                    s.notice = Some(format!("의미 검색 모델을 불러오지 못해 키워드 검색만 합니다: {e}"));
                    return Ok(s);
                }
            }
        } else {
            None
        };
        Self::from_sets(store, probe, embedder)
    }

    /// 시험·평가: 임베더와 "지금 지문"을 직접 준다.
    pub fn with_embedder(store: &Store, home: &Home, embedder: Option<Box<dyn Embed>>, fp_now: Option<&str>) -> Result<Searcher, String> {
        let sets = vectors::load_sets(home, &store.meta, fp_now)?;
        Self::from_sets(store, sets, embedder)
    }

    fn from_sets(store: &Store, sets: Sets, embedder: Option<Box<dyn Embed>>) -> Result<Searcher, String> {
        let dates = store.meta.newest_mtimes().map_err(user_error)?.into_iter().map(|(id, ns)| (id, query::calendar::day_of_unix_ns(ns))).collect();
        let chunked = store.meta.chunked_contents().map_err(user_error)?;
        // 문서 제목(없으면 파일 이름)에 적힌 해 — "작년 경마시행계획" 이 2025년 계획을 찾게(사용자 요구 2026-10-15).
        let title_years = store
            .meta
            .title_texts()
            .map_err(user_error)?
            .into_iter()
            .filter_map(|(id, t)| {
                let ys = query::title_years(&t);
                (!ys.is_empty()).then_some((id, ys))
            })
            .collect();
        let notice = match sets.state {
            SetsState::ModelChanged => Some("검색 모델이나 설정이 바뀌어 의미 검색을 쓰지 않습니다. `kra-search index` 를 실행하면 다시 만듭니다.".to_string()),
            SetsState::Unreadable => Some("의미 검색 색인을 읽지 못해 키워드 검색만 합니다. `kra-search index` 를 실행하세요.".to_string()),
            SetsState::Ready | SetsState::Empty => None,
        };
        let embedder = if sets.state == SetsState::Ready { embedder } else { None };
        Ok(Searcher {
            sets,
            docs: MapMeta { dates, chunked, title_years },
            embedder,
            profile: model::profile(),
            table: DateTable::builtin(),
            params: HybridParams::default(),
            notice,
        })
    }

    /// 의미 검색을 실제로 쓰는가(벡터가 있고 지문이 맞고 모델을 불러옴). 모델을 불러오지 못한 채 색인하면 벡터가 없어
    /// `notice` 없이도 키워드만 쓰게 된다 — 평가는 이것으로 확인한다.
    pub fn semantic_ready(&self) -> bool {
        self.sets.state == SetsState::Ready && self.embedder.is_some()
    }

    pub fn search(&mut self, store: &Store, q: &str, today: Day, limit: usize) -> Result<(ParsedQuery, Vec<HybridHit>), String> {
        let pq = query::parse(q, today, &self.table);
        let texts = hybrid::query_texts(&pq, &self.params);
        let qv = match self.embedder.as_mut() {
            Some(e) => match e.embed(&[self.profile.render_query(&texts.semantic)]) {
                Ok(mut v) if v.len() == 1 => v.pop(),
                Ok(_) => None,
                Err(err) => {
                    self.notice = Some(format!("검색어의 뜻을 계산하지 못해 키워드 검색만 했습니다: {err}"));
                    None
                }
            },
            None => None,
        };
        // 경로 없는 내용(반영 안 된 삭제)을 건너뛰므로 넉넉히 받는다.
        let raw = hybrid::search(&store.keyword, &self.sets.cards, &self.sets.chunks, &pq, qv.as_deref(), &self.params, today, &self.docs, limit + 20)
            .map_err(|e| format!("검색 색인 오류: {e}"))?;
        let mut out = Vec::new();
        for h in raw {
            let Some(hit) = build_hit(store, h.id, h.score as f32)? else { continue };
            out.push(HybridHit { hit, why: h });
            if out.len() == limit {
                break;
            }
        }
        Ok((pq, out))
    }
}

/// "왜 이 결과?" 한 줄(UI D3·D7 용어).
pub fn why_line(h: &hybrid::Hit) -> String {
    let mut v = Vec::new();
    if let Some(r) = h.keyword_rank {
        v.push(format!("키워드 일치 {r}위"));
    }
    match (h.chunk, h.card) {
        (Some((r, _)), _) => v.push(format!("뜻이 비슷함 {r}위")),
        (None, Some((r, _))) => v.push(format!("뜻이 비슷함(빠른 의미 검색) {r}위")),
        _ => {}
    }
    if h.date_boost > 0.0 {
        v.push("기간 안".into());
    }
    v.join(" · ")
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
