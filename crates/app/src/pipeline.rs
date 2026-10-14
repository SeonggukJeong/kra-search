//! 색인 파이프라인(스펙 §6.1 의 1단계 "키워드"): 훑기 → 바뀐 파일만 해시 → 같은 내용이면 재사용, 아니면 워커로 추출
//! → meta 에 기록(Pending) → 주기적으로 키워드 색인에 반영(flush). 지난번 일시 실패는 한 번 다시 읽는다(스펙 §6.9).

use crate::store::Store;
use extract::format::MAX_FILE_BYTES;
use extract::runner::Outcome;
use extract::worker::Worker;
use index::keyword::KeywordDoc;
use index::meta::{ContentData, MetaError};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 이만큼 처리할 때마다 키워드 색인에 반영한다(중간에 꺼져도 잃는 작업이 적도록).
const FLUSH_EVERY: usize = 200;
/// flush 한 번에 다시 쓰는 내용 수.
const FLUSH_BATCH: usize = 500;

#[derive(Debug, Default)]
pub struct IndexSummary {
    pub scanned: usize,
    pub changed: usize,
    pub removed: usize,
    pub extracted: usize,
    pub reused: usize,
    pub failed: usize,
    pub unreadable: usize,
    pub retried: usize,
    pub cloud_only: usize,
    pub changed_while_reading: usize,
}

/// 색인 오류. `MetaCorrupt` 는 meta.db 쪽 손상(키 불일치·페이지 손상) — 열 때가 아니라 도중에 드러날 수 있어
/// `index` 명령이 한 번 처음부터 다시 만든다(main.rs).
#[derive(Debug)]
pub enum IndexError {
    MetaCorrupt,
    Other(String),
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexError::MetaCorrupt => write!(f, "메타 DB 손상"),
            IndexError::Other(s) => write!(f, "{s}"),
        }
    }
}

impl From<MetaError> for IndexError {
    fn from(e: MetaError) -> Self {
        match e {
            MetaError::WrongKeyOrCorrupt => IndexError::MetaCorrupt,
            e => IndexError::Other(e.to_string()),
        }
    }
}

/// meta 가 아닌 오류(키워드 색인 등).
fn err(e: impl std::fmt::Display) -> IndexError {
    IndexError::Other(e.to_string())
}

fn content_data(o: Outcome) -> ContentData {
    match o {
        Outcome::Ok { doc, distribution, .. } => ContentData { fail: None, title: doc.title.clone(), distribution, doc: Some(doc) },
        // 고정 코드로 저장한다 — 변형 이름(Debug)을 바꿔도 저장된 값과 fail_label 이 어긋나지 않게.
        Outcome::Failed { kind, .. } => ContentData { fail: Some(kind.code().to_string()), title: None, distribution: false, doc: None },
        Outcome::Timeout => ContentData { fail: Some("Timeout".into()), title: None, distribution: false, doc: None },
        Outcome::Crashed { .. } => ContentData { fail: Some("Crashed".into()), title: None, distribution: false, doc: None },
    }
}

fn extract(worker: &mut Worker, path: &Path, size: u64, timeout: Duration) -> ContentData {
    if size > MAX_FILE_BYTES {
        // 100MB 초과: 워커에 보내지 않는다(스펙 §6.5).
        return ContentData { fail: Some(extract::FailKind::TooLarge.code().into()), title: None, distribution: false, doc: None };
    }
    content_data(worker.extract(path, timeout))
}

fn file_name(p: &str) -> String {
    Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or(p).to_string()
}

pub fn run_index(store: &mut Store, worker: &mut Worker, roots: &[PathBuf], timeout: Duration) -> Result<IndexSummary, IndexError> {
    let report = crawler::scan(roots, &crawler::ScanOptions::default());
    let known = store.meta.known_files()?;
    // 사라진 루트·잠깐 못 읽은 폴더·클라우드 전용 파일 아래의 경로는 지운 것으로 보지 않는다(DEC-040).
    let diff = crawler::diff(&report.files, &known, &report.keep_under());
    // 지난 실행까지 시간 초과·비정상 종료·열기 실패로 끝난 내용. 이번 변경을 처리한 뒤 한 번씩 다시 읽는다(스펙 §6.9 "1회 재시도").
    // 이번 실행에서 처음 실패한 것은 다음 실행에서 — 같은 원인(바쁜 PC·잠금)이 바로 풀리지 않으므로.
    let retry = store.meta.retry_candidates()?;
    let mut s = IndexSummary {
        scanned: report.files.len(),
        changed: diff.changed.len(),
        removed: diff.removed.len(),
        unreadable: report.unreadable,
        cloud_only: report.cloud_only,
        ..Default::default()
    };

    for path in &diff.removed {
        store.meta.remove_file(path)?;
    }
    for (i, f) in diff.changed.iter().enumerate() {
        eprint!("\r문서 읽는 중 {}/{}", i + 1, diff.changed.len());
        let p = Path::new(&f.path);
        // 다른 프로그램이 잠갔거나 사라진 파일 — 기록하지 않으므로 다음 실행의 diff 에 다시 나온다.
        let Some(before) = crawler::stat(p) else {
            s.unreadable += 1;
            continue;
        };
        let Ok(hash) = crawler::content_key(f) else {
            s.unreadable += 1;
            continue;
        };
        let id = match store.meta.content_by_hash(hash)? {
            Some(c) => {
                s.reused += 1;
                c.id
            }
            None => {
                let d = extract(worker, p, f.size, timeout);
                // 해시와 추출 사이에 파일이 바뀌었으면 해시와 다른 판의 글자다 — 기록하지 않고 다음 실행에서 다시 읽는다.
                // 비교는 같은 방법(stat)으로 잰 값끼리 한다(Windows 폴더 목록의 크기는 늦게 갱신될 수 있음).
                if crawler::stat(p) != Some(before) {
                    s.changed_while_reading += 1;
                    continue;
                }
                s.failed += d.fail.is_some() as usize;
                s.extracted += 1;
                store.meta.insert_content(hash, &d)?
            }
        };
        store.meta.upsert_file(&f.path, f.size, f.mtime_ns, id)?;
        if (i + 1) % FLUSH_EVERY == 0 {
            flush(store)?;
        }
    }
    if !diff.changed.is_empty() {
        eprintln!();
    }
    for (id, _) in retry {
        // 이번 실행에서 그 내용의 파일이 모두 바뀌거나 지워졌으면 건너뛴다. 경로는 지금 기준으로 다시 고른다.
        let Some(f) = store.meta.files_of(id)?.into_iter().next() else { continue };
        let p = Path::new(&f.path);
        let Some(before) = crawler::stat(p) else { continue };
        let d = extract(worker, p, before.0, timeout);
        if crawler::stat(p) != Some(before) {
            s.changed_while_reading += 1;
            continue;
        }
        s.retried += 1;
        s.failed += d.fail.is_some() as usize;
        store.meta.retry_content(id, &d)?;
    }
    flush(store)?;
    Ok(s)
}

/// Pending 내용을 키워드 색인에 다시 쓰고, 경로가 하나도 없는 내용은 지운다.
/// 순서: 키워드 색인 쓰기 → 커밋 → meta 에 Indexed 기록(finish_flush). 그 사이에 꺼져도 다음 실행에서 같은 일을 다시 한다.
pub fn flush(store: &mut Store) -> Result<usize, IndexError> {
    let mut total = 0;
    loop {
        let pending = store.meta.pending(FLUSH_BATCH)?;
        let orphans = store.meta.orphans()?;
        if pending.is_empty() && orphans.is_empty() {
            return Ok(total);
        }
        let mut indexed = Vec::new();
        for &id in &pending {
            if orphans.contains(&id) {
                continue;
            }
            let names: Vec<String> = store.meta.files_of(id)?.iter().map(|f| file_name(&f.path)).collect();
            let doc = store.meta.doc(id)?;
            store.keyword.upsert(&KeywordDoc::new(id as u64, doc.as_ref(), &names)).map_err(err)?;
            indexed.push(id);
        }
        for &id in &orphans {
            store.keyword.delete(id as u64).map_err(err)?;
        }
        let op = store.keyword.commit().map_err(err)?;
        store.meta.finish_flush(&indexed, &orphans, op)?;
        total += indexed.len() + orphans.len();
    }
}
