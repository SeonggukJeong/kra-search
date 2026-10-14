//! llama.cpp 로컬 패치가 저장소에 있고 vendor 소스에 실제로 적용됐는지 확인한다.
//! llama.cpp 를 올릴 때 패치를 빠뜨리거나 지우면 여기서 떨어진다.
//! 절차: docs/engineering/runbooks/upgrade-llama-cpp.md · 배경: docs/engineering/incidents.md INC-004
use std::path::Path;

/// (패치 파일, 바뀌는 소스, 적용 후 소스에 있어야 하는 문자열)
const REQUIRED: &[(&str, &str, &str)] = &[(
    "0001-skip-logits-buffer-for-pooled-embeddings.patch",
    "src/llama-context.cpp",
    "bool has_logits     = !(cparams.embeddings && cparams.pooling_type != LLAMA_POOLING_TYPE_NONE);",
)];

#[test]
fn local_patches_are_present_and_applied() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (patch, file, marker) in REQUIRED {
        assert!(root.join("patches").join(patch).is_file(), "패치 파일이 없다: patches/{patch}");
        let src = std::fs::read_to_string(root.join("vendor/llama.cpp").join(file)).unwrap();
        assert!(src.contains(marker), "{patch} 가 vendor/llama.cpp/{file} 에 적용돼 있지 않다");
    }
}
