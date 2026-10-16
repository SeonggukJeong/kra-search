//! 실제 EmbeddingGemma 2 로 색인 → 검색(명령줄 그대로). `bash tools/fetch-models.sh` 뒤 --include-ignored.

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
#[ignore = "needs model file"]
fn real_model_index_and_semantic_search() {
    let home = tempfile::tempdir().unwrap();
    let model = root().join("models/embeddinggemma-2-Q8_0.gguf");
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_kra-search"))
            .arg("--home").arg(home.path())
            .arg("--model").arg(&model)
            .args(args)
            .output()
            .unwrap();
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success(), "{args:?}\n{text}");
        text
    };
    // KI-003: llama.cpp·ggml 로그는 화면에 나오지 않는다 — 특히 ggml 의 "load_backend: loaded CPU backend from <경로>" 는
    // 실행 파일 폴더(사용자 이름이 든 경로일 수 있음)를 그대로 찍는다(최종 리뷰 m-9).
    let exe_dir = Path::new(env!("CARGO_BIN_EXE_kra-search")).parent().unwrap().to_str().unwrap().to_string();
    let no_engine_log = |t: &str| {
        assert!(!t.contains("llama_model_loader"), "llama.cpp 로그가 화면에 나오면 안 된다(KI-003):\n{t}");
        assert!(!t.contains("load_backend"), "ggml 백엔드 로그가 화면에 나오면 안 된다(KI-003):\n{t}");
        assert!(!t.contains(&exe_dir), "실행 파일 폴더 경로가 화면에 나오면 안 된다:\n{t}");
    };
    let fx = root().join("crates/extract/tests/fixtures/office");
    let s = run(&["index", "--root", fx.to_str().unwrap()]);
    assert!(s.contains("의미 검색: 빠른"), "{s}");
    no_engine_log(&s);
    // 문서 낱말과 겹치지 않는 일상 표현 — 의미 경로로 찾는다
    let r = run(&["search", "--now", "2026-10-15", "말", "경주", "시설", "운영", "방안"]);
    assert!(r.contains("뜻이 비슷함"), "{r}");
    no_engine_log(&r);
}
