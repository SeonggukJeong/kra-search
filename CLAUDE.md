# kra-search

한국 공공기관(한국마사회 우선)용 Windows 로컬 문서 검색 프로그램. Rust + llama.cpp(EmbeddingGemma 2) + rhwp(HWP), UI 는 Tauri 2. 저사양 PC·폐쇄망·비전문 사용자 대상.
설계: `docs/superpowers/specs/2026-10-12-kra-search-design.md` · UI: `docs/design/ui-decisions.md` · 현재 진행: `docs/m0/results.md`

## 반드시 지킬 것
- **실행 중 네트워크 0.** HTTP 크레이트 금지(`deny.toml`). 사용자 문서는 PC 밖으로 나가지 않는다.
- **진단 보고서(`kra-diag`)에는 문서 내용·경로·파일명·사용자명·컴퓨터명·파일 바이트를 남기지 않는다**(INC-010). 진단 도구는 PC 를 바꾸지 않는다(레지스트리·파일·인증서, DEC-022).
- 평가 문서 원본(`eval/public/docs/`, `eval/kra/docs/`, `eval/downloads/`)과 `*.gguf` 는 커밋하지 않는다.
- UI 는 `docs/design/ui-decisions.md`(D1~D8)를 그대로 따른다. 바꾸려면 사용자 승인 후 그 문서의 변경 기록에 남긴다.
- 커밋 이메일은 저장소 설정(`seongguk.dev@gmail.com`)을 쓴다. 문서·주석은 한국어.

## 작업 전에 볼 문서 (위험 영역)
| 이런 작업이면 | 먼저 볼 것 |
|---|---|
| llama.cpp 버전 변경, `crates/llama-sys` 수정 | `docs/engineering/runbooks/upgrade-llama-cpp.md` — **로컬 패치(logits 버퍼) 재적용 필수** |
| rhwp 버전 변경, `crates/extract` 수정 | `docs/engineering/runbooks/upgrade-rhwp.md` — `rag::build_chunks` 로 되돌리지 말 것(INC-011) |
| 임베딩 엔진·모델 교체(`crates/embed`) | `docs/engineering/embedding-model.md` |
| Windows 빌드·CI·릴리즈 워크플로 | `docs/engineering/incidents.md` INC-005·006, `runbooks/release-diag.md` |
| 진단 도구(`crates/diag`) | INC-010·012, DEC-022 |
| 평가(`crates/m0eval`, `eval/`) | `eval/public/README.md`, `eval/kra/README.md`, INC-008·014 |
| 이상해 보이는 코드를 "정리"하려 할 때 | `docs/engineering/decisions.md` 에서 이유 확인 |
| 새 마일스톤·영역 작업 시작 | `docs/engineering/known-issues.md` 의 "계기" 열 |

## 기록 규칙 (docs/engineering/README.md)
문제를 고치면 `incidents.md`(INC + 재발 방지 장치), 스펙에 없는 것을 정하면 `decisions.md`(DEC), 미루면 `known-issues.md`(KI). 번호는 바꾸지 않는다.
작업 기록부(`.superpowers/sdd/`)는 gitignore 된 임시 공간이다 — 그 안의 `Ruling:`·`minor (deferred)` 는 지우기 전에 반드시 위 문서로 옮긴다.

## 개발 명령
```bash
bash tools/fetch-rhwp.sh          # rhwp 소스(third_party/rhwp)
bash tools/fetch-pdfium.sh        # PDF 추출 엔진(third_party/pdfium, 시험에 필요)
bash tools/fetch-models.sh        # 개발용 GGUF(models/)
cargo test --workspace --release
cargo test -p embed --release --test model --test reference -- --include-ignored --test-threads 1
cargo test -p diag --release --test memory -- --include-ignored --test-threads 1
python3 tools/eval/fetch_all.py && python3 tools/eval/import_local.py eval/kra
cargo run -p m0eval --release -- --eval eval/public --eval eval/kra
```
