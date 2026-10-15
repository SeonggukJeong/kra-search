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
| [INC-016](#inc-016) | Windows CI 에서 `fetch-pdfium.sh` 의 curl 이 쓰기 실패(23) | CI·빌드 | 스크립트의 `native_path` + 주석, Windows CI |
| [INC-017](#inc-017) | 큰 문서·상주 워커가 색인 메모리를 키움(90MB TXT 앱 743MB, 워커 누적 490MB) | extract·app | `extract/tests/formats.rs`·`app/tests/pipeline.rs` 큰 TXT 시험, `extract/tests/worker.rs` 재활용 시험, app 의 컴파일 때 상한 일치 확인 |
| [INC-018](#inc-018) | pdfium 이 없으면 모든 PDF 가 영구히 "열 수 없는 파일" | app | `app/tests/cli.rs` `pdf_is_skipped_without_pdfium_and_read_later` |
| [INC-019](#inc-019) | 실패 분류 섞임: 내부 상한이 "100MB 넘는 파일", 진단의 비정상 종료가 시간 초과로 | extract·app·diag | `FailKind::TooComplex` + 코드 고정 시험, `app` 안내 시험, `diag` 집계 시험 |
| [INC-020](#inc-020) | m1-diag-1 이 M2 에 필요한 수치를 못 냄(청크 400 기준·전체 추정 없음) | diag | `embed::chunk::BODY_CHUNK` 공유 상수, 요약 시험, 릴리즈 전 "다음 마일스톤 수치" 점검(런북 1단계) |
| [INC-021](#inc-021) | 키워드 점수가 같은 문서의 순서가 색인 세그먼트 배치에 따라 달라짐(일치 검사에서 발견) | index | `top_ids` 정렬 키 (점수↓, 내용 id↑), `keyword::tests::ties_break_by_content_id_regardless_of_segments`, 일치 검사 (i) 상위 100 완전 일치 |

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

### INC-016
Windows CI 에서 pdfium 내려받기 실패 · 2026-10-14
- **증상**: `bash tools/fetch-pdfium.sh` 가 `curl: (23) client returned ERROR on write` 로 종료 → Windows 잡이 `cargo build` 전에 중단(macOS 는 통과).
- **원인**: 스크립트가 `MSYS_NO_PATHCONV=1` 로 경로 변환을 껐는데 `curl -o` 에 MSYS 경로(`/d/a/...`)를 그대로 줘서, 네이티브 `curl.exe` 가 현재 드라이브의 `\d\a\...`(없는 폴더)에 쓰려다 실패.
- **고친 방법**: curl 에 넘기는 경로만 `cygpath -m`(`native_path`)으로 변환. tar·mv·rm 은 MSYS 프로그램이라 그대로(tar 는 `D:/` 를 원격 호스트로 읽어 변환하면 오히려 깨짐). `fetch-rhwp.sh` 는 git·python 에 `cygpath -m` 경로를 이미 쓰고 있어 CI 에서 동작 — 그대로 둠.
- **교훈**: 경로 변환을 끄면 네이티브 Windows 프로그램(curl·git·python)에 넘기는 경로를 직접 바꿔야 한다. Windows 전용 경로 문제는 macOS 에서 안 보이므로 새 스크립트는 Windows CI 에서 처음 돌려 본다.

### INC-017
큰 문서·상주 워커가 색인 메모리를 키움 · 2026-10-15 (M1 최종 리뷰)
- **증상**: 90MB 한국어 TXT 하나를 색인하면 앱 약 743MB·워커 약 580MB, meta.db 가 97MB 늘었다. 공개 세트 1,000개 색인에서는 상주 워커가 60~490MB 를 오갔고, results.md 는 508MB 최대값을 "색인 쓰기 버퍼·meta 쓰기·사전·워커 응답"으로 잘못 설명했다.
- **원인**: (1) 500만 자 상한(DEC-041)이 앱에서 `KeywordDoc` 을 만들 때만 걸려, 워커는 문서 전체를 JSON 한 줄로 보내고 앱은 그것을 통째로 받아 meta.db 에 저장했다. (2) 상주 워커가 큰 입력 파일(그림 많은 10~48MB HWPX)마다 파일 크기의 약 3배씩 늘고 돌려주지 않았다 — 응답은 수 KB 라 응답 크기와 무관.
- **고친 방법**: `extract::MAX_DOC_CHARS`(500만 자, `BODY_MAX_CHARS` 와 같은 값)로 `extract_bytes` 가 모든 형식의 글자를 자르고 `Stats.truncated` 를 켠다. TXT·CSV 는 디코딩 전에 2,000만 바이트로 자르고 PDF 는 상한을 채우면 남은 쪽을 읽지 않는다(DEC-036 (5)). 워커는 읽은 크기 합 64MB·1,000개·응답 16MB 를 넘으면 새 프로세스로 바꾼다(DEC-033 (2)). 결과: 90MB TXT 앱 196MB·워커 190MB·meta.db 12MB, 공개 세트 워커 약 180~240MB(KI-037 남은 부분).
- **재발 방지 장치**: `extract/tests/formats.rs` `huge_txt_is_capped_and_marked_truncated`(30MB 한글·15MB 영문), `app/tests/pipeline.rs` `huge_txt_is_stored_capped`(meta 의 문서 JSON 크기), `extract/tests/worker.rs` 재활용 시험 3개, `app/src/lib.rs` 의 `const _: () = assert!(MAX_DOC_CHARS == BODY_MAX_CHARS)`.
- **교훈**: 상한은 데이터가 프로세스 경계를 넘기 **전**에 건다. 메모리 최대값은 앱과 자식 프로세스를 따로 재고(`time -l` 은 섞임), 원인을 쪼개 확인하기 전에는 결과 문서에 원인을 쓰지 않는다.

### INC-018
pdfium 이 없으면 모든 PDF 가 영구히 "열 수 없는 파일" · 2026-10-15 (M1 최종 리뷰)
- **증상**: 실행 파일 옆 `pdfium.dll` 이 없거나 불러오지 못하면 모든 PDF 가 `Unsupported` 로 기록되어 "열 수 없는 파일 — 파일 이름으로만 찾을 수 있음"이 되고, DLL 을 되돌려 놓아도 파일이 바뀔 때까지 다시 읽지 않았다(`Unsupported` 는 재시도 대상이 아님).
- **원인**: 계획 결함 — 워커는 pdfium 불러오기 실패를 무시하고 PDF 를 `Unsupported` 로 돌려주며, 앱은 이것을 다른 실패와 똑같이 meta 에 기록했다.
- **고친 방법**: `index` 시작 때 앱이 같은 절대 경로로 `extract::pdf::init` 을 불러 보고, 실패하면 PDF 는 meta 에 기록하지 않고 건너뛴다(재시도 후보도). stderr 한 줄 안내, 요약에 "PDF 건너뜀 N개(다음에 다시)"(DEC-035 추가분).
- **재발 방지 장치**: `app/tests/cli.rs` `pdf_is_skipped_without_pdfium_and_read_later` — 없는 경로로 색인하면 PDF 가 기록되지 않고, 올바른 경로로 다시 색인하면 읽힌다.
- **교훈**: 환경 문제(부품 없음)로 생긴 실패를 문서 탓 실패와 같은 칸에 영구 기록하지 않는다 — 환경이 돌아오면 저절로 다시 처리되게 한다.

### INC-019
실패 분류 섞임 · 2026-10-15 (M1 최종 리뷰)
- **증상**: 3MB ODS 도 내부 상한(ODS 200만 칸·XML 파트 64MB 등)을 넘으면 "100MB 넘는 파일"로 안내됐고, 진단 보고서의 형식별 `timeout` 에 비정상 종료가 섞였다.
- **원인**: `FailKind::TooLarge` 하나를 100MB 파일 상한과 내부 상한에 같이 썼다. 진단은 `Outcome::Crashed` 를 형식별 `timeout` 에 더했다.
- **고친 방법**: `FailKind::TooComplex`(코드 "TooComplex", 화면 "내용이 너무 많은 문서") 추가 — `TooLarge` 는 `MAX_FILE_BYTES` 에만. 진단은 `too_complex`·`truncated`(전체)와 `crashed`(형식별)를 따로 세고 요약에도 따로 보인다(DEC-036 (6)).
- **재발 방지 장치**: `extract/tests/formats.rs` `fail_kind_codes_are_stable`, `extract` 의 sheet·xmlutil 단위 시험(내부 상한 → TooComplex), `app/src/search.rs` `too_complex_is_not_shown_as_too_large`, `diag/src/extract_stats.rs` `crash_timeout_too_complex_and_truncated_are_counted_apart`, `diag/src/report.rs` 요약 시험, `diag/tests/report.rs` 새 항목 확인.
- **교훈**: 사용자에게 보이는 실패 안내와 진단 집계는 원인이 다르면 칸도 다르게 둔다 — 하나로 묶으면 안내가 틀리고 현장 숫자를 해석할 수 없다.

### INC-020
진단 릴리즈가 다음 마일스톤에 쓸 수치를 못 냄 · 2026-10-15 (m1-diag-1 태그 직후, 폐쇄망 반입 전)
- **증상**: m1-diag-1 의 본문 청크 통계와 임베딩 속도 벤치가 400토큰 청크 기준이었다 — M0 가 정한 크기는 300(docs/m0/results.md). 보고서에 문서당 청크 중앙값·p90 만 있어 실제 PC 의 본문 전체 임베딩 시간을 계산할 수 없었고, 성능·효율 코어 수(KI-017 의 약속)와 날짜 가산(M2)의 근거인 수정일 분포도 없었다. 사용자가 "M2 로 넘어가는 데 무리 없나" 물어서 발견.
- **원인**: M0 에서 청크 크기를 정한 뒤 진단의 청크 설정(DEC-003, 값을 코드에 직접 씀)을 갱신하지 않았다. 진단 항목을 "지금 마일스톤이 재고 싶은 것"으로만 정했고, 다음 마일스톤이 이 결과로 정할 것과 대조하지 않았다.
- **고친 방법**: 앱과 진단이 같은 `embed::chunk::BODY_CHUNK` 를 쓴다. 형식별 청크·청크 입력 토큰 합과 찾은 파일 전체 추정, 실측 카드 길이와 본문 전체의 모델별 환산 시간, 성능·효율 코어 수와 그 1·2배 스레드, 수정일 경과 구간과 가장 많이 몰린 하루의 비율을 더해 m1-diag-2 로 다시 냈다(DEC-045). m1-diag-1 은 지우지 않고 대체 표시(DEC-019).
- **재발 방지 장치**: 공유 상수 `BODY_CHUNK`(크기를 바꾸면 진단도 같이 바뀜), `diag/src/report.rs` `summary_shows_m2_estimates`·`cpu_line_shows_hybrid_split_only_when_hybrid`, `diag/src/extract_stats.rs` `chunk_totals_accumulate_per_format`·`estimate_scales_each_format_by_found_over_processed`·`mtime_buckets_and_top_day_share`, `diag/tests/report.rs` 새 항목 확인. 런북 `release-diag.md` 1단계에 "다음 마일스톤이 이 결과로 정할 것과 보고서 항목 대조"를 추가.
- **교훈**: 폐쇄망 측정은 한 번 가져가는 데 사용자의 하루가 든다 — 태그 전에 다음 마일스톤 계획이 이 보고서에서 읽을 숫자를 목록으로 만들어 대조한다. 결정한 설정값은 진단에 복사하지 말고 공유 상수로 가리킨다.

### INC-021
키워드 점수가 같은 문서의 순서가 색인 세그먼트 배치에 따라 달라짐 · 2026-10-16 (M2 Task 17 일치 검사)
- **증상**: 일치 검사 (i)(DEC-059: 제품이 저장한 벡터를 m0eval 에 그대로 넣어 순위 비교)에서 상위 100개가 순서까지 같은 질의가 196/255. 상위 10개는 모두 같았고, 21~57위 근처에서 이웃한 두세 문서의 자리가 바뀌었다(예: Q021 35위부터 P0240·P0116·P0723 순서).
- **원인**: `KeywordIndex::top_ids` 가 tantivy `TopDocs::order_by_score()` 순서를 그대로 썼다. tantivy 는 점수가 같은 문서를 문서 주소(세그먼트·세그먼트 안 순번)로 가르는데, 0.26.2 는 세그먼트를 무작위 UUID 를 키로 한 HashMap(프로세스마다 씨앗이 다름) 순서로 훑는다 — 그래서 동점 순서는 색인 이력(넣은 순서·커밋 횟수·병합)뿐 아니라 **커밋할 때마다, 실행할 때마다** 무작위로 바뀐다(한 세그먼트 안에서도 최상위 N 힙의 동점 순서는 일정하지 않음). 제품 색인은 `FLUSH_EVERY` 마다 커밋해 세그먼트가 여럿이고, m0eval 은 메모리 색인에 한 번에 커밋한다. 같은 BM25 점수(같은 서식 문서·같은 필드 길이)인 문서의 키워드 순위가 서로 달라 RRF 결합 순위가 갈렸다. 제품 안에서도 같은 색인의 같은 질의 결과가 실행마다 바뀔 수 있었다. 점수가 같은 문서가 `keyword_limit`(200) 경계에 걸리면 어느 쪽이 목록에 들어가는지도 달라졌다.
- **고친 방법**: `top_ids` 의 정렬 키를 (BM25 점수 내림차순, `doc_id` 빠른 필드 오름차순) 으로 바꿨다(`TopDocs::order_by` 튜플 키 — 상한 경계도 같은 규칙). 저장 필드를 읽지 않으므로 질의당 비용은 그대로(5만 문서 메모리 색인, 상위 200: 0.34~5.6ms → 0.45~5.8ms, 이전 코드의 저장 필드 읽기 200회는 빠짐). 앱 평가 덤프를 다시 만들어(색인은 재사용) (i) 255/255.
- **재발 방지 장치**: `index::keyword::tests::ties_break_by_content_id_regardless_of_segments` — 동점 문서 8개를 한 세그먼트로 한 번, 하나씩 커밋(넣는 순서를 바꿔 가며)한 색인으로 네 번 만들어 같은 순서(id 오름차순)인지, 상한 3 과 구절 목록에서도 같은지 본다. 처음의 문서 3개 시험은 고치기 전 코드로도 6번에 1번꼴로 우연히 통과했다(세그먼트 순서가 무작위) — 고치기 전 코드로 되돌려 20번 돌려 20번 모두 실패, 고친 코드로 20번 모두 통과를 확인했다. 일치 검사 (i) 가 상위 100 을 순서까지 비교한다(`m0eval --parity`, DEC-059).
- **영향**: 고치기 전의 평가 숫자 — `docs/m1/eval-keyword.md`, `docs/m2/eval-hybrid.md`(Task 16), 그것을 인용한 DEC-058 의 수치, KI-049 의 순위 변화, `params.json` 의 선택 — 은 무작위 동점 순서와 8개씩 묶어 임베딩한 검색어 벡터로 쟀다. 동점 문서의 순서만 바뀌므로 차이는 작고, 앱 평가 nDCG@10 은 고치기 전후 0.8762 로 같았다. M2 평가는 고친 뒤(검색어 하나씩 임베딩) 다시 쟀다 — 고른 값·카드 상한·데이터 뒷부분 결정은 그대로, 검증용 하이브리드 0.7769 vs 키워드 0.7044 · 첫 페이지 97.0% vs 91.0% 도 그대로(`docs/m2/eval-hybrid.md`, DEC-058 갱신). M1 키워드 보고서는 다시 만들지 않았다.
- **교훈**: 검색 엔진의 동점 순서는 저장 배치의 부산물이다. 점수가 같을 수 있는 목록은 모두 안정된 키(내용 id)로 가른다 — 하이브리드 코어의 다른 목록(카드·청크·결합)은 이미 id 로 가르고 있었고, 키워드 목록만 빠져 있었다.
