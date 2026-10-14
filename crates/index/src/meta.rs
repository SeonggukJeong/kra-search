//! 메타 DB(meta.db) — SQLCipher(키는 platform::keys). 앱만 쓴다(스펙 §5.1).
//! 내용(content) = 같은 바이트의 파일 묶음(xxh3-128, 스펙 §6.4). 추출·색인은 내용 단위, 경로(file)는 여러 개.
//! 키워드 색인과의 일관성(스펙 §6.8): 내용을 Pending 으로 두고 → 키워드 색인에 쓰고 커밋 → finish_flush 로 Indexed.
//! 중간에 죽으면 Pending 이 남아 다음 실행에서 다시 쓴다(키워드 쪽은 delete+add 라 여러 번 써도 같다).

use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug)]
pub enum MetaError {
    /// 키가 틀렸거나 파일이 깨짐 — 둘을 구분할 수 없다. 앱은 data 를 지우고 다시 색인한다(스펙 §5.3).
    WrongKeyOrCorrupt,
    Io(std::io::Error),
    Sql(rusqlite::Error),
}

impl From<rusqlite::Error> for MetaError {
    fn from(e: rusqlite::Error) -> Self {
        // 키 불일치(NotADatabase)와 페이지 손상(DatabaseCorrupt)은 고칠 방법이 다시 색인뿐이다.
        match e.sqlite_error_code() {
            Some(rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt) => MetaError::WrongKeyOrCorrupt,
            _ => MetaError::Sql(e),
        }
    }
}

impl std::fmt::Display for MetaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetaError::WrongKeyOrCorrupt => write!(f, "메타 DB 를 열 수 없음(키 불일치 또는 손상)"),
            MetaError::Io(e) => write!(f, "메타 DB 폴더 오류: {e}"),
            MetaError::Sql(e) => write!(f, "메타 DB 오류: {e}"),
        }
    }
}

pub type Result<T> = std::result::Result<T, MetaError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Indexed,
}

pub struct ContentData {
    pub fail: Option<String>,
    pub title: Option<String>,
    pub distribution: bool,
    pub doc: Option<doc::Document>,
}

#[derive(Debug, Clone)]
pub struct Content {
    pub id: i64,
    pub hash: u128,
    pub state: State,
    pub fail: Option<String>,
    pub title: Option<String>,
    pub distribution: bool,
    pub retries: u32,
}

#[derive(Debug, Clone)]
pub struct FileRow {
    pub path: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub content_id: Option<i64>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Counts {
    pub files: u64,
    pub contents: u64,
    pub failed: u64,
    pub pending: u64,
}

const SCHEMA_VERSION: &str = "1";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS kv(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS root(path TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS content(
  id INTEGER PRIMARY KEY,
  hash BLOB NOT NULL UNIQUE,
  state INTEGER NOT NULL,
  fail TEXT,
  title TEXT,
  distribution INTEGER NOT NULL DEFAULT 0,
  doc TEXT,
  retries INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS content_state ON content(state);
CREATE TABLE IF NOT EXISTS file(
  path TEXT PRIMARY KEY,
  size INTEGER NOT NULL,
  mtime_ns INTEGER NOT NULL,
  content_id INTEGER REFERENCES content(id)
);
CREATE INDEX IF NOT EXISTS file_content ON file(content_id);
";

pub struct Meta {
    conn: Connection,
}

fn hex(key: &[u8]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

fn state_of(v: i64) -> State {
    if v == 1 { State::Indexed } else { State::Pending }
}

fn content_row(r: &rusqlite::Row) -> rusqlite::Result<Content> {
    let hash: Vec<u8> = r.get(1)?;
    // 16바이트가 아니면 손상된 행 — panic 대신 SQLITE_CORRUPT 로 올린다. From 이 WrongKeyOrCorrupt 로 바꿔
    // 앱이 다시 색인한다(고칠 방법이 그것뿐, 스펙 §5.3·§6.8).
    let h = <[u8; 16]>::try_from(&hash[..]).map_err(|_| {
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT), Some("해시 길이가 16바이트가 아님".into()))
    })?;
    Ok(Content {
        id: r.get(0)?,
        hash: u128::from_le_bytes(h),
        state: state_of(r.get(2)?),
        fail: r.get(3)?,
        title: r.get(4)?,
        distribution: r.get::<_, i64>(5)? != 0,
        retries: r.get::<_, i64>(6)? as u32,
    })
}

const CONTENT_COLS: &str = "id, hash, state, fail, title, distribution, retries";

impl Meta {
    pub fn open(path: &Path, key: &[u8; 32]) -> Result<Meta> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(MetaError::Io)?;
        }
        let conn = Connection::open(path)?;
        // 키는 연결에서 가장 먼저. 원시 키 형식 x'…' 은 키 유도(PBKDF2)를 건너뛴다.
        conn.pragma_update(None, "key", format!("x'{}'", hex(key)))?;
        // `index` 가 쓰는 동안 `search`·`status` 가 읽어도 SQLITE_BUSY 로 바로 실패하지 않게(DEC-039).
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // 키가 틀리면 여기(첫 읽기)에서 NotADatabase 가 난다 — From 이 WrongKeyOrCorrupt 로 바꾼다.
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // 스키마 버전을 먼저 확인한다 — 미래 버전 DB 에 현재 SCHEMA 를 적용하다 "no such column" 으로 매번 실패하지 않고 다시 색인하게.
        conn.execute_batch("CREATE TABLE IF NOT EXISTS kv(key TEXT PRIMARY KEY, value TEXT NOT NULL);")?;
        let v: Option<String> = conn.query_row("SELECT value FROM kv WHERE key='schema'", [], |r| r.get(0)).optional()?;
        match v.as_deref() {
            // 다른 스키마 = 다른 버전의 앱이 만든 색인 → 다시 색인(스펙 §6.8)
            Some(x) if x != SCHEMA_VERSION => return Err(MetaError::WrongKeyOrCorrupt),
            _ => {}
        }
        conn.execute_batch(SCHEMA)?;
        if v.is_none() {
            conn.execute("INSERT INTO kv(key, value) VALUES('schema', ?1)", [SCHEMA_VERSION])?;
        }
        let m = Meta { conn };
        Ok(m)
    }

    pub fn roots(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare_cached("SELECT path FROM root ORDER BY path")?;
        let v = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(v)
    }

    pub fn set_roots(&mut self, roots: &[String]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM root", [])?;
        for r in roots {
            tx.execute("INSERT OR IGNORE INTO root(path) VALUES(?1)", [r])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn known_files(&self) -> Result<HashMap<String, (u64, i64)>> {
        let mut st = self.conn.prepare_cached("SELECT path, size, mtime_ns FROM file")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)?))))?;
        Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
    }

    pub fn content_by_hash(&self, hash: u128) -> Result<Option<Content>> {
        let sql = format!("SELECT {CONTENT_COLS} FROM content WHERE hash = ?1");
        Ok(self.conn.query_row(&sql, [hash.to_le_bytes().to_vec()], content_row).optional()?)
    }

    pub fn content(&self, id: i64) -> Result<Option<Content>> {
        let sql = format!("SELECT {CONTENT_COLS} FROM content WHERE id = ?1");
        Ok(self.conn.query_row(&sql, [id], content_row).optional()?)
    }

    fn doc_json(d: &ContentData) -> Option<String> {
        d.doc.as_ref().map(|doc| serde_json::to_string(doc).expect("문서 직렬화"))
    }

    pub fn insert_content(&mut self, hash: u128, d: &ContentData) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO content(hash, state, fail, title, distribution, doc) VALUES(?1, 0, ?2, ?3, ?4, ?5)",
            params![hash.to_le_bytes().to_vec(), d.fail, d.title, d.distribution as i64, Self::doc_json(d)],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn retry_content(&mut self, id: i64, d: &ContentData) -> Result<()> {
        self.conn.execute(
            "UPDATE content SET state = 0, fail = ?2, title = ?3, distribution = ?4, doc = ?5, retries = retries + 1 WHERE id = ?1",
            params![id, d.fail, d.title, d.distribution as i64, Self::doc_json(d)],
        )?;
        Ok(())
    }

    pub fn upsert_file(&mut self, path: &str, size: u64, mtime_ns: i64, content_id: i64) -> Result<Option<i64>> {
        let tx = self.conn.transaction()?;
        let old: Option<Option<i64>> = tx.query_row("SELECT content_id FROM file WHERE path = ?1", [path], |r| r.get(0)).optional()?;
        tx.execute(
            "INSERT INTO file(path, size, mtime_ns, content_id) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET size = excluded.size, mtime_ns = excluded.mtime_ns, content_id = excluded.content_id",
            params![path, size as i64, mtime_ns, content_id],
        )?;
        // 파일 이름 목록이 바뀌었으니 새 내용과 이전 내용 모두 키워드 색인을 다시 쓴다.
        tx.execute("UPDATE content SET state = 0 WHERE id = ?1", [content_id])?;
        if let Some(Some(o)) = old {
            tx.execute("UPDATE content SET state = 0 WHERE id = ?1", [o])?;
        }
        tx.commit()?;
        Ok(old.flatten())
    }

    pub fn remove_file(&mut self, path: &str) -> Result<Option<i64>> {
        let tx = self.conn.transaction()?;
        let old: Option<Option<i64>> = tx.query_row("SELECT content_id FROM file WHERE path = ?1", [path], |r| r.get(0)).optional()?;
        tx.execute("DELETE FROM file WHERE path = ?1", [path])?;
        if let Some(Some(o)) = old {
            tx.execute("UPDATE content SET state = 0 WHERE id = ?1", [o])?;
        }
        tx.commit()?;
        Ok(old.flatten())
    }

    pub fn mark_pending(&mut self, content_id: i64) -> Result<()> {
        self.conn.execute("UPDATE content SET state = 0 WHERE id = ?1", [content_id])?;
        Ok(())
    }

    pub fn mark_all_pending(&mut self) -> Result<()> {
        self.conn.execute("UPDATE content SET state = 0", [])?;
        Ok(())
    }

    pub fn pending(&self, limit: usize) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare_cached("SELECT id FROM content WHERE state = 0 ORDER BY id LIMIT ?1")?;
        let v = st.query_map([limit as i64], |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(v)
    }

    pub fn orphans(&self) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare_cached("SELECT id FROM content c WHERE NOT EXISTS (SELECT 1 FROM file f WHERE f.content_id = c.id)")?;
        let v = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(v)
    }

    pub fn doc(&self, content_id: i64) -> Result<Option<doc::Document>> {
        let s: Option<Option<String>> = self.conn.query_row("SELECT doc FROM content WHERE id = ?1", [content_id], |r| r.get(0)).optional()?;
        match s.flatten() {
            None => Ok(None),
            // 손상·비호환 JSON 을 조용히 "본문 없음"으로 바꾸면 파일명만 색인된다 — 손상으로 올려 앱이 다시 색인하게 한다(스펙 §5.3·§6.8).
            Some(j) => serde_json::from_str(&j).map(Some).map_err(|_| MetaError::WrongKeyOrCorrupt),
        }
    }

    pub fn files_of(&self, content_id: i64) -> Result<Vec<FileRow>> {
        let mut st = self.conn.prepare_cached("SELECT path, size, mtime_ns, content_id FROM file WHERE content_id = ?1 ORDER BY mtime_ns DESC, path")?;
        let v = st
            .query_map([content_id], |r| Ok(FileRow { path: r.get(0)?, size: r.get::<_, i64>(1)? as u64, mtime_ns: r.get(2)?, content_id: r.get(3)? }))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn retry_candidates(&self) -> Result<Vec<(i64, String)>> {
        let mut st = self.conn.prepare_cached(
            "SELECT id, p FROM (
               SELECT c.id AS id,
                      (SELECT f.path FROM file f WHERE f.content_id = c.id ORDER BY f.mtime_ns DESC, f.path LIMIT 1) AS p
               FROM content c
               WHERE c.fail IN ('Timeout', 'Crashed', 'Unreadable') AND c.retries = 0
             ) WHERE p IS NOT NULL ORDER BY id",
        )?;
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<(i64, String)>>>()?;
        Ok(v)
    }

    pub fn finish_flush(&mut self, indexed: &[i64], deleted: &[i64], opstamp: u64) -> Result<()> {
        let tx = self.conn.transaction()?;
        for id in indexed {
            tx.execute("UPDATE content SET state = 1 WHERE id = ?1", [id])?;
        }
        for id in deleted {
            tx.execute("DELETE FROM content WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM file WHERE content_id = ?1)", [id])?;
        }
        tx.execute("INSERT INTO kv(key, value) VALUES('keyword_opstamp', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [opstamp.to_string()])?;
        tx.commit()?;
        Ok(())
    }

    pub fn keyword_opstamp(&self) -> Result<u64> {
        let v: Option<String> = self.conn.query_row("SELECT value FROM kv WHERE key='keyword_opstamp'", [], |r| r.get(0)).optional()?;
        Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
    }

    pub fn counts(&self) -> Result<Counts> {
        let one = |sql: &str| -> Result<u64> { Ok(self.conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as u64) };
        Ok(Counts {
            files: one("SELECT count(*) FROM file")?,
            contents: one("SELECT count(*) FROM content")?,
            failed: one("SELECT count(*) FROM content WHERE fail IS NOT NULL")?,
            pending: one("SELECT count(*) FROM content WHERE state = 0")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(title: &str) -> ContentData {
        ContentData {
            fail: None,
            title: Some(title.into()),
            distribution: false,
            doc: Some(doc::Document { title: Some(title.into()), blocks: vec![doc::Block { kind: doc::BlockKind::Paragraph, text: "본문 비밀표지".into() }] }),
        }
    }

    #[test]
    fn wrong_key_is_detected_and_file_is_encrypted() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("meta.db");
        let key = [7u8; 32];
        {
            let mut m = Meta::open(&p, &key).unwrap();
            let id = m.insert_content(42, &data("내부통제규정")).unwrap();
            m.upsert_file("C:/문서/규정.hwp", 10, 1, id).unwrap();
        }
        assert!(matches!(Meta::open(&p, &[8u8; 32]), Err(MetaError::WrongKeyOrCorrupt)));
        let raw = std::fs::read(&p).unwrap();
        assert!(!raw.windows("비밀표지".len()).any(|w| w == "비밀표지".as_bytes()), "평문이 디스크에 남으면 안 됨");
        assert!(!raw.starts_with(b"SQLite format 3"));
        let m = Meta::open(&p, &key).unwrap();
        assert_eq!(m.counts().unwrap(), Counts { files: 1, contents: 1, failed: 0, pending: 1 });
    }

    #[test]
    fn flush_cycle_and_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Meta::open(&dir.path().join("meta.db"), &[1u8; 32]).unwrap();
        let a = m.insert_content(1, &data("가")).unwrap();
        m.upsert_file("/d/a.hwp", 1, 10, a).unwrap();
        m.upsert_file("/d/a-복사본.hwp", 1, 20, a).unwrap();
        assert_eq!(m.pending(10).unwrap(), vec![a]);
        m.finish_flush(&[a], &[], 5).unwrap();
        assert!(m.pending(10).unwrap().is_empty());
        assert_eq!(m.keyword_opstamp().unwrap(), 5);
        assert_eq!(m.files_of(a).unwrap().iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["/d/a-복사본.hwp", "/d/a.hwp"]);

        // 경로 하나 삭제 → 다시 Pending(파일 이름 목록 변경), 아직 고아 아님
        assert_eq!(m.remove_file("/d/a.hwp").unwrap(), Some(a));
        assert_eq!(m.pending(10).unwrap(), vec![a]);
        assert!(m.orphans().unwrap().is_empty());
        // 마지막 경로 삭제 → 고아
        m.remove_file("/d/a-복사본.hwp").unwrap();
        assert_eq!(m.orphans().unwrap(), vec![a]);
        m.finish_flush(&[], &[a], 6).unwrap();
        assert!(m.content(a).unwrap().is_none());
        assert_eq!(m.content_by_hash(1).unwrap().map(|c| c.id), None);
    }

    #[test]
    fn doc_roundtrip_and_retry() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Meta::open(&dir.path().join("meta.db"), &[2u8; 32]).unwrap();
        let failed = ContentData { fail: Some("Timeout".into()), title: None, distribution: false, doc: None };
        let id = m.insert_content(u128::MAX, &failed).unwrap();
        assert_eq!(m.content_by_hash(u128::MAX).unwrap().unwrap().fail.as_deref(), Some("Timeout"));
        assert!(m.doc(id).unwrap().is_none());
        m.retry_content(id, &data("다시")).unwrap();
        let c = m.content(id).unwrap().unwrap();
        assert_eq!((c.retries, c.fail, c.state), (1, None, State::Pending));
        assert_eq!(m.doc(id).unwrap().unwrap().blocks[0].text, "본문 비밀표지");
    }

    #[test]
    fn retry_candidates_are_transient_failures_not_yet_retried() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Meta::open(&dir.path().join("meta.db"), &[4u8; 32]).unwrap();
        let fail = |k: &str| ContentData { fail: Some(k.into()), title: None, distribution: false, doc: None };
        let timeout = m.insert_content(1, &fail("Timeout")).unwrap();
        m.upsert_file("/d/old.pdf", 1, 10, timeout).unwrap();
        m.upsert_file("/d/new.pdf", 1, 20, timeout).unwrap();
        let unreadable = m.insert_content(2, &fail("Unreadable")).unwrap();
        m.upsert_file("/d/locked.docx", 1, 10, unreadable).unwrap();
        let drm = m.insert_content(3, &fail("Drm")).unwrap(); // 다시 해도 같은 결과 — 대상 아님
        m.upsert_file("/d/drm.hwp", 1, 10, drm).unwrap();
        let crashed_orphan = m.insert_content(4, &fail("Crashed")).unwrap(); // 경로 없음 — 대상 아님
        let ok = m.insert_content(5, &data("정상")).unwrap();
        m.upsert_file("/d/ok.hwp", 1, 10, ok).unwrap();
        assert_eq!(m.retry_candidates().unwrap(), vec![(timeout, "/d/new.pdf".to_string()), (unreadable, "/d/locked.docx".to_string())]);
        assert!(m.content(crashed_orphan).unwrap().is_some());
        // 한 번 재시도하면(결과가 또 실패여도) 다시 후보가 되지 않는다.
        m.retry_content(timeout, &fail("Timeout")).unwrap();
        assert_eq!(m.retry_candidates().unwrap(), vec![(unreadable, "/d/locked.docx".to_string())]);
    }

    #[test]
    fn corrupt_and_not_a_database_map_to_rebuild() {
        for code in [rusqlite::ffi::SQLITE_CORRUPT, rusqlite::ffi::SQLITE_NOTADB] {
            let e = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None);
            assert!(matches!(MetaError::from(e), MetaError::WrongKeyOrCorrupt), "{code}");
        }
        let busy = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY), None);
        assert!(matches!(MetaError::from(busy), MetaError::Sql(_)), "일시적 오류는 다시 색인 대상이 아님");
    }

    #[test]
    fn roots_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Meta::open(&dir.path().join("meta.db"), &[3u8; 32]).unwrap();
        m.set_roots(&["/b".into(), "/a".into(), "/a".into()]).unwrap();
        assert_eq!(m.roots().unwrap(), vec!["/a".to_string(), "/b".to_string()]);
    }

    #[test]
    fn other_schema_version_means_rebuild_before_schema_is_applied() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("meta.db");
        let key = [5u8; 32];
        drop(Meta::open(&p, &key).unwrap());
        {
            let conn = Connection::open(&p).unwrap();
            conn.pragma_update(None, "key", format!("x'{}'", hex(&key))).unwrap();
            conn.execute("UPDATE kv SET value = '2' WHERE key = 'schema'", []).unwrap();
        }
        assert!(matches!(Meta::open(&p, &key), Err(MetaError::WrongKeyOrCorrupt)));
    }

    #[test]
    fn damaged_rows_are_errors_not_panics_or_silent_none() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Meta::open(&dir.path().join("meta.db"), &[6u8; 32]).unwrap();
        let id = m.insert_content(1, &data("가")).unwrap();
        m.conn.execute("UPDATE content SET doc = '{깨짐' WHERE id = ?1", [id]).unwrap();
        // 손상된 행은 다시 색인으로만 고칠 수 있다 — 앱이 다시 만들도록 WrongKeyOrCorrupt(스펙 §5.3·§6.8).
        assert!(matches!(m.doc(id), Err(MetaError::WrongKeyOrCorrupt)));
        m.conn.execute("UPDATE content SET hash = x'0102' WHERE id = ?1", [id]).unwrap();
        assert!(matches!(m.content(id), Err(MetaError::WrongKeyOrCorrupt)));
        assert!(matches!(m.content_by_hash(1), Ok(None)), "행을 못 찾은 것은 손상이 아니다");
    }
}
