//! 평가 세트 → 실제 경로: 복사본 이름·수정일, 그리고 eval --set 이 순위 덤프를 남기는지(모델 없이 — 키워드만).

use app::evalset::{id_of, materialize, queries, sha256_file};
use query::calendar::{day_of_unix_ns, parse_ymd};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

fn set_named(root: &Path, name: &str) -> std::path::PathBuf {
    let set = root.join(name);
    std::fs::create_dir_all(set.join("docs")).unwrap();
    let fx = Path::new(env!("CARGO_MANIFEST_DIR")).join("../extract/tests/fixtures/office");
    std::fs::copy(fx.join("sample.docx"), set.join("docs/A0001.docx")).unwrap();
    std::fs::copy(fx.join("utf8.txt"), set.join("docs/A0002.txt")).unwrap();
    let sha = |f: &str| sha256_file(&set.join("docs").join(f)).unwrap();
    std::fs::write(
        set.join("manifest.toml"),
        format!(
            "[[doc]]\nid = \"A0001\"\ntitle = \"운영 계획: 서울/부산\"\nsha256 = \"{}\"\next = \"docx\"\ndate = \"2026-09-01\"\n\n[[doc]]\nid = \"A0002\"\ntitle = \"메모\"\nsha256 = \"{}\"\next = \"txt\"\ndate = \"2025-05-01\"\n",
            sha("A0001.docx"),
            sha("A0002.txt")
        ),
    )
    .unwrap();
    std::fs::write(
        set.join("queries.tsv"),
        "qid\tquery\ttype\trelevant\nZ001\t렛츠런파크\tkeyword\tA0001,A0002\nZ002\t작년 렛츠런파크\tdate\tA0002\t2025-01-01..2025-12-31\nZ003\t정답 없음\tkeyword\t , \nZ004\t연말\tdate\tA0002, \t2025-11-01..2025-12-31\trecurring\n",
    )
    .unwrap();
    set
}

fn set_dir(root: &Path) -> std::path::PathBuf {
    set_named(root, "mini")
}

fn mtime_ns(f: &Path) -> i64 {
    std::fs::metadata(f).unwrap().modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_nanos() as i64
}

#[test]
fn materialize_names_and_dates_files() {
    let d = tempfile::tempdir().unwrap();
    let set = set_dir(d.path());
    let out = d.path().join("out");
    assert_eq!(materialize(std::slice::from_ref(&set), &out).unwrap(), 2);
    let f = out.join("mini").join("운영 계획 서울 부산 (A0001).docx");
    assert!(f.exists());
    assert_eq!(id_of(f.to_str().unwrap()).as_deref(), Some("A0001"));
    assert_eq!(day_of_unix_ns(mtime_ns(&f)), parse_ymd("2026-09-01").unwrap());
    std::fs::write(out.join("mini").join("남은 파일.txt"), b"x").unwrap();
    materialize(std::slice::from_ref(&set), &out).unwrap();
    assert!(!out.join("mini").join("남은 파일.txt").exists(), "매니페스트에 없는 파일은 지운다");
    // 같은 질의 규칙(m0eval 과 공용): 넷째 열까지, 빈 id 는 버리고, 정답 없는 질의는 뺀다.
    let qs = queries(&set).unwrap();
    assert_eq!(qs.iter().map(|q| q.qid.as_str()).collect::<Vec<_>>(), vec!["Z001", "Z002", "Z004"]);
    assert_eq!(qs[1].relevant.iter().collect::<Vec<_>>(), vec!["A0002"]);
    assert_eq!(qs[2].relevant.iter().collect::<Vec<_>>(), vec!["A0002"]);
    assert_eq!(qs[2].kind, "date");
}

/// P10: 복사본은 크기가 아니라 SHA-256 으로 확인한다 — 같은 크기의 다른 내용도 다시 복사하고, 내용이 맞으면 수정일만 고친다.
#[test]
fn materialize_recopies_by_sha_and_fixes_mtime() {
    let d = tempfile::tempdir().unwrap();
    let set = set_dir(d.path());
    let out = d.path().join("out");
    materialize(std::slice::from_ref(&set), &out).unwrap();
    let f = out.join("mini").join("메모 (A0002).txt");
    let good = std::fs::read(&f).unwrap();
    let want = mtime_ns(&f);
    // 같은 크기, 다른 내용
    let mut bad = good.clone();
    bad[0] ^= 0x20;
    std::fs::write(&f, &bad).unwrap();
    materialize(std::slice::from_ref(&set), &out).unwrap();
    assert_eq!(std::fs::read(&f).unwrap(), good, "SHA-256 이 다르면 다시 복사");
    assert_eq!(mtime_ns(&f), want);
    // 내용은 맞고 수정일만 틀림
    std::fs::File::options().write(true).open(&f).unwrap().set_modified(UNIX_EPOCH + Duration::from_secs(1_000_000_000)).unwrap();
    materialize(std::slice::from_ref(&set), &out).unwrap();
    assert_eq!(mtime_ns(&f), want, "수정일을 다시 맞춘다");
    // 다시 복사해야 하는데 원본이 매니페스트와 다르면 오류(원본 확인은 tools/eval/check_sets.py 의 몫 — 복사할 때만 걸린다)
    std::fs::write(set.join("docs/A0002.txt"), b"other").unwrap();
    std::fs::remove_file(&f).unwrap();
    let e = materialize(std::slice::from_ref(&set), &out).unwrap_err();
    assert!(e.contains("A0002"), "{e}");
}

/// P9: 이번에 고르지 않은 세트 폴더(지난 실행의 --set eval/data 등)는 지운다 — 색인에 섞이지 않게.
#[test]
fn materialize_removes_unrequested_set_folders() {
    let d = tempfile::tempdir().unwrap();
    let a = set_named(d.path(), "mini");
    let b = set_named(&d.path().join("other"), "extra");
    let out = d.path().join("out");
    assert_eq!(materialize(&[a.clone(), b.clone()], &out).unwrap(), 4);
    assert!(out.join("extra").is_dir());
    std::fs::write(out.join("떠도는 파일.txt"), b"x").unwrap();
    assert_eq!(materialize(std::slice::from_ref(&a), &out).unwrap(), 2);
    let left: Vec<String> = std::fs::read_dir(&out).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    assert_eq!(left, vec!["mini"], "요청한 세트 폴더만 남는다");
    // 폴더 이름이 같은 두 세트는 복사본이 섞이므로 받지 않는다.
    let e = materialize(&[a.clone(), d.path().join("other").join("mini")], &out);
    assert!(e.is_err());
}

#[test]
fn eval_set_cli_writes_report_and_dump() {
    let d = tempfile::tempdir().unwrap();
    let set = set_dir(d.path());
    let home = d.path().join("home");
    let dump = d.path().join("ranks.json");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_kra-search"))
        .arg("--home")
        .arg(&home)
        .arg("--model")
        .arg(d.path().join("모델없음.gguf"))
        .args(["eval", "--now", "2026-10-15", "--keyword-only", "--set"])
        .arg(&set)
        .arg("--dump")
        .arg(&dump)
        .output()
        .unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("nDCG@10"), "{text}");
    let ranks: std::collections::BTreeMap<String, Vec<String>> = serde_json::from_str(&std::fs::read_to_string(&dump).unwrap()).unwrap();
    let mut z1 = ranks["Z001"].clone();
    z1.sort();
    assert_eq!(z1, vec!["A0001", "A0002"], "복사본 파일 이름의 (id) 로 문서를 알아본다: {ranks:?}");
    assert!(ranks["Z002"].contains(&"A0002".to_string()), "{ranks:?}");
    assert!(!ranks.contains_key("Z003"), "정답 없는 질의는 빠진다");
}

/// 모델이 없으면(시험용 --keyword-only 없이) 색인 전에 오류로 끝난다. --home 없이도 오류.
#[test]
fn eval_set_requires_model_and_home() {
    let d = tempfile::tempdir().unwrap();
    let set = set_dir(d.path());
    let home = d.path().join("home");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_kra-search"))
        .arg("--home")
        .arg(&home)
        .arg("--model")
        .arg(d.path().join("모델없음.gguf"))
        .args(["eval", "--set"])
        .arg(&set)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("모델"));
    assert!(!home.join("eval-docs").exists(), "모델이 없으면 복사·색인하지 않는다");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_kra-search")).args(["eval", "--keyword-only", "--set"]).arg(&set).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--home"));
}

/// 모델 파일은 있지만 불러오지 못하면(망가진 GGUF) 색인은 키워드만 만든다 — eval 은 키워드 숫자를 조용히 내지 않고 실패한다.
#[test]
fn eval_set_fails_when_semantic_index_is_missing() {
    let d = tempfile::tempdir().unwrap();
    let set = set_dir(d.path());
    let home = d.path().join("home");
    let bad = d.path().join("망가진.gguf");
    std::fs::write(&bad, b"not a gguf model at all").unwrap();
    let dump = d.path().join("ranks.json");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_kra-search"))
        .arg("--home")
        .arg(&home)
        .arg("--model")
        .arg(&bad)
        .args(["eval", "--now", "2026-10-15", "--set"])
        .arg(&set)
        .arg("--dump")
        .arg(&dump)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{err}");
    assert!(err.contains("평가에는 의미 검색이 있어야"), "{err}");
    assert!(!dump.exists(), "키워드만으로 만든 순위는 남기지 않는다");
}

/// --home 이 기본 색인 위치(내 색인)를 가리키면 거절한다.
#[test]
fn eval_set_refuses_default_home() {
    let d = tempfile::tempdir().unwrap();
    let set = set_dir(d.path());
    let user = d.path().join("사용자");
    std::fs::create_dir_all(&user).unwrap();
    let default_home = if cfg!(windows) { user.join("KraSearch") } else { user.join(".kra-search") };
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_kra-search"))
        .env("HOME", &user)
        .env("USERPROFILE", &user)
        .env("LOCALAPPDATA", &user)
        .arg("--home")
        .arg(default_home.join("."))
        .args(["eval", "--keyword-only", "--set"])
        .arg(&set)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{err}");
    assert!(err.contains("--home"), "{err}");
    assert!(!default_home.join("eval-docs").exists(), "복사하지 않는다");
}
