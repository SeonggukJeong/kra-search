//! kra-search 종단 시험: 임시 홈·임시 문서 폴더에서 index → search.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../extract/tests/fixtures")
}

fn pdfium() -> PathBuf {
    let plat = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "mac-arm64/lib",
        ("macos", "x86_64") => "mac-x64/lib",
        ("windows", "x86_64") => "win-x64/bin",
        _ => "linux-x64/lib",
    };
    // canonicalize 는 Windows 에서 `\\?\` 경로를 만든다 — absolute 를 쓴다(DEC-040).
    let p = std::path::absolute(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/pdfium").join(plat).join(extract::pdf::LIB_NAME)).unwrap();
    assert!(p.exists(), "bash tools/fetch-pdfium.sh 먼저: {}", p.display());
    p
}

struct Env {
    home: tempfile::TempDir,
    docs: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        let docs = tempfile::tempdir().unwrap();
        for (src, dst) in [
            ("office/sample.docx", "운영계획.docx"),
            ("office/sample.docx", "운영계획 - 복사본.docx"),
            ("schedule.hwp", "추진일정.hwp"),
            ("pdf/korean.pdf", "시행계획.pdf"),
            ("office/cp949.csv", "예산.csv"),
        ] {
            std::fs::copy(fixtures().join(src), docs.path().join(dst)).unwrap();
        }
        std::fs::write(docs.path().join("깨진문서.hwp"), b"garbage bytes").unwrap();
        Env { home: tempfile::tempdir().unwrap(), docs }
    }

    fn exec(&self, args: &[&str]) -> (Output, String) {
        self.exec_with_pdfium(&pdfium(), args)
    }

    fn exec_with_pdfium(&self, pdfium: &Path, args: &[&str]) -> (Output, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_kra-search"))
            .arg("--home").arg(self.home.path())
            .arg("--pdfium").arg(pdfium)
            // 개발 PC 의 target/release/models/ 에 실제 모델이 있어도 늘 키워드만 쓰게(실모델은 따로 본다).
            .arg("--model").arg(self.home.path().join("모델없음.gguf"))
            .args(args)
            .output()
            .unwrap();
        // 안내 문장(복구 등)은 stderr 로 나온다 — 시험은 둘 다 본다.
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        (out, text)
    }

    fn run(&self, args: &[&str]) -> String {
        let (out, text) = self.exec(args);
        assert!(out.status.success(), "{args:?} 실패\n{text}");
        text
    }

    fn run_fail(&self, args: &[&str]) -> String {
        let (out, text) = self.exec(args);
        assert!(!out.status.success(), "{args:?} 는 실패해야 한다\n{text}");
        text
    }

    fn index(&self) -> String {
        self.run(&["index", "--root", self.docs.path().to_str().unwrap()])
    }
}

#[test]
fn index_and_search_all_formats() {
    let e = Env::new();
    let s = e.index();
    assert!(s.contains("파일 6개"), "{s}");
    // 요약도 UI D7 용어(추출 실패 → "열 수 없는 파일")
    assert!(s.contains("열 수 없는 파일 1개(다시 시도 포함)") && !s.contains("못 읽음"), "{s}");
    let r = e.run(&["search", "렛츠런파크"]);
    assert!(r.contains("운영계획") && r.contains("시행계획.pdf"), "{r}");
    assert!(r.contains("같은 내용 1개 더"), "복사본은 결과 한 줄로 묶인다:\n{r}");
    // 따옴표 없이 낱말 여러 개, '-' 로 시작하는 검색어도 받는다.
    let r = e.run(&["search", "경마", "시행"]);
    assert!(r.contains("예산.csv"), "{r}");
    let r = e.run(&["search", "--limit", "5", "-렛츠런파크"]);
    assert!(r.contains("운영계획"), "{r}");
    let r = e.run(&["search", "깨진문서"]);
    assert!(r.contains("깨진문서.hwp") && r.contains("열 수 없는 파일"), "실패한 파일도 이름으로 찾힌다(UI D7 용어):\n{r}");
    let st = e.run(&["status"]);
    assert!(st.contains("파일 6") && st.contains("내용 5"), "{st}");
    assert!(!st.contains("키워드 색인"), "화면 문구는 UI D7 용어(키워드 색인 → 검색):\n{st}");
}

/// 같은 내용 경로가 많아도 한 결과에 3개까지만 보이고 나머지는 개수로(명령줄이 경로로 넘치지 않게).
#[test]
fn duplicate_paths_are_capped_in_search_output() {
    let e = Env::new();
    for i in 1..=4 {
        std::fs::copy(fixtures().join("office/sample.docx"), e.docs.path().join(format!("사본{i}.docx"))).unwrap();
    }
    e.index();
    let r = e.run(&["search", "렛츠런파크"]);
    let line = r.lines().find(|l| l.contains("같은 내용")).unwrap_or_else(|| panic!("{r}"));
    assert!(line.contains("같은 내용 5개 더:") && line.ends_with("외 2개"), "{line}");
    assert_eq!(line.matches(".docx").count(), 3, "{line}");
}

/// pdfium 을 못 불러오면 PDF 를 기록하지 않고 건너뛴다 — "열 수 없는 파일"로 굳지 않고, 도구가 돌아오면 다음 index 에서 읽힌다.
#[test]
fn pdf_is_skipped_without_pdfium_and_read_later() {
    let e = Env::new();
    let missing = e.home.path().join("없는폴더").join(extract::pdf::LIB_NAME);
    let root = e.docs.path().to_str().unwrap().to_string();
    let (out, s) = e.exec_with_pdfium(&missing, &["index", "--root", &root]);
    assert!(out.status.success(), "{s}");
    assert!(s.contains("PDF 는 이번에 건너뜁니다") && s.contains("PDF 건너뜀 1개"), "{s}");
    assert!(!s.contains(missing.to_str().unwrap()), "안내에 경로를 넣지 않는다:\n{s}");
    let st = e.run(&["status"]);
    assert!(st.contains("파일 5 "), "PDF 는 기록하지 않는다:\n{st}");
    let r = e.run(&["search", "시행계획"]);
    assert!(!r.contains("시행계획.pdf"), "{r}");
    let s = e.index();
    assert!(!s.contains("건너뜁니다") && s.contains("바뀐 파일 1개"), "{s}");
    let r = e.run(&["search", "렛츠런파크"]);
    assert!(r.contains("시행계획.pdf"), "{r}");
}

#[test]
fn reindex_without_changes_does_nothing() {
    let e = Env::new();
    e.index();
    let s = e.index();
    assert!(s.contains("바뀐 파일 0개"), "{s}");
}

#[test]
fn duplicates_rename_and_delete() {
    let e = Env::new();
    e.index();
    let d = e.docs.path();
    std::fs::rename(d.join("운영계획.docx"), d.join("2026 사업계획.docx")).unwrap();
    e.index();
    let r = e.run(&["search", "렛츠런파크"]);
    // 같은 내용의 경로는 모두 출력된다. 옛 이름("…/운영계획.docx")은 없어야 한다("운영계획 - 복사본.docx" 는 남음).
    assert!(r.contains("2026 사업계획.docx") && !r.contains("운영계획.docx"), "{r}");
    let r = e.run(&["search", "사업계획"]);
    assert!(r.contains("2026 사업계획.docx"), "새 이름으로 찾힌다:\n{r}");
    std::fs::remove_file(d.join("2026 사업계획.docx")).unwrap();
    std::fs::remove_file(d.join("운영계획 - 복사본.docx")).unwrap();
    e.index();
    let r = e.run(&["search", "글상자"]);
    assert!(!r.contains(".docx"), "마지막 경로가 지워지면 결과에서 사라진다:\n{r}");
}

#[test]
fn oversized_file_is_filename_only() {
    let e = Env::new();
    let f = std::fs::File::create(e.docs.path().join("대용량보고서.pdf")).unwrap();
    f.set_len(extract::format::MAX_FILE_BYTES + 1).unwrap();
    e.index();
    let r = e.run(&["search", "대용량보고서"]);
    assert!(r.contains("대용량보고서.pdf") && r.contains("100MB 넘는 파일"), "{r}");
}

#[test]
fn key_loss_rebuilds_index() {
    let e = Env::new();
    e.index();
    std::fs::write(e.home.path().join("keys/meta.key"), b"broken").unwrap();
    let s = e.index();
    assert!(s.contains("다시 만듭니다"), "{s}");
    let r = e.run(&["search", "렛츠런파크"]);
    assert!(r.contains("운영계획"), "{r}");
}

#[test]
fn keyword_index_loss_is_rebuilt() {
    let e = Env::new();
    e.index();
    std::fs::remove_dir_all(e.home.path().join("data/tantivy")).unwrap();
    // 검색은 복구하지 않고 안내만 한다(읽기 전용 — 색인 중인 다른 프로세스의 파일을 건드리지 않음).
    let r = e.run_fail(&["search", "렛츠런파크"]);
    assert!(r.contains("index"), "{r}");
    e.index();
    let r = e.run(&["search", "렛츠런파크"]);
    assert!(r.contains("운영계획"), "{r}");
}

#[test]
fn corrupt_meta_pages_are_rebuilt() {
    let e = Env::new();
    e.index();
    // 첫 쪽(4096바이트) 뒤를 모두 망가뜨린다. 손상이 열 때 드러나든 색인 도중 드러나든 index 가 다시 만들어야 한다.
    let db = e.home.path().join("data/meta.db");
    for ext in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(e.home.path().join(format!("data/meta.db{ext}")));
    }
    let mut bytes = std::fs::read(&db).unwrap();
    assert!(bytes.len() > 4096 * 2, "시험 전제: meta.db 가 두 쪽 이상");
    for b in &mut bytes[4096..] {
        *b ^= 0x5A;
    }
    std::fs::write(&db, &bytes).unwrap();
    // 검색은 고치지 않고 쉬운 말로 index 를 안내한다(내부 오류 이름 없이).
    let r = e.run_fail(&["search", "렛츠런파크"]);
    assert!(r.contains("kra-search index") && !r.contains("DB"), "{r}");
    let s = e.index();
    assert!(s.contains("다시 만듭니다"), "{s}");
    let r = e.run(&["search", "렛츠런파크"]);
    assert!(r.contains("운영계획"), "{r}");
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for ent in std::fs::read_dir(from).unwrap() {
        let ent = ent.unwrap();
        let dst = to.join(ent.file_name());
        if ent.file_type().unwrap().is_dir() {
            copy_dir(&ent.path(), &dst);
        } else {
            std::fs::copy(ent.path(), dst).unwrap();
        }
    }
}

/// meta.db 의 어느 쪽이 깨져도 index 가 다시 만든다 — 손상이 열 때 드러나든, 루트 목록을 바꾸다가 드러나든, 색인 도중 드러나든.
/// 쪽마다 따로: 처음 색인한 홈을 되살리고 그 쪽 하나만 망가뜨린다(쪽 배치는 SQLite 가 정하므로 모든 쪽을 돈다).
#[test]
fn any_corrupt_meta_page_is_rebuilt() {
    let e = Env::new();
    e.index();
    for ext in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(e.home.path().join(format!("data/meta.db{ext}")));
    }
    let pristine = tempfile::tempdir().unwrap();
    copy_dir(e.home.path(), pristine.path());
    let db = e.home.path().join("data/meta.db");
    let pages = std::fs::metadata(&db).unwrap().len() as usize / 4096;
    let mut mid_run = 0;
    for page in 0..pages {
        std::fs::remove_dir_all(e.home.path()).unwrap();
        copy_dir(pristine.path(), e.home.path());
        let mut bytes = std::fs::read(&db).unwrap();
        for b in &mut bytes[page * 4096..(page + 1) * 4096] {
            *b ^= 0x5A;
        }
        std::fs::write(&db, &bytes).unwrap();
        let s = e.index();
        mid_run += s.contains("색인 데이터가 손상되어") as usize;
        let r = e.run(&["search", "렛츠런파크"]);
        assert!(r.contains("운영계획"), "{page}쪽 손상 뒤\n{s}\n{r}");
    }
    assert!(mid_run > 0, "열 때가 아니라 도중에 드러나는 손상도 시험되어야 한다({pages}쪽)");
}

/// 손상된 행(읽을 수 없는 doc JSON): index 가 매번 같은 오류로 실패하지 않고 한 번 다시 만든다(스펙 §5.3·§6.8).
#[test]
fn damaged_doc_row_is_rebuilt() {
    let e = Env::new();
    e.index();
    let home = platform::paths::Home::new(e.home.path());
    let key = platform::keys::load_or_create(&home.meta_key()).unwrap();
    {
        let c = rusqlite::Connection::open(home.meta_db()).unwrap();
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        c.pragma_update(None, "key", format!("x'{hex}'")).unwrap();
        assert!(c.execute("UPDATE content SET doc = '{깨짐' WHERE doc IS NOT NULL", []).unwrap() > 0);
    }
    // 이름을 바꾸면 그 내용이 Pending 이 되어 flush 가 doc 을 읽는다 — 거기서 손상이 드러난다.
    let d = e.docs.path();
    std::fs::rename(d.join("운영계획.docx"), d.join("새이름.docx")).unwrap();
    let s = e.index();
    assert!(s.contains("다시 만듭니다"), "{s}");
    let r = e.run(&["search", "렛츠런파크"]);
    assert!(r.contains("새이름.docx"), "{r}");
}

#[test]
fn search_before_index_says_run_index() {
    let e = Env::new();
    let r = e.run_fail(&["search", "렛츠런파크"]);
    assert!(r.contains("kra-search index"), "{r}");
    assert!(!e.home.path().join("data").exists(), "검색은 아무것도 만들지 않는다");
}

#[test]
fn second_index_is_refused_while_locked() {
    let e = Env::new();
    let _held = app::lock_index(&platform::paths::Home::new(e.home.path())).unwrap();
    let r = e.run_fail(&["index", "--root", e.docs.path().to_str().unwrap()]);
    assert!(r.contains("이미 색인 중"), "{r}");
}

/// 다른 프로그램이 나눔 없이 열어 둔 파일: 색인이 멈추지 않고 그 파일만 건너뛰며, 놓인 뒤 다음 index 에서 읽힌다.
#[cfg(windows)]
#[test]
fn locked_file_is_skipped_then_indexed_later() {
    use std::os::windows::fs::OpenOptionsExt;
    let e = Env::new();
    let p = e.docs.path().join("잠긴문서.txt");
    std::fs::write(&p, "잠금시험표지 본문").unwrap();
    let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&p).unwrap();
    let s = e.index();
    assert!(s.contains("접근 못 함 1개"), "{s}");
    drop(held);
    e.index();
    let r = e.run(&["search", "잠금시험표지"]);
    assert!(r.contains("잠긴문서.txt"), "{r}");
}

#[test]
fn date_chip_is_shown_and_now_is_validated() {
    let e = Env::new();
    e.index();
    let r = e.run(&["search", "--now", "2026-10-15", "작년", "렛츠런파크"]);
    assert!(r.contains("📅 작년 → 2025-01-01 ~ 2025-12-31"), "{r}");
    let r = e.run_fail(&["search", "--now", "2026-13-01", "렛츠런파크"]);
    assert!(r.contains("YYYY-MM-DD"), "{r}");
}

#[test]
fn zero_threads_is_rejected() {
    let e = Env::new();
    let r = e.run_fail(&["--threads", "0", "status"]);
    assert!(r.contains("--threads"), "{r}");
}
