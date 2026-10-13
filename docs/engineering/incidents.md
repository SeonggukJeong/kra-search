# 사고 기록 (발견해서 고친 문제)

같은 실수를 되풀이하지 않기 위한 기록. 각 항목은 **증상 → 원인 → 고친 방법 → 재발 방지 장치 → 관련 위치** 순서다.
새 문제를 고치면 맨 아래에 다음 번호로 추가한다(번호는 바꾸지 않는다 — 코드 주석이 번호로 가리킨다).

| ID | 한 줄 요약 | 영역 | 재발 방지 장치 |
|---|---|---|---|
| [INC-001](#inc-001) | llama.cpp 통합 앱(app/common) 빌드 실패 | llama-sys | build.rs CMake 옵션 |
| [INC-002](#inc-002) | 임베딩 차원이 768이 아니라 512로 나옴 | embed | `embed/tests/model.rs` 차원·의미 시험 |
| [INC-003](#inc-003) | rhwp 저장소 8.6GB · 부분 복제의 함정들 | extract·CI | `tools/fetch-rhwp.sh`, CI |
| [INC-004](#inc-004) | 임베딩 중 메모리 1.65GB (logits 버퍼) | llama-sys | 패치 + `llama-sys/tests/patches.rs` + CI `diag --test memory` |
| [INC-005](#inc-005) | Windows 모델 로드 시 0xC0000409 비정상 종료 | Windows 빌드 | `/MD` + VC++ 런타임 동봉, Windows CI 실모델 시험 |
| [INC-006](#inc-006) | Windows llama.cpp가 최적화 없이 빌드(17배 느림) | Windows 빌드 | build.rs 의 CMakeCache 최적화 검사 |
| [INC-007](#inc-007) | 배치마다 decode→encode 경고 폭주 | embed | engine.rs encode 우선 |
| [INC-008](#inc-008) | 평가 세트 천장 효과(모든 설정 nDCG ≥ 0.98) | 평가 | 1,000건 + 본문 중간(deep) 질의 |
| [INC-009](#inc-009) | 긴 표 머리글에서 청크 분할 무한 반복 | embed | `chunk.rs` 단위 시험 + 실토크나이저 시험 |
| [INC-010](#inc-010) | 진단 보고서에 파일 앞부분 바이트(16진수)가 남을 수 있음 | diag(개인정보) | `diag/tests/report.rs` |
| [INC-011](#inc-011) | **HWP 본문 대량 누락**(규정 조문 74% 누락) | extract | `walk.rs` 단위 시험 + `extract/tests/coverage.rs` + kra-diag 커버리지 |
| [INC-012](#inc-012) | 진단 도구 현장 견고성 묶음(중간 저장·쓰기 폴더·오류 창 등) | diag | `diag/tests/report.rs`, extract_stats 시험 |
| [INC-013](#inc-013) | 백엔드 개수 검사가 지연 로드 환경의 시험을 깨뜨림 | embed | backend_dir 있을 때만 검사 |
| [INC-014](#inc-014) | 평가 캐시가 추출 결과 변경을 무시(낡은 임베딩 재사용) | 평가 | `m0eval/src/cache.rs` 입력 해시 키 |
| [INC-015](#inc-015) | CI가 메모리 회귀 시험을 돌리지 않음 | CI | ci.yml 실모델 단계에 `diag --test memory` |

---

### INC-001
llama.cpp 통합 앱(app/common) 빌드 실패 · 2026-10-13 · M0 Task 4
- **증상**: b11574 에서 `cargo build -p llama-sys` 가 common 헤더를 못 찾아 실패.
- **원인**: upstream 이 통합 실행 파일 `app/` 을 기본으로 빌드하는데 이것이 `common` 에 의존한다. 우리는 `libllama` 만 필요하다.
- **고친 방법**: `LLAMA_BUILD_APP=OFF`, `LLAMA_BUILD_COMMON=OFF`(+ TESTS/EXAMPLES/TOOLS/SERVER OFF).
- **재발 방지**: `crates/llama-sys/build.rs` 의 옵션 표. llama.cpp 를 올릴 때 옵션 이름이 바뀌었는지 본다([업그레이드 절차](runbooks/upgrade-llama-cpp.md)).

### INC-002
임베딩 차원이 512로 나옴 · 2026-10-13 · M0 Task 5
- **증상**: `llama_model_n_embd` 가 512를 돌려줘 벡터 차원이 512.
- **원인**: EmbeddingGemma 2 는 내부 hidden 512 → 출력 투영 768. 풀링된 출력 차원은 `llama_model_n_embd_out`.
- **고친 방법**: `engine.rs` 가 `llama_model_n_embd_out` 사용. 모델 로드 옵션도 b11574 API에 맞춰 `use_mmap` 대신 `load_mode = LLAMA_LOAD_MODE_MMAP`.
- **재발 방지**: `crates/embed/tests/model.rs`(768차원·한국어 의미), `tests/reference.rs`(원본 sentence-transformers 대비 코사인 ≥ 0.99).
- **주의**: 모델을 바꿀 때(D6 사용자 교체 모델) 출력 차원은 반드시 `n_embd_out` 으로 읽는다 → [embedding-model.md](embedding-model.md).

### INC-003
rhwp 저장소 8.6GB와 부분 복제의 함정 · 2026-10-13 · M0 Task 7
- **증상**: cargo git 의존으로 rhwp 를 받으면 샘플·테스트 때문에 depth 1 에도 수 GB. 부분 복제로 바꾸자 연쇄 문제:
  1. rhwp 소스가 `include_bytes!` 로 `src/` 밖 파일(폰트·빈 문서 템플릿·문서)을 포함 → 빌드 실패.
  2. Windows Git Bash 가 `/src/` 같은 sparse 패턴을 Windows 경로로 바꿈.
  3. 남은 `.git` 때문에 cargo 가 패키지 파일 목록을 git 으로 만들다 실패(지문 계산 오류).
  4. `third_party/rhwp` 가 우리 워크스페이스 멤버로 자동 편입.
- **고친 방법**: `tools/fetch-rhwp.sh` — blobless+sparse 복제, `include_bytes!` 대상 24개를 sparse 목록에 명시, `MSYS_NO_PATHCONV`+`cygpath`, 체크아웃 후 `.git` 삭제(버전은 `.rev` 파일), rhwp `Cargo.toml` 다듬기. 루트 `Cargo.toml` 에 `exclude = ["third_party"]`.
- **재발 방지**: CI(macOS·Windows·deny)가 매번 이 스크립트로 받는다. rhwp 를 올릴 때 → [upgrade-rhwp.md](runbooks/upgrade-rhwp.md).

### INC-004
임베딩 중 최대 메모리 1.65GB · 2026-10-13 · M0 Task 11
- **증상**: Q8 모델 대량 임베딩 중 RSS 가 1,650MB까지 올라감(저사양 PC 목표 위반).
- **원인**: llama.cpp `llama_context::output_reserve` 가 풀링 임베딩 컨텍스트에서도 `n_vocab × n_outputs` logits 버퍼를 잡는다. 어휘가 262k 라 1,024 토큰이면 약 1GB.
- **고친 방법**: 로컬 패치 `crates/llama-sys/patches/0001-skip-logits-buffer-for-pooled-embeddings.patch` — `has_logits = !(embeddings && pooling != NONE)`. 결과 651MB. 임베딩 값은 그대로(참조 코사인 시험 통과).
- **재발 방지(3중)**:
  1. `build.rs` 가 패치를 적용하고, 안 맞으면 빌드 실패(메시지가 업그레이드 절차를 가리킴).
  2. `crates/llama-sys/tests/patches.rs` — 패치 파일 존재 + vendor 소스에 적용 여부(모델 없이 매 CI).
  3. `crates/diag/tests/memory.rs` — 대량 임베딩 최대 메모리 < 로드 직후 + 300MB(CI 실모델 단계, INC-015).
- **관련**: [patches/README.md](../../crates/llama-sys/patches/README.md), [upgrade-llama-cpp.md](runbooks/upgrade-llama-cpp.md).

### INC-005
Windows 모델 로드 시 0xC0000409 비정상 종료 · 2026-10-13 · M0 Task 12 (릴리즈 스모크 시험에서 발견)
- **증상**: 패키지한 `kra-diag.exe bench` 가 모델 로드 순간 fast-fail(0xC0000409)로 죽음. CI 단위 시험은 통과(실모델 시험이 Windows 에서 안 돌았음).
- **원인**: llama/ggml DLL 마다 정적 CRT(`/MT`)가 들어가 DLL 사이에서 CRT 자원(힙·FILE*)이 섞임.
- **고친 방법**: llama/ggml DLL 은 동적 CRT(`CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL`), VC++ 런타임 DLL(vcruntime140, vcruntime140_1, msvcp140)을 exe 옆에 동봉(앱 로컬 배포). 스펙 §11 "CRT 정적 링크"는 **우리 exe 에만** 적용된다.
- **재발 방지**: CI Windows 잡이 실모델 시험(model·reference·memory)을 돌린다. 릴리즈 워크플로가 패키지 상태로 `env`·`bench` 스모크 시험.

### INC-006
Windows llama.cpp 최적화 없이 빌드(약 17배 느림) · 2026-10-13 · M0 Task 12
- **증상**: m0-diag-2 Windows 빌드 Q4 2스레드 초당 29토큰(Mac 1스레드 445).
- **원인**: `cmake` 크레이트가 `CMAKE_<LANG>_FLAGS_RELEASE` 를 cc 크레이트 값(`-nologo -MT -Brepro -W0`)으로 덮어써 `/O2` 가 빠짐.
- **고친 방법**: Windows 에서 `CMAKE_C(XX)_FLAGS(_RELEASE)` 를 직접 지정(`/O2 /Ob2 /DNDEBUG`, `/EHsc`, `/utf-8`). 직접 지정하면 cmake 크레이트가 덮어쓰지 않는다. MSVC vs clang 비교 결과 clang 이 약 5% 빠를 뿐이라 MSVC 유지(DEC-020).
- **재발 방지**: `build.rs` 가 빌드 후 `CMakeCache.txt` 의 `CMAKE_C_FLAGS_RELEASE` 에 `/O2`·`-O2`·`-O3` 가 없으면 빌드 실패. 릴리즈 스모크 수치(2코어 러너 Q4 약 380~580 토큰/초)와 비교.
- **교훈**: m0-diag-2·3 은 "사용 금지" 표시. 사용자에게 넘기기 전 스모크 수치를 이전 값과 비교한다([release-diag.md](runbooks/release-diag.md)).

### INC-007
배치마다 decode→encode 경고 · 2026-10-13 · M0 Task 10
- **증상**: EmbeddingGemma 2 임베딩마다 llama.cpp 가 "decode → encode" 경고를 찍음.
- **원인**: 인코더 전용 모델인데 `llama_decode` 를 먼저 불렀다.
- **고친 방법**: `engine.rs` 가 `llama_encode` 를 먼저 부르고 실패하면 decode 로 대체.

### INC-008
평가 세트 천장 효과 · 2026-10-13 · M0 Task 9~10
- **증상**: 보도자료 200건·질의 100개에서 모든 설정이 nDCG@10 ≥ 0.98 → 양자화·차원·청크 비교가 불가능.
- **고친 방법**: 800건 추가(총 1,000건) + 제목·첫 문단이 아닌 **본문 중간**을 묻는 `deep` 질의 30개. 정답 재검증.
- **교훈**: 평가 세트를 만들면 먼저 "설정 간 차이가 보이는지"부터 확인한다. 쉬운 질의만으로는 결정 근거가 안 된다.

### INC-009
긴 표 머리글에서 청크 분할 무한 반복 · 2026-10-13 · M0 최종 리뷰(Critical)
- **증상**: 머리글이 아주 긴 표에서 `build_chunks` 가 끝나지 않음(m0-diag-3 이 실제 문서에서 멈출 수 있었음 → "사용 금지").
- **원인**: `split_long` 이 잘라낼 위치를 0으로 계산하면 진전이 없었다. 표 머리글을 행마다 붙이는데 머리글이 예산보다 길면 행이 들어갈 자리가 없었다.
- **고친 방법**: `split_long` 은 최소 한 글자는 진전. 표 머리글은 `max_tokens/3` 으로 자르고, 더 긴 머리글은 자체 청크로도 낸다.
- **재발 방지**: `embed/src/chunk.rs` 단위 시험(`long_table_header_terminates_and_respects_max`, `split_long_always_makes_progress`), 실토크나이저 시험(`long_table_header_with_real_tokenizer_terminates`).

### INC-010
진단 보고서에 파일 바이트가 남을 수 있음 · 2026-10-13 · M0 최종 리뷰(개인정보)
- **증상**: "HWP 아님" 파일의 앞 8바이트를 16진수로 보고 → 텍스트 파일이면 본문 앞부분이 그대로 드러날 수 있음.
- **고친 방법**: `magic::report_label` 이 PDF/RTF/XML/HWP3/DRM/텍스트 등 이름으로 바꾸고, 알 수 없는 **이진** 헤더만 `기타:xxxxxxxx`, 5건 미만은 "기타(5건 미만)"로 합친다.
- **재발 방지**: `diag/tests/report.rs`(`report_never_contains_file_bytes`), `extract::magic` 시험. 보고서에는 문서 내용·경로·파일명·사용자명·컴퓨터명을 남기지 않는다(CLAUDE.md 규칙).

### INC-011
HWP 본문 대량 누락 · 2026-10-13 · 사용자 제공 내부통제규정으로 발견
- **증상**: 「내부통제규정」 개정전문 HWP 에서 6,552자 중 1,682자만 추출(커버리지 25.7%). 조문의 항·호가 통째로 빠짐. 공개 보도자료 997건도 93건이 5% 이상 누락(글상자·각주·중첩 표).
- **원인**: 어댑터가 rhwp `rag::build_chunks` 를 썼는데 이것은
  1. **본문 문단이 없는 제목**(규정의 "② …", "1. …" 처럼 한 줄짜리 항·호가 제목으로 분류됨)을 청크로 내지 않는다.
  2. 글상자·묶음 개체 안 글자, 각주·미주, 셀 안의 중첩 표를 다루지 않는다.
  "추출 성공률"은 파싱 성공만 셌기 때문에 M0 평가에서 드러나지 않았다.
- **고친 방법**: `crates/extract/src/walk.rs` — rhwp 문서 모델을 직접 문서 순서대로 순회한다. 제목 판정만 rhwp `build_structure` 를 쓰고, 표는 셀 모델에서 직접 머리글·행으로 선형화한다(DEC-027). 결과: 개정전문 25.7% → 100%, 공개 997건 평균 99.96%(최저 93.8%, 남은 차이는 조판이 그리는 자동 번호).
- **재발 방지**:
  1. `walk.rs` 단위 시험 10개(본문 없는 조문 줄, 글상자·묶음, 각주, 중첩 표, 병합 셀, 1×1 표 등).
  2. `extract/tests/coverage.rs` — 고정 자료 8개(공공누리 공개 문서 3개 포함)의 커버리지 ≥ 0.98. 글상자·표·제목 처리를 일부러 끄면 떨어지는 것을 확인(돌연변이 시험).
  3. `extract::coverage::measure` — rhwp 조판 텍스트 대비 커버리지. kra-diag 가 실제 PC 문서 최대 500개 표본으로 보고(m0-diag-5부터).
- **교훈**: 추출은 "성공/실패"만이 아니라 **얼마나 빠짐없이** 뽑았는지를 재야 한다. 라이브러리의 편의 함수(RAG 청커 등)는 목적이 달라 버리는 것이 있다.

### INC-012
진단 도구 현장 견고성 묶음 · 2026-10-13 · M0 최종 리뷰(Important)
- **증상과 고친 방법**:
  - 중간에 멈추면 결과가 하나도 안 남음 → 단계마다·200파일마다 임시 파일 후 교체로 저장.
  - 실행 폴더에 쓸 수 없으면 실패 → 실행 폴더 → exe 폴더 → 임시 폴더 순으로 대체.
  - 오류 시 창이 바로 닫힘 → panic hook 이 Enter 를 기다림.
  - 토큰 통계를 위해 부모 프로세스가 전체 모델을 로드 → `Tokenizer`(vocab_only) 사용.
  - CPU 백엔드 DLL 이 없을 때 "모델 로드 실패"로 보고 → `ggml_backend_dev_count` 검사 후 "백신이 격리했는지 확인" 안내.
  - 비정상 종료 대화상자·WER 덤프에 힙 포함 → `SetErrorMode(SEM_FAILCRITICALERRORS|SEM_NOGPFAULTERRORBOX)` + `WerSetFlags(NO_UI|NOHEAP)`.
  - EFS 점검이 실제로 파일을 암호화·인증서를 만들 수 있었음 → 읽기 전용 점검(EditionID, 볼륨 `FILE_SUPPORTS_ENCRYPTION`, `EfsConfiguration` 정책).
  - 요약 txt 에 BOM 이 없어 옛 메모장에서 한글 깨짐 → UTF-8 BOM + CRLF.
- **재발 방지**: `diag/tests/report.rs`, `extract_stats` 시험. 진단 도구는 **PC 를 바꾸지 않는다**(레지스트리·파일·인증서) — CLAUDE.md 규칙.

### INC-013
백엔드 개수 검사가 시험을 깨뜨림 · 2026-10-13 · M0 최종 리뷰 수정 중
- **증상**: INC-012 의 `ggml_backend_dev_count()==0` 검사 추가 후 Windows 에서 `backend_dir=None` 시험이 실패.
- **원인**: llama.cpp 는 백엔드 폴더를 주지 않으면 모델 로드 때 백엔드를 **지연 로드**한다. 로드 전에는 개수가 0.
- **고친 방법**: 폴더를 명시했을 때만 검사.

### INC-014
평가 캐시가 낡은 임베딩을 재사용 · 2026-10-13 · INC-011 수정 후 재평가 준비 중
- **증상**: 캐시 파일 이름이 `<모델>/<종류>.json` 뿐이라 추출 결과·청크 설정이 바뀌어도 옛 벡터를 그대로 씀(리뷰에서 사소한 문제로 미뤄 둔 항목이 실제로 문제가 됨).
- **고친 방법**: 캐시 키에 입력 (id, 텍스트) 전체의 FNV-1a 64 해시를 넣음(`m0eval/src/cache.rs`).
- **재발 방지**: `cache.rs` 단위 시험 3개.

### INC-015
CI 가 메모리 회귀 시험을 돌리지 않음 · 2026-10-13
- **증상**: INC-004 의 메모리 시험(`diag/tests/memory.rs`)이 `#[ignore]`(모델 필요)인데 CI 실모델 단계는 `embed` 시험만 돌림 → 패치가 빠져도 CI 가 통과했을 것.
- **고친 방법**: CI 실모델 단계에 `cargo test -p diag --release --test memory -- --include-ignored`. 모델 없이 도는 `llama-sys/tests/patches.rs` 도 추가.
- **교훈**: `#[ignore]` 시험을 만들면 **어디서 돌리는지**를 같이 정한다.
