---
paths:
  - "crates/llama-sys/**"
  - ".gitmodules"
  - "crates/embed/src/engine.rs"
---
# llama.cpp 영역 규칙
- 버전을 바꾸면 `docs/engineering/runbooks/upgrade-llama-cpp.md` 를 처음부터 끝까지 따른다.
- 로컬 패치 `crates/llama-sys/patches/0001-skip-logits-buffer-for-pooled-embeddings.patch` 를 빠뜨리거나 지우지 않는다(INC-004: 없으면 임베딩 중 메모리 +1GB). 확인: `cargo test -p llama-sys --release --test patches`, `cargo test -p diag --release --test memory -- --include-ignored`.
- `build.rs` 의 Windows 설정(동적 CRT `/MD`, 최적화 플래그 직접 지정, CMakeCache 최적화 검사)은 사고(INC-005, INC-006)의 재발 방지다 — 지우거나 "단순화"하지 않는다.
- 출력 차원은 `llama_model_n_embd_out`(INC-002), 호출은 `llama_encode` 우선(INC-007).
