# llama.cpp 올리기

`crates/llama-sys/vendor/llama.cpp`(git 서브모듈)를 새 버전으로 바꿀 때의 절차. **하나라도 건너뛰지 않는다** — 각 단계는 실제로 겪은 사고(INC)에서 나왔다.

## 현재 버전
| 항목 | 값 |
|---|---|
| 태그/커밋 | b11574 (`8bb3cf0d2`, describe `v0.6.0-145-g8bb3cf0d2`) |
| 로컬 패치 | `0001-skip-logits-buffer-for-pooled-embeddings` ([patches/README.md](../../../crates/llama-sys/patches/README.md)) |
| 기준 속도 | GitHub windows 러너(2코어) Q4 약 530~580 토큰/초(2스레드), Q8 약 400 — m0-diag-4 스모크 |
| 기준 메모리 | Q8 대량 임베딩 최대 ≈ 로드 직후 + 300MB 미만(패치 없으면 +1GB) |

## 절차
1. **배경 읽기**: [incidents.md](../incidents.md) INC-001, 002, 004, 005, 006, 007, 013.
2. **버전 바꾸기**
   ```bash
   git -C crates/llama-sys/vendor/llama.cpp checkout -- .      # 빌드가 적용한 패치를 되돌림(KI-007)
   git -C crates/llama-sys/vendor/llama.cpp fetch --tags
   git -C crates/llama-sys/vendor/llama.cpp checkout <새 태그>
   ```
3. **패치 다시 맞추기**(INC-004) — `cargo build -p llama-sys --release`
   - "failed to apply … upgrade-llama-cpp.md" 로 빌드가 멈추면: `src/llama-context.cpp` 의 `output_reserve`(또는 logits 버퍼를 잡는 새 위치)를 찾아 **풀링 임베딩 컨텍스트(`cparams.embeddings && pooling_type != NONE`)에서는 logits 버퍼를 잡지 않도록** 같은 의미로 고치고 `git -C … diff > crates/llama-sys/patches/0001-….patch` 로 다시 만든다.
   - `crates/llama-sys/tests/patches.rs` 의 확인 문자열(marker)도 새 코드에 맞게 고친다.
   - upstream 이 같은 문제를 이미 고쳤다면: 패치 없이 아래 6번 메모리 시험이 통과하는지 확인한 뒤에만 패치와 marker 를 지우고 patches/README.md 에 기록한다.
4. **빌드 옵션 확인**(INC-001, 005, 006) — `crates/llama-sys/build.rs`
   - `LLAMA_BUILD_*`, `GGML_*` 옵션 이름이 바뀌었는지 upstream `CMakeLists.txt` 와 대조.
   - Windows: `BUILD_SHARED_LIBS`, `GGML_BACKEND_DL`, `GGML_CPU_ALL_VARIANTS`, `CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL`(동적 CRT), 최적화 플래그 직접 지정 유지. 최적화 검사 assert 를 지우지 않는다.
   - 링크할 라이브러리 이름(`llama`, `ggml`, `ggml-base`, `ggml-cpu`)이 바뀌었는지.
5. **API 변경 확인**(INC-002, 007, 013) — `crates/embed/src/engine.rs` 가 쓰는 것:
   `llama_model_load_from_file`, `llama_model_default_params`(`load_mode`, `vocab_only`), `llama_init_from_model`, `llama_context_default_params`(`embeddings`, `pooling_type = MEAN`), `llama_model_n_embd_out`, `llama_encode`/`llama_decode`, `llama_get_embeddings_seq`, `llama_get_memory`/`llama_memory_clear`, `llama_tokenize`, `llama_batch_*`, `llama_model_has_encoder/decoder`, `ggml_backend_load_all_from_path`, `ggml_backend_dev_count`, `llama_print_system_info`.
   bindgen 허용 목록(`build.rs`)에 새로 필요한 함수가 있으면 추가.
6. **시험**(전부 통과해야 함)
   ```bash
   cargo test -p llama-sys --release --test patches
   bash tools/fetch-models.sh
   cargo test -p embed --release --test model --test reference -- --include-ignored --test-threads 1   # 차원 768, 원본 대비 코사인 ≥ 0.99
   cargo test -p diag --release --test memory -- --include-ignored --test-threads 1                    # logits 패치 효과
   cargo test --workspace --release
   ```
7. **Windows 확인**: 브랜치를 푸시해 CI(windows 실모델 시험) 통과 확인. 속도는 `perf-probe` 워크플로(수동 실행)나 진단 릴리즈 스모크 수치를 위 기준 속도와 비교 — 크게 느려졌으면 최적화 플래그부터 의심(INC-006).
8. **기록**: 이 문서의 "현재 버전" 표, patches/README.md, 문제가 있었다면 incidents.md 에 새 INC.
