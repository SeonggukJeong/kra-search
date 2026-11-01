//! 폴더 훑기: 대상 확장자 파일의 경로·크기·수정 시각을 모으고, 지난번과 비교해 바뀐 것만 고른다.
//! 폴더 감시(USN·ReadDirectoryChangesW)는 M5·M6 — M1 은 `kra-search index` 실행 때마다 훑는다.

use extract::format::{Format, MAX_FILE_BYTES};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq)]
pub struct Scanned {
    pub path: String,
    pub size: u64,
    pub mtime_ns: i64,
}

/// 스펙 §6.3 기본 제외(이름, 대소문자 무시) + 개발 폴더.
pub const DEFAULT_EXCLUDED_DIRS: &[&str] = &[
    "windows", "program files", "program files (x86)", "appdata", "$recycle.bin",
    "system volume information", ".git", "node_modules",
];

pub struct ScanOptions {
    pub excluded_dirs: Vec<String>,
    /// 이 경로 아래 폴더는 훑지 않는다. 단 **루트의 진짜 하위**일 때만 — 루트가 이 안에 있으면 사용자가 고른 것이므로 훑는다.
    pub excluded_prefixes: Vec<PathBuf>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        let excluded_prefixes = ["TEMP", "TMP"].iter().filter_map(std::env::var_os).map(PathBuf::from).collect();
        ScanOptions { excluded_dirs: DEFAULT_EXCLUDED_DIRS.iter().map(|s| s.to_string()).collect(), excluded_prefixes }
    }
}

#[derive(Debug, Default)]
pub struct ScanReport {
    pub files: Vec<Scanned>,
    pub missing_roots: Vec<PathBuf>,
    /// 읽지 못한 폴더·파일과 클라우드 전용 파일의 경로. 잠깐의 오류(권한·네트워크·USB)로
    /// 그 아래 파일을 "지운 것"으로 보면 추출 결과를 버리고 다음에 전부 다시 읽게 된다 — 그래서 남긴다.
    pub errored: Vec<PathBuf>,
    pub skipped_non_utf8: usize,
    pub unreadable: usize,
    pub cloud_only: usize,
}

impl ScanReport {
    /// diff 에서 "안 보였어도 지우지 않을" 경로 목록.
    pub fn keep_under(&self) -> Vec<PathBuf> {
        self.missing_roots.iter().chain(&self.errored).cloned().collect()
    }
}

/// 대상 파일인가: 지원 확장자이고 Office 임시 파일(`~$…`)이 아님.
fn is_target(p: &Path) -> bool {
    let office_temp = p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("~$"));
    !office_temp && Format::from_path(p).is_some()
}

fn mtime_ns(m: &std::fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos().min(i64::MAX as u128) as i64).unwrap_or(0)
}

/// OneDrive "온라인 전용" 같은 클라우드 자리표시자인가. 이런 파일을 읽으면 OS 가 내려받는다(네트워크·디스크 변화).
/// walkdir 의 메타데이터는 폴더 목록에서 오므로(파일을 열지 않음) 확인 자체는 내려받기를 일으키지 않는다.
#[cfg(windows)]
fn cloud_only(m: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
    const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x40000;
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x400000;
    m.file_attributes() & (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS) != 0
}

#[cfg(not(windows))]
fn cloud_only(_: &std::fs::Metadata) -> bool {
    false
}

pub fn scan(roots: &[PathBuf], opt: &ScanOptions) -> ScanReport {
    let mut r = ScanReport::default();
    let excluded: HashSet<String> = opt.excluded_dirs.iter().map(|s| s.to_lowercase()).collect();
    for root in roots {
        if !root.is_dir() {
            r.missing_roots.push(root.clone());
            continue;
        }
        // 제외 접두어(%TEMP% 등)는 루트의 진짜 하위일 때만 쓴다 — 사용자가 그 안을 루트로 고르면 그대로 훑는다.
        let prefixes: Vec<&PathBuf> = opt.excluded_prefixes.iter().filter(|p| p.starts_with(root) && p.as_path() != root.as_path()).collect();
        // 클라우드 자리표시자 폴더(일부만 내려받은 OneDrive 폴더 등)는 들어가지 않는다 — 열거도 내려받기를 일으킬 수 있다(kra-diag 와 같은 규칙).
        // 그 아래 아는 파일을 지운 것으로 보지 않도록 경로를 errored 에 남긴다. 클라우드가 아닌 OS 에서는 아무 일도 하지 않는다.
        let mut cloud_dirs: Vec<PathBuf> = Vec::new();
        let walker = WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| {
            if e.depth() == 0 || !e.file_type().is_dir() {
                return true;
            }
            let name_ok = e.file_name().to_str().is_none_or(|n| !excluded.contains(&n.to_lowercase()));
            if !name_ok || prefixes.iter().any(|p| e.path().starts_with(p)) {
                return false;
            }
            if e.metadata().is_ok_and(|m| cloud_only(&m)) {
                cloud_dirs.push(e.path().to_path_buf());
                return false;
            }
            true
        });
        for entry in walker {
            let e = match entry {
                Ok(e) => e,
                Err(err) => {
                    // 권한 없는 폴더·사라진 파일 — 그것만 건너뛰고, 그 아래 아는 파일은 지우지 않도록 경로를 남긴다.
                    r.unreadable += 1;
                    if let Some(p) = err.path() {
                        r.errored.push(p.to_path_buf());
                    }
                    continue;
                }
            };
            if !e.file_type().is_file() || !is_target(e.path()) {
                continue;
            }
            let Some(path) = e.path().to_str() else {
                r.skipped_non_utf8 += 1;
                continue;
            };
            match e.metadata() {
                Ok(m) if cloud_only(&m) => {
                    r.cloud_only += 1;
                    r.errored.push(e.path().to_path_buf());
                }
                Ok(m) => r.files.push(Scanned { path: path.to_string(), size: m.len(), mtime_ns: mtime_ns(&m) }),
                Err(_) => {
                    r.unreadable += 1;
                    r.errored.push(e.path().to_path_buf());
                }
            }
        }
        r.cloud_only += cloud_dirs.len();
        r.errored.extend(cloud_dirs);
    }
    r
}

pub struct Diff {
    pub changed: Vec<Scanned>,
    pub removed: Vec<String>,
}

pub fn diff(scanned: &[Scanned], known: &HashMap<String, (u64, i64)>, keep_under: &[PathBuf]) -> Diff {
    let mut changed: Vec<Scanned> = scanned
        .iter()
        .filter(|s| known.get(&s.path) != Some(&(s.size, s.mtime_ns)))
        .cloned()
        .collect();
    // 최근 수정 먼저, 같으면 경로 문자열 오름차순(바이트 순). 처리 순서대로 내용 id 가 매겨지고 검색 코어는 점수가 같으면
    // id 로 가르므로, 같은 시각 파일의 순서가 훑은 순서(파일 시스템마다 다름)에 따라 바뀌지 않게 한다(P2, 일치 검사 DEC-052).
    changed.sort_by(|a, b| b.mtime_ns.cmp(&a.mtime_ns).then_with(|| a.path.cmp(&b.path)));
    let seen: HashSet<&str> = scanned.iter().map(|s| s.path.as_str()).collect();
    let mut removed: Vec<String> = known
        .keys()
        .filter(|p| !seen.contains(p.as_str()))
        .filter(|p| !keep_under.iter().any(|r| Path::new(p.as_str()).starts_with(r)))
        .cloned()
        .collect();
    removed.sort();
    Diff { changed, removed }
}

pub fn hash_file(path: &Path) -> std::io::Result<u128> {
    let mut f = std::fs::File::open(path)?;
    let mut h = xxhash_rust::xxh3::Xxh3Default::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.digest128())
}

pub fn content_key(f: &Scanned) -> std::io::Result<u128> {
    if f.size > MAX_FILE_BYTES {
        // 100MB 초과는 내용을 읽지 않는다(스펙 §6.5) — 수 GB 파일을 바뀔 때마다 끝까지 읽지 않도록 메타데이터로 키를 만든다.
        // 같은 내용의 큰 복사본끼리 묶이지 않지만 어차피 파일명만 색인한다.
        return Ok(xxhash_rust::xxh3::xxh3_128(format!("big\0{}\0{}\0{}", f.size, f.mtime_ns, f.path).as_bytes()));
    }
    hash_file(Path::new(&f.path))
}

pub fn stat(path: &Path) -> Option<(u64, i64)> {
    std::fs::metadata(path).ok().map(|m| (m.len(), mtime_ns(&m)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    fn names(r: &ScanReport) -> Vec<String> {
        let mut v: Vec<String> = r.files.iter().map(|s| Path::new(&s.path).file_name().unwrap().to_str().unwrap().to_string()).collect();
        v.sort();
        v
    }

    /// 시험은 실행 환경의 %TEMP% 와 떼어 놓는다(tempdir 가 바로 그 아래에 생긴다).
    fn opts() -> ScanOptions {
        ScanOptions { excluded_prefixes: vec![], ..ScanOptions::default() }
    }

    #[test]
    fn scans_targets_and_skips_excluded() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        touch(&root.join("보고서.hwp"), b"a");
        touch(&root.join("하위/표.XLSX"), b"b");
        touch(&root.join("하위/~$임시.docx"), b"c");
        touch(&root.join("하위/메모.doc"), b"d");
        touch(&root.join("AppData/설정.txt"), b"e");
        touch(&root.join("node_modules/x.txt"), b"f");
        touch(&root.join("임시폴더/y.txt"), b"g");
        let opt = ScanOptions { excluded_prefixes: vec![root.join("임시폴더")], ..opts() };
        let r = scan(&[root.to_path_buf()], &opt);
        assert_eq!(names(&r), vec!["보고서.hwp", "표.XLSX"]);
    }

    #[test]
    fn excluded_prefix_applies_only_below_root() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        touch(&root.join("하위/a.txt"), b"a");
        touch(&root.join("임시/b.txt"), b"b");
        // 접두어가 루트 아래면 그 폴더만 뺀다.
        let below = ScanOptions { excluded_prefixes: vec![root.join("임시")], ..opts() };
        assert_eq!(names(&scan(&[root.to_path_buf()], &below)), vec!["a.txt"]);
        // 루트가 접두어와 같거나 그 안에 있으면(사용자가 %TEMP% 아래를 골랐으면) 빼지 않는다.
        let around = ScanOptions { excluded_prefixes: vec![root.to_path_buf(), root.parent().unwrap().to_path_buf()], ..opts() };
        assert_eq!(names(&scan(&[root.to_path_buf()], &around)), vec!["a.txt", "b.txt"]);
    }

    /// Windows 의 MAX_PATH(260)는 UTF-16 단위로 센다 — `OsStr::len()` 은 WTF-8 바이트라 한글이 3배로 잡힌다.
    fn path_units(p: &Path) -> usize {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            p.as_os_str().encode_wide().count()
        }
        #[cfg(not(windows))]
        {
            p.as_os_str().len()
        }
    }

    #[test]
    fn long_paths_are_scanned() {
        let d = tempfile::tempdir().unwrap();
        let mut p = d.path().to_path_buf();
        while path_units(&p) < 300 {
            p.push("아주긴폴더이름입니다");
        }
        let file = p.join("깊은 문서.txt");
        assert!(path_units(&file) > 260, "시험 경로가 MAX_PATH 를 넘어야 한다: {}", path_units(&file));
        touch(&file, b"x");
        let r = scan(&[d.path().to_path_buf()], &opts());
        assert_eq!(names(&r), vec!["깊은 문서.txt"]);
    }

    #[cfg(unix)]
    #[test]
    fn skips_unreadable_and_non_utf8_without_aborting() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("ok.txt"), b"x");
        let locked = d.path().join("잠긴폴더");
        touch(&locked.join("숨은.txt"), b"y");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = scan(&[d.path().to_path_buf()], &opts());
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(names(&r), vec!["ok.txt"]);
        assert!(r.unreadable >= 1);
    }

    #[cfg(windows)]
    #[test]
    fn skips_unreadable_and_non_utf8_without_aborting() {
        use std::os::windows::ffi::OsStringExt;
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("ok.txt"), b"x");
        // 짝 없는 서로게이트(0xD800) — UTF-8 로 바꿀 수 없는 Windows 파일 이름
        let bad: Vec<u16> = "깨진".encode_utf16().chain([0xD800]).chain(".txt".encode_utf16()).collect();
        std::fs::write(d.path().join(std::ffi::OsString::from_wide(&bad)), b"y").unwrap();
        let r = scan(&[d.path().to_path_buf()], &opts());
        assert_eq!(names(&r), vec!["ok.txt"]);
        assert_eq!(r.skipped_non_utf8, 1);
    }

    /// 잠깐 못 읽은 폴더 아래 파일을 "지운 것"으로 보면 추출 결과를 버리고 다음에 전부 다시 읽는다.
    #[cfg(unix)]
    #[test]
    fn unreadable_folder_is_not_treated_as_deleted() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("ok.txt"), b"x");
        let locked = d.path().join("잠깐못읽음");
        touch(&locked.join("안.txt"), b"y");
        let first = scan(&[d.path().to_path_buf()], &opts());
        assert_eq!(first.files.len(), 2);
        let known: HashMap<String, (u64, i64)> = first.files.iter().map(|s| (s.path.clone(), (s.size, s.mtime_ns))).collect();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = scan(&[d.path().to_path_buf()], &opts());
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(r.errored.iter().any(|p| p == &locked), "{:?}", r.errored);
        let df = diff(&r.files, &known, &r.keep_under());
        assert!(df.removed.is_empty(), "{:?}", df.removed);
    }

    /// 클라우드 자리표시자 폴더(OFFLINE 속성)는 들어가지 않고, 그 아래 아는 파일을 지운 것으로 보지 않는다.
    /// `attrib +O` 는 속성만 바꾼다(내려받기·네트워크 없음).
    #[cfg(windows)]
    #[test]
    fn cloud_placeholder_folder_is_skipped_not_deleted() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("ok.txt"), b"x");
        let cloud = d.path().join("원드라이브");
        touch(&cloud.join("온라인.txt"), b"y");
        let first = scan(&[d.path().to_path_buf()], &opts());
        assert_eq!(first.files.len(), 2);
        let known: HashMap<String, (u64, i64)> = first.files.iter().map(|s| (s.path.clone(), (s.size, s.mtime_ns))).collect();
        let _ = std::process::Command::new("attrib").arg("+O").arg(&cloud).status();
        // attrib 은 실패해도 0 을 돌려줄 수 있다 — 속성이 실제로 켜졌는지 확인한다.
        assert!(std::fs::symlink_metadata(&cloud).is_ok_and(|m| cloud_only(&m)), "시험 전제: OFFLINE 속성");
        let r = scan(&[d.path().to_path_buf()], &opts());
        let _ = std::process::Command::new("attrib").arg("-O").arg(&cloud).status();
        assert_eq!(names(&r), vec!["ok.txt"]);
        assert_eq!(r.cloud_only, 1);
        assert!(r.errored.iter().any(|p| p == &cloud), "{:?}", r.errored);
        let df = diff(&r.files, &known, &r.keep_under());
        assert!(df.removed.is_empty(), "{:?}", df.removed);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_loop_does_not_hang() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("폴더/문서.txt"), b"x");
        std::os::unix::fs::symlink(d.path().join("폴더"), d.path().join("폴더/고리")).unwrap();
        let r = scan(&[d.path().to_path_buf()], &opts());
        assert_eq!(names(&r), vec!["문서.txt"]);
    }

    #[test]
    fn diff_finds_changes_and_keeps_missing_roots() {
        let s = |p: &str, size: u64, t: i64| Scanned { path: p.into(), size, mtime_ns: t };
        let known: HashMap<String, (u64, i64)> = [
            ("/a/same.hwp".to_string(), (1, 1)),
            ("/a/edited.hwp".to_string(), (1, 1)),
            ("/a/gone.hwp".to_string(), (1, 1)),
            ("/usb/kept.hwp".to_string(), (1, 1)),
            ("/a/locked/kept2.hwp".to_string(), (1, 1)),
        ]
        .into();
        let scanned = vec![s("/a/same.hwp", 1, 1), s("/a/edited.hwp", 2, 5), s("/a/new.hwp", 1, 9)];
        let d = diff(&scanned, &known, &[PathBuf::from("/usb"), PathBuf::from("/a/locked")]);
        assert_eq!(d.changed.iter().map(|x| x.path.as_str()).collect::<Vec<_>>(), vec!["/a/new.hwp", "/a/edited.hwp"]);
        assert_eq!(d.removed, vec!["/a/gone.hwp".to_string()]);
    }

    /// 수정 시각이 같으면 경로 오름차순 — 훑은 순서(파일 시스템마다 다름)와 무관하게 같은 순서(P2).
    #[test]
    fn diff_breaks_mtime_ties_by_path() {
        let s = |p: &str, t: i64| Scanned { path: p.into(), size: 1, mtime_ns: t };
        let scanned = vec![s("/r/public/나.hwp", 5), s("/r/kra/가.hwp", 5), s("/r/public/가.hwp", 5), s("/r/data/z.csv", 9), s("/r/data/a.csv", 1)];
        let mut rev = scanned.clone();
        rev.reverse();
        for input in [scanned, rev] {
            let d = diff(&input, &HashMap::new(), &[]);
            assert_eq!(
                d.changed.iter().map(|x| x.path.as_str()).collect::<Vec<_>>(),
                vec!["/r/data/z.csv", "/r/kra/가.hwp", "/r/public/가.hwp", "/r/public/나.hwp", "/r/data/a.csv"]
            );
        }
    }

    #[test]
    fn missing_root_is_reported() {
        let r = scan(&[PathBuf::from("/없는/루트")], &opts());
        assert_eq!(r.missing_roots, vec![PathBuf::from("/없는/루트")]);
    }

    #[test]
    fn hash_matches_one_shot_and_detects_change() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.bin");
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        assert_eq!(hash_file(&p).unwrap(), xxhash_rust::xxh3::xxh3_128(&data));
        std::fs::write(&p, &data[1..]).unwrap();
        assert_ne!(hash_file(&p).unwrap(), xxhash_rust::xxh3::xxh3_128(&data));
    }

    #[test]
    fn big_file_key_uses_metadata_without_reading() {
        let d = tempfile::tempdir().unwrap();
        // 없는 경로 — 파일을 열면 실패하므로, 키가 나오면 읽지 않았다는 뜻이다.
        let big = Scanned { path: d.path().join("없는 큰 파일.pdf").to_str().unwrap().into(), size: MAX_FILE_BYTES + 1, mtime_ns: 7 };
        let k = content_key(&big).unwrap();
        assert_eq!(k, content_key(&big).unwrap(), "같은 메타데이터면 같은 키");
        assert_ne!(k, content_key(&Scanned { mtime_ns: 8, ..big.clone() }).unwrap(), "수정 시각이 바뀌면 키도 바뀐다");
        // 100MB 이하는 내용 해시
        let p = d.path().join("작은.txt");
        std::fs::write(&p, b"abc").unwrap();
        let small = Scanned { path: p.to_str().unwrap().into(), size: 3, mtime_ns: 7 };
        assert_eq!(content_key(&small).unwrap(), xxhash_rust::xxh3::xxh3_128(b"abc"));
        assert_eq!(stat(&p).map(|s| s.0), Some(3));
        assert_eq!(stat(&d.path().join("없음")), None);
    }
}
