//! 평가 세트 공용(DEC-051·052): 평가 문서 복사본의 파일 이름, 복사(materialize), 질의 읽기, 조정용/검증용 분할, 지표.
//! m0eval(실험)과 `kra-search eval`(제품)이 같은 함수를 쓴다 — 파일 이름이 다르면 제목 없는 문서의 카드 머리와
//! 키워드 파일명 필드가 달라져 일치 검사가 깨진다. 질의 규칙이 다르면 비교하는 질의 집합이 달라진다.

use query::calendar::{noon_kst_unix_secs, parse_ymd};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

/// `"{제목} ({id}).{확장자}"` — Windows 금지 문자·제어 문자는 공백, 공백은 하나로, 제목은 80자까지.
pub fn file_name(id: &str, title: &str, ext: &str) -> String {
    let t: String = title.chars().map(|c| if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { ' ' } else { c }).collect();
    let t: String = t.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect();
    format!("{} ({id}).{ext}", t.trim_end_matches(['.', ' ']))
}

/// 복사본 파일 이름(또는 경로)에서 평가 문서 id 를 꺼낸다.
pub fn id_of(file_name: &str) -> Option<String> {
    let stem = Path::new(file_name).file_stem()?.to_str()?;
    let open = stem.rfind(" (")?;
    let id = stem[open + 2..].strip_suffix(')')?;
    (!id.is_empty()).then(|| id.to_string())
}

/// FNV-1a 64 — 러스트 버전이 바뀌어도 같은 값.
fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// 조정용 qid 집합: 유형마다 qid 해시 순서로 앞 60%(올림). 나머지는 검증용(DEC-051).
pub fn tune_split<'a>(items: impl IntoIterator<Item = (&'a str, &'a str)>) -> HashSet<String> {
    let mut by: HashMap<&str, Vec<&str>> = HashMap::new();
    for (q, k) in items {
        by.entry(k).or_default().push(q);
    }
    let mut out = HashSet::new();
    for (_, mut v) in by {
        v.sort_by_key(|q| (fnv(q), *q));
        let n = (v.len() * 6).div_ceil(10);
        out.extend(v.into_iter().take(n).map(String::from));
    }
    out
}

pub fn ndcg_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    let dcg: f64 = ranked.iter().take(k).enumerate().filter(|(_, d)| relevant.contains(*d)).map(|(i, _)| 1.0 / ((i + 2) as f64).log2()).sum();
    let idcg: f64 = (0..relevant.len().min(k)).map(|i| 1.0 / ((i + 2) as f64).log2()).sum();
    if idcg == 0.0 {
        0.0
    } else {
        dcg / idcg
    }
}

pub fn recall_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    if relevant.is_empty() {
        return 0.0;
    }
    ranked.iter().take(k).filter(|d| relevant.contains(*d)).count() as f64 / relevant.len() as f64
}

/// 첫 정답 문서의 순위(1부터).
pub fn first_relevant_rank(ranked: &[String], relevant: &HashSet<String>) -> Option<usize> {
    ranked.iter().position(|d| relevant.contains(d)).map(|i| i + 1)
}

/// 첫 페이지(화면 한 쪽)에 보이는 결과 수.
pub const FIRST_PAGE: usize = 10;

/// 첫 페이지 적중: 정답 문서가 하나라도 상위 10 안에 있다(사용자 원칙 2026-10-15: "항상 최상위가 아니어도 첫 페이지에 정확한 결과를").
pub fn first_page_hit(ranked: &[String], relevant: &HashSet<String>) -> bool {
    first_relevant_rank(ranked, relevant).is_some_and(|r| r <= FIRST_PAGE)
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
    sha256: String,
    date: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EvalQuery {
    pub qid: String,
    pub text: String,
    pub kind: String,
    pub relevant: HashSet<String>,
}

/// 파일의 SHA-256(소문자 16진수) — 매니페스트 `sha256` 과 같은 꼴.
pub fn sha256_file(p: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn mtime_of(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

/// 평가 세트 문서를 `{out}/{세트 폴더 이름}/{file_name}` 으로 복사하고 수정일을 매니페스트 날짜 정오(한국 시각)로 둔다(DEC-052).
/// 복사본의 SHA-256 이 매니페스트와 같으면 다시 복사하지 않고(크기만으로는 같은 크기의 다른 내용을 놓친다, P10), 수정일이
/// 틀렸을 때만 고친다. 다시 복사한 뒤에도 매니페스트와 다르면 원본이 바뀐 것이라 오류 — m0eval 은 매니페스트 판을 쓴다고 보기 때문.
/// 매니페스트에 없는 파일과 이번에 고르지 않은 세트 폴더(지난 실행의 `--set eval/data` 등)는 지운다 — 색인에 섞이지 않게(P9).
/// date 가 없거나 폴더 이름이 같은 세트가 둘이면 오류.
pub fn materialize(sets: &[PathBuf], out: &Path) -> Result<usize, String> {
    let mut names = HashSet::new();
    for set in sets {
        let name = set.file_name().and_then(|x| x.to_str()).ok_or_else(|| format!("세트 폴더 이름을 읽지 못함: {}", set.display()))?;
        if !names.insert(name.to_string()) {
            return Err(format!("폴더 이름이 같은 평가 세트가 둘입니다: {name}"));
        }
    }
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    for e in std::fs::read_dir(out).map_err(|e| format!("{}: {e}", out.display()))?.flatten() {
        if e.file_name().to_str().is_some_and(|f| names.contains(f)) {
            continue;
        }
        let p = e.path();
        let r = if e.file_type().is_ok_and(|t| t.is_dir()) { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        r.map_err(|e| format!("{}: {e}", p.display()))?;
    }
    let mut n = 0;
    for set in sets {
        let name = set.file_name().and_then(|x| x.to_str()).expect("위에서 확인");
        let dir = out.join(name);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let text = std::fs::read_to_string(set.join("manifest.toml")).map_err(|e| format!("{}: {e}", set.display()))?;
        let m: Manifest = toml::from_str(&text).map_err(|e| format!("{}/manifest.toml: {e}", set.display()))?;
        let mut keep = HashSet::new();
        for e in m.doc {
            let date = e.date.as_deref().and_then(parse_ymd).ok_or_else(|| format!("{} 에 date 가 없습니다(python3 tools/eval/check_sets.py 로 확인)", e.id))?;
            let src = set.join("docs").join(format!("{}.{}", e.id, e.ext));
            if !src.is_file() {
                return Err(format!("평가 문서가 없습니다: {} (tools/eval/fetch_all.py · import_local.py · make_data_set.py 먼저)", src.display()));
            }
            let fname = file_name(&e.id, &e.title, &e.ext);
            let dst = dir.join(&fname);
            let want = e.sha256.to_ascii_lowercase();
            if sha256_file(&dst).ok().as_deref() != Some(want.as_str()) {
                std::fs::copy(&src, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
                if sha256_file(&dst).map_err(|e| format!("{}: {e}", dst.display()))? != want {
                    return Err(format!("{} 원본이 매니페스트 sha256 과 다릅니다(python3 tools/eval/check_sets.py 로 확인)", e.id));
                }
            }
            let t = UNIX_EPOCH + Duration::from_secs(noon_kst_unix_secs(date) as u64);
            if mtime_of(&dst) != Some(t) {
                std::fs::File::options().write(true).open(&dst).and_then(|f| f.set_modified(t)).map_err(|e| format!("{}: {e}", dst.display()))?;
            }
            keep.insert(fname);
            n += 1;
        }
        for e in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
            if !e.file_name().to_str().is_some_and(|f| keep.contains(f)) {
                let p = e.path();
                let r = if e.file_type().is_ok_and(|t| t.is_dir()) { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
                r.map_err(|e| format!("{}: {e}", p.display()))?;
            }
        }
    }
    Ok(n)
}

/// queries.tsv 읽기 — m0eval(실험)과 `kra-search eval --set`(제품)이 이 함수 하나를 쓴다(일치 검사가 같은 질의 집합을 비교하게).
/// qid · 검색어 · 유형 · 정답(쉼표) 네 열만 쓴다. 다섯째(date 질의의 기간)·여섯째(recurring) 열은 사람이 읽는 표시라 점수에 쓰지 않는다.
/// 정답의 빈 id 는 버리고, 정답이 하나도 없는 질의는 모든 지표에서 빼며 표준 오류에 경고를 남긴다(정답 없는 질의는 nDCG 가 늘 0 이라
/// 평균을 깎기만 한다). 열이 셋보다 적은 줄은 오류.
pub fn queries(set: &Path) -> Result<Vec<EvalQuery>, String> {
    let text = std::fs::read_to_string(set.join("queries.tsv")).map_err(|e| format!("{}/queries.tsv: {e}", set.display()))?;
    let mut out = Vec::new();
    for (i, l) in text.lines().enumerate().skip(1) {
        if l.trim().is_empty() {
            continue;
        }
        let c: Vec<&str> = l.split('\t').collect();
        if c.len() < 3 {
            return Err(format!("{}/queries.tsv {}째 줄: 열이 모자랍니다", set.display(), i + 1));
        }
        let q = EvalQuery {
            qid: c[0].trim().into(),
            text: c[1].into(),
            kind: c[2].trim().into(),
            relevant: c.get(3).copied().unwrap_or("").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        };
        if q.relevant.is_empty() {
            eprintln!("경고: 정답이 없는 질의 {} 는 평가에서 뺍니다({})", q.qid, set.display());
            continue;
        }
        out.push(q);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_safe_and_carry_the_id() {
        assert_eq!(file_name("P0001", "근로복지공단, 장애학생 방학 중  교육·돌봄", "hwpx"), "근로복지공단, 장애학생 방학 중 교육·돌봄 (P0001).hwpx");
        assert_eq!(file_name("K0001", "「규정」 a/b:c*?\"<>|", "hwp"), "「규정」 a b c (K0001).hwp");
        assert_eq!(id_of("근로복지공단 (P0001).hwpx").as_deref(), Some("P0001"));
        assert_eq!(id_of("/x/y/2025년 계획(안) (K0011).hwp").as_deref(), Some("K0011"));
        assert_eq!(id_of("그냥 파일.hwp"), None);
        let long = "가".repeat(200);
        assert_eq!(file_name("P1", &long, "hwp").chars().count(), 80 + " (P1).hwp".chars().count());
    }

    #[test]
    fn tune_split_is_stable_and_stratified() {
        let items: Vec<(String, String)> = (0..10).map(|i| (format!("Q{i:03}"), "a".to_string())).chain((0..5).map(|i| (format!("R{i:03}"), "b".to_string()))).collect();
        let s1 = tune_split(items.iter().map(|(q, k)| (q.as_str(), k.as_str())));
        let s2 = tune_split(items.iter().rev().map(|(q, k)| (q.as_str(), k.as_str())));
        assert_eq!(s1, s2, "입력 순서와 무관");
        assert_eq!(s1.iter().filter(|q| q.starts_with('Q')).count(), 6);
        assert_eq!(s1.iter().filter(|q| q.starts_with('R')).count(), 3, "5 × 0.6 = 3");
    }

    /// 넷째 열까지만 쓰고, 빈 id 는 버리고, 정답 없는 질의는 뺀다(m0eval 과 같은 규칙 — 이 함수를 같이 쓴다).
    #[test]
    fn queries_use_four_columns_and_skip_unanswered() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("queries.tsv"),
            "qid\tquery\ttype\trelevant\nT001\t작년 계획\tdate\tK0014, \t2025-01-01..2025-12-31\nT003\t연말 계획\tdate\tK0009,K0010\t2025-11-01..2025-12-31\trecurring\nZ001\t정답 없음\tkeyword\t , \nZ002\t열 없음\tkeyword\n\nQ001\t돌봄\tsemantic\tP0001\n",
        )
        .unwrap();
        let q = queries(d.path()).unwrap();
        let ids: Vec<&str> = q.iter().map(|x| x.qid.as_str()).collect();
        assert_eq!(ids, vec!["T001", "T003", "Q001"]);
        assert_eq!(q[0].relevant, HashSet::from(["K0014".to_string()]));
        assert_eq!(q[1].relevant.len(), 2);
        assert_eq!(q[1].kind, "date");
        std::fs::write(d.path().join("queries.tsv"), "qid\tquery\ttype\trelevant\nX\t둘뿐\n").unwrap();
        assert!(queries(d.path()).is_err());
    }

    #[test]
    fn metrics() {
        let r: Vec<String> = ["a", "b", "c"].iter().map(|x| x.to_string()).collect();
        let rel: HashSet<String> = ["b".to_string()].into_iter().collect();
        assert!((ndcg_at_k(&r, &rel, 10) - 1.0 / 3f64.log2()).abs() < 1e-9);
        assert_eq!(first_relevant_rank(&r, &rel), Some(2));
        assert_eq!(recall_at_k(&r, &rel, 1), 0.0);
        assert!(first_page_hit(&r, &rel));
        let far: Vec<String> = (0..10).map(|i| format!("x{i}")).chain(["b".to_string()]).collect();
        assert_eq!(first_relevant_rank(&far, &rel), Some(11));
        assert!(!first_page_hit(&far, &rel), "11위는 첫 페이지 밖");
        assert!(first_page_hit(&far[1..], &rel), "10위는 첫 페이지");
    }
}
