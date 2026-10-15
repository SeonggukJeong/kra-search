//! 색인 저장소 열기.
//! - `index`(쓰기 주체): 잠금 파일로 한 번에 하나만. 키를 못 풀거나 DB 가 깨졌으면 색인 파일을 지우고 새로 만들고(스펙 §5.3),
//!   키워드 색인을 잃었거나 버전이 다르거나 뒤처졌으면 새로 만든 뒤 meta 의 내용을 모두 Pending 으로 돌려 다시 쓴다(스펙 §6.8).
//! - `search`·`status`: 잠그지 않고 읽기만 한다. 복구(삭제·Pending 표시)는 하지 않는다 — 색인 중인 다른 프로세스의 파일을 건드리지 않도록(DEC-039).
//!
//! 사용자에게 대화상자를 띄우지 않는다(§3.3). 안내는 stderr 한 줄.

use crate::pipeline::IndexError;
use index::keyword::KeywordIndex;
use index::meta::{Meta, MetaError};
use index::tokenize::Analyzers;
use platform::keys::{self, KeyError};
use platform::paths::Home;
use std::path::Path;

pub struct Store {
    pub meta: Meta,
    pub keyword: KeywordIndex,
}

const NO_INDEX: &str = "색인이 없습니다. 먼저 `kra-search index` 를 실행하세요.";
const NEEDS_INDEX: &str = "색인을 열 수 없습니다. `kra-search index` 를 실행하면 다시 만듭니다.";

/// 쓰기 주체 단일 인스턴스 잠금. 돌려준 File 을 들고 있는 동안 유효하고, 프로세스가 끝나면 OS 가 푼다.
pub fn lock_index(home: &Home) -> Result<std::fs::File, String> {
    std::fs::create_dir_all(home.root()).map_err(|e| format!("저장 위치를 만들지 못함: {e}"))?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.lock_file())
        .map_err(|e| format!("잠금 파일을 열지 못함: {e}"))?;
    match f.try_lock() {
        Ok(()) => Ok(f),
        Err(std::fs::TryLockError::WouldBlock) => Err("이미 색인 중입니다. 끝난 뒤 다시 실행하세요.".into()),
        Err(std::fs::TryLockError::Error(e)) => Err(format!("잠금 파일 오류: {e}")),
    }
}

/// meta 오류 → 사용자에게 보일 쉬운 말(내부 이름·Debug 출력 없이, UI D7).
pub(crate) fn user_error(e: MetaError) -> String {
    match e {
        MetaError::WrongKeyOrCorrupt => NEEDS_INDEX.into(),
        MetaError::Io(e) => format!("색인 저장 위치를 읽거나 쓰지 못했습니다: {e}"),
        MetaError::Sql(e) => format!("색인 데이터를 읽거나 쓰지 못했습니다: {e}"),
    }
}

fn remove_keyword_dir(home: &Home) -> Result<(), String> {
    match std::fs::remove_dir_all(home.keyword_dir()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("검색 색인을 지우지 못함: {e}")),
    }
}

fn remove_file_if_exists(p: &Path) -> Result<(), String> {
    match std::fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("색인 파일을 지우지 못함: {e}")),
    }
}

/// 우리가 만든 파일만 지운다 — `--home` 을 잘못 줘도 남의 `data` 폴더를 통째로 지우지 않게.
/// 저장된 색인 폴더 목록(roots)도 meta.db 에 있으므로 함께 사라진다 — 부르는 쪽이 다시 저장한다(KI-030).
/// 키워드 색인을 먼저 지운다 — 지우지 못하면(Windows 에서 다른 프로세스가 읽는 중) meta 는 그대로 두고 끝낸다.
/// 반대 순서면 새 meta 옆에 옛 키워드 색인이 남는다(try_open 은 opstamp 가 다르면 어느 쪽이든 다시 만들지만, 남기지 않는 편이 낫다).
pub fn reset_store(home: &Home) -> Result<(), String> {
    remove_keyword_dir(home)?;
    // 의미 검색 벡터도 지운다 — 남으면 새 meta 의 내용 id 와 엉킨다(DEC-048).
    match std::fs::remove_dir_all(home.vectors_dir()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("의미 검색 색인을 지우지 못함: {e}")),
    }
    let db = home.meta_db();
    remove_file_if_exists(&db)?;
    for ext in ["-wal", "-shm"] {
        let mut p = db.clone().into_os_string();
        p.push(ext);
        remove_file_if_exists(Path::new(&p))?;
    }
    remove_file_if_exists(&home.meta_key())
}

/// 키워드 색인을 지우고 빈 색인을 만든 뒤 meta 의 모든 내용을 다시 쓰게 한다.
fn rebuild_keyword(home: &Home, meta: &mut Meta, a: &Analyzers) -> Result<KeywordIndex, IndexError> {
    remove_keyword_dir(home).map_err(IndexError::Other)?;
    meta.mark_all_pending()?;
    KeywordIndex::open(&home.keyword_dir(), a).map_err(|e| IndexError::Other(format!("검색 색인을 만들지 못함: {e}")))
}

/// 한 번 열어 본다. meta 손상(키 불일치·페이지 손상)과 키 분실은 어디서 드러나든 `MetaCorrupt` 로 돌려준다
/// — 열기(첫 쪽)는 멀쩡해도 opstamp 읽기·Pending 표시에서 손상된 쪽을 만날 수 있다.
fn try_open(home: &Home, a: &Analyzers) -> Result<Store, IndexError> {
    let key = match keys::load_or_create(&home.meta_key()) {
        Ok(k) => k,
        Err(KeyError::Unreadable) => return Err(IndexError::MetaCorrupt),
        // 일시적일 수 있는 오류는 지우지 않고 끝낸다(다음 실행에서 다시 시도).
        Err(KeyError::Io(e)) => return Err(IndexError::Other(format!("색인 데이터 암호화 키를 읽지 못했습니다: {e}. 잠시 뒤 다시 실행하세요."))),
    };
    let mut meta = Meta::open(&home.meta_db(), &key)?;
    // meta 쪽 기준을 먼저 읽는다(키워드 쪽을 먼저 읽으면 그 사이 커밋으로 거꾸로 비교될 수 있음).
    let meta_op = meta.keyword_opstamp()?;
    let mut keyword = match KeywordIndex::open(&home.keyword_dir(), a) {
        Ok(k) => k,
        // 색인 폴더 손상·스키마/토크나이저 버전 불일치(KEYWORD_VERSION, DEC-041)
        Err(_) => rebuild_keyword(home, &mut meta, a)?,
    };
    // 키워드 색인이 meta 가 아는 마지막 커밋과 다르면 새로 만든다.
    // 뒤처짐: 폴더 유실·옛 복사본 복원 — 옛 문서가 남지 않게. 앞섬: 새 meta 옆에 옛 색인이 남음(복구 중 못 지움) — 옛 문서가
    // 새 내용 id 와 겹치지 않게. 키워드 커밋 뒤 finish_flush 전에 꺼진 경우도 앞섬이지만, 드문 일이라 다시 쓰는 비용을 받아들인다.
    if keyword.opstamp() != meta_op {
        drop(keyword);
        keyword = rebuild_keyword(home, &mut meta, a)?;
    }
    Ok(Store { meta, keyword })
}

/// `index` 전용 — 부르기 전에 `lock_index` 로 잠가야 한다. meta 가 손상됐거나 키를 못 풀면 한 번 처음부터 다시 만든다(스펙 §5.3).
pub fn open_store(home: &Home, a: &Analyzers) -> Result<Store, String> {
    match try_open(home, a) {
        Err(IndexError::MetaCorrupt) => {
            eprintln!("색인을 열 수 없어 처음부터 다시 만듭니다.");
            reset_store(home)?;
            try_open(home, a).map_err(|e| e.to_string())
        }
        r => r.map_err(|e| e.to_string()),
    }
}

/// `search`·`status` 전용 — 아무것도 만들거나 지우지 않는다.
pub fn open_store_readonly(home: &Home, a: &Analyzers) -> Result<Store, String> {
    // 알려진 경쟁: 확인과 열기 사이에 index 가 reset 하면 아래 열기(Meta::open·KeywordIndex::open 은 없으면 만든다)가 파일을 만들 수 있다 — 드물고 다음 index 가 바로잡는다(KI, Task 18).
    // Meta::open 은 스키마·WAL 설정을 쓴다 — 내용이 있는 meta.db 일 때만 연다(빈 파일에 스키마를 만들지 않게).
    let has_db = std::fs::metadata(home.meta_db()).map(|m| m.len() > 0).unwrap_or(false);
    if !home.meta_key().exists() || !has_db {
        return Err(NO_INDEX.into());
    }
    let key = keys::load_or_create(&home.meta_key()).map_err(|_| NEEDS_INDEX.to_string())?;
    let meta = Meta::open(&home.meta_db(), &key).map_err(user_error)?;
    if !home.keyword_dir().join("meta.json").exists() {
        return Err(NEEDS_INDEX.into());
    }
    let keyword = KeywordIndex::open(&home.keyword_dir(), a).map_err(|_| NEEDS_INDEX.to_string())?;
    Ok(Store { meta, keyword })
}
