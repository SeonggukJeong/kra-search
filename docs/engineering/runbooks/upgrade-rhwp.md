# rhwp 올리기

HWP/HWPX 파서 rhwp(MIT, https://github.com/edwardkim/rhwp)를 새 리비전으로 바꿀 때의 절차.

## 현재 버전
| 항목 | 값 |
|---|---|
| 리비전 | `1a76570e833917d15817415a53c09ad61ab3203f` (crate v0.8.7) |
| 받는 방법 | `tools/fetch-rhwp.sh` (`REV`, `SPEC_VERSION=5`) → `third_party/rhwp`(gitignore) |
| 기준 커버리지 | 공개 평가 997건 평균 0.9996, 0.95 미만 2건(P0083 0.942, P0777 0.938 — 자동 번호), 내부통제규정 2건 1.000 |

## 우리가 쓰는 rhwp API (바뀌면 여기부터 깨진다)
- `rhwp::parser::parse_document`, `ParseError` 변형(`EncryptedDocument`, `CryptoError`, `UnsupportedFormat{code}`, `Hwp3Error`, `HmlError`) → `extract/src/hwp.rs` `classify`
- `parsed.header.distribution`(배포용 문서)
- `rhwp::document_core::queries::structure::{build_structure, StructureMode::Auto, StructureNode{section, paragraph, children}}` → 제목 판정
- 모델: `Document.sections[].paragraphs[]`, `Paragraph{text, controls}`, `Control::{Table, Shape, Footnote, Endnote, Header, Footer}`, `ShapeObject::Group{children}`, `ShapeObject::drawing().text_box.paragraphs`, `Table{row_count, col_count, cells, caption}`, `Cell{row, col, row_span, col_span, is_header, paragraphs}`
- 커버리지 기준: `rhwp::wasm_api::HwpDocument::{from_bytes, page_count, extract_page_text_native}`

## 절차
1. **배경 읽기**: [incidents.md](../incidents.md) INC-003(부분 복제 함정), INC-011(본문 누락), [decisions.md](../decisions.md) DEC-008, DEC-027, DEC-028.
2. **리비전 바꾸기**: `tools/fetch-rhwp.sh` 의 `REV` 수정 → `bash tools/fetch-rhwp.sh`.
   - 빌드가 `include_bytes!`/`include_str!` 대상 파일을 못 찾으면: `grep -rn "include_bytes!\|include_str!" third_party/rhwp/src third_party/rhwp/crates` 로 `src/` 밖 경로를 찾아 스크립트의 `EXTRA` 목록에 추가하고 **`SPEC_VERSION` 을 올린다**(안 올리면 이미 받은 PC 가 다시 받지 않음).
   - rhwp `Cargo.toml` 구조가 바뀌면 스크립트 끝의 다듬기(python) 부분을 고친다.
3. **`rag::build_chunks` 로 되돌리지 않는다**(INC-011). 새 버전에 "전체 텍스트" 같은 편의 함수가 생겨도 아래 커버리지 시험으로 같은 수준을 증명하기 전에는 바꾸지 않는다.
4. **시험**
   ```bash
   cargo test -p extract                       # walk.rs 단위 시험 + tests/coverage.rs(고정 자료 커버리지 ≥ 0.98)
   python3 tools/eval/fetch_all.py              # 공개 평가 문서
   cargo run --release -p extract --example coverage -- eval/public/docs/* | sort -t$'\t' -k2 -n | head   # 기준 커버리지와 비교
   cargo test --workspace --release
   ```
   추출 결과가 바뀌면 평가 캐시는 자동으로 무효화된다(INC-014) → `cargo run -p m0eval --release -- --eval eval/public --eval eval/kra` 로 품질 비교.
5. **cargo-deny**: rhwp 하위 의존의 보안 권고가 바뀌었는지 CI deny 잡 확인, `deny.toml` 무시 목록 재검토(DEC-026).
6. **기록**: 이 문서의 "현재 버전" 표, `crates/extract/tests/fixtures/README.md`(rhwp 샘플 출처 rev), 문제가 있었다면 incidents.md.
