use std::process::Command;

#[test]
fn report_never_contains_paths() {
    let dir = std::env::temp_dir().join("kra diag 홍길동 주민 900101");
    let sub = dir.join("인사").join("채용_응시원서_홍길동.hwp");
    std::fs::create_dir_all(sub.parent().unwrap()).unwrap();
    std::fs::write(&sub, b"NOT-A-HWP-FILE").unwrap();
    let out_dir = std::env::temp_dir().join("kra-diag-out");
    std::fs::create_dir_all(&out_dir).unwrap();
    let st = Command::new(env!("CARGO_BIN_EXE_kra-diag"))
        .args(["extract-stats", "--root"])
        .arg(&dir)
        .current_dir(&out_dir)
        .status()
        .unwrap();
    assert!(st.success());
    let json = std::fs::read_to_string(out_dir.join("kra-diag-report.json")).unwrap();
    let txt = std::fs::read_to_string(out_dir.join("kra-diag-summary.txt")).unwrap();
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default();
    for needle in ["홍길동", "900101", "인사", "채용", "응시원서", "kra diag", user.as_str(), "\\\\", "/Users", "C:\\", "/tmp", "Temp"] {
        if needle.is_empty() {
            continue;
        }
        assert!(!json.contains(needle) && !txt.contains(needle), "leaked {needle:?}");
    }
    assert!(json.contains("\"not_container_magic_top\""));
}

#[test]
fn bench_records_model_error() {
    let out_dir = std::env::temp_dir().join("kra-diag-out2");
    std::fs::create_dir_all(&out_dir).unwrap();
    let st = Command::new(env!("CARGO_BIN_EXE_kra-diag"))
        .args(["bench", "--skip-flat", "--models"])
        .arg(out_dir.join("nope"))
        .current_dir(&out_dir)
        .status()
        .unwrap();
    assert!(st.success());
    let json = std::fs::read_to_string(out_dir.join("kra-diag-report.json")).unwrap();
    assert!(json.contains("모델 파일을 찾지 못했습니다"));
}

#[test]
fn flat_bench_really_searches() {
    // 측정 코드가 결과를 버리면 컴파일러가 검색을 지울 수 있다 → 결과로 체크섬을 만든다.
    let out_dir = std::env::temp_dir().join("kra-diag-out3");
    std::fs::create_dir_all(&out_dir).unwrap();
    let st = Command::new(env!("CARGO_BIN_EXE_kra-diag"))
        .args(["bench", "--flat-n", "200000", "--models"])
        .arg(out_dir.join("nope"))
        .current_dir(&out_dir)
        .status()
        .unwrap();
    assert!(st.success());
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out_dir.join("kra-diag-report.json")).unwrap()).unwrap();
    let flat = &v["bench"]["flat"];
    assert_eq!(flat["n"], 200000);
    // 결과 id 의 합: 검색이 실제로 수행되어 결과를 냈다는 증거(시간은 PC마다 다르므로 검사하지 않음)
    assert!(flat["result_checksum"].as_u64().unwrap() > 0, "{flat}");
}

#[test]
fn model_bench_runs_in_child_process() {
    let models = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../models"));
    if !models.join("embeddinggemma-2-Q4.gguf").exists() {
        eprintln!("skip: no model");
        return;
    }
    let out_dir = std::env::temp_dir().join("kra-diag-out4");
    std::fs::create_dir_all(&out_dir).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_kra-diag"))
        .args(["bench-one", "--threads", "2"])
        .arg(models.join("embeddinggemma-2-Q4.gguf"))
        .current_dir(&out_dir)
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["model_file"], "embeddinggemma-2-Q4.gguf");
    assert!(v["rss_mb_after_load"].as_u64().unwrap() > 0);
    assert!(v["tokens_per_sec"][0][1].as_f64().unwrap() > 0.0);
}

#[test]
fn report_never_contains_file_bytes() {
    // CP949 "홍길동 응시원서" 로 시작하는 텍스트를 .hwp 로 저장한 파일
    let dir = std::env::temp_dir().join("kra-diag-bytes");
    std::fs::create_dir_all(&dir).unwrap();
    let cp949_name: [u8; 6] = [0xC8, 0xAB, 0xB1, 0xE6, 0xB5, 0xBF];
    let mut body = cp949_name.to_vec();
    body.extend_from_slice(b" 2025 resume text");
    std::fs::write(dir.join("a.hwp"), &body).unwrap();
    let out_dir = std::env::temp_dir().join("kra-diag-out5");
    std::fs::create_dir_all(&out_dir).unwrap();
    let st = Command::new(env!("CARGO_BIN_EXE_kra-diag")).args(["extract-stats", "--root"]).arg(&dir).current_dir(&out_dir).status().unwrap();
    assert!(st.success());
    let json = std::fs::read_to_string(out_dir.join("kra-diag-report.json")).unwrap().to_lowercase();
    for needle in ["c8ab", "b1e6", "b5bf", "abb1", "e6b5"] {
        assert!(!json.contains(needle), "file bytes leaked as hex: {needle}");
    }
    assert!(json.contains("텍스트"), "text-like file should be labeled, not hex-dumped");
}
