//! 의미 검색 벡터 파일 관리(DEC-047·048): 임베딩 지문 폴더, 세대 파일, meta 기준 정합성, 압축, 검색용 읽기.
//! 쓰는 쪽(index)은 파일에 덧붙이고 fsync 한 뒤 meta 에 길이·슬롯을 기록한다. 읽는 쪽(search)은 meta 가 아는 만큼만 읽는다.

use crate::pipeline::IndexError;
use crate::store::user_error;
use embed::profile::ModelProfile;
use index::hybrid::VectorSet;
use index::meta::{Meta, VecKind};
use platform::paths::Home;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use vector::record::Layout;
use vector::store::{read_each, VecFile};

pub const LAYOUT: Layout = Layout::M0;
/// 죽은 슬롯이 이 비율을 넘으면 실행 끝에 압축한다.
const DEAD_RATIO: f64 = 0.3;
const KINDS: [VecKind; 2] = [VecKind::Cards, VecKind::Chunks];

pub fn file_path(dir: &Path, k: VecKind, gen: u64) -> PathBuf {
    dir.join(format!("{}.{gen}.vec", k.name()))
}

/// 임베딩 지문(DEC-047): 모델 파일 + 모델 프로필 + 카드·청크·순서 설정(`plan::tag`) + 레코드 형식.
pub fn fingerprint(model_hash: u128, profile: &ModelProfile) -> String {
    fingerprint_with(model_hash, profile, &embed::plan::tag())
}

/// `fingerprint` 의 본체 — plan 설정(tag)을 받아 구성 요소마다 지문이 바뀌는지 시험할 수 있게 한다.
fn fingerprint_with(model_hash: u128, profile: &ModelProfile, plan_tag: &str) -> String {
    let s = format!(
        "model={model_hash:032x}\nprofile={}\nplan={plan_tag}\nlayout={}x{}\nrecord=1",
        serde_json::to_string(profile).expect("프로필 직렬화"),
        LAYOUT.dim,
        LAYOUT.bit_dim
    );
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(s.as_bytes()))
}

/// 모델 파일 해시(xxh3-128). kv 에 둔 "크기:수정시각:해시"의 앞부분이 같으면 다시 읽지 않는다(모델 300MB).
pub fn model_hash(cached: Option<&str>, path: &Path) -> std::io::Result<(u128, String)> {
    let m = std::fs::metadata(path)?;
    let mtime = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    let key = format!("{}:{}", m.len(), mtime);
    if let Some((k, h)) = cached.and_then(|c| c.rsplit_once(':')) {
        if k == key {
            if let Ok(v) = u128::from_str_radix(h, 16) {
                return Ok((v, format!("{key}:{v:032x}")));
            }
        }
    }
    let h = crawler::hash_file(path)?;
    Ok((h, format!("{key}:{h:032x}")))
}

fn io_err(e: impl std::fmt::Display) -> IndexError {
    IndexError::Other(format!("의미 검색 색인 파일 오류: {e}"))
}

fn kv_u64(meta: &Meta, key: &str) -> Result<u64, IndexError> {
    Ok(meta.kv(key)?.and_then(|s| s.parse().ok()).unwrap_or(0))
}

pub struct VecStore {
    pub dir: PathBuf,
    pub cards: VecFile,
    pub chunks: VecFile,
}

impl VecStore {
    pub fn file(&mut self, k: VecKind) -> &mut VecFile {
        match k {
            VecKind::Cards => &mut self.cards,
            VecKind::Chunks => &mut self.chunks,
        }
    }
}

/// 다른 지문 폴더와, 지금 폴더의 옛 세대 파일을 지운다. 못 지우면(Windows 에서 검색 중인 다른 프로세스) 다음 실행에서.
fn remove_stale(root: &Path, dir: &Path, gens: &[(VecKind, u64)]) {
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            if e.path() != dir {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let keep = gens.iter().any(|(k, g)| e.path() == file_path(dir, *k, *g));
            if !keep {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// 색인용으로 연다. 지문이 다르거나 파일이 짧거나 형식이 다르면 벡터만 비우고 새로 만든다(키워드·meta 내용은 그대로).
pub fn open_for_write(home: &Home, meta: &mut Meta, fp: &str) -> Result<VecStore, IndexError> {
    let root = home.vectors_dir();
    let dir = root.join(fp);
    let prev = meta.kv("vec_fp")?;
    if prev.as_deref() == Some(fp) {
        let mut gens = Vec::new();
        let mut files = Vec::new();
        for k in KINDS {
            let (g, len) = (kv_u64(meta, &format!("{}_gen", k.name()))?, kv_u64(meta, &format!("{}_len", k.name()))?);
            gens.push((k, g));
            files.push(VecFile::open(&file_path(&dir, k, g), LAYOUT, len));
        }
        let chunks = files.pop().expect("2개");
        let cards = files.pop().expect("2개");
        match (cards, chunks) {
            (Ok(cards), Ok(chunks)) => {
                remove_stale(&root, &dir, &gens);
                return Ok(VecStore { dir, cards, chunks });
            }
            _ => eprintln!("의미 검색 색인이 손상되어 다시 만듭니다."),
        }
    } else if prev.is_some() {
        eprintln!("검색 모델이나 설정이 바뀌어 의미 검색 색인을 다시 만듭니다.");
    }
    meta.clear_vectors()?;
    std::fs::create_dir_all(&dir).map_err(io_err)?;
    let cards = VecFile::create(&file_path(&dir, VecKind::Cards, 0), LAYOUT).map_err(io_err)?;
    let chunks = VecFile::create(&file_path(&dir, VecKind::Chunks, 0), LAYOUT).map_err(io_err)?;
    meta.set_kv("vec_fp", fp)?;
    remove_stale(&root, &dir, &[(VecKind::Cards, 0), (VecKind::Chunks, 0)]);
    Ok(VecStore { dir, cards, chunks })
}

/// 죽은 슬롯이 많으면 살아 있는 레코드만 새 세대 파일로 옮긴다(슬롯 번호를 다시 매김).
pub fn compact_if_needed(meta: &mut Meta, store: &mut VecStore) -> Result<(), IndexError> {
    for k in KINDS {
        let live = meta.live_slots(k)?;
        let len = store.file(k).len();
        if len == 0 || (len as usize - live.len()) as f64 / len as f64 <= DEAD_RATIO {
            continue;
        }
        let gen = kv_u64(meta, &format!("{}_gen", k.name()))?;
        let (old, new) = (file_path(&store.dir, k, gen), file_path(&store.dir, k, gen + 1));
        let mut out = VecFile::create(&new, LAYOUT).map_err(io_err)?;
        let owners: HashMap<u64, i64> = live.into_iter().collect();
        let mut map: HashMap<u64, u64> = HashMap::new();
        let mut batch = Vec::new();
        read_each(&old, LAYOUT, len, |slot, rec| {
            if owners.contains_key(&slot) {
                let next = map.len() as u64;
                map.insert(slot, next);
                batch.push(rec);
                if batch.len() == 4096 {
                    out.append(&batch)?;
                    batch.clear();
                }
            }
            Ok(())
        })
        .map_err(io_err)?;
        out.append(&batch).map_err(io_err)?;
        meta.remap_slots(k, &map, gen + 1, out.len())?;
        *store.file(k) = out;
        // 옛 세대 파일은 여기서 지우지 않는다 — 다른 창의 검색이 압축 전 스냅샷(옛 세대)을 읽는 중일 수 있다(Review Focus 5).
        // 다음 실행의 open_for_write 가 remove_stale 로 지운다(스펙 §3.2·DEC-048).
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetsState {
    Ready,
    /// 의미 검색 색인이 아직 없음
    Empty,
    /// 지금 모델·설정의 지문과 색인 지문이 다름 — index 를 다시 해야 함
    ModelChanged,
    /// 파일을 읽지 못함(유실·손상)
    Unreadable,
}

pub struct Sets {
    pub cards: VectorSet,
    pub chunks: VectorSet,
    pub state: SetsState,
}

/// 검색용 읽기. `fp_now` 가 있으면 색인 지문과 비교한다(다르면 섞지 않고 빈 집합 — Review Focus 3).
pub fn load_sets(home: &Home, meta: &Meta, fp_now: Option<&str>) -> Result<Sets, String> {
    let empty = |state| Sets { cards: VectorSet::new(LAYOUT), chunks: VectorSet::new(LAYOUT), state };
    let Some(fp) = meta.kv("vec_fp").map_err(user_error)? else { return Ok(empty(SetsState::Empty)) };
    if fp_now.is_some_and(|f| f != fp) {
        return Ok(empty(SetsState::ModelChanged));
    }
    let dir = home.vectors_dir().join(&fp);
    let mut sets = empty(SetsState::Ready);
    for k in KINDS {
        let (gen, len, live) = meta.vector_snapshot(k).map_err(user_error)?;
        let owners: HashMap<u64, i64> = live.into_iter().collect();
        let set = match k {
            VecKind::Cards => &mut sets.cards,
            VecKind::Chunks => &mut sets.chunks,
        };
        let r = read_each(&file_path(&dir, k, gen), LAYOUT, len, |slot, rec| {
            if let Some(o) = owners.get(&slot) {
                set.push(*o, &rec);
            }
            Ok(())
        });
        if r.is_err() {
            return Ok(empty(SetsState::Unreadable));
        }
    }
    if sets.cards.is_empty() && sets.chunks.is_empty() {
        sets.state = SetsState::Empty;
    }
    Ok(sets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use index::meta::{ContentData, Meta};
    use vector::record::Record;

    fn setup() -> (tempfile::TempDir, Home, Meta, i64, i64) {
        let d = tempfile::tempdir().unwrap();
        let home = Home::new(d.path());
        let mut m = Meta::open(&home.meta_db(), &[1u8; 32]).unwrap();
        let mut ids = Vec::new();
        for (h, p) in [(1u128, "/d/a.hwp"), (2, "/d/b.hwp")] {
            let doc = doc::Document { title: Some(p.into()), blocks: vec![] };
            let id = m.insert_content(h, &ContentData { fail: None, title: None, distribution: false, doc: Some(doc) }).unwrap();
            m.upsert_file(p, 1, 1, id).unwrap();
            m.finish_flush(&[id], &[], 1).unwrap();
            ids.push(id);
        }
        (d, home, m, ids[0], ids[1])
    }

    fn rec(x: f32) -> Record {
        let mut v = vec![0.01f32; 768];
        v[0] = x;
        Record::from_full(&v, LAYOUT).unwrap()
    }

    #[test]
    fn fingerprint_changes_with_model_or_profile() {
        let p = ModelProfile::embeddinggemma2();
        let a = fingerprint(1, &p);
        assert_eq!(a.len(), 32);
        assert_eq!(a, fingerprint(1, &p));
        assert_ne!(a, fingerprint(2, &p));
        let mut q = p.clone();
        q.query_template = "query: {text}".into();
        assert_ne!(a, fingerprint(1, &q));
    }

    /// DEC-047: 카드 상한·청크 설정(plan::tag) 이 바뀌어도 지문이 바뀐다.
    #[test]
    fn fingerprint_changes_with_plan_settings() {
        let p = ModelProfile::embeddinggemma2();
        let tag = embed::plan::tag();
        assert_eq!(fingerprint(1, &p), fingerprint_with(1, &p, &tag), "공개 함수는 실제 tag 를 쓴다");
        let base = fingerprint_with(1, &p, &tag);

        // 상수에서 만든다 — 평가(Task 16)가 CARD_TOKENS 를 바꿔도 시험이 깨지지 않게.
        let card = format!("card={}", embed::plan::CARD_TOKENS);
        let card_changed = tag.replacen(&card, &format!("card={}", embed::plan::CARD_TOKENS + 1), 1);
        assert_ne!(card_changed, tag, "카드 상한 부분이 실제로 바뀌었다");
        assert_ne!(base, fingerprint_with(1, &p, &card_changed));

        let b = embed::chunk::BODY_CHUNK;
        let chunk = format!("chunk={}/{}", b.target_tokens, b.max_tokens);
        let chunk_changed = tag.replacen(&chunk, &format!("chunk={}/{}", b.target_tokens - 50, b.max_tokens), 1);
        assert_ne!(chunk_changed, tag, "청크 설정 부분이 실제로 바뀌었다");
        assert_ne!(base, fingerprint_with(1, &p, &chunk_changed));
    }

    #[test]
    fn model_hash_uses_cache_when_size_and_time_match() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("m.gguf");
        std::fs::write(&f, b"model bytes").unwrap();
        let (h, cache) = model_hash(None, &f).unwrap();
        let (h2, cache2) = model_hash(Some(&cache), &f).unwrap();
        assert_eq!((h, &cache), (h2, &cache2));
        let fake = format!("{}:{:032x}", cache.rsplit_once(':').unwrap().0, 7u128);
        assert_eq!(model_hash(Some(&fake), &f).unwrap().0, 7, "크기·시각이 같으면 파일을 다시 읽지 않는다");
    }

    #[test]
    fn open_new_then_reopen_and_fingerprint_change_clears() {
        let (_d, home, mut m, a, b) = setup();
        let mut vs = open_for_write(&home, &mut m, "fp1").unwrap();
        let first = vs.cards.append(&[rec(1.0), rec(2.0)]).unwrap();
        m.set_cards(&[(a, Some(first)), (b, Some(first + 1))], vs.cards.len()).unwrap();
        drop(vs);
        let vs = open_for_write(&home, &mut m, "fp1").unwrap();
        assert_eq!(vs.cards.len(), 2);
        drop(vs);
        let vs = open_for_write(&home, &mut m, "fp2").unwrap();
        assert_eq!(vs.cards.len(), 0);
        assert!(m.live_slots(VecKind::Cards).unwrap().is_empty());
        assert!(!home.vectors_dir().join("fp1").exists(), "옛 지문 폴더는 지운다");
    }

    #[test]
    fn damaged_file_rebuilds_vectors_only() {
        let (_d, home, mut m, a, _b) = setup();
        let mut vs = open_for_write(&home, &mut m, "fp").unwrap();
        vs.cards.append(&[rec(1.0)]).unwrap();
        m.set_cards(&[(a, Some(0))], 1).unwrap();
        drop(vs);
        std::fs::write(file_path(&home.vectors_dir().join("fp"), VecKind::Cards, 0), b"broken").unwrap();
        let vs = open_for_write(&home, &mut m, "fp").unwrap();
        assert_eq!(vs.cards.len(), 0);
        assert_eq!(m.card_todo().unwrap().len(), 2, "카드를 다시 만든다");
        assert_eq!(m.counts().unwrap().contents, 2, "meta 내용은 그대로");
    }

    /// Review Focus 5: index 가 파일에 덧붙였지만 meta 에 아직 기록하지 않은 꼬리는 검색이 읽지 않는다.
    #[test]
    fn reader_ignores_unrecorded_tail() {
        let (_d, home, mut m, a, b) = setup();
        let mut vs = open_for_write(&home, &mut m, "fp").unwrap();
        vs.cards.append(&[rec(1.0)]).unwrap();
        m.set_cards(&[(a, Some(0))], 1).unwrap();
        vs.cards.append(&[rec(2.0)]).unwrap(); // b 몫 — meta 기록 전
        let s = load_sets(&home, &m, Some("fp")).unwrap();
        assert_eq!(s.state, SetsState::Ready);
        assert_eq!(s.cards.owners, vec![a]);
        let _ = b;
    }

    #[test]
    fn model_change_and_missing_files_are_reported() {
        let (_d, home, mut m, a, _b) = setup();
        assert_eq!(load_sets(&home, &m, None).unwrap().state, SetsState::Empty);
        let mut vs = open_for_write(&home, &mut m, "fp").unwrap();
        vs.cards.append(&[rec(1.0)]).unwrap();
        m.set_cards(&[(a, Some(0))], 1).unwrap();
        drop(vs);
        assert_eq!(load_sets(&home, &m, Some("other")).unwrap().state, SetsState::ModelChanged);
        std::fs::remove_file(file_path(&home.vectors_dir().join("fp"), VecKind::Cards, 0)).unwrap();
        assert_eq!(load_sets(&home, &m, Some("fp")).unwrap().state, SetsState::Unreadable);
    }

    /// 다른 프로세스(검색)가 옛 세대 파일을 연 채로 압축해도 실패하지 않고, 옛 파일은 압축 직후가 아니라 다음 실행에서 지운다
    /// (Windows 는 열린 파일을 바꿔치기할 수 없어 세대 파일을 쓴다, DEC-048 — CI 의 windows-latest 에서도 돈다).
    /// Windows 의 공유 모드 충돌은 핸들 단위라, 같은 프로세스에서 핸들을 하나 더 열어 두는 것으로 다른 프로세스와 같은 실패 방식을 재현한다.
    #[test]
    fn compaction_while_old_generation_is_open() {
        let (_d, home, mut m, a, b) = setup();
        let mut vs = open_for_write(&home, &mut m, "fp").unwrap();
        vs.cards.append(&[rec(1.0), rec(2.0), rec(3.0), rec(4.0)]).unwrap();
        m.set_cards(&[(a, Some(1)), (b, Some(3))], 4).unwrap();
        let old = file_path(&home.vectors_dir().join("fp"), VecKind::Cards, 0);
        let reader = std::fs::File::open(&old).unwrap();
        compact_if_needed(&mut m, &mut vs).unwrap();
        assert_eq!(vs.cards.len(), 2);
        assert_eq!(load_sets(&home, &m, Some("fp")).unwrap().cards.owners, vec![a, b], "새 세대로 읽는다");
        drop(reader);
        // 압축 직후에는 옛 세대가 남아 있다 — 압축 전 스냅샷(세대 0, 길이 4)을 쥔 다른 창의 검색이 아직 읽는 중일 수 있다.
        assert!(old.exists(), "옛 세대는 다음 실행에서 지운다");
        let mut n = 0;
        read_each(&old, LAYOUT, 4, |_, _| {
            n += 1;
            Ok(())
        })
        .expect("옛 스냅샷으로도 읽힌다");
        assert_eq!(n, 4);
        drop(vs);
        let _vs = open_for_write(&home, &mut m, "fp").unwrap();
        assert!(!old.exists(), "다음 실행에서 옛 세대를 지운다");
    }

    #[test]
    fn compaction_keeps_live_records_and_owners() {
        let (_d, home, mut m, a, b) = setup();
        let mut vs = open_for_write(&home, &mut m, "fp").unwrap();
        vs.cards.append(&[rec(1.0), rec(2.0), rec(3.0), rec(4.0)]).unwrap();
        m.set_cards(&[(a, Some(1)), (b, Some(3))], 4).unwrap(); // 0·2 는 죽은 슬롯(50%)
        compact_if_needed(&mut m, &mut vs).unwrap();
        assert_eq!(vs.cards.len(), 2);
        let mut live = m.live_slots(VecKind::Cards).unwrap();
        live.sort();
        assert_eq!(live, vec![(0, a), (1, b)]);
        let s = load_sets(&home, &m, Some("fp")).unwrap();
        assert_eq!(s.cards.owners, vec![a, b]);
        assert_eq!(s.cards.index.search_exact(&{ let mut v = vec![0.01f32; 768]; v[0] = 4.0; v }, 1)[0].0, 1, "b 의 레코드(4.0)가 슬롯 1 로 옮겨짐");
        drop(vs);
        let vs = open_for_write(&home, &mut m, "fp").unwrap();
        assert_eq!(vs.cards.len(), 2, "새 세대로 다시 열린다");
        assert!(!file_path(&home.vectors_dir().join("fp"), VecKind::Cards, 0).exists(), "옛 세대는 지운다");
    }
}