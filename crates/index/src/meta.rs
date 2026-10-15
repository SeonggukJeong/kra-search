//! 메타 DB(meta.db) — SQLCipher(키는 platform::keys). 앱만 쓴다(스펙 §5.1).
//! 내용(content) = 같은 바이트의 파일 묶음(xxh3-128, 스펙 §6.4). 추출·색인은 내용 단위, 경로(file)는 여러 개.
//! 키워드 색인과의 일관성(스펙 §6.8): 내용을 Pending 으로 두고 → 키워드 색인에 쓰고 커밋 → finish_flush 로 Indexed.
//! 중간에 죽으면 Pending 이 남아 다음 실행에서 다시 쓴다(키워드 쪽은 delete+add 라 여러 번 써도 같다).

use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
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
            Some(rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt) => {
                MetaError::WrongKeyOrCorrupt
            }
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

const SCHEMA_VERSION: &str = "2";

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
CREATE TABLE IF NOT EXISTS card(
  content_id INTEGER PRIMARY KEY REFERENCES content(id) ON DELETE CASCADE,
  slot INTEGER
);
CREATE TABLE IF NOT EXISTS body(
  content_id INTEGER PRIMARY KEY REFERENCES content(id) ON DELETE CASCADE,
  total INTEGER NOT NULL,
  data INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS chunk(
  content_id INTEGER NOT NULL REFERENCES content(id) ON DELETE CASCADE,
  pos INTEGER NOT NULL,
  slot INTEGER,
  PRIMARY KEY(content_id, pos)
);
CREATE INDEX IF NOT EXISTS chunk_slot ON chunk(slot);
CREATE INDEX IF NOT EXISTS card_slot ON card(slot);
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VecKind {
    Cards,
    Chunks,
}

impl VecKind {
    pub fn name(self) -> &'static str {
        match self {
            VecKind::Cards => "cards",
            VecKind::Chunks => "chunks",
        }
    }
    fn table(self) -> &'static str {
        match self {
            VecKind::Cards => "card",
            VecKind::Chunks => "chunk",
        }
    }
}

/// (세대, 길이, 살아 있는 슬롯 → 내용 id)
pub type VectorSnapshot = (u64, u64, Vec<(u64, i64)>);

#[derive(Debug, Clone, PartialEq)]
pub struct EmbedTodo {
    pub id: i64,
    pub path: String,
    pub mtime_ns: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BodyRow {
    pub id: i64,
    pub total: usize,
    pub data: bool,
    pub mtime_ns: i64,
    pub path: String,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct EmbedCounts {
    pub cards_done: u64,
    pub cards_total: u64,
    /// 본문 ①+② (화면의 "정밀 의미 검색 N%")
    pub main_done: u64,
    pub main_total: u64,
    /// 본문 ③ 데이터 형식 뒷부분
    pub tail_done: u64,
    pub tail_total: u64,
    /// 아직 청크 수를 세지 않은 내용
    pub unplanned: u64,
}

/// 가장 최근 경로(경로·수정 시각). 경로 없는 내용(고아)은 빠진다.
const NEWEST: &str = "JOIN file f ON f.path = (SELECT path FROM file WHERE content_id = c.id ORDER BY mtime_ns DESC, path LIMIT 1)";

pub struct Meta {
    conn: Connection,
}

fn hex(key: &[u8]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

fn state_of(v: i64) -> State {
    if v == 1 {
        State::Indexed
    } else {
        State::Pending
    }
}

fn content_row(r: &rusqlite::Row) -> rusqlite::Result<Content> {
    let hash: Vec<u8> = r.get(1)?;
    // 16바이트가 아니면 손상된 행 — panic 대신 SQLITE_CORRUPT 로 올린다. From 이 WrongKeyOrCorrupt 로 바꿔
    // 앱이 다시 색인한다(고칠 방법이 그것뿐, 스펙 §5.3·§6.8).
    let h = <[u8; 16]>::try_from(&hash[..]).map_err(|_| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
            Some("해시 길이가 16바이트가 아님".into()),
        )
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
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
            r.get::<_, i64>(0)
        })?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // 스키마 버전을 먼저 확인한다 — 미래 버전 DB 에 현재 SCHEMA 를 적용하다 "no such column" 으로 매번 실패하지 않고 다시 색인하게.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS kv(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        let v: Option<String> = conn
            .query_row("SELECT value FROM kv WHERE key='schema'", [], |r| r.get(0))
            .optional()?;
        match v.as_deref() {
            // v1(M1) → v2: 임베딩 테이블만 더한다 — 다시 추출하지 않는다(DEC-054).
            // 두 문장이 따로지만 다시 실행해도 안전하다: SCHEMA 는 전부 IF NOT EXISTS 이고 버전은 마지막에 바꾼다.
            Some("1") => {
                conn.execute_batch(SCHEMA)?;
                conn.execute(
                    "UPDATE kv SET value = ?1 WHERE key = 'schema'",
                    [SCHEMA_VERSION],
                )?;
            }
            // 다른 스키마 = 다른 버전의 앱이 만든 색인 → 다시 색인(스펙 §6.8)
            Some(x) if x != SCHEMA_VERSION => return Err(MetaError::WrongKeyOrCorrupt),
            _ => {}
        }
        conn.execute_batch(SCHEMA)?;
        if v.is_none() {
            conn.execute(
                "INSERT INTO kv(key, value) VALUES('schema', ?1)",
                [SCHEMA_VERSION],
            )?;
        }
        let m = Meta { conn };
        Ok(m)
    }

    pub fn roots(&self) -> Result<Vec<String>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT path FROM root ORDER BY path")?;
        let v = st
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
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
        let mut st = self
            .conn
            .prepare_cached("SELECT path, size, mtime_ns FROM file")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)?),
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
    }

    pub fn content_by_hash(&self, hash: u128) -> Result<Option<Content>> {
        let sql = format!("SELECT {CONTENT_COLS} FROM content WHERE hash = ?1");
        Ok(self
            .conn
            .query_row(&sql, [hash.to_le_bytes().to_vec()], content_row)
            .optional()?)
    }

    pub fn content(&self, id: i64) -> Result<Option<Content>> {
        let sql = format!("SELECT {CONTENT_COLS} FROM content WHERE id = ?1");
        Ok(self.conn.query_row(&sql, [id], content_row).optional()?)
    }

    fn doc_json(d: &ContentData) -> Option<String> {
        d.doc
            .as_ref()
            .map(|doc| serde_json::to_string(doc).expect("문서 직렬화"))
    }

    pub fn insert_content(&mut self, hash: u128, d: &ContentData) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO content(hash, state, fail, title, distribution, doc) VALUES(?1, 0, ?2, ?3, ?4, ?5)",
            params![hash.to_le_bytes().to_vec(), d.fail, d.title, d.distribution as i64, Self::doc_json(d)],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn retry_content(&mut self, id: i64, d: &ContentData) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE content SET state = 0, fail = ?2, title = ?3, distribution = ?4, doc = ?5, retries = retries + 1 WHERE id = ?1",
            params![id, d.fail, d.title, d.distribution as i64, Self::doc_json(d)],
        )?;
        // 문서가 바뀌었으니 의미 검색 벡터도 무효(DEC-048) — 다음 임베딩 단계가 다시 만든다.
        for t in ["card", "body", "chunk"] {
            tx.execute(&format!("DELETE FROM {t} WHERE content_id = ?1"), [id])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_file(
        &mut self,
        path: &str,
        size: u64,
        mtime_ns: i64,
        content_id: i64,
    ) -> Result<Option<i64>> {
        let tx = self.conn.transaction()?;
        let old: Option<Option<i64>> = tx
            .query_row("SELECT content_id FROM file WHERE path = ?1", [path], |r| {
                r.get(0)
            })
            .optional()?;
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
        let old: Option<Option<i64>> = tx
            .query_row("SELECT content_id FROM file WHERE path = ?1", [path], |r| {
                r.get(0)
            })
            .optional()?;
        tx.execute("DELETE FROM file WHERE path = ?1", [path])?;
        if let Some(Some(o)) = old {
            tx.execute("UPDATE content SET state = 0 WHERE id = ?1", [o])?;
        }
        tx.commit()?;
        Ok(old.flatten())
    }

    pub fn mark_pending(&mut self, content_id: i64) -> Result<()> {
        self.conn
            .execute("UPDATE content SET state = 0 WHERE id = ?1", [content_id])?;
        Ok(())
    }

    pub fn mark_all_pending(&mut self) -> Result<()> {
        self.conn.execute("UPDATE content SET state = 0", [])?;
        Ok(())
    }

    pub fn pending(&self, limit: usize) -> Result<Vec<i64>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT id FROM content WHERE state = 0 ORDER BY id LIMIT ?1")?;
        let v = st
            .query_map([limit as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(v)
    }

    pub fn orphans(&self) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare_cached("SELECT id FROM content c WHERE NOT EXISTS (SELECT 1 FROM file f WHERE f.content_id = c.id)")?;
        let v = st
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(v)
    }

    pub fn doc(&self, content_id: i64) -> Result<Option<doc::Document>> {
        let s: Option<Option<String>> = self
            .conn
            .query_row("SELECT doc FROM content WHERE id = ?1", [content_id], |r| {
                r.get(0)
            })
            .optional()?;
        match s.flatten() {
            None => Ok(None),
            // 손상·비호환 JSON 을 조용히 "본문 없음"으로 바꾸면 파일명만 색인된다 — 손상으로 올려 앱이 다시 색인하게 한다(스펙 §5.3·§6.8).
            Some(j) => serde_json::from_str(&j)
                .map(Some)
                .map_err(|_| MetaError::WrongKeyOrCorrupt),
        }
    }

    pub fn files_of(&self, content_id: i64) -> Result<Vec<FileRow>> {
        let mut st = self.conn.prepare_cached("SELECT path, size, mtime_ns, content_id FROM file WHERE content_id = ?1 ORDER BY mtime_ns DESC, path")?;
        let v = st
            .query_map([content_id], |r| {
                Ok(FileRow {
                    path: r.get(0)?,
                    size: r.get::<_, i64>(1)? as u64,
                    mtime_ns: r.get(2)?,
                    content_id: r.get(3)?,
                })
            })?
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
        let v = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<(i64, String)>>>()?;
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
        let v: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM kv WHERE key='keyword_opstamp'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
    }

    pub fn counts(&self) -> Result<Counts> {
        let one = |sql: &str| -> Result<u64> {
            Ok(self.conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as u64)
        };
        Ok(Counts {
            files: one("SELECT count(*) FROM file")?,
            contents: one("SELECT count(*) FROM content")?,
            failed: one("SELECT count(*) FROM content WHERE fail IS NOT NULL")?,
            pending: one("SELECT count(*) FROM content WHERE state = 0")?,
        })
    }

    fn todo(&self, missing: &str) -> Result<Vec<EmbedTodo>> {
        let sql = format!(
            "SELECT c.id, f.path, f.mtime_ns FROM content c {NEWEST}
             WHERE c.state = 1 AND c.doc IS NOT NULL AND NOT EXISTS (SELECT 1 FROM {missing} x WHERE x.content_id = c.id) ORDER BY c.id"
        );
        let mut st = self.conn.prepare(&sql)?;
        let v = st
            .query_map([], |r| {
                Ok(EmbedTodo {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    mtime_ns: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// 카드를 만들 내용: 키워드에 반영됐고(Indexed) 본문이 있고 카드 행이 없다.
    pub fn card_todo(&self) -> Result<Vec<EmbedTodo>> {
        self.todo("card")
    }

    /// 청크 수를 세야 할 내용.
    pub fn body_todo(&self) -> Result<Vec<EmbedTodo>> {
        self.todo("body")
    }

    fn set_len(tx: &rusqlite::Transaction, key: &str, len: u64) -> rusqlite::Result<usize> {
        tx.execute("INSERT INTO kv(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, len.to_string()])
    }

    /// 카드 행(slot None = 빈 카드·실패)과 파일 길이를 한 트랜잭션에(DEC-048: 파일 fsync 뒤에 부른다).
    pub fn set_cards(&mut self, rows: &[(i64, Option<u64>)], cards_len: u64) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (id, slot) in rows {
            tx.execute(
                "INSERT OR REPLACE INTO card(content_id, slot) VALUES(?1, ?2)",
                params![id, slot.map(|s| s as i64)],
            )?;
        }
        Self::set_len(&tx, "cards_len", cards_len)?;
        tx.commit()?;
        Ok(())
    }

    /// 본문 청크 수 행(total = 청크 수, data = 데이터 형식 여부)을 기록한다.
    pub fn set_bodies(&mut self, rows: &[(i64, usize, bool)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (id, total, data) in rows {
            tx.execute(
                "INSERT OR REPLACE INTO body(content_id, total, data) VALUES(?1, ?2, ?3)",
                params![id, *total as i64, *data as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 청크 수가 센 내용의 행(가장 최근 경로 포함). 경로 없는 내용은 빠진다.
    pub fn bodies(&self) -> Result<Vec<BodyRow>> {
        let sql = format!("SELECT c.id, b.total, b.data, f.mtime_ns, f.path FROM body b JOIN content c ON c.id = b.content_id {NEWEST} ORDER BY c.id");
        let mut st = self.conn.prepare(&sql)?;
        let v = st
            .query_map([], |r| {
                Ok(BodyRow {
                    id: r.get(0)?,
                    total: r.get::<_, i64>(1)? as usize,
                    data: r.get::<_, i64>(2)? != 0,
                    mtime_ns: r.get(3)?,
                    path: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// 그 내용에서 이미 처리한 청크 위치(슬롯 없는 빈 청크 포함).
    pub fn chunk_positions(&self, id: i64) -> Result<HashSet<usize>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT pos FROM chunk WHERE content_id = ?1")?;
        let v = st
            .query_map([id], |r| r.get::<_, i64>(0))?
            .map(|x| x.map(|p| p as usize))
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        Ok(v)
    }

    /// 청크 행(slot None = 빈 청크·실패)과 파일 길이를 한 트랜잭션에(DEC-048: 파일 fsync 뒤에 부른다).
    pub fn add_chunks(
        &mut self,
        rows: &[(i64, usize, Option<u64>)],
        chunks_len: u64,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (id, pos, slot) in rows {
            tx.execute(
                "INSERT OR REPLACE INTO chunk(content_id, pos, slot) VALUES(?1, ?2, ?3)",
                params![id, *pos as i64, slot.map(|s| s as i64)],
            )?;
        }
        Self::set_len(&tx, "chunks_len", chunks_len)?;
        tx.commit()?;
        Ok(())
    }

    fn live_in(conn: &Connection, kind: VecKind) -> rusqlite::Result<Vec<(u64, i64)>> {
        let sql = format!(
            "SELECT slot, content_id FROM {} WHERE slot IS NOT NULL ORDER BY slot",
            kind.table()
        );
        let mut st = conn.prepare(&sql)?;
        let v = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)))?
            .collect();
        v
    }

    /// 살아 있는 슬롯(슬롯 → 내용 id).
    pub fn live_slots(&self, kind: VecKind) -> Result<Vec<(u64, i64)>> {
        Ok(Self::live_in(&self.conn, kind)?)
    }

    /// 검색용: 세대·길이·살아 있는 슬롯을 한 읽기 트랜잭션에서(색인 중 압축과 엇갈리지 않게).
    pub fn vector_snapshot(&self, kind: VecKind) -> Result<VectorSnapshot> {
        let tx = self.conn.unchecked_transaction()?;
        let get = |k: String| -> rusqlite::Result<u64> {
            Ok(tx
                .query_row("SELECT value FROM kv WHERE key = ?1", [k], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?
                .and_then(|s| s.parse().ok())
                .unwrap_or(0))
        };
        let gen = get(format!("{}_gen", kind.name()))?;
        let len = get(format!("{}_len", kind.name()))?;
        let live = Self::live_in(&tx, kind)?;
        tx.commit()?;
        Ok((gen, len, live))
    }

    /// 벡터 슬롯이 있는 청크를 하나라도 가진 내용.
    pub fn chunked_contents(&self) -> Result<HashSet<i64>> {
        let mut st = self
            .conn
            .prepare("SELECT DISTINCT content_id FROM chunk WHERE slot IS NOT NULL")?;
        let v = st
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<HashSet<i64>>>()?;
        Ok(v)
    }

    /// 내용 → 가장 최근 경로의 수정 시각(문서 날짜, 스펙 §8.1 M2).
    pub fn newest_mtimes(&self) -> Result<HashMap<i64, i64>> {
        let mut st = self.conn.prepare("SELECT content_id, MAX(mtime_ns) FROM file WHERE content_id IS NOT NULL GROUP BY content_id")?;
        let v = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<HashMap<i64, i64>>>()?;
        Ok(v)
    }

    /// 내용 → 화면에 보이는 제목. 검색 핵심이 제목에서 연도를 읽는 데 쓴다("작년 경마시행계획" 이 2025년 계획을
    /// 찾게). 제목이 비어 있지 않으면 제목, 아니면 가장 최근 경로의 파일 이름(확장자 포함 — 호출자는 연도만 훑는다).
    /// 경로 없는 내용(고아)은 빠진다.
    pub fn title_texts(&self) -> Result<HashMap<i64, String>> {
        let sql = format!("SELECT c.id, c.title, f.path FROM content c {NEWEST}");
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, title, path) = row?;
            let text = match title.as_deref().map(str::trim) {
                Some(t) if !t.is_empty() => t.to_string(),
                // 경로 구분자는 `/`·`\` 둘 다(Windows 경로가 그대로 들어온다).
                _ => path.rsplit(['/', '\\']).next().unwrap_or(&path).to_string(),
            };
            out.insert(id, text);
        }
        Ok(out)
    }

    /// kv 값 하나를 읽는다(없으면 None).
    pub fn kv(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    /// kv 값 하나를 쓴다(있으면 덮어쓴다).
    pub fn set_kv(&mut self, key: &str, value: &str) -> Result<()> {
        self.conn.execute("INSERT INTO kv(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [key, value])?;
        Ok(())
    }

    /// 벡터 행을 모두 지운다(지문이 바뀌었거나 벡터 파일이 깨짐). 길이·세대는 0.
    pub fn clear_vectors(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM chunk; DELETE FROM body; DELETE FROM card;")?;
        for k in ["cards_len", "chunks_len", "cards_gen", "chunks_gen"] {
            Self::set_len(&tx, k, 0)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// `map` 은 그 종류의 살아 있는 슬롯을 모두 덮어야 한다(빠진 슬롯은 옛 번호 그대로 남아 압축된 새 파일과 어긋난다).
    /// 압축 뒤 슬롯 번호를 바꾸고 새 세대·길이를 기록한다(옛 번호와 새 번호가 겹치지 않게 음수를 거친다).
    pub fn remap_slots(
        &mut self,
        kind: VecKind,
        map: &HashMap<u64, u64>,
        gen: u64,
        len: u64,
    ) -> Result<()> {
        let t = kind.table();
        let tx = self.conn.transaction()?;
        for (old, new) in map {
            tx.execute(
                &format!("UPDATE {t} SET slot = ?1 WHERE slot = ?2"),
                params![-1 - *new as i64, *old as i64],
            )?;
        }
        tx.execute(
            &format!("UPDATE {t} SET slot = -1 - slot WHERE slot < 0"),
            [],
        )?;
        Self::set_len(&tx, &format!("{}_gen", kind.name()), gen)?;
        Self::set_len(&tx, &format!("{}_len", kind.name()), len)?;
        tx.commit()?;
        Ok(())
    }

    /// 모든 쪽을 읽어 본다(DEC-056). 검색은 이제 표 전체(수정일·청크 여부)를 읽으므로, index 가 끝날 때 여기서
    /// 손상을 찾아 한 번 처음부터 다시 만든다 — 그러지 않으면 index 가 읽지 않는 쪽의 손상 때문에 검색만 계속 실패한다.
    pub fn check(&self) -> Result<()> {
        let first: String = self
            .conn
            .query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if first == "ok" {
            Ok(())
        } else {
            Err(MetaError::WrongKeyOrCorrupt)
        }
    }

    /// 임베딩 진행 수(카드·본문 앞부분·데이터 뒷부분·미계획). data_head = 데이터 형식에서 먼저 처리하는 청크 수.
    pub fn embed_counts(&self, data_head: usize) -> Result<EmbedCounts> {
        // 매개변수 없는 질의와 data_head(?1) 를 쓰는 질의를 나눈다 — 안 쓰는 ?1 을 억지로 붙이지 않는다.
        let run = |sql: &str, bind: &[i64]| -> Result<u64> {
            Ok(self
                .conn
                .query_row(sql, rusqlite::params_from_iter(bind), |r| {
                    r.get::<_, Option<i64>>(0)
                })?
                .unwrap_or(0) as u64)
        };
        let none = |sql: &str| run(sql, &[]);
        let head = |sql: &str| run(sql, &[data_head as i64]);
        let has_path = "EXISTS (SELECT 1 FROM file f WHERE f.content_id = c.id)";
        Ok(EmbedCounts {
            cards_done: none(&format!("SELECT count(*) FROM card k JOIN content c ON c.id = k.content_id WHERE {has_path}"))?,
            cards_total: none(&format!("SELECT count(*) FROM content c WHERE c.doc IS NOT NULL AND {has_path}"))?,
            main_done: head("SELECT count(*) FROM chunk k JOIN body b ON b.content_id = k.content_id WHERE b.data = 0 OR k.pos < ?1")?,
            main_total: head("SELECT SUM(CASE WHEN data = 1 THEN MIN(total, ?1) ELSE total END) FROM body")?,
            tail_done: head("SELECT count(*) FROM chunk k JOIN body b ON b.content_id = k.content_id WHERE b.data = 1 AND k.pos >= ?1")?,
            tail_total: head("SELECT SUM(CASE WHEN data = 1 AND total > ?1 THEN total - ?1 ELSE 0 END) FROM body")?,
            unplanned: none(&format!("SELECT count(*) FROM content c WHERE c.doc IS NOT NULL AND {has_path} AND NOT EXISTS (SELECT 1 FROM body b WHERE b.content_id = c.id)"))?,
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
            doc: Some(doc::Document {
                title: Some(title.into()),
                blocks: vec![doc::Block {
                    kind: doc::BlockKind::Paragraph,
                    text: "본문 비밀표지".into(),
                }],
            }),
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
        assert!(matches!(
            Meta::open(&p, &[8u8; 32]),
            Err(MetaError::WrongKeyOrCorrupt)
        ));
        let raw = std::fs::read(&p).unwrap();
        assert!(
            !raw.windows("비밀표지".len())
                .any(|w| w == "비밀표지".as_bytes()),
            "평문이 디스크에 남으면 안 됨"
        );
        assert!(!raw.starts_with(b"SQLite format 3"));
        let m = Meta::open(&p, &key).unwrap();
        assert_eq!(
            m.counts().unwrap(),
            Counts {
                files: 1,
                contents: 1,
                failed: 0,
                pending: 1
            }
        );
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
        assert_eq!(
            m.files_of(a)
                .unwrap()
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/d/a-복사본.hwp", "/d/a.hwp"]
        );

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
        let failed = ContentData {
            fail: Some("Timeout".into()),
            title: None,
            distribution: false,
            doc: None,
        };
        let id = m.insert_content(u128::MAX, &failed).unwrap();
        assert_eq!(
            m.content_by_hash(u128::MAX)
                .unwrap()
                .unwrap()
                .fail
                .as_deref(),
            Some("Timeout")
        );
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
        let fail = |k: &str| ContentData {
            fail: Some(k.into()),
            title: None,
            distribution: false,
            doc: None,
        };
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
        assert_eq!(
            m.retry_candidates().unwrap(),
            vec![
                (timeout, "/d/new.pdf".to_string()),
                (unreadable, "/d/locked.docx".to_string())
            ]
        );
        assert!(m.content(crashed_orphan).unwrap().is_some());
        // 한 번 재시도하면(결과가 또 실패여도) 다시 후보가 되지 않는다.
        m.retry_content(timeout, &fail("Timeout")).unwrap();
        assert_eq!(
            m.retry_candidates().unwrap(),
            vec![(unreadable, "/d/locked.docx".to_string())]
        );
    }

    #[test]
    fn corrupt_and_not_a_database_map_to_rebuild() {
        for code in [rusqlite::ffi::SQLITE_CORRUPT, rusqlite::ffi::SQLITE_NOTADB] {
            let e = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None);
            assert!(
                matches!(MetaError::from(e), MetaError::WrongKeyOrCorrupt),
                "{code}"
            );
        }
        let busy = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            None,
        );
        assert!(
            matches!(MetaError::from(busy), MetaError::Sql(_)),
            "일시적 오류는 다시 색인 대상이 아님"
        );
    }

    #[test]
    fn roots_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Meta::open(&dir.path().join("meta.db"), &[3u8; 32]).unwrap();
        m.set_roots(&["/b".into(), "/a".into(), "/a".into()])
            .unwrap();
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
            conn.pragma_update(None, "key", format!("x'{}'", hex(&key)))
                .unwrap();
            conn.execute("UPDATE kv SET value = '99' WHERE key = 'schema'", [])
                .unwrap();
        }
        assert!(matches!(
            Meta::open(&p, &key),
            Err(MetaError::WrongKeyOrCorrupt)
        ));
    }

    fn indexed(m: &mut Meta, hash: u128, path: &str, mtime: i64) -> i64 {
        let id = m.insert_content(hash, &data(path)).unwrap();
        m.upsert_file(path, 1, mtime, id).unwrap();
        m.finish_flush(&[id], &[], 1).unwrap();
        id
    }

    /// DEC-054: v1 색인은 다시 추출하지 않고 v2 로 이전한다.
    #[test]
    fn v1_database_migrates_without_losing_contents() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("meta.db");
        let key = [9u8; 32];
        {
            let mut m = Meta::open(&p, &key).unwrap();
            indexed(&mut m, 1, "/d/a.hwp", 10);
            m.conn.execute_batch("DROP TABLE chunk; DROP TABLE body; DROP TABLE card; UPDATE kv SET value = '1' WHERE key = 'schema';").unwrap();
        }
        let m = Meta::open(&p, &key).unwrap();
        assert_eq!(m.counts().unwrap().contents, 1);
        assert_eq!(m.kv("schema").unwrap().as_deref(), Some("2"));
        assert_eq!(m.card_todo().unwrap().len(), 1);
        let idx: i64 = m.conn.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name IN ('card_slot', 'chunk_slot')", [], |r| r.get(0)).unwrap();
        assert_eq!(idx, 2, "이전 뒤에도 슬롯 색인이 있어야 한다");
    }

    #[test]
    fn card_and_body_flow() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Meta::open(&dir.path().join("meta.db"), &[11u8; 32]).unwrap();
        let a = indexed(&mut m, 1, "/d/a.hwp", 10);
        let b = indexed(&mut m, 2, "/d/b.csv", 20);
        let pending = m.insert_content(3, &data("c")).unwrap(); // 키워드 반영 전 — 아직 대상 아님
        m.upsert_file("/d/c.hwp", 1, 30, pending).unwrap();
        let failed = m
            .insert_content(
                4,
                &ContentData {
                    fail: Some("Drm".into()),
                    title: None,
                    distribution: false,
                    doc: None,
                },
            )
            .unwrap();
        m.upsert_file("/d/drm.hwp", 1, 40, failed).unwrap();
        m.finish_flush(&[failed], &[], 2).unwrap();
        let todo: Vec<i64> = m.card_todo().unwrap().iter().map(|t| t.id).collect();
        assert_eq!(todo, vec![a, b], "본문 있고 키워드에 반영된 것만");
        assert_eq!(m.card_todo().unwrap()[1].path, "/d/b.csv");
        m.set_cards(&[(a, Some(0)), (b, None)], 1).unwrap();
        assert!(m.card_todo().unwrap().is_empty());
        assert_eq!(m.kv("cards_len").unwrap().as_deref(), Some("1"));
        assert_eq!(
            m.live_slots(VecKind::Cards).unwrap(),
            vec![(0, a)],
            "빈 카드(None)는 슬롯 없음"
        );

        assert_eq!(m.body_todo().unwrap().len(), 2);
        m.set_bodies(&[(a, 3, false), (b, 25, true)]).unwrap();
        assert!(m.body_todo().unwrap().is_empty());
        m.add_chunks(
            &[
                (a, 0, Some(0)),
                (a, 1, Some(1)),
                (b, 0, Some(2)),
                (b, 21, Some(3)),
                (b, 1, None),
            ],
            4,
        )
        .unwrap();
        assert_eq!(m.chunk_positions(b).unwrap(), HashSet::from([0, 1, 21]));
        assert_eq!(m.chunked_contents().unwrap(), HashSet::from([a, b]));
        let c = m.embed_counts(20).unwrap();
        assert_eq!(
            (c.cards_done, c.cards_total),
            (2, 3),
            "분모에는 키워드 반영 전인 c 도 든다(곧 대상)"
        );
        assert_eq!(
            (c.main_done, c.main_total),
            (4, 3 + 20),
            "a 2개 + b 앞부분 2개(0, 1) / a 3 + b 앞 20"
        );
        assert_eq!((c.tail_done, c.tail_total), (1, 5));
        assert_eq!(c.unplanned, 1, "c 는 아직 청크 수를 세지 않았다");
        let rows = m.bodies().unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| (r.id, r.total, r.data))
                .collect::<Vec<_>>(),
            vec![(a, 3, false), (b, 25, true)]
        );
        assert_eq!(m.newest_mtimes().unwrap()[&b], 20);
    }

    #[test]
    fn retry_and_orphan_delete_drop_vector_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Meta::open(&dir.path().join("meta.db"), &[12u8; 32]).unwrap();
        let a = indexed(&mut m, 1, "/d/a.hwp", 10);
        m.set_cards(&[(a, Some(0))], 1).unwrap();
        m.set_bodies(&[(a, 1, false)]).unwrap();
        m.add_chunks(&[(a, 0, Some(0))], 1).unwrap();
        m.retry_content(a, &data("다시")).unwrap();
        assert!(m.live_slots(VecKind::Cards).unwrap().is_empty());
        assert!(m.live_slots(VecKind::Chunks).unwrap().is_empty());
        assert!(m.chunked_contents().unwrap().is_empty());

        m.finish_flush(&[a], &[], 2).unwrap();
        m.set_cards(&[(a, Some(1))], 2).unwrap();
        m.remove_file("/d/a.hwp").unwrap();
        m.finish_flush(&[], &[a], 3).unwrap(); // 고아 정리 → 외래 키로 card 행도
        assert!(m.live_slots(VecKind::Cards).unwrap().is_empty());
    }

    #[test]
    fn check_finds_damaged_pages() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("meta.db");
        let key = [14u8; 32];
        {
            let mut m = Meta::open(&p, &key).unwrap();
            for i in 0..200u128 {
                indexed(&mut m, i, &format!("/d/{i}.hwp"), i as i64);
            }
            m.check().unwrap();
            m.conn
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .unwrap();
        }
        let mut bytes = std::fs::read(&p).unwrap();
        let last = bytes.len() / 4096 - 1;
        for b in &mut bytes[last * 4096..(last + 1) * 4096] {
            *b ^= 0x5A;
        }
        std::fs::write(&p, &bytes).unwrap();
        let r = Meta::open(&p, &key).and_then(|m| m.check());
        assert!(matches!(r, Err(MetaError::WrongKeyOrCorrupt)), "{r:?}");
    }

    #[test]
    fn remap_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Meta::open(&dir.path().join("meta.db"), &[13u8; 32]).unwrap();
        let a = indexed(&mut m, 1, "/d/a.hwp", 10);
        let b = indexed(&mut m, 2, "/d/b.hwp", 20);
        m.set_cards(&[(a, Some(3)), (b, Some(7))], 8).unwrap();
        let map: HashMap<u64, u64> = [(3, 0), (7, 1)].into_iter().collect();
        m.remap_slots(VecKind::Cards, &map, 1, 2).unwrap();
        let mut live = m.live_slots(VecKind::Cards).unwrap();
        live.sort();
        assert_eq!(live, vec![(0, a), (1, b)]);
        assert_eq!(m.vector_snapshot(VecKind::Cards).unwrap().0, 1);
        assert_eq!(m.vector_snapshot(VecKind::Cards).unwrap().1, 2);
        m.clear_vectors().unwrap();
        assert!(m.live_slots(VecKind::Cards).unwrap().is_empty());
        assert_eq!(m.kv("cards_len").unwrap().as_deref(), Some("0"));
        assert_eq!(m.card_todo().unwrap().len(), 2);
    }

    #[test]
    fn title_texts_prefers_title_then_newest_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Meta::open(&dir.path().join("meta.db"), &[15u8; 32]).unwrap();
        // 제목이 있는 내용
        let titled = m.insert_content(1, &data("2025 경마시행계획")).unwrap();
        m.upsert_file("/d/x.hwp", 1, 10, titled).unwrap();
        // 제목이 없는 내용 — 가장 최근 경로의 파일 이름(확장자 포함)
        let untitled = m
            .insert_content(
                2,
                &ContentData {
                    fail: None,
                    title: Some("  ".into()),
                    distribution: false,
                    doc: None,
                },
            )
            .unwrap();
        m.upsert_file("/d/옛날/2023 계획.hwp", 1, 10, untitled)
            .unwrap();
        m.upsert_file("/d/새것/2025 시행계획.hwp", 1, 20, untitled)
            .unwrap();
        // 경로 없는 내용 — 빠진다
        let orphan = m.insert_content(3, &data("고아")).unwrap();
        // Windows 경로(`\`)
        let win = m.insert_content(4, &ContentData { fail: None, title: None, distribution: false, doc: None }).unwrap();
        m.upsert_file(r"C:\d\2025년 계획.hwp", 1, 10, win).unwrap();
        let t = m.title_texts().unwrap();
        assert_eq!(
            t.get(&titled).map(String::as_str),
            Some("2025 경마시행계획")
        );
        assert_eq!(
            t.get(&untitled).map(String::as_str),
            Some("2025 시행계획.hwp")
        );
        assert!(!t.contains_key(&orphan));
        assert_eq!(t.get(&win).map(String::as_str), Some("2025년 계획.hwp"));
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn damaged_rows_are_errors_not_panics_or_silent_none() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Meta::open(&dir.path().join("meta.db"), &[6u8; 32]).unwrap();
        let id = m.insert_content(1, &data("가")).unwrap();
        m.conn
            .execute("UPDATE content SET doc = '{깨짐' WHERE id = ?1", [id])
            .unwrap();
        // 손상된 행은 다시 색인으로만 고칠 수 있다 — 앱이 다시 만들도록 WrongKeyOrCorrupt(스펙 §5.3·§6.8).
        assert!(matches!(m.doc(id), Err(MetaError::WrongKeyOrCorrupt)));
        m.conn
            .execute("UPDATE content SET hash = x'0102' WHERE id = ?1", [id])
            .unwrap();
        assert!(matches!(m.content(id), Err(MetaError::WrongKeyOrCorrupt)));
        assert!(
            matches!(m.content_by_hash(1), Ok(None)),
            "행을 못 찾은 것은 손상이 아니다"
        );
    }
}
