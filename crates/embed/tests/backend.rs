//! 계산 모듈 DLL(ggml-cpu-*.dll)이 없을 때 원인을 알 수 있는 오류를 내야 한다.
//! (macOS/Linux 는 정적 링크라 해당 없음 — Windows CI 에서만 검사)

#[cfg(windows)]
#[test]
fn missing_cpu_backend_dlls_is_a_backend_error() {
    let empty = std::env::temp_dir().join("kra-empty-backend-dir");
    std::fs::create_dir_all(&empty).unwrap();
    let e = embed::engine::init(Some(&empty)).err().expect("expected an error");
    assert!(matches!(e, embed::engine::EmbedError::Backend(_)), "{e:?}");
}

#[cfg(not(windows))]
#[test]
fn static_build_always_has_cpu_backend() {
    embed::engine::init(Some(std::path::Path::new("/nonexistent"))).unwrap();
}
