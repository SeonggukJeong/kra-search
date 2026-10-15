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
    let fx = root().join("crates/extract/tests/fixtures/office");
    let s = run(&["index", "--root", fx.to_str().unwrap()]);
    assert!(s.contains("의미 검색: 빠른"), "{s}");
    assert!(!s.contains("llama_model_loader"), "llama.cpp 로그가 화면에 나오면 안 된다(KI-003):\n{s}");
    // 문서 낱말과 겹치지 않는 일상 표현 — 의미 경로로 찾는다
    let r = run(&["search", "--now", "2026-10-15", "말", "경주", "시설", "운영", "방안"]);
    assert!(r.contains("뜻이 비슷함"), "{r}");
}
