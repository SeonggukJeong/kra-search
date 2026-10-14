# M1 추출·키워드 검색 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 스펙의 모든 문서 형식(HWP·HWPX·PDF·DOCX·XLSX·PPTX·ODT·ODS·ODP·TXT·CSV)에서 글자를 뽑아, 형태소·한글 2글자 조각(bigram) 키워드 색인에 넣고, 명령줄 `kra-search index` / `kra-search search` 로 찾을 수 있게 한다. 공개·마사회 평가 세트로 키워드 검색 점수를 기록하고, 새 형식의 실제 문서 추출 통계를 폐쇄망 진단 릴리즈(`m1-diag-1`)로 확인한다.

**Architecture:** 추출은 형식별 모듈(`extract`)이 `doc::Document` 를 만들고, 상주 워커 프로세스(`kra-search worker`)가 파일을 하나씩 받아 결과를 JSON 한 줄로 돌려준다(멈춤·비정상 종료는 워커만 죽고 앱이 재시작). 앱(`kra-search`)이 유일한 쓰기 주체로, 폴더를 훑어(`crawler`) 바뀐 파일만 내용 해시(xxh3-128)로 중복을 걸러 추출하고, SQLCipher 메타 DB(`index::meta`)와 tantivy 키워드 색인(`index::keyword`)에 넣는다. 키 보호(DPAPI)와 데이터 위치는 `platform` 크레이트가 맡는다. 벡터 연산·평면 저장소는 가벼운 `vector` 크레이트로 떼어 내 키워드 쪽 빌드가 llama.cpp 를 끌고 오지 않게 한다(KI-008).

**Tech Stack:** Rust 1.97.0(edition 2021) · tantivy 0.26.2 · lindera 6.2.0(`embed-ko-dic`, mecab-ko-dic) · unicode-normalization · rusqlite 0.40(`bundled-sqlcipher-vendored-openssl`) · pdfium-render 0.9.4 + bblanchon/pdfium-binaries `chromium/7881` · zip 8.6 · quick-xml 0.42 · calamine 0.36 · chardetng 1.0 · encoding_rs 0.8 · csv 1.4 · xxhash-rust 0.8(xxh3) · walkdir 2 · getrandom 0.4 · windows 0.62(DPAPI) · clap 4 · tempfile 3(시험용) · Python 3 + uv(시험 자료 생성 전용)

**Spec:** `docs/superpowers/specs/2026-10-12-kra-search-design.md` — 특히 §3.1(형식), §4(기술 스택), §5.1(프로세스·워커·쓰기 주체·30초 제한), §5.3(데이터 위치·키 분실 시 재색인), §6.3~6.5·6.8·6.9(범위·중복·추출·일관성·실패 처리), §7.1·7.4·7.6(SQLCipher·네트워크 0·DLL 절대 경로), §8.2(키워드 필드 가중치·bigram 구절), §12.2(진단 보고서), §14 M1. 함께 볼 것: `docs/engineering/known-issues.md`(계기 "M1"), `decisions.md`(DEC-027·028·030), `incidents.md`(INC-010·011), `docs/m0/results.md`.

## 범위 결정 (스펙이 M1 에 정하지 않은 것 — 이 계획에서 정함)

| 항목 | 결정 | 이유 |
|---|---|---|
| 메타 DB 암호화(SQLCipher)·키 보호(DPAPI) | **M1 에 넣는다** | Windows 에서 OpenSSL 동봉 빌드가 처음 도는 곳이라 위험을 일찍 드러낸다. 평문 DB 로 시작하면 나중에 옮기는 작업이 생김 |
| 워커 | **상주 워커**(파일마다 프로세스 생성 안 함), 제한 시간은 **벽시계 30초** | 스펙 §5.1 은 "CPU 시간 30초" — Windows CPU 시간 측정은 M5(상주·성능 프로필)에서 KI 로 미룸 |
| 폴더 감시(USN·ReadDirectoryChangesW) | **하지 않음**. `kra-search index` 실행 때 mtime·크기 비교로 바뀐 파일만 | 상주 앱은 M5, 파일명 서비스는 M6 |
| EFS 적용, 개인정보 치환, 공문 구조, 버전 묶기 | **하지 않음**(M4·M5) | 스펙 §14 마일스톤 배정 |
| 옛 형식(.xls·.doc·.ppt), 한셀·한쇼 | **하지 않음** | 스펙 §3.2·§16 "추후 검토" |
| 자동 번호(KI-011) | **미룸**. 대신 진단에 "제N조 자동 번호를 쓰는 HWP 문단 수"를 센다 | 공개·마사회 문서 1,005건에서 0건(조사 2026-10-14). 실제 PC 에서 나오면 약 130줄 이식으로 해결 가능 |
| 한국어 형태소 사전 | lindera `embed-ko-dic`(실행 파일에 내장, +87MB). 빌드 때 받는 사전 압축 파일은 우리 스크립트가 **SHA-256 고정**으로 미리 받아 둔다 | 실행 중 네트워크 0 유지. 사전 폴더를 따로 배포하는 방식보다 단순 |
| 키워드 평가 | `m0eval --keyword` 로 공개 + 마사회 세트 nDCG@10·Recall@20 기록 | M2 "하이브리드 > 키워드 단독"의 기준점 |
| 진단 릴리즈 | `m1-diag-1`: 형식별 추출 통계, PDF 글자 깨짐 비율, 자동 번호 문단 수, **스레드 수별 속도(KI-017)**, **실제 카드 길이** | 폐쇄망 반출이 하루 걸리므로 한 번에 모은다 |

## Global Constraints

- Rust 툴체인 `1.97.0`(`rust-toolchain.toml`), **edition 2021** — `if let … && …`(let-chain) 금지(2024 전용). 조사 코드에 let-chain 이 있으면 중첩 `if` 로 바꾼다.
- **실행 중 네트워크 0.** 출시 실행 파일(`kra-search`, `kra-diag`)의 일반 의존 그래프에 HTTP 크레이트(reqwest, hyper, ureq, isahc, curl, attohttpc)가 없어야 한다. 예외는 lindera 사전 **빌드 스크립트**의 `ureq` 하나(빌드 때만) — `deny.toml` 의 `wrappers` 와 `tools/check-no-net.sh`(일반 그래프에 없음 확인)로 지킨다.
- 빌드·개발용 다운로드(`tools/fetch-*.sh`)는 **버전·리비전 고정 + SHA-256 검증**. 제품은 받은 파일을 동봉만 한다.
- **pdfium DLL 은 실행 파일 폴더의 절대 경로로만 로드**(스펙 §7.6). 환경 변수·검색 경로로 찾지 않는다. 개발·시험은 `--pdfium <경로>` 인자 또는 함수 인자로 명시 경로를 준다.
- 파일당 제한 시간 30초, 100MB 초과 파일은 내용 추출 생략(파일명만 색인) — 스펙 §5.1·§6.5.
- 앱만 색인(tantivy·meta.db)에 쓴다. 워커는 추출 결과를 표준 출력으로 돌려줄 뿐이다(스펙 §5.1).
- 메타 DB 는 SQLCipher, 키는 32바이트 난수를 **DPAPI(CurrentUser)** 로 보호해 `keys\meta.key` 에 둔다. 키를 못 풀거나 DB 가 안 열리면 `data\` 를 지우고 다시 색인한다(스펙 §5.3). macOS 는 개발용 스텁(보호 없음).
- 키워드 필드 가중치: 문서 제목 3.0 > 파일명 2.5 > 소제목 2.0 > 본문 1.0, bigram 필드는 같은 필드 형태소 가중치의 0.5배(스펙 §8.2, 초기값 — M2 `--eval` 로 조정).
- HWP 어댑터는 rhwp 문서 모델 직접 순회(`walk.rs`) 유지 — `rag::build_chunks` 로 되돌리지 않는다(INC-011).
- **진단 보고서에는 문서 내용·경로·파일명·사용자명·컴퓨터명·파일 바이트를 남기지 않는다**(INC-010). 새 항목은 집계 숫자만.
- 저장소에 넣는 시험 자료는 합성이거나 공공누리 제1유형·MIT 표본(DEC-030). 상용 글꼴을 임베드한 PDF 를 만들지 않는다(비임베드 CID 글꼴·Helvetica 만).
- 평가 문서 원본, `*.gguf`, `third_party/` 는 커밋하지 않는다.
- 문서·주석은 한국어. 커밋 메시지 끝에 `Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>`. 저장소 `user.email` 은 `seongguk.dev@gmail.com`(이미 설정됨).
- 문제를 고치면 `incidents.md`(INC + 재발 방지), 스펙에 없는 것을 정하면 `decisions.md`(DEC), 미루면 `known-issues.md`(KI). 번호는 이어서 쓴다(현재 마지막: INC-015, DEC-032, KI-017).

## Review Focus

1. **이상한 경로** — 한글·공백·260자 넘는 경로, UTF-8 이 아닌 경로, 읽기 권한 없는 폴더, 심볼릭 링크 고리. 색인이 멈추지 않고 그 파일만 건너뛰거나 처리해야 한다. → Task 14 `skips_unreadable_and_non_utf8_without_aborting`, `long_paths_are_scanned`.
2. **색인 도중 강제 종료** — 키워드 커밋과 메타 DB 갱신 사이에서 죽어도 다음 실행이 중복·유실 없이 이어져야 한다. → Task 15 `resume_after_interrupted_flush_has_no_duplicates`.
3. **같은 내용 여러 파일·이름 바꾸기·삭제** — 결과가 중복되지 않고, 새 이름으로 찾히고, 마지막 경로가 지워지면 결과에서 사라져야 한다. → Task 15 `duplicates_rename_and_delete`.
4. **크거나 이상한 문서** — 압축 폭탄 XML 파트, 100만 행 반복 ODS, 0바이트 파일, 확장자와 내용이 다른 파일, 100MB 초과, 워커를 멈추게 하는 파일. 분류되고(파일명만 색인), 워커가 재시작되며, 다음 파일은 정상 처리돼야 한다. → Task 2 `extension_mismatch_and_empty_are_classified`, Task 4 `oversized_xml_part_is_capped`, Task 6 `huge_repeated_rows_are_cheap`, Task 9 `worker_survives_hang_and_crash`, Task 15 `oversized_file_is_filename_only`.
5. **이상한 검색어** — 빈 문자열, 공백만, 기호만, 1,000자, 전각 영숫자(ＫＲＡ), 한 글자, 따옴표. 패닉 없이 빈 결과나 정상 결과. → Task 13 `odd_queries_do_not_panic`, `fullwidth_query_matches_halfwidth_text`.

---

## 파일 구조

```
crates/
  vector/     (새) src/lib.rs(← embed/src/vector.rs), src/flat.rs(← index/src/flat.rs)     벡터 연산·평면 저장소
  embed/      src/lib.rs: `pub use ::vector;` (경로 embed::vector::* 유지), vector.rs 삭제
  extract/    src/lib.rs        FailKind·Extracted·Stats·extract_bytes(형식 분기)
              src/format.rs     Format(확장자) · MAX_FILE_BYTES
              src/norm.rs       글자 정리(사설 영역·대시 5개 이상·공백), 블록 추가
              src/table.rs      일반 표 → TableHeader/TableRow
              src/xmlutil.rs    zip + quick-xml 0.42 공용
              src/hwp.rs        (수정) 형식 감지는 rhwp 에 맡김(KI-010), 자동 번호 문단 수
              src/walk.rs       (수정) norm 사용, 캡션(KI-015)
              src/ooxml.rs      DOCX·PPTX
              src/odf.rs        ODT·ODP
              src/sheet.rs      XLSX·ODS(calamine)
              src/text.rs       TXT·CSV(인코딩 감지)
              src/pdf.rs        PDF(pdfium, 명시 경로 로드, 깨짐 감지)
              src/worker.rs     상주 워커 프로토콜(서버·클라이언트)
              src/cli.rs        (수정) 파일 하나 추출(크기·읽기 오류·형식 분기)
              src/bin/extract-one.rs (수정) 시험용 멈춤 훅을 여기로(KI-009)
              tests/formats.rs, tests/worker.rs, tests/fixtures/{*.docx,*.pptx,…,*.pdf,formatting_table.hml}
  platform/   (새) src/lib.rs paths(데이터·키 위치), keys(난수 키·DPAPI 보호·불러오기)
  index/      src/lib.rs, src/tokenize.rs(형태소·bigram·NFKC), src/keyword.rs(tantivy), src/meta.rs(SQLCipher)
  crawler/    (새) src/lib.rs   폴더 훑기·제외 규칙·차이 계산·내용 해시
  app/        (새) bin `kra-search`: src/main.rs(명령), src/pipeline.rs(색인), src/search.rs(검색·출력), tests/cli.rs
  m0eval/     src/keyword.rs(키워드 평가), src/corpus.rs(매니페스트 제목 보관)
  diag/       형식별 추출 통계·PDF 품질·자동 번호·스레드 측정·카드 길이(→ m1-diag-1)
tools/
  fetch-pdfium.sh   fetch-ko-dic.sh   check-no-net.sh
  fixtures/make_office.py  fixtures/make_pdf.py
.cargo/config.toml     LINDERA_BUILD_DICTIONARY_CACHE_DIR = third_party/ko-dic
deny.toml              ureq(빌드 전용) wrappers 예외, webpki-roots 라이선스 예외
.github/workflows/ci.yml, release-diag.yml(← release-m0.yml)
docs/m1/results.md, docs/m1/eval-keyword.md, docs/diag/README-diag.txt(← docs/m0/README-diag.txt)
docs/engineering/runbooks/upgrade-lindera-pdfium.md
```

---
### Task 1: `vector` 크레이트 분리 (KI-008)

벡터 연산(`embed/src/vector.rs`)과 평면 저장소(`index/src/flat.rs`)를 의존성 없는 `vector` 크레이트로 옮긴다. 이후 `index` 에 tantivy·lindera·SQLCipher 가 들어가도 `kra-diag` 는 그것들을 빌드하지 않는다.

**Files:**
- Create: `crates/vector/Cargo.toml`, `crates/vector/src/lib.rs`(← `git mv crates/embed/src/vector.rs`), `crates/vector/src/flat.rs`(← `git mv crates/index/src/flat.rs`)
- Modify: `crates/embed/Cargo.toml`, `crates/embed/src/lib.rs`, `crates/index/Cargo.toml`, `crates/index/src/lib.rs`, `crates/diag/Cargo.toml`, `crates/diag/src/bench.rs:7`, `crates/m0eval/Cargo.toml`, `crates/m0eval/src/experiments.rs:9-10`, `docs/engineering/known-issues.md`(KI-008)

**Interfaces:**
- Produces: `vector::{truncate_normalize, quantize_i8, dot_i8, sign_bits, hamming, asym_score, cosine}`(기존 `embed::vector` 함수 그대로), `vector::flat::FlatIndex`. `embed::vector::*` 경로는 재수출로 계속 동작한다.

- [ ] **Step 1: 파일 옮기기**

```bash
mkdir -p crates/vector/src
git mv crates/embed/src/vector.rs crates/vector/src/lib.rs
git mv crates/index/src/flat.rs crates/vector/src/flat.rs
```

- [ ] **Step 2: `crates/vector/Cargo.toml`**

```toml
[package]
name = "vector"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
```

- [ ] **Step 3: 모듈 연결**

`crates/vector/src/lib.rs` 맨 위 문서 주석 바로 아래(첫 `pub fn` 위)에 추가:

```rust
pub mod flat;
```

`crates/vector/src/flat.rs` 3행:

```rust
use crate::{asym_score, dot_i8, hamming, quantize_i8, sign_bits, truncate_normalize};
```

`crates/embed/src/lib.rs` 의 `pub mod vector;` 를 다음으로 바꾼다:

```rust
/// 벡터 연산은 `vector` 크레이트로 옮겼다(KI-008). 기존 경로 `embed::vector::*` 를 유지하려고 재수출한다.
pub use ::vector;
```

`crates/embed/Cargo.toml` `[dependencies]` 에 `vector = { path = "../vector" }` 추가.

`crates/index/src/lib.rs` 전체:

```rust
//! 키워드 색인(tantivy)·메타 DB(SQLCipher). 벡터 연산·평면 저장소는 `vector` 크레이트.
```

`crates/index/Cargo.toml` 의 `[dependencies]` 에서 `embed = { path = "../embed" }` 를 지운다(빈 표로 둔다).

`crates/diag/Cargo.toml`, `crates/m0eval/Cargo.toml`: `index = { path = "../index" }` 를 `vector = { path = "../vector" }` 로 바꾼다.
`crates/diag/src/bench.rs:7` → `use vector::flat::FlatIndex;`
`crates/m0eval/src/experiments.rs:9-10` →

```rust
use vector::flat::FlatIndex;
use vector::truncate_normalize;
```

- [ ] **Step 4: 시험 — 옮긴 단위 시험이 그대로 통과하고 index 가 llama-sys 를 끌지 않는지**

Run: `cargo test -p vector --release && cargo build --workspace --release && cargo tree -p index | grep -c llama-sys`
Expected: vector 시험 PASS(기존 vector.rs·flat.rs 시험 전부), 빌드 성공, 마지막 줄 `0`.

- [ ] **Step 5: KI-008 해결 표시**

`docs/engineering/known-issues.md` KI-008 행의 상태 칸을 `해결: vector 크레이트 분리(M1 Task 1)` 로 바꾼다.

- [ ] **Step 6: Commit**

```bash
git add -A crates/vector crates/embed crates/index crates/diag crates/m0eval docs/engineering/known-issues.md Cargo.lock
git commit -m "refactor: split vector math and flat index into vector crate (KI-008)

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: 추출 공통 틀 — 형식 판별·실패 분류·글자 정리 (KI-009, KI-010)

**Files:**
- Create: `crates/extract/src/format.rs`, `crates/extract/src/norm.rs`, `crates/extract/src/table.rs`, `crates/extract/tests/formats.rs`, `crates/extract/tests/fixtures/formatting_table.hml`(rhwp `samples/hml/formatting_table.hml` 복사, MIT)
- Modify: `crates/extract/src/lib.rs`, `crates/extract/src/hwp.rs`, `crates/extract/src/walk.rs`, `crates/extract/src/runner.rs`, `crates/extract/src/cli.rs`, `crates/extract/src/bin/extract-one.rs`, `crates/extract/tests/runner.rs`, `crates/extract/tests/fixtures/README.md`, `crates/diag/src/report.rs`, `crates/diag/src/extract_stats.rs`, `docs/engineering/known-issues.md`

**Interfaces:**
- Produces (이후 모든 추출 Task 가 씀):

```rust
// extract::format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Format { Hwp, Hwpx, Pdf, Docx, Pptx, Xlsx, Odt, Odp, Ods, Txt, Csv }
impl Format {
    pub const ALL: [Format; 11];
    pub fn from_path(p: &std::path::Path) -> Option<Format>;
    pub fn ext(self) -> &'static str;
}
pub const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;

// extract (lib.rs)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum FailKind { Encrypted, Drm, Corrupt, NotContainer, Unsupported, TooLarge, NoText, Unreadable }
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Stats { pub pdf_pages: u32, pub pdf_bad_pages: u32, pub article_numbered_paras: u32 }
pub struct Extracted { pub doc: doc::Document, pub distribution: bool, pub stats: Stats }
pub fn extract_bytes(format: Format, bytes: &[u8]) -> Result<Extracted, (FailKind, String)>;
/// DRM 복호화 플러그인 자리(정의만, 구현 없음 — 스펙 §6.5)
pub trait Decryptor: Send + Sync { fn decrypt(&self, bytes: &[u8]) -> Option<Vec<u8>>; }

// extract::norm (crate 내부)
pub(crate) fn clean(s: &str) -> String;            // 한 줄로: 사설 영역 글자 제거, 대시 5개 이상 연속 제거, 공백 정리
pub(crate) fn clean_lines(s: &str) -> String;      // 줄마다 clean, 빈 줄 버림, "\n" 으로 잇기
pub(crate) fn push(out: &mut Vec<doc::Block>, kind: doc::BlockKind, text: &str); // clean_lines 결과가 비어 있지 않으면 추가

// extract::table (crate 내부)
pub(crate) fn push_table(out: &mut Vec<doc::Block>, rows: &[Vec<String>]);

// extract::runner — Ok 에 stats 추가(serde 기본값으로 옛 JSON 도 읽힘)
pub enum Outcome {
    Ok { doc: Document, distribution: bool, #[serde(default)] coverage: Option<f32>, #[serde(default)] stats: Stats },
    Failed { kind: FailKind, magic: String }, Timeout, Crashed { code: Option<i32> },
}
```

`extract::hwp::FailKind` 경로는 `pub use crate::FailKind;` 로 유지한다(diag 가 씀).

- [ ] **Step 1: 실패하는 시험 작성** — `crates/extract/tests/formats.rs`

```rust
//! 형식 판별·실패 분류 시험. 형식별 내용 시험은 각 형식 Task 에서 이 파일에 덧붙인다.

use extract::cli::extract_one;
use extract::runner::Outcome;
use extract::FailKind;
use std::path::{Path, PathBuf};

fn fx(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// 임시 폴더에 이름·내용을 정해 파일을 만든다.
fn temp_file(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(name);
    std::fs::write(&p, bytes).unwrap();
    (dir, p)
}

fn fail_kind(o: &Outcome) -> Option<FailKind> {
    match o {
        Outcome::Failed { kind, .. } => Some(kind.clone()),
        _ => None,
    }
}

#[test]
fn format_from_extension_is_case_insensitive() {
    use extract::format::Format;
    assert_eq!(Format::from_path(Path::new("a/보고서.HWPX")), Some(Format::Hwpx));
    assert_eq!(Format::from_path(Path::new("x.Pdf")), Some(Format::Pdf));
    assert_eq!(Format::from_path(Path::new("x.doc")), None);
    assert_eq!(Format::from_path(Path::new("확장자없음")), None);
    for f in Format::ALL {
        assert_eq!(Format::from_path(Path::new(&format!("a.{}", f.ext()))), Some(f));
    }
}

#[test]
fn extension_mismatch_and_empty_are_classified() {
    let (_d, empty) = temp_file("빈.hwp", b"");
    assert_eq!(fail_kind(&extract_one(&empty)), Some(FailKind::NotContainer));
    let (_d, unknown) = temp_file("메모.doc", b"anything");
    assert_eq!(fail_kind(&extract_one(&unknown)), Some(FailKind::Unsupported));
    // 어떤 확장자든 DRM 헤더면 DRM
    let (_d, drm) = temp_file("보안.pdf", b"\x9b DRMONE  rest of file");
    assert_eq!(fail_kind(&extract_one(&drm)), Some(FailKind::Drm));
}

#[test]
fn oversized_file_is_too_large_without_reading() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("큰파일.hwp");
    let f = std::fs::File::create(&p).unwrap();
    f.set_len(extract::format::MAX_FILE_BYTES + 1).unwrap(); // 희소 파일 — 디스크를 쓰지 않는다
    assert_eq!(fail_kind(&extract_one(&p)), Some(FailKind::TooLarge));
}

#[test]
fn missing_file_is_unreadable() {
    assert_eq!(fail_kind(&extract_one(Path::new("/없는/경로/문서.hwp"))), Some(FailKind::Unreadable));
}

#[test]
fn hwpml_saved_as_hwp_is_extracted() {
    // KI-010: HWPML(XML) 문서를 .hwp 로 저장한 경우 — 예전에는 "HWP 아님"으로 분류됐다.
    let bytes = std::fs::read(fx("formatting_table.hml")).unwrap();
    let (_d, p) = temp_file("xml문서.hwp", &bytes);
    match extract_one(&p) {
        Outcome::Ok { doc, .. } => assert!(!doc.blocks.is_empty()),
        other => panic!("{other:?}"),
    }
}
```

`crates/extract/Cargo.toml` 에 `[dev-dependencies] tempfile = "3"` 추가. rhwp 표본 복사:

```bash
cp third_party/rhwp/samples/hml/formatting_table.hml crates/extract/tests/fixtures/
```

`crates/extract/tests/fixtures/README.md` 의 "rhwp 저장소에서 복사" 표에 `| formatting_table.hml | samples/hml/formatting_table.hml (HWPML 2.91, KI-010 시험) |` 행 추가.

- [ ] **Step 2: 실패 확인**

Run: `cargo test -p extract --release --test formats`
Expected: 컴파일 실패(`extract::format`, `extract::FailKind` 없음).

- [ ] **Step 3: `crates/extract/src/format.rs`**

```rust
//! 확장자로 문서 형식을 정한다. 내용 검사(DRM·컨테이너)는 형식별 추출기가 한다.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Format {
    Hwp,
    Hwpx,
    Pdf,
    Docx,
    Pptx,
    Xlsx,
    Odt,
    Odp,
    Ods,
    Txt,
    Csv,
}

/// 이보다 큰 파일은 내용을 추출하지 않는다(스펙 §6.5) — 파일명만 색인.
pub const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;

impl Format {
    pub const ALL: [Format; 11] = [
        Format::Hwp, Format::Hwpx, Format::Pdf, Format::Docx, Format::Pptx, Format::Xlsx,
        Format::Odt, Format::Odp, Format::Ods, Format::Txt, Format::Csv,
    ];

    pub fn ext(self) -> &'static str {
        match self {
            Format::Hwp => "hwp",
            Format::Hwpx => "hwpx",
            Format::Pdf => "pdf",
            Format::Docx => "docx",
            Format::Pptx => "pptx",
            Format::Xlsx => "xlsx",
            Format::Odt => "odt",
            Format::Odp => "odp",
            Format::Ods => "ods",
            Format::Txt => "txt",
            Format::Csv => "csv",
        }
    }

    pub fn from_path(p: &Path) -> Option<Format> {
        let ext = p.extension()?.to_str()?.to_ascii_lowercase();
        Format::ALL.into_iter().find(|f| f.ext() == ext)
    }
}
```

- [ ] **Step 4: `crates/extract/src/norm.rs`**

```rust
//! 추출 글자 정리(모든 형식 공통).

use doc::{Block, BlockKind};

fn is_private_use(c: char) -> bool {
    matches!(c as u32, 0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD)
}

/// 같은 줄표 글자가 이만큼 이상 이어지면 지운다 — 신구대조표의 "------"(현행과 같음) 표시 등(KI-012).
const DASH_RUN_MIN: usize = 5;

fn is_dash(c: char) -> bool {
    matches!(c, '-' | '─' | '━' | '－' | '―' | '—')
}

/// 줄표가 DASH_RUN_MIN 개 이상 이어진 구간을 공백 하나로 바꾼다.
fn drop_dash_runs(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if is_dash(chars[i]) {
            let start = i;
            while i < chars.len() && chars[i] == chars[start] {
                i += 1;
            }
            if i - start >= DASH_RUN_MIN {
                out.push(' ');
            } else {
                out.extend(&chars[start..i]);
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// 한 줄로: 한컴 전용 글자(사설 영역) 제거, 긴 줄표 제거, 연속 공백(줄바꿈 포함)을 하나로.
pub(crate) fn clean(s: &str) -> String {
    let s: String = s.chars().map(|c| if is_private_use(c) { ' ' } else { c }).collect();
    drop_dash_runs(&s).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 줄 구조를 남기고 줄마다 `clean`. 빈 줄은 버린다.
pub(crate) fn clean_lines(s: &str) -> String {
    s.lines().map(clean).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// 정리한 글자가 비어 있지 않으면 블록으로 추가한다.
pub(crate) fn push(out: &mut Vec<Block>, kind: BlockKind, text: &str) {
    let t = clean_lines(text);
    if !t.is_empty() {
        out.push(Block { kind, text: t });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dash_runs_are_dropped_short_dashes_kept() {
        assert_eq!(clean("현행 ------ 개정안"), "현행 개정안");
        assert_eq!(clean("──────────"), "");
        assert_eq!(clean("KRA-2026 — 2--3"), "KRA-2026 — 2--3");
    }

    #[test]
    fn private_use_and_spaces_are_cleaned() {
        assert_eq!(clean("\u{F081C} 본인  서명\t끝"), "본인 서명 끝");
    }

    #[test]
    fn clean_lines_keeps_line_structure() {
        assert_eq!(clean_lines(" 첫 줄 \r\n\r\n  둘째   줄 \n"), "첫 줄\n둘째 줄");
        let mut v = Vec::new();
        push(&mut v, BlockKind::Paragraph, "  \n ");
        assert!(v.is_empty());
    }
}
```

- [ ] **Step 5: `crates/extract/src/table.rs`** — HWP 외 형식의 표(행 × 칸 글자)를 블록으로

```rust
//! 일반 표(행 × 칸 글자) → TableHeader("열1 | 열2") + TableRow("열1: 값 | 열2: 값").
//! HWP 표는 병합 정보를 쓰는 walk.rs 가 따로 처리한다.

use crate::norm::{clean, push};
use doc::{Block, BlockKind};

/// 머리글 행 = 채워진 칸이 min(2, 최대 채움 수) 이상인 첫 행. 그 앞 행(표 위 제목 줄 등)은 문단으로 낸다.
/// 머리글 칸이 비면 값만 쓰고, 값이 빈 칸은 생략하며, 값이 모두 빈 행은 버린다.
pub(crate) fn push_table(out: &mut Vec<Block>, rows: &[Vec<String>]) {
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| r.iter().map(|c| clean(c)).collect::<Vec<_>>())
        .filter(|r| r.iter().any(|c| !c.is_empty()))
        .collect();
    let filled = |r: &Vec<String>| r.iter().filter(|c| !c.is_empty()).count();
    let Some(max_filled) = rows.iter().map(filled).max() else {
        return;
    };
    let h = rows.iter().position(|r| filled(r) >= max_filled.min(2)).unwrap_or(0);
    for pre in &rows[..h] {
        push(out, BlockKind::Paragraph, &pre.join(" "));
    }
    let header = &rows[h];
    // 머리글 끝쪽 빈 칸은 잘라낸다(시트의 넓은 사용 범위 대비).
    let width = header.iter().rposition(|c| !c.is_empty()).map_or(0, |i| i + 1);
    push(out, BlockKind::TableHeader, &header[..width].join(" | "));
    for row in &rows[h + 1..] {
        let parts: Vec<String> = row
            .iter()
            .enumerate()
            .filter(|(_, v)| !v.is_empty())
            .map(|(i, v)| match header.get(i).filter(|h| !h.is_empty()) {
                Some(h) => format!("{h}: {v}"),
                None => v.clone(),
            })
            .collect();
        push(out, BlockKind::TableRow, &parts.join(" | "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(v: &[&[&str]]) -> Vec<Vec<String>> {
        v.iter().map(|r| r.iter().map(|s| s.to_string()).collect()).collect()
    }

    #[test]
    fn title_row_above_table_is_paragraph() {
        let mut out = Vec::new();
        push_table(&mut out, &rows(&[&["2026년 1분기 실적", "", ""], &["경마장", "매출", "비고"], &["서울", "100", ""]]));
        let got: Vec<(BlockKind, &str)> = out.iter().map(|b| (b.kind.clone(), b.text.as_str())).collect();
        assert_eq!(got, vec![
            (BlockKind::Paragraph, "2026년 1분기 실적"),
            (BlockKind::TableHeader, "경마장 | 매출 | 비고"),
            (BlockKind::TableRow, "경마장: 서울 | 매출: 100"),
        ]);
    }

    #[test]
    fn empty_table_emits_nothing() {
        let mut out = Vec::new();
        push_table(&mut out, &rows(&[&["", ""], &[" ", ""]]));
        assert!(out.is_empty());
    }
}
```

- [ ] **Step 6: `crates/extract/src/lib.rs`**

```rust
//! 파일 → `doc::Document`. 형식별 추출기는 이 크레이트 밖으로 파서 타입을 내보내지 않는다.

pub mod cli;
pub mod coverage;
pub mod format;
pub mod hwp;
pub mod magic;
pub(crate) mod norm;
pub mod runner;
pub(crate) mod table;
mod walk;

use format::Format;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum FailKind {
    Encrypted,
    Drm,
    Corrupt,
    NotContainer,
    Unsupported,
    /// 100MB 초과 — 내용을 읽지 않음
    TooLarge,
    /// 글자 층이 없거나 깨짐(스캔 PDF·ToUnicode 없는 PDF 등) — 파일명만 색인
    NoText,
    /// 읽기 오류(권한 없음·다른 프로그램이 잠금·사라짐)
    Unreadable,
}

/// 진단·평가용 문서 단위 통계(내용 없음, 숫자만).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Stats {
    /// PDF 쪽 수와, 글자가 비었거나 깨져 버린 쪽 수
    pub pdf_pages: u32,
    pub pdf_bad_pages: u32,
    /// HWP: "제N조" 꼴 자동 번호(개요·문단 번호)를 쓰는 문단 수 — KI-011 판단용
    pub article_numbered_paras: u32,
}

pub struct Extracted {
    pub doc: doc::Document,
    pub distribution: bool,
    pub stats: Stats,
}

/// 문서보안(DRM) 복호화 플러그인 자리(스펙 §6.5) — 인터페이스만 정의하고 구현은 없다(Fasoo SDK 연동은 로드맵 §16).
/// 복호화할 수 없으면 None.
pub trait Decryptor: Send + Sync {
    fn decrypt(&self, bytes: &[u8]) -> Option<Vec<u8>>;
}

/// 바이트에서 추출. 실패하면 (유형, 앞 8바이트 16진수 — 진단 통계용)을 돌려준다.
pub fn extract_bytes(format: Format, bytes: &[u8]) -> Result<Extracted, (FailKind, String)> {
    if bytes.is_empty() {
        return Err((FailKind::NotContainer, String::new()));
    }
    // 문서보안(DRM)은 원래 형식과 상관없이 파일 전체를 감싼다.
    if magic::is_known_drm(bytes) {
        return Err((FailKind::Drm, magic::magic_hex(bytes)));
    }
    match format {
        Format::Hwp | Format::Hwpx => hwp::extract_hwp(bytes),
        // 나머지 형식은 이후 Task 에서 채운다.
        _ => Err((FailKind::Unsupported, String::new())),
    }
}
```

- [ ] **Step 7: `crates/extract/src/hwp.rs`** — 형식 감지는 rhwp 에 맡긴다(HWP 3.x·HWPML 도 rhwp 가 읽는다, KI-010)

```rust
//! rhwp 파싱 → `doc::Document` 어댑터(본문 순회는 walk.rs). rhwp 타입은 extract 크레이트 밖으로 내보내지 않는다.

pub use crate::FailKind;
use crate::magic::magic_hex;
use crate::{Extracted, Stats};
use doc::{BlockKind, Document};

/// HWP·HWPX·HWP 3.x·HWPML 모두 rhwp 가 형식을 감지해 읽는다.
pub fn extract_hwp(bytes: &[u8]) -> Result<Extracted, (FailKind, String)> {
    let parsed = rhwp::parser::parse_document(bytes).map_err(|e| (classify(&e), magic_hex(bytes)))?;
    let blocks = crate::walk::document_blocks(&parsed);
    let title = blocks.iter().find(|b| b.kind == BlockKind::Heading).map(|b| b.text.clone());
    let stats = Stats { article_numbered_paras: article_numbered_paras(&parsed), ..Stats::default() };
    Ok(Extracted { doc: Document { title, blocks }, distribution: parsed.header.distribution, stats })
}

fn classify(e: &rhwp::parser::ParseError) -> FailKind {
    use rhwp::parser::ParseError as P;
    match e {
        P::EncryptedDocument | P::CryptoError(_) => FailKind::Encrypted,
        P::UnsupportedFormat { code, .. } if code.contains("DRM") => FailKind::Drm,
        // 빈 파일·알 수 없는 형식 = 한글 문서가 아님
        P::UnsupportedFormat { .. } => FailKind::NotContainer,
        P::Hwp3Error(_) | P::HmlError(_) => FailKind::Unsupported,
        _ => FailKind::Corrupt,
    }
}

/// "제N조" 꼴 자동 번호를 쓰는 본문 문단 수. 번호 형식 문자열(예: "제^1조")에 '조'가 있는 개요·문단 번호를 센다.
/// KI-011(자동 번호 미추출) 을 고칠지 판단하는 진단 통계 — 내용은 남기지 않는다.
pub(crate) fn article_numbered_paras(d: &rhwp::model::document::Document) -> u32 {
    use rhwp::model::style::HeadType;
    let mut n = 0;
    for s in &d.sections {
        for p in &s.paragraphs {
            let Some(ps) = d.doc_info.para_shapes.get(p.para_shape_id as usize) else { continue };
            let id = match ps.head_type {
                HeadType::Outline if ps.numbering_id == 0 => s.section_def.outline_numbering_id,
                HeadType::Outline | HeadType::Number => ps.numbering_id,
                _ => continue,
            };
            let Some(num) = (id as usize).checked_sub(1).and_then(|i| d.doc_info.numberings.get(i)) else { continue };
            if num.level_formats.get(ps.para_level as usize).is_some_and(|f| f.contains('조')) {
                n += 1;
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use rhwp::model::document::{Document, Section};
    use rhwp::model::paragraph::Paragraph;
    use rhwp::model::style::{HeadType, Numbering, ParaShape};

    #[test]
    fn counts_article_style_auto_numbering() {
        let mut d = Document::default();
        d.doc_info.para_shapes.push(ParaShape::default()); // 0: 번호 없음
        d.doc_info.para_shapes.push(ParaShape { head_type: HeadType::Number, numbering_id: 1, para_level: 0, ..ParaShape::default() });
        d.doc_info.para_shapes.push(ParaShape { head_type: HeadType::Number, numbering_id: 2, para_level: 0, ..ParaShape::default() });
        let mut article = Numbering::default();
        article.level_formats[0] = "제^1조".into();
        let mut plain = Numbering::default();
        plain.level_formats[0] = "^1.".into();
        d.doc_info.numberings = vec![article, plain];
        let para = |shape: u16| Paragraph { text: "본문".into(), para_shape_id: shape, ..Paragraph::new_empty() };
        d.sections.push(Section { paragraphs: vec![para(0), para(1), para(1), para(2)], ..Section::default() });
        assert_eq!(super::article_numbered_paras(&d), 2);
    }
}
```

> rhwp 필드 이름이 다르면(rhwp 를 올렸을 때) `third_party/rhwp/src/model/style.rs`(`ParaShape.head_type/numbering_id/para_level`, `Numbering.level_formats`)와 `document.rs`(`Section.section_def.outline_numbering_id`)를 보고 맞춘다.

- [ ] **Step 8: `crates/extract/src/walk.rs`** — 공통 정리 함수를 쓴다

`walk.rs` 의 `is_private_use`·`clean`·`push_text` 정의(약 44~58행)를 지우고 아래로 바꾼다. 다른 곳의 `clean(...)` 호출은 그대로 둔다(같은 이름).

```rust
use crate::norm::clean;

fn push_text(out: &mut Vec<Block>, kind: BlockKind, text: &str) {
    let t = clean(text);
    if !t.is_empty() {
        out.push(Block { kind, text: t });
    }
}
```

- [ ] **Step 9: `crates/extract/src/runner.rs`** — `Outcome::Ok` 에 `stats` 추가

```rust
use crate::{FailKind, Stats};
```

`Outcome::Ok` 변형을 다음으로 바꾼다:

```rust
    Ok {
        doc: Document,
        distribution: bool,
        #[serde(default)]
        coverage: Option<f32>,
        #[serde(default)]
        stats: Stats,
    },
```

(`use crate::hwp::FailKind;` 줄은 지운다.)

- [ ] **Step 10: `crates/extract/src/cli.rs`** — 크기·읽기 오류·형식 분기. 시험용 멈춤 훅은 라이브러리에서 뺀다(KI-009)

```rust
//! 파일 하나 추출. `extract-one` 바이너리, `kra-diag extract-one`, 상주 워커(worker.rs)가 함께 쓴다.

use crate::format::{Format, MAX_FILE_BYTES};
use crate::magic::magic_hex;
use crate::runner::Outcome;
use crate::FailKind;
use std::path::Path;

pub fn extract_one(path: &Path) -> Outcome {
    extract_one_with(path, false)
}

fn failed(kind: FailKind) -> Outcome {
    Outcome::Failed { kind, magic: String::new() }
}

/// `measure_coverage` 면 조판 텍스트 대비 커버리지도 잰다(HWP 만, 느림 — 진단·평가 전용).
pub fn extract_one_with(path: &Path, measure_coverage: bool) -> Outcome {
    let Some(format) = Format::from_path(path) else { return failed(FailKind::Unsupported) };
    match std::fs::metadata(path) {
        Err(_) => return failed(FailKind::Unreadable),
        Ok(m) if m.len() > MAX_FILE_BYTES => return failed(FailKind::TooLarge),
        Ok(_) => {}
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return failed(FailKind::Unreadable),
    };
    match std::panic::catch_unwind(|| crate::extract_bytes(format, &bytes)) {
        Ok(Ok(x)) => {
            let hwp = matches!(format, Format::Hwp | Format::Hwpx);
            let coverage = if measure_coverage && hwp { crate::coverage::measure(&bytes, &x.doc.blocks) } else { None };
            Outcome::Ok { doc: x.doc, distribution: x.distribution, coverage, stats: x.stats }
        }
        Ok(Err((kind, magic))) => Outcome::Failed { kind, magic },
        Err(_) => Outcome::Failed { kind: FailKind::Corrupt, magic: magic_hex(&bytes) },
    }
}

/// 결과를 stdout 한 줄 JSON으로 출력한다.
pub fn extract_one_main(path: &Path, measure_coverage: bool) {
    println!("{}", serde_json::to_string(&extract_one_with(path, measure_coverage)).unwrap());
}
```

`crates/extract/src/bin/extract-one.rs` — 시험용 멈춤 훅은 이 개발용 바이너리에만 둔다(출시 실행 파일 `kra-search`·`kra-diag` 에는 없음):

```rust
fn main() {
    // 시험 전용: 시간 초과 처리 확인(tests/runner.rs). 출시 실행 파일에는 이 훅이 없다(KI-009).
    if std::env::var("KRA_TEST_HANG").as_deref() == Ok("1") {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
    let mut args = std::env::args_os().skip(1);
    let path = args.next().expect("usage: extract-one <file> [--coverage]");
    let coverage = args.any(|a| a == "--coverage");
    extract::cli::extract_one_main(std::path::Path::new(&path), coverage);
}
```

`crates/extract/tests/runner.rs` 에서 `extract::hwp::FailKind` 를 쓰는 곳은 그대로 동작한다(재수출). `Outcome::Ok { .. }` 패턴은 그대로, 구조체 생성 코드가 있으면 `stats: Default::default()` 를 넣는다. 시간 초과 시험이 `run_command(cmd …)` 로 `extract-one` 바이너리를 직접 실행하는지 확인한다(라이브러리를 거치지 않으면 그대로 통과).

- [ ] **Step 11: diag 컴파일 맞추기** — 새 실패 유형 집계

`crates/diag/src/report.rs` `ExtractInfo` 의 `crashed` 아래에 추가:

```rust
    #[serde(default)]
    pub too_large: usize,
    #[serde(default)]
    pub no_text: usize,
    #[serde(default)]
    pub unreadable: usize,
```

`crates/diag/src/extract_stats.rs`:
- `clone_counts` 에 `too_large: x.too_large, no_text: x.no_text, unreadable: x.unreadable,` 추가.
- `match kind` 에 세 갈래 추가:

```rust
                FailKind::TooLarge => a.x.too_large += 1,
                FailKind::NoText => a.x.no_text += 1,
                FailKind::Unreadable => a.x.unreadable += 1,
```

- `Outcome::Ok { doc, distribution, coverage }` 패턴을 `Outcome::Ok { doc, distribution, coverage, .. }` 로, 시험의 생성 코드 `Outcome::Ok { doc: …, distribution: false, coverage: … }` 에 `stats: Default::default()` 추가.

- [ ] **Step 12: 시험 통과 확인**

Run: `cargo test -p extract --release && cargo test -p diag --release`
Expected: PASS(새 시험 6개 + norm·table·hwp 단위 시험 + 기존 시험 전부). 기존 `tests/runner.rs` 의 "HWP 아님" 시험이 `NotContainer` 와 16진수 머리를 그대로 받는지 확인.

- [ ] **Step 13: KI 기록**

`docs/engineering/known-issues.md`:
- KI-009 상태 → `해결: 멈춤 훅을 개발용 extract-one 바이너리로 이동(M1 Task 2)`
- KI-010 상태 → `해결: 형식 감지를 rhwp 에 맡겨 HWP 3.x·HWPML 추출, 읽기 오류는 Unreadable(M1 Task 2)`
- KI-012 상태 → `해결: norm::clean 이 같은 줄표 5개 이상 연속 제거(M1 Task 2)`

- [ ] **Step 14: Commit**

```bash
git add -A crates/extract crates/diag docs/engineering/known-issues.md Cargo.lock
git commit -m "extract: format dispatch, new failure kinds, shared text cleanup (KI-009, KI-010, KI-012)

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: HWP 캡션 추출 (KI-015)

**Files:**
- Modify: `crates/extract/src/walk.rs`, `docs/engineering/known-issues.md`

**Interfaces:**
- Consumes: `walk::document_blocks`(기존), `norm::clean`(Task 2)
- Produces: 그림·묶음 개체·그리기 개체의 캡션 문단이 `Paragraph` 블록으로 나온다.

- [ ] **Step 1: 실패하는 시험** — `walk.rs` 의 `mod tests` 에 추가

```rust
    #[test]
    fn picture_and_shape_captions_are_kept() {
        use rhwp::model::image::Picture;
        use rhwp::model::shape::Caption;
        let cap = |t: &str| Caption { paragraphs: vec![para(t)], ..Caption::default() };
        let pic = Picture { caption: Some(cap("그림 1. 조직도")), ..Picture::default() };
        let mut rect = RectangleShape::default();
        rect.drawing.caption = Some(cap("그림 2. 흐름도"));
        let group = GroupShape { caption: Some(cap("그림 3. 묶음")), ..GroupShape::default() };
        let d = doc_of(vec![
            with_control("", Control::Picture(Box::new(pic))),
            with_control("", Control::Shape(Box::new(ShapeObject::Rectangle(rect)))),
            with_control("", Control::Shape(Box::new(ShapeObject::Group(group)))),
        ]);
        let text = all_text(&document_blocks(&d));
        for needle in ["그림 1. 조직도", "그림 2. 흐름도", "그림 3. 묶음"] {
            assert!(text.contains(needle), "{needle} 누락:\n{text}");
        }
    }
```

- [ ] **Step 2: 실패 확인**

Run: `cargo test -p extract --release --lib walk`
Expected: FAIL — 캡션 글자 누락.

- [ ] **Step 3: 구현** — `walk.rs`

`control_blocks` 의 `match c` 에 그림 갈래 추가:

```rust
        Control::Picture(p) => caption_blocks(p.caption.as_ref(), out, depth),
```

`shape_blocks` 를 다음으로 바꾼다:

```rust
fn shape_blocks(s: &ShapeObject, out: &mut Vec<Block>, depth: usize) {
    if depth >= MAX_DEPTH {
        return;
    }
    match s {
        ShapeObject::Group(g) => {
            caption_blocks(g.caption.as_ref(), out, depth);
            g.children.iter().for_each(|c| shape_blocks(c, out, depth + 1));
        }
        ShapeObject::Picture(p) => caption_blocks(p.caption.as_ref(), out, depth),
        other => {
            if let Some(d) = other.drawing() {
                if let Some(tb) = d.text_box.as_ref() {
                    tb.paragraphs.iter().for_each(|p| inner_paragraph(p, out, depth));
                }
                caption_blocks(d.caption.as_ref(), out, depth);
            }
        }
    }
}

/// 개체 캡션("그림 1. …")의 문단.
fn caption_blocks(cap: Option<&rhwp::model::shape::Caption>, out: &mut Vec<Block>, depth: usize) {
    if let Some(c) = cap {
        c.paragraphs.iter().for_each(|p| inner_paragraph(p, out, depth));
    }
}
```

> `Control::Picture` 변형 이름·`Picture.caption` 필드는 `third_party/rhwp/src/model/control.rs`, `image.rs` 기준(2026-10-14 확인). 다르면 그 파일을 보고 맞춘다.

- [ ] **Step 4: 통과 확인 + 회귀**

Run: `cargo test -p extract --release`
Expected: PASS(기존 `tests/coverage.rs` 커버리지 시험 포함 — 캡션 추가로 커버리지가 내려가면 안 된다).

- [ ] **Step 5: KI-015 해결, KI-011 측정 기록**

`known-issues.md`:
- KI-015 상태 → `해결: 그림·그리기·묶음 개체 캡션(M1 Task 3)`
- KI-011 "고칠 방법" 칸 끝에 덧붙임: `조사(2026-10-14): 공개·마사회 1,005건에서 "제^1조" 자동 번호 0건, 자동 번호 문단 58개(기본 "1./가./1)" 목록). 공개 API 없음(rhwp 번호 계산은 pub(crate)) — 공개 모델로 약 130줄 이식하면 렌더러와 1,005건 전부 일치. m1-diag-1 의 "제N조 자동 번호 문단 수"가 0 보다 크면 이식한다.` 계기 칸 → `m1-diag-1 결과`.

- [ ] **Step 6: Commit**

```bash
git add crates/extract/src/walk.rs docs/engineering/known-issues.md
git commit -m "extract(hwp): index picture, drawing and group captions (KI-015)

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---
### Task 4: DOCX·PPTX 추출 + 사무 문서 시험 자료 생성기

**Files:**
- Create: `tools/fixtures/make_office.py`, `crates/extract/tests/fixtures/office/`(생성물: `sample.docx`, `sample.pptx`, `sample.odt`, `sample.odp`, `sample.xlsx`, `sample.ods`, `utf8.txt`, `utf8_bom.txt`, `cp949.txt`, `cp949_short.txt`, `cp949_ext.txt`, `utf16le_bom.txt`, `utf16be_bom.txt`, `utf16le_nobom.txt`, `ascii.txt`, `cp949.csv`), `crates/extract/src/xmlutil.rs`, `crates/extract/src/ooxml.rs`
- Modify: `crates/extract/Cargo.toml`, `crates/extract/src/lib.rs`, `crates/extract/tests/formats.rs`, `crates/extract/tests/fixtures/README.md`

**Interfaces:**
- Consumes: `norm::push`, `table::push_table`, `FailKind`(Task 2)
- Produces:

```rust
// extract::xmlutil (crate 내부, Task 5·6 도 씀)
pub(crate) type Zip<'a> = zip::ZipArchive<std::io::Cursor<&'a [u8]>>;
pub(crate) const MAX_PART: u64 = 64 * 1024 * 1024;
pub(crate) fn open_zip(bytes: &[u8]) -> Result<Zip<'_>, FailKind>;
pub(crate) fn read_part(zip: &mut Zip, name: &str) -> Result<Option<String>, FailKind>;           // 없으면 None, MAX_PART 초과면 TooLarge
pub(crate) fn read_part_capped(zip: &mut Zip, name: &str, cap: u64) -> Result<Option<String>, FailKind>;
pub(crate) fn push_ref(buf: &mut String, r: &quick_xml::events::BytesRef);
pub(crate) fn attr(e: &quick_xml::events::BytesStart, local: &str) -> Option<String>;
pub(crate) fn first_element_text(xml: &str, local: &str) -> Option<String>;
// extract::ooxml
pub(crate) fn docx(bytes: &[u8]) -> Result<doc::Document, FailKind>;
pub(crate) fn pptx(bytes: &[u8]) -> Result<doc::Document, FailKind>;
```

- [ ] **Step 1: 시험 자료 생성기** — `tools/fixtures/make_office.py` (표준 라이브러리만)

```python
# 시험용 DOCX/PPTX/ODT/ODP/XLSX/ODS/TXT/CSV 생성기 — 표준 라이브러리(zipfile)만 사용. 내용은 모두 합성.
# 사용: python3 -I tools/fixtures/make_office.py crates/extract/tests/fixtures/office
import os, sys, zipfile

OUT = sys.argv[1]
os.makedirs(OUT, exist_ok=True)
XML = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
FIXED_TIME = (2026, 1, 1, 0, 0, 0)  # 다시 만들어도 같은 바이트가 나오게 고정


def write_zip(name, parts, mimetype=None):
    """parts: [(경로, 문자열)]. ODF 는 mimetype 을 무압축 첫 항목으로 넣는다."""
    with zipfile.ZipFile(os.path.join(OUT, name), "w", zipfile.ZIP_DEFLATED) as z:
        if mimetype:
            z.writestr(zipfile.ZipInfo("mimetype", FIXED_TIME), mimetype, compress_type=zipfile.ZIP_STORED)
        for path, text in parts:
            z.writestr(zipfile.ZipInfo(path, FIXED_TIME), text.encode("utf-8"), compress_type=zipfile.ZIP_DEFLATED)


def core_xml(title):
    return XML + (
        '<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" '
        'xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" '
        'xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">'
        f'<dc:title>{title}</dc:title><dc:creator>작성자</dc:creator></cp:coreProperties>')


# ---------------- DOCX ----------------
W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" ' \
    'xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" ' \
    'xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" ' \
    'xmlns:v="urn:schemas-microsoft-com:vml"'

def p(text, style=None, ppr_extra=""):
    style_xml = f'<w:pStyle w:val="{style}"/>' if style else ""
    ppr = f'<w:pPr>{style_xml}{ppr_extra}</w:pPr>' if (style or ppr_extra) else ""
    return f'<w:p>{ppr}<w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p>'

def tc(*paras):
    return "<w:tc><w:tcPr/>" + "".join(p(x) for x in paras) + "</w:tc>"

docx_body = "".join([
    p("사업 개요", style="1"),                      # 한국어 Word: 제목 1 의 styleId 가 "1"
    p("본 문서는 렛츠런파크 운영 계획이다."),
    # 여러 run + 탭 + 줄바꿈 + 엔티티 + 문자참조(&#xAC00; = 가)
    '<w:p><w:r><w:t>항목</w:t></w:r><w:r><w:tab/><w:t>A&amp;B &lt;중요&gt; &#xAC00;나다</w:t></w:r>'
    '<w:r><w:br/><w:t>둘째 줄</w:t></w:r></w:p>',
    p("세부 추진 계획", style="Heading2"),          # 영문 Word styleId
    p("개요 수준만 지정된 제목", ppr_extra='<w:outlineLvl w:val="2"/>'),
    # 변경 추적 삭제 글·필드 코드는 빠져야 한다
    '<w:p><w:r><w:t>남은 글</w:t></w:r><w:del w:id="1" w:author="x"><w:r><w:delText>지운 글</w:delText></w:r></w:del>'
    '<w:r><w:instrText> PAGE </w:instrText></w:r></w:p>',
    # 표: 첫 행 = 머리글, 한 칸에 문단 두 개
    '<w:tbl><w:tblPr/><w:tblGrid/>'
    '<w:tr>' + tc("구분") + tc("예산(백만원)") + tc("비고") + '</w:tr>'
    '<w:tr>' + tc("경마 시행") + tc("1,200") + tc("전년 대비", "5% 증가") + '</w:tr>'
    '<w:tr>' + tc("시설 개선") + tc("300") + tc("") + '</w:tr>'
    '</w:tbl>',
    # 글상자: mc:Choice 와 mc:Fallback 에 같은 글이 두 번 들어 있다 → 한 번만 나와야 함
    '<w:p><w:r><w:t>글상자 앞</w:t></w:r><w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing><wps:txbx><w:txbxContent>'
    + p("글상자 안 문장") + '</w:txbxContent></wps:txbx></w:drawing></mc:Choice><mc:Fallback><w:pict><v:textbox><w:txbxContent>'
    + p("글상자 안 문장") + '</w:txbxContent></v:textbox></w:pict></mc:Fallback></mc:AlternateContent></w:r></w:p>',
    p("맺음말"),
])
docx_document = XML + f'<w:document {W}><w:body>{docx_body}<w:sectPr/></w:body></w:document>'
docx_styles = XML + (
    '<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
    '<w:style w:type="paragraph" w:default="1" w:styleId="a"><w:name w:val="Normal"/></w:style>'
    '<w:style w:type="paragraph" w:styleId="1"><w:name w:val="heading 1"/><w:basedOn w:val="a"/>'
    '<w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style>'
    '<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/></w:style>'
    '</w:styles>')
write_zip("sample.docx", [
    ("[Content_Types].xml", XML + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
     '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
     '<Default Extension="xml" ContentType="application/xml"/>'
     '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
     '<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>'
     '<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/></Types>'),
    ("_rels/.rels", XML + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
     '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>'
     '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>'),
    ("word/_rels/document.xml.rels", XML + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
     '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>'),
    ("word/document.xml", docx_document),
    ("word/styles.xml", docx_styles),
    ("docProps/core.xml", core_xml("2026 운영 계획 &amp; 예산")),
])

# ---------------- PPTX ----------------
PNS = 'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" ' \
      'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" ' \
      'xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"'

def sp(ph_type, paras):
    ph = f'<p:ph type="{ph_type}"/>' if ph_type else '<p:ph idx="1"/>'
    body = "".join(f'<a:p>{x}</a:p>' for x in paras)
    return (f'<p:sp><p:nvSpPr><p:cNvPr id="2" name="s"/><p:cNvSpPr/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:spPr/>'
            f'<p:txBody><a:bodyPr/><a:lstStyle/>{body}</p:txBody></p:sp>')

def run(t):
    return f'<a:r><a:rPr lang="ko-KR"/><a:t>{t}</a:t></a:r>'

def slide(*shapes):
    return XML + f'<p:sld {PNS}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{"".join(shapes)}</p:spTree></p:cSld></p:sld>'

pptx_table = ('<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="t"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm/>'
              '<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblGrid/>'
              '<a:tr h="0"><a:tc><a:txBody><a:p>' + run("지역") + '</a:p></a:txBody></a:tc><a:tc><a:txBody><a:p>' + run("입장객") + '</a:p></a:txBody></a:tc></a:tr>'
              '<a:tr h="0"><a:tc><a:txBody><a:p>' + run("서울") + '</a:p></a:txBody></a:tc><a:tc><a:txBody><a:p>' + run("12,000") + '</a:p></a:txBody></a:tc></a:tr>'
              '</a:tbl></a:graphicData></a:graphic></p:graphicFrame>')
slides = {
    "slide1.xml": slide(sp("ctrTitle", [run("더비 운영 계획")]), sp("subTitle", [run("2026년 3월")])),
    "slide2.xml": slide(sp("title", [run("마지막 슬라이드")]), sp(None, [run("감사합니다")])),
    "slide10.xml": slide(sp("title", [run("추진 일정")]), sp(None, [run("1단계: 준비") + '<a:br/>' + run("2단계: 시행"), run("세부 항목 &amp; 비고")]), pptx_table),
}
# 발표 순서: slide1 → slide10 → slide2 (파일 이름 숫자 순서와 다름)
order = [("rId2", "slide1.xml"), ("rId3", "slide10.xml"), ("rId4", "slide2.xml")]
pres = XML + f'<p:presentation {PNS}><p:sldIdLst>' + "".join(
    f'<p:sldId id="{256 + i}" r:id="{rid}"/>' for i, (rid, _) in enumerate(order)) + '</p:sldIdLst></p:presentation>'
pres_rels = XML + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">' + "".join(
    f'<Relationship Id="{rid}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/{f}"/>'
    for rid, f in order) + '</Relationships>'
write_zip("sample.pptx", [
    ("[Content_Types].xml", XML + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
     '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
     '<Default Extension="xml" ContentType="application/xml"/>'
     '<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>'
     + "".join(f'<Override PartName="/ppt/slides/{f}" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>' for f in slides)
     + '</Types>'),
    ("_rels/.rels", XML + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
     '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>'),
    ("ppt/presentation.xml", pres),
    ("ppt/_rels/presentation.xml.rels", pres_rels),
    *[(f"ppt/slides/{f}", s) for f, s in slides.items()],
    ("docProps/core.xml", core_xml("더비 발표")),
])

# ---------------- ODT / ODP ----------------
ONS = ('xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" '
       'xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" '
       'xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" '
       'xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" '
       'xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" '
       'xmlns:dc="http://purl.org/dc/elements/1.1/" '
       'xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0"')

def odf_parts(mime, content_body, title):
    manifest = XML + ('<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3">'
                      f'<manifest:file-entry manifest:full-path="/" manifest:media-type="{mime}"/>'
                      '<manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>'
                      '<manifest:file-entry manifest:full-path="meta.xml" manifest:media-type="text/xml"/></manifest:manifest>')
    content = XML + f'<office:document-content {ONS} office:version="1.3"><office:body>{content_body}</office:body></office:document-content>'
    meta = XML + f'<office:document-meta {ONS} office:version="1.3"><office:meta><dc:title>{title}</dc:title></office:meta></office:document-meta>'
    return [("META-INF/manifest.xml", manifest), ("content.xml", content), ("meta.xml", meta)]

def cell(t):
    return f'<table:table-cell office:value-type="string"><text:p>{t}</text:p></table:table-cell>'

odt_body = ('<office:text>'
            '<text:h text:outline-level="1">제1장 <text:span>총칙</text:span></text:h>'
            '<text:p>목적<text:s/>및<text:s text:c="3"/>범위<text:tab/>탭 뒤<text:line-break/>다음 줄</text:p>'
            '<text:p>각주가 있는 문장<text:note text:note-class="footnote"><text:note-citation>1</text:note-citation>'
            '<text:note-body><text:p>각주 본문</text:p></text:note-body></text:note> 이어지는 글</text:p>'
            '<text:list><text:list-item><text:p>목록 항목 &amp; 하나</text:p></text:list-item></text:list>'
            '<table:table table:name="표1"><table:table-column table:number-columns-repeated="3"/>'
            '<table:table-header-rows><table:table-row>' + cell("구분") + cell("수량") + cell("비고") + '</table:table-row></table:table-header-rows>'
            '<table:table-row>' + cell("말") + cell("12") + '<table:table-cell/></table:table-row>'
            '<table:table-row><table:table-cell table:number-columns-spanned="2" office:value-type="string"><text:p>합쳐진 칸</text:p></table:table-cell>'
            '<table:covered-table-cell/>' + cell("끝") + '</table:table-row>'
            '</table:table>'
            '<text:p>맺음</text:p>'
            '</office:text>')
write_zip("sample.odt", odf_parts("application/vnd.oasis.opendocument.text", odt_body, "규정 개정안"),
          mimetype="application/vnd.oasis.opendocument.text")

def frame(cls, inner):
    c = f' presentation:class="{cls}"' if cls else ""
    return f'<draw:frame{c}><draw:text-box>{inner}</draw:text-box></draw:frame>'

odp_body = ('<office:presentation>'
            '<draw:page draw:name="page1">' + frame("title", "<text:p>발표 제목</text:p>") + frame("subtitle", "<text:p>부제목</text:p>") + '</draw:page>'
            '<draw:page draw:name="page2">' + frame("title", "<text:p>둘째 장</text:p>")
            + frame("outline", "<text:list><text:list-item><text:p>요점 하나</text:p></text:list-item><text:list-item><text:p>요점 둘</text:p></text:list-item></text:list>")
            + '<presentation:notes><draw:frame presentation:class="notes"><draw:text-box><text:p>발표자 메모</text:p></draw:text-box></draw:frame></presentation:notes>'
            + '</draw:page>'
            '</office:presentation>')
write_zip("sample.odp", odf_parts("application/vnd.oasis.opendocument.presentation", odp_body, "발표 자료"),
          mimetype="application/vnd.oasis.opendocument.presentation")

# ---------------- ODS ----------------
ods_body = ('<office:spreadsheet>'
            '<table:table table:name="예산">'
            '<table:table-row><table:table-cell office:value-type="string"><text:p>구분</text:p></table:table-cell>'
            '<table:table-cell office:value-type="string"><text:p>금액</text:p></table:table-cell>'
            '<table:table-cell office:value-type="string"><text:p>일자</text:p></table:table-cell></table:table-row>'
            '<table:table-row><table:table-cell office:value-type="string"><text:p>경마</text:p></table:table-cell>'
            '<table:table-cell office:value-type="float" office:value="1200"><text:p>1,200</text:p></table:table-cell>'
            '<table:table-cell office:value-type="date" office:date-value="2026-03-01"><text:p>2026-03-01</text:p></table:table-cell></table:table-row>'
            '<table:table-row><table:table-cell office:value-type="string"><text:p>시설</text:p></table:table-cell>'
            '<table:table-cell office:value-type="float" office:value="0.5"><text:p>0.5</text:p></table:table-cell>'
            '<table:table-cell/></table:table-row>'
            # 실제 LibreOffice 파일처럼 끝에 거대한 반복 빈 행
            '<table:table-row table:number-rows-repeated="1048573"><table:table-cell table:number-columns-repeated="1024"/></table:table-row>'
            '</table:table>'
            '<table:table table:name="빈 시트"><table:table-row><table:table-cell/></table:table-row></table:table>'
            '</office:spreadsheet>')
write_zip("sample.ods", odf_parts("application/vnd.oasis.opendocument.spreadsheet", ods_body, "예산표"),
          mimetype="application/vnd.oasis.opendocument.spreadsheet")

# ---------------- XLSX (최소 구성) ----------------
def c_str(ref, idx):
    return f'<c r="{ref}" t="s"><v>{idx}</v></c>'
shared = ["구분", "금액", "일자", "경마 시행", "시설 개선", "비고"]
sheet1 = XML + ('<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>'
                # 1행은 비어 있음(행 자체 없음) → 2행이 첫 비어있지 않은 행
                '<row r="2">' + c_str("A2", 0) + c_str("B2", 1) + c_str("C2", 2) + c_str("D2", 5) + '</row>'
                '<row r="3">' + c_str("A3", 3) + '<c r="B3"><v>1200</v></c><c r="C3" s="1"><v>46082</v></c>'
                '<c r="D3" t="inlineStr"><is><t>인라인 문자열</t></is></c></row>'
                '<row r="4">' + c_str("A4", 4) + '<c r="B4"><v>300.25</v></c><c r="D4" t="b"><v>1</v></c></row>'
                '<row r="5"><c r="B5"><f>SUM(B3:B4)</f><v>1500.25</v></c></row>'
                '</sheetData></worksheet>')
sheet2 = XML + '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/></worksheet>'
sst = XML + f'<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{len(shared)}" uniqueCount="{len(shared)}">' + \
    "".join(f"<si><t>{s}</t></si>" for s in shared) + '</sst>'
styles = XML + ('<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
                '<cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="14" applyNumberFormat="1"/></cellXfs></styleSheet>')
workbook = XML + ('<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
                  'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>'
                  '<sheet name="예산" sheetId="1" r:id="rId1"/><sheet name="빈 시트" sheetId="2" r:id="rId2"/></sheets></workbook>')
wb_rels = XML + ('<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
                 '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>'
                 '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>'
                 '<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>'
                 '<Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>')
write_zip("sample.xlsx", [
    ("[Content_Types].xml", XML + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
     '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
     '<Default Extension="xml" ContentType="application/xml"/>'
     '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
     '<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
     '<Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
     '<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>'
     '<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>'),
    ("_rels/.rels", XML + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
     '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>'),
    ("xl/workbook.xml", workbook), ("xl/_rels/workbook.xml.rels", wb_rels),
    ("xl/worksheets/sheet1.xml", sheet1), ("xl/worksheets/sheet2.xml", sheet2),
    ("xl/sharedStrings.xml", sst), ("xl/styles.xml", styles),
    ("docProps/core.xml", core_xml("예산 집계")),
])

# ---------------- TXT / CSV ----------------
KO = "한국마사회 경마 시행 계획\n렛츠런파크 서울 운영 방안을 보고합니다.\n"
def wb(name, data):
    with open(os.path.join(OUT, name), "wb") as f:
        f.write(data)
wb("utf8.txt", KO.encode("utf-8"))
wb("utf8_bom.txt", b"\xef\xbb\xbf" + KO.encode("utf-8"))
wb("cp949.txt", KO.encode("cp949"))
wb("cp949_short.txt", "회의록".encode("cp949"))            # 6바이트 — 감지 어려운 짧은 입력
wb("cp949_ext.txt", "똠방각하 펲뷁 샾".encode("cp949"))     # 완성형 밖(확장 영역) 글자
wb("utf16le_bom.txt", "﻿".encode("utf-16-le") + KO.encode("utf-16-le"))
wb("utf16be_bom.txt", "﻿".encode("utf-16-be") + KO.encode("utf-16-be"))
wb("utf16le_nobom.txt", KO.encode("utf-16-le"))
wb("ascii.txt", b"plain ascii only\n")
wb("cp949.csv", ('구분,금액,비고\n경마 시행,"1,200","따옴표 ""안"" 글"\n시설 개선,300,\n'
                 '"여러 줄\n칸",5,끝\n').encode("cp949"))
print("ok")
```

생성·확인:

```bash
python3 -I tools/fixtures/make_office.py crates/extract/tests/fixtures/office
ls -la crates/extract/tests/fixtures/office   # 16개 파일, 모두 수 KB 이하
```

`crates/extract/tests/fixtures/README.md` 끝에 추가:

```markdown
## 사무 문서·텍스트 (`office/`)
`tools/fixtures/make_office.py` 로 만든 합성 문서(표준 라이브러리 zipfile, 내용은 모두 지어낸 것).
다시 만들 때: `python3 -I tools/fixtures/make_office.py crates/extract/tests/fixtures/office` — ZIP 시각을 고정해 같은 바이트가 나온다.
```

- [ ] **Step 2: 실패하는 시험** — `crates/extract/tests/formats.rs` 에 추가

```rust
fn office(name: &str) -> PathBuf {
    fx(&format!("office/{name}"))
}

/// 성공 결과의 (제목, [(블록 종류, 글자)]).
fn blocks(o: Outcome) -> (Option<String>, Vec<(doc::BlockKind, String)>) {
    match o {
        Outcome::Ok { doc, .. } => (doc.title, doc.blocks.into_iter().map(|b| (b.kind, b.text)).collect()),
        other => panic!("추출 실패: {other:?}"),
    }
}

fn kt(k: doc::BlockKind, t: &str) -> (doc::BlockKind, String) {
    (k, t.to_string())
}

#[test]
fn docx_headings_tables_textbox_once() {
    use doc::BlockKind::*;
    let (title, b) = blocks(extract_one(&office("sample.docx")));
    assert_eq!(title.as_deref(), Some("2026 운영 계획 & 예산"));
    assert_eq!(b, vec![
        kt(Heading, "사업 개요"),
        kt(Paragraph, "본 문서는 렛츠런파크 운영 계획이다."),
        kt(Paragraph, "항목 A&B <중요> 가나다\n둘째 줄"),
        kt(Heading, "세부 추진 계획"),
        kt(Heading, "개요 수준만 지정된 제목"),
        kt(Paragraph, "남은 글"),
        kt(TableHeader, "구분 | 예산(백만원) | 비고"),
        kt(TableRow, "구분: 경마 시행 | 예산(백만원): 1,200 | 비고: 전년 대비 5% 증가"),
        kt(TableRow, "구분: 시설 개선 | 예산(백만원): 300"),
        kt(Paragraph, "글상자 안 문장"),
        kt(Paragraph, "글상자 앞"),
        kt(Paragraph, "맺음말"),
    ]);
}

#[test]
fn pptx_follows_presentation_order() {
    use doc::BlockKind::*;
    let (title, b) = blocks(extract_one(&office("sample.pptx")));
    assert_eq!(title.as_deref(), Some("더비 발표"));
    assert_eq!(b, vec![
        kt(Heading, "더비 운영 계획"),
        kt(Paragraph, "2026년 3월"),
        kt(Heading, "추진 일정"),
        kt(Paragraph, "1단계: 준비\n2단계: 시행"),
        kt(Paragraph, "세부 항목 & 비고"),
        kt(TableHeader, "지역 | 입장객"),
        kt(TableRow, "지역: 서울 | 입장객: 12,000"),
        kt(Heading, "마지막 슬라이드"),
        kt(Paragraph, "감사합니다"),
    ]);
}

#[test]
fn not_a_zip_with_office_extension_is_not_container() {
    let (_d, p) = temp_file("가짜.docx", "그냥 글자".as_bytes());
    assert_eq!(fail_kind(&extract_one(&p)), Some(FailKind::NotContainer));
}

#[test]
fn password_protected_office_is_encrypted() {
    // 암호를 건 DOCX/XLSX/PPTX 는 ZIP 이 아니라 OLE(복합 파일) 안에 EncryptedPackage 로 들어 있다.
    let mut ole = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    ole.extend_from_slice(&[0u8; 504]);
    let (_d, p) = temp_file("암호.xlsx", &ole);
    assert_eq!(fail_kind(&extract_one(&p)), Some(FailKind::Encrypted));
}
```

- [ ] **Step 3: 실패 확인**

Run: `cargo test -p extract --release --test formats`
Expected: 새 시험 FAIL(DOCX/PPTX 가 `Unsupported`).

- [ ] **Step 4: 의존성** — `crates/extract/Cargo.toml` `[dependencies]` 에 추가(rhwp 와 같은 버전이라 중복 빌드 없음)

```toml
zip = { version = "8.6", default-features = false, features = ["deflate"] }
quick-xml = "0.42"
```

- [ ] **Step 5: `crates/extract/src/xmlutil.rs`**

```rust
//! zip + quick-xml 0.42 공용 도우미(DOCX·PPTX·ODF·XLSX 제목).
//! quick-xml 0.42 는 `&amp;` 같은 참조를 Text 와 별도의 `Event::GeneralRef` 로 준다 — 모든 읽기 경로에서 처리해야 글자가 빠지지 않는다.

use crate::FailKind;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use std::io::{Cursor, Read};

pub(crate) type Zip<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

/// XML 파트 하나의 최대 크기(압축 해제 후) — 압축 폭탄 방어.
pub(crate) const MAX_PART: u64 = 64 * 1024 * 1024;

pub(crate) fn open_zip(bytes: &[u8]) -> Result<Zip<'_>, FailKind> {
    zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| FailKind::Corrupt)
}

/// zip 안의 파트를 UTF-8 문자열로 읽는다. 없으면 None, MAX_PART 를 넘으면 TooLarge.
pub(crate) fn read_part(zip: &mut Zip, name: &str) -> Result<Option<String>, FailKind> {
    read_part_capped(zip, name, MAX_PART)
}

pub(crate) fn read_part_capped(zip: &mut Zip, name: &str, cap: u64) -> Result<Option<String>, FailKind> {
    let file = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(_) => return Err(FailKind::Corrupt),
    };
    let mut buf = Vec::new();
    file.take(cap + 1).read_to_end(&mut buf).map_err(|_| FailKind::Corrupt)?;
    if buf.len() as u64 > cap {
        return Err(FailKind::TooLarge);
    }
    let s = String::from_utf8(buf).map_err(|_| FailKind::Corrupt)?;
    Ok(Some(s.strip_prefix('\u{FEFF}').map(str::to_owned).unwrap_or(s)))
}

/// `&amp;` `&#xAC00;` 같은 참조를 글자로 바꿔 붙인다.
pub(crate) fn push_ref(buf: &mut String, r: &BytesRef) {
    match r.resolve_char_ref() {
        Ok(Some(c)) => buf.push(c),
        Ok(None) => {
            if let Some(s) = resolve_predefined_entity(r) {
                buf.push_str(s);
            }
        }
        Err(_) => {}
    }
}

/// 접두사를 무시하고 지역 이름이 `local` 인 속성 값(참조 해제).
pub(crate) fn attr(e: &BytesStart, local: &str) -> Option<String> {
    e.attributes().with_checks(false).flatten().find_map(|a| {
        if a.key.local_name().as_ref() == local {
            a.normalized_value(XmlVersion::Implicit1_0).ok().map(|v| v.into_owned())
        } else {
            None
        }
    })
}

/// 지역 이름이 `local` 인 첫 요소의 글자(core.xml·meta.xml 의 dc:title 용).
pub(crate) fn first_element_text(xml: &str, local: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    let mut inside = false;
    let mut out = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.local_name().as_ref() == local => inside = true,
            Ok(Event::End(e)) if inside && e.local_name().as_ref() == local => break,
            Ok(Event::Text(t)) if inside => out.push_str(&t),
            Ok(Event::GeneralRef(r)) if inside => push_ref(&mut out, &r),
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let t = crate::norm::clean(&out);
    (!t.is_empty()).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn oversized_xml_part_is_capped() {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        w.start_file("big.xml", opts).unwrap();
        w.write_all(&vec![b'a'; 2048]).unwrap();
        let bytes = w.finish().unwrap().into_inner();
        let mut z = open_zip(&bytes).unwrap();
        assert_eq!(read_part_capped(&mut z, "big.xml", 1024), Err(FailKind::TooLarge));
        assert_eq!(read_part_capped(&mut z, "big.xml", 4096).unwrap().map(|s| s.len()), Some(2048));
        assert_eq!(read_part_capped(&mut z, "없음.xml", 4096), Ok(None));
    }

    #[test]
    fn entities_inside_title_are_resolved() {
        let xml = r#"<cp:coreProperties xmlns:cp="x" xmlns:dc="y"><dc:title>A &amp; &#xAC00;</dc:title></cp:coreProperties>"#;
        assert_eq!(first_element_text(xml, "title").as_deref(), Some("A & 가"));
    }
}
```

> 시험에서 `zip::ZipWriter` 를 쓰려면 zip 의 쓰기 기능이 필요하다. `default-features = false` 에서도 `ZipWriter`·`SimpleFileOptions`·`CompressionMethod::Stored` 는 있다(zip 8.6 `src/write.rs`, `types.rs:157` 확인). 컴파일이 안 되면 `[dev-dependencies]` 에 `zip = { version = "8.6", default-features = false, features = ["deflate"] }` 를 다시 적어 본다.

- [ ] **Step 6: `crates/extract/src/ooxml.rs`**

```rust
//! DOCX / PPTX 추출 (zip + quick-xml 0.42). 머리말·꼬리말·각주·메모·발표자 메모는 읽지 않는다.

use crate::norm::push;
use crate::table::push_table;
use crate::xmlutil::{attr, first_element_text, open_zip, push_ref, read_part, Zip};
use crate::FailKind;
use doc::{Block, BlockKind, Document};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::{HashMap, HashSet};

/// 표 하나(칸 안 중첩 표는 스택으로 다룬다).
#[derive(Default)]
struct Table {
    rows: Vec<Vec<String>>,
    row: Option<Vec<String>>,
    cell: Option<String>,
}

impl Table {
    /// 문단 글자를 현재 칸에 붙인다(칸 안 문단은 공백으로 잇는다). 칸이 없으면 false.
    fn append(&mut self, text: &str) -> bool {
        match self.cell.as_mut() {
            Some(c) => {
                if !c.is_empty() && !text.trim().is_empty() {
                    c.push(' ');
                }
                c.push_str(text.trim());
                true
            }
            None => false,
        }
    }
    fn start_row(&mut self) {
        self.row = Some(Vec::new());
    }
    fn start_cell(&mut self) {
        self.cell = Some(String::new());
    }
    fn end_cell(&mut self) {
        let c = self.cell.take().unwrap_or_default();
        self.row.get_or_insert_with(Vec::new).push(c);
    }
    fn end_row(&mut self) {
        if let Some(r) = self.row.take() {
            self.rows.push(r);
        }
    }
}

/// 문단 하나가 끝났을 때: 표 칸 안이면 칸에, 아니면 블록으로.
fn finish_para(out: &mut Vec<Block>, tables: &mut [Table], text: &str, heading: bool) {
    if let Some(t) = tables.last_mut() {
        if t.append(text) {
            return;
        }
    }
    push(out, if heading { BlockKind::Heading } else { BlockKind::Paragraph }, text);
}

/// 표가 끝났을 때: 바깥 표 칸 안(중첩 표)이면 평문으로 칸에, 아니면 표 블록으로.
fn finish_table(out: &mut Vec<Block>, tables: &mut Vec<Table>) {
    let Some(t) = tables.pop() else { return };
    if let Some(outer) = tables.last_mut() {
        let flat: Vec<String> = t.rows.iter().map(|r| r.join(" ")).collect();
        if outer.append(&flat.join(" ")) {
            return;
        }
    }
    push_table(out, &t.rows);
}

// ---------------------------------------------------------------- DOCX

/// styles.xml → 제목 스타일 ID. 이름이 heading*/title/제목* 이거나 outlineLvl 0~8 이면 제목.
/// 한국어 Word 는 "제목 1" 의 styleId 가 "1" 처럼 숫자라 ID 만으로는 판단할 수 없다.
fn heading_styles(xml: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    let mut reader = Reader::from_str(xml);
    let mut cur: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.local_name().as_ref() == "style" => cur = attr(&e, "styleId"),
            Ok(Event::Empty(e)) => {
                let Some(id) = cur.as_ref() else { continue };
                match e.local_name().as_ref() {
                    "name" => {
                        let n = attr(&e, "val").unwrap_or_default().to_lowercase();
                        if n.starts_with("heading") || n == "title" || n.starts_with("제목") {
                            set.insert(id.clone());
                        }
                    }
                    "outlineLvl" => {
                        if attr(&e, "val").and_then(|v| v.parse::<u8>().ok()).is_some_and(|v| v < 9) {
                            set.insert(id.clone());
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == "style" => cur = None,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    set
}

pub(crate) fn docx(bytes: &[u8]) -> Result<Document, FailKind> {
    let mut zip = open_zip(bytes)?;
    let xml = read_part(&mut zip, "word/document.xml")?.ok_or(FailKind::Corrupt)?;
    let headings = read_part(&mut zip, "word/styles.xml")?.map(|s| heading_styles(&s)).unwrap_or_default();
    let title = read_part(&mut zip, "docProps/core.xml")?.and_then(|s| first_element_text(&s, "title"));

    struct Para {
        text: String,
        heading: bool,
    }
    let mut out = Vec::new();
    let mut paras: Vec<Para> = Vec::new(); // 글상자 안 문단은 바깥 문단 안에 중첩된다 → 스택
    let mut tables: Vec<Table> = Vec::new();
    let mut in_t = false; // <w:t> 안
    let mut in_ppr = false; // <w:pPr> 안(탭 정의 w:tab 을 글자 탭과 구분)
    let mut skip = 0usize; // mc:Fallback 깊이(글상자 글 중복 제거)

    let mut reader = Reader::from_str(&xml);
    loop {
        let ev = reader.read_event().map_err(|_| FailKind::Corrupt)?;
        if skip > 0 {
            match ev {
                Event::Start(_) => skip += 1,
                Event::End(_) => skip -= 1,
                Event::Eof => break,
                _ => {}
            }
            continue;
        }
        match ev {
            Event::Start(e) => match e.local_name().as_ref() {
                "Fallback" => skip = 1,
                "p" => paras.push(Para { text: String::new(), heading: false }),
                "pPr" => in_ppr = true,
                "t" => in_t = true,
                "tbl" => tables.push(Table::default()),
                "tr" => {
                    if let Some(t) = tables.last_mut() {
                        t.start_row();
                    }
                }
                "tc" => {
                    if let Some(t) = tables.last_mut() {
                        t.start_cell();
                    }
                }
                _ => {}
            },
            Event::Empty(e) => {
                let Some(p) = paras.last_mut() else { continue };
                match e.local_name().as_ref() {
                    "pStyle" if in_ppr => {
                        if attr(&e, "val").is_some_and(|v| headings.contains(&v)) {
                            p.heading = true;
                        }
                    }
                    "outlineLvl" if in_ppr => {
                        if attr(&e, "val").and_then(|v| v.parse::<u8>().ok()).is_some_and(|v| v < 9) {
                            p.heading = true;
                        }
                    }
                    "tab" if !in_ppr => p.text.push('\t'),
                    "br" | "cr" => p.text.push('\n'),
                    "noBreakHyphen" => p.text.push('-'),
                    _ => {}
                }
            }
            Event::Text(t) if in_t => {
                if let Some(p) = paras.last_mut() {
                    p.text.push_str(&t);
                }
            }
            Event::GeneralRef(r) if in_t => {
                if let Some(p) = paras.last_mut() {
                    push_ref(&mut p.text, &r);
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_t = false,
                "pPr" => in_ppr = false,
                "p" => {
                    if let Some(p) = paras.pop() {
                        finish_para(&mut out, &mut tables, &p.text, p.heading);
                    }
                }
                "tc" => {
                    if let Some(t) = tables.last_mut() {
                        t.end_cell();
                    }
                }
                "tr" => {
                    if let Some(t) = tables.last_mut() {
                        t.end_row();
                    }
                }
                "tbl" => finish_table(&mut out, &mut tables),
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(Document { title, blocks: out })
}

// ---------------------------------------------------------------- PPTX

/// 발표 순서대로 슬라이드 파트 경로: presentation.xml 의 sldIdLst(r:id) → rels 의 Target.
/// 실패하면 ppt/slides/slideN.xml 을 N 숫자 순으로.
fn slide_paths(zip: &mut Zip) -> Result<Vec<String>, FailKind> {
    let mut ordered = Vec::new();
    let pres = read_part(zip, "ppt/presentation.xml")?;
    let rels = read_part(zip, "ppt/_rels/presentation.xml.rels")?;
    if let (Some(pres), Some(rels)) = (pres, rels) {
        let mut targets = HashMap::new();
        let mut r = Reader::from_str(&rels);
        loop {
            match r.read_event() {
                Ok(Event::Empty(e)) | Ok(Event::Start(e)) if e.local_name().as_ref() == "Relationship" => {
                    if let (Some(id), Some(t)) = (attr(&e, "Id"), attr(&e, "Target")) {
                        let path = match t.strip_prefix('/') {
                            Some(abs) => abs.to_string(),
                            None => format!("ppt/{t}"),
                        };
                        targets.insert(id, path);
                    }
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
        let mut r = Reader::from_str(&pres);
        loop {
            match r.read_event() {
                Ok(Event::Empty(e)) | Ok(Event::Start(e)) if e.local_name().as_ref() == "sldId" => {
                    // 속성 "id"(숫자)와 "r:id"(관계)는 지역 이름이 같다 → 접두사 있는 쪽이 관계 ID
                    let rid = e.attributes().with_checks(false).flatten().find_map(|a| {
                        if a.key.local_name().as_ref() == "id" && a.key.prefix().is_some() {
                            Some(a.value.to_string())
                        } else {
                            None
                        }
                    });
                    if let Some(p) = rid.and_then(|id| targets.get(&id).cloned()) {
                        ordered.push(p);
                    }
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
    }
    if ordered.is_empty() {
        let mut numbered: Vec<(u32, String)> = zip
            .file_names()
            .filter_map(|n| {
                let num = n.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?.parse().ok()?;
                Some((num, n.to_string()))
            })
            .collect();
        numbered.sort();
        ordered = numbered.into_iter().map(|(_, n)| n).collect();
    }
    Ok(ordered)
}

/// 슬라이드 하나: 제목 자리표시자(title/ctrTitle) 문단 → Heading, 나머지 → Paragraph, a:tbl → 표.
fn pptx_slide(xml: &str, out: &mut Vec<Block>) -> Result<(), FailKind> {
    let mut title_shape = false;
    let mut para: Option<String> = None;
    let mut in_t = false;
    let mut tables: Vec<Table> = Vec::new();
    let mut title = String::new();
    let mut body = Vec::new();

    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event().map_err(|_| FailKind::Corrupt)? {
            Event::Start(e) => match e.local_name().as_ref() {
                "sp" => title_shape = false,
                "p" => para = Some(String::new()),
                "t" => in_t = true,
                "tbl" => tables.push(Table::default()),
                "tr" => {
                    if let Some(t) = tables.last_mut() {
                        t.start_row();
                    }
                }
                "tc" => {
                    if let Some(t) = tables.last_mut() {
                        t.start_cell();
                    }
                }
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                "ph" => title_shape = matches!(attr(&e, "type").as_deref(), Some("title" | "ctrTitle")),
                "br" => {
                    if let Some(p) = para.as_mut() {
                        p.push('\n');
                    }
                }
                _ => {}
            },
            Event::Text(t) if in_t => {
                if let Some(p) = para.as_mut() {
                    p.push_str(&t);
                }
            }
            Event::GeneralRef(r) if in_t => {
                if let Some(p) = para.as_mut() {
                    push_ref(p, &r);
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_t = false,
                "p" => {
                    let Some(text) = para.take() else { continue };
                    if title_shape && tables.is_empty() {
                        if !title.is_empty() {
                            title.push(' ');
                        }
                        title.push_str(text.trim());
                    } else {
                        finish_para(&mut body, &mut tables, &text, false);
                    }
                }
                "sp" => title_shape = false,
                "tc" => {
                    if let Some(t) = tables.last_mut() {
                        t.end_cell();
                    }
                }
                "tr" => {
                    if let Some(t) = tables.last_mut() {
                        t.end_row();
                    }
                }
                "tbl" => finish_table(&mut body, &mut tables),
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    push(out, BlockKind::Heading, &title);
    out.append(&mut body);
    Ok(())
}

pub(crate) fn pptx(bytes: &[u8]) -> Result<Document, FailKind> {
    let mut zip = open_zip(bytes)?;
    let title = read_part(&mut zip, "docProps/core.xml")?.and_then(|s| first_element_text(&s, "title"));
    let mut out = Vec::new();
    for path in slide_paths(&mut zip)? {
        if let Some(xml) = read_part(&mut zip, &path)? {
            pptx_slide(&xml, &mut out)?;
        }
    }
    Ok(Document { title, blocks: out })
}
```

- [ ] **Step 7: `lib.rs` 연결** — 모듈 선언에 `mod ooxml;`, `pub(crate) mod xmlutil;` 추가. `extract_bytes` 의 `match format` 앞뒤를 다음으로 바꾼다(ZIP 형식 공통 검사 포함):

```rust
    use magic::{sniff, Container};
    let zip_based = matches!(format, Format::Docx | Format::Pptx | Format::Xlsx | Format::Odt | Format::Odp | Format::Ods);
    if zip_based {
        match sniff(bytes) {
            Container::Zip => {}
            // 암호를 건 OOXML 은 OLE 복합 파일 안의 EncryptedPackage 로 저장된다.
            Container::Ole if matches!(format, Format::Docx | Format::Pptx | Format::Xlsx) => {
                return Err((FailKind::Encrypted, magic::magic_hex(bytes)))
            }
            _ => return Err((FailKind::NotContainer, magic::magic_hex(bytes))),
        }
    }
    let plain = |r: Result<doc::Document, FailKind>| {
        r.map(|doc| Extracted { doc, distribution: false, stats: Stats::default() }).map_err(|k| (k, magic::magic_hex(bytes)))
    };
    match format {
        Format::Hwp | Format::Hwpx => hwp::extract_hwp(bytes),
        Format::Docx => plain(ooxml::docx(bytes)),
        Format::Pptx => plain(ooxml::pptx(bytes)),
        // 나머지 형식은 이후 Task 에서 채운다.
        _ => Err((FailKind::Unsupported, String::new())),
    }
```

- [ ] **Step 8: 통과 확인**

Run: `cargo test -p extract --release`
Expected: PASS(`docx_headings_tables_textbox_once`, `pptx_follows_presentation_order`, `not_a_zip_with_office_extension_is_not_container`, `password_protected_office_is_encrypted`, xmlutil 단위 시험 2개 포함). `Cargo.lock` 에 quick-xml·zip 새 버전이 생기지 않았는지(`grep -c 'name = "quick-xml"' Cargo.lock` = 1).

- [ ] **Step 9: Commit**

```bash
git add tools/fixtures/make_office.py crates/extract Cargo.lock
git commit -m "extract: DOCX and PPTX extraction with synthetic office fixtures

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: ODT·ODP 추출

**Files:**
- Create: `crates/extract/src/odf.rs`
- Modify: `crates/extract/src/lib.rs`, `crates/extract/tests/formats.rs`

**Interfaces:**
- Consumes: `xmlutil::{open_zip, read_part, attr, push_ref, first_element_text}`(Task 4), `norm::push`, `table::push_table`
- Produces: `pub(crate) fn odf(bytes: &[u8]) -> Result<doc::Document, FailKind>`(ODT·ODP 공용, Task 6 의 ODS 는 calamine)

- [ ] **Step 1: 실패하는 시험** — `formats.rs` 에 추가

```rust
#[test]
fn odt_headings_spaces_footnote_table() {
    use doc::BlockKind::*;
    let (title, b) = blocks(extract_one(&office("sample.odt")));
    assert_eq!(title.as_deref(), Some("규정 개정안"));
    assert_eq!(b, vec![
        kt(Heading, "제1장 총칙"),
        kt(Paragraph, "목적 및 범위 탭 뒤\n다음 줄"),
        kt(Paragraph, "각주가 있는 문장 각주 본문 이어지는 글"),
        kt(Paragraph, "목록 항목 & 하나"),
        kt(TableHeader, "구분 | 수량 | 비고"),
        kt(TableRow, "구분: 말 | 수량: 12"),
        kt(TableRow, "구분: 합쳐진 칸 | 비고: 끝"),
        kt(Paragraph, "맺음"),
    ]);
}

#[test]
fn odp_titles_are_headings_and_notes_are_skipped() {
    use doc::BlockKind::*;
    let (title, b) = blocks(extract_one(&office("sample.odp")));
    assert_eq!(title.as_deref(), Some("발표 자료"));
    assert_eq!(b, vec![
        kt(Heading, "발표 제목"),
        kt(Paragraph, "부제목"),
        kt(Heading, "둘째 장"),
        kt(Paragraph, "요점 하나"),
        kt(Paragraph, "요점 둘"),
    ]);
}
```

- [ ] **Step 2: 실패 확인** — Run: `cargo test -p extract --release --test formats odt_ odp_` → FAIL(`Unsupported`)

- [ ] **Step 3: `crates/extract/src/odf.rs`**

```rust
//! ODT / ODP 추출 (content.xml + meta.xml). 발표자 메모·메모(주석)·변경 추적 삭제 글·각주 번호는 뺀다.

use crate::norm::push;
use crate::table::push_table;
use crate::xmlutil::{attr, first_element_text, open_zip, push_ref, read_part};
use crate::FailKind;
use doc::{BlockKind, Document};
use quick_xml::events::Event;
use quick_xml::Reader;

#[derive(Default)]
struct Table {
    rows: Vec<Vec<String>>,
    row: Option<Vec<String>>,
    cell: Option<String>,
}

/// 반복 칸(number-columns-repeated)을 펼치는 최대 개수 — 빈 칸 1,024개 같은 것을 막는다.
const MAX_REPEAT: usize = 64;

pub(crate) fn odf(bytes: &[u8]) -> Result<Document, FailKind> {
    let mut zip = open_zip(bytes)?;
    let xml = read_part(&mut zip, "content.xml")?.ok_or(FailKind::Corrupt)?;
    let title = read_part(&mut zip, "meta.xml")?.and_then(|s| first_element_text(&s, "title"));

    struct Para {
        text: String,
        heading: bool,
    }
    let mut out = Vec::new();
    let mut paras: Vec<Para> = Vec::new(); // 각주 본문 문단은 바깥 문단 안에 중첩 → 스택
    let mut tables: Vec<Table> = Vec::new();
    let mut skip = 0usize;
    let mut title_frame = false; // ODP: presentation:class="title" 틀 안
    let mut repeat = 1usize; // 현재 칸의 number-columns-repeated

    let mut reader = Reader::from_str(&xml);
    loop {
        let ev = reader.read_event().map_err(|_| FailKind::Corrupt)?;
        if skip > 0 {
            match ev {
                Event::Start(_) => skip += 1,
                Event::End(_) => skip -= 1,
                Event::Eof => break,
                _ => {}
            }
            continue;
        }
        match ev {
            Event::Start(e) => match e.local_name().as_ref() {
                "notes" | "annotation" | "tracked-changes" | "note-citation" => skip = 1,
                "h" => paras.push(Para { text: String::new(), heading: true }),
                "p" => paras.push(Para { text: String::new(), heading: title_frame }),
                "frame" => title_frame = attr(&e, "class").as_deref() == Some("title"),
                "table" => tables.push(Table::default()),
                "table-row" => {
                    if let Some(t) = tables.last_mut() {
                        t.row = Some(Vec::new());
                    }
                }
                "table-cell" => {
                    if let Some(t) = tables.last_mut() {
                        t.cell = Some(String::new());
                        repeat = attr(&e, "number-columns-repeated").and_then(|v| v.parse().ok()).unwrap_or(1);
                    }
                }
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                "s" => {
                    if let Some(p) = paras.last_mut() {
                        let n = attr(&e, "c").and_then(|v| v.parse().ok()).unwrap_or(1usize);
                        p.text.extend(std::iter::repeat_n(' ', n.min(1000)));
                    }
                }
                "tab" => {
                    if let Some(p) = paras.last_mut() {
                        p.text.push('\t');
                    }
                }
                "line-break" => {
                    if let Some(p) = paras.last_mut() {
                        p.text.push('\n');
                    }
                }
                // 빈 칸·병합으로 가려진 칸도 열 위치를 지키려고 빈 문자열로 넣는다.
                "table-cell" | "covered-table-cell" => {
                    if let Some(row) = tables.last_mut().and_then(|t| t.row.as_mut()) {
                        let n = attr(&e, "number-columns-repeated").and_then(|v| v.parse().ok()).unwrap_or(1usize);
                        row.extend(std::iter::repeat_n(String::new(), n.min(MAX_REPEAT)));
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if let Some(p) = paras.last_mut() {
                    p.text.push_str(&t);
                }
            }
            Event::GeneralRef(r) => {
                if let Some(p) = paras.last_mut() {
                    push_ref(&mut p.text, &r);
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                "h" | "p" => {
                    let Some(p) = paras.pop() else { continue };
                    if let Some(cell) = tables.last_mut().and_then(|t| t.cell.as_mut()) {
                        if !cell.is_empty() {
                            cell.push(' ');
                        }
                        cell.push_str(p.text.trim());
                    } else if let Some(outer) = paras.last_mut() {
                        // 각주 본문 등 중첩 문단: 바깥 문단 뒤에 붙인다.
                        outer.text.push(' ');
                        outer.text.push_str(&p.text);
                    } else {
                        push(&mut out, if p.heading { BlockKind::Heading } else { BlockKind::Paragraph }, &p.text);
                    }
                }
                "frame" => title_frame = false,
                "table-cell" => {
                    if let Some(t) = tables.last_mut() {
                        let c = t.cell.take().unwrap_or_default();
                        t.row.get_or_insert_with(Vec::new).extend(std::iter::repeat_n(c, repeat.min(MAX_REPEAT)));
                        repeat = 1;
                    }
                }
                "table-row" => {
                    if let Some(t) = tables.last_mut() {
                        if let Some(r) = t.row.take() {
                            t.rows.push(r);
                        }
                    }
                }
                "table" => {
                    let Some(t) = tables.pop() else { continue };
                    let flat = t.rows.iter().map(|r| r.join(" ")).collect::<Vec<_>>().join(" ");
                    if let Some(cell) = tables.last_mut().and_then(|o| o.cell.as_mut()) {
                        cell.push(' ');
                        cell.push_str(&flat);
                    } else {
                        push_table(&mut out, &t.rows);
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(Document { title, blocks: out })
}
```

- [ ] **Step 4: `lib.rs` 연결** — `mod odf;` 추가, `match format` 에 `Format::Odt | Format::Odp => plain(odf::odf(bytes)),`

- [ ] **Step 5: 통과 확인** — Run: `cargo test -p extract --release` → PASS

- [ ] **Step 6: Commit**

```bash
git add crates/extract
git commit -m "extract: ODT and ODP extraction

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---
### Task 6: XLSX·ODS 추출 (calamine)

**Files:**
- Create: `crates/extract/src/sheet.rs`
- Modify: `crates/extract/Cargo.toml`, `crates/extract/src/lib.rs`, `crates/extract/tests/formats.rs`

**Interfaces:**
- Consumes: `table::push_table`, `norm::push`, `xmlutil::{open_zip, read_part, first_element_text}`
- Produces: `pub(crate) fn sheet(bytes: &[u8], ods: bool) -> Result<doc::Document, FailKind>` — 시트마다 `Heading(시트 이름)` + 표 블록. 빈 시트는 건너뛴다. 시트당 최대 `MAX_ROWS_PER_SHEET`(50,000)행.

- [ ] **Step 1: 실패하는 시험** — `formats.rs` 에 추가

```rust
#[test]
fn xlsx_sheets_dates_bools_formulas() {
    use doc::BlockKind::*;
    let (title, b) = blocks(extract_one(&office("sample.xlsx")));
    assert_eq!(title.as_deref(), Some("예산 집계"));
    assert_eq!(b, vec![
        kt(Heading, "예산"),
        kt(TableHeader, "구분 | 금액 | 일자 | 비고"),
        kt(TableRow, "구분: 경마 시행 | 금액: 1200 | 일자: 2026-03-01 | 비고: 인라인 문자열"),
        kt(TableRow, "구분: 시설 개선 | 금액: 300.25 | 비고: TRUE"),
        kt(TableRow, "금액: 1500.25"),
    ]);
}

#[test]
fn huge_repeated_rows_are_cheap() {
    use doc::BlockKind::*;
    // sample.ods 끝에는 1,048,573행 × 1,024칸 반복 빈 행이 있다(LibreOffice 파일에 흔함).
    let t = std::time::Instant::now();
    let (title, b) = blocks(extract_one(&office("sample.ods")));
    assert!(t.elapsed() < std::time::Duration::from_secs(5), "{:?}", t.elapsed());
    assert_eq!(title.as_deref(), Some("예산표"));
    assert_eq!(b, vec![
        kt(Heading, "예산"),
        kt(TableHeader, "구분 | 금액 | 일자"),
        kt(TableRow, "구분: 경마 | 금액: 1200 | 일자: 2026-03-01"),
        kt(TableRow, "구분: 시설 | 금액: 0.5"),
    ]);
}
```

- [ ] **Step 2: 실패 확인** — Run: `cargo test -p extract --release --test formats xlsx_ huge_` → FAIL

- [ ] **Step 3: 의존성** — `crates/extract/Cargo.toml`: `calamine = { version = "0.36.1", default-features = false }`
  (calamine 0.36.1 은 quick-xml 0.41 을 따로 끌어온다 — cargo-deny 는 중복 경고만 낸다. calamine 이 0.42 로 올라가면 정리한다.)

- [ ] **Step 4: `crates/extract/src/sheet.rs`**

```rust
//! XLSX / ODS 추출 (calamine). 셀 값은 수식 결과(저장된 값)를 쓴다. 옛 .xls 는 범위 밖(스펙 §16).

use crate::norm::push;
use crate::table::push_table;
use crate::xmlutil::{first_element_text, open_zip, read_part};
use crate::FailKind;
use calamine::{open_workbook_from_rs, Data, Ods, Reader, Xlsx};
use doc::{Block, BlockKind, Document};
use std::io::Cursor;

/// 시트 하나에서 읽는 최대 행 수 — 거대한 데이터 시트가 키워드 색인을 채우지 않도록(DEC 기록).
pub(crate) const MAX_ROWS_PER_SHEET: usize = 50_000;

/// 셀 값을 검색용 문자열로.
fn cell(d: &Data) -> String {
    match d {
        Data::Empty | Data::Error(_) => String::new(),
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
        Data::Float(f) => f.to_string(),
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        // chrono 기능 없이 변환(1900/1904 기준·1900 윤년 버그 처리 포함)
        Data::DateTime(dt) if dt.is_datetime() => {
            let (y, m, d, hh, mm, ss, _) = dt.to_ymd_hms_milli();
            if (hh, mm, ss) == (0, 0, 0) {
                format!("{y:04}-{m:02}-{d:02}")
            } else {
                format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}")
            }
        }
        Data::DateTime(dt) => dt.as_f64().to_string(), // 기간(duration)
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
    }
}

fn read_sheets<R, RS>(wb: &mut R, out: &mut Vec<Block>) -> Result<(), FailKind>
where
    R: Reader<RS>,
    RS: std::io::Read + std::io::Seek,
{
    for name in wb.sheet_names() {
        let range = wb.worksheet_range(&name).map_err(|_| FailKind::Corrupt)?;
        let rows: Vec<Vec<String>> = range.rows().take(MAX_ROWS_PER_SHEET).map(|r| r.iter().map(cell).collect()).collect();
        if rows.iter().all(|r| r.iter().all(String::is_empty)) {
            continue; // 빈 시트는 이름도 내지 않는다
        }
        push(out, BlockKind::Heading, &name);
        push_table(out, &rows);
    }
    Ok(())
}

pub(crate) fn sheet(bytes: &[u8], ods: bool) -> Result<Document, FailKind> {
    let mut out = Vec::new();
    if ods {
        let mut wb: Ods<_> = open_workbook_from_rs(Cursor::new(bytes)).map_err(|_| FailKind::Corrupt)?;
        read_sheets(&mut wb, &mut out)?;
    } else {
        let mut wb: Xlsx<_> = open_workbook_from_rs(Cursor::new(bytes)).map_err(|_| FailKind::Corrupt)?;
        read_sheets(&mut wb, &mut out)?;
    }
    // calamine 은 문서 속성을 주지 않으므로 제목은 직접 읽는다.
    let mut zip = open_zip(bytes)?;
    let title = read_part(&mut zip, if ods { "meta.xml" } else { "docProps/core.xml" })?.and_then(|s| first_element_text(&s, "title"));
    Ok(Document { title, blocks: out })
}
```

- [ ] **Step 5: `lib.rs` 연결** — `mod sheet;`, `match format` 에 `Format::Xlsx => plain(sheet::sheet(bytes, false)),`, `Format::Ods => plain(sheet::sheet(bytes, true)),`

- [ ] **Step 6: 통과 확인** — Run: `cargo test -p extract --release` → PASS

- [ ] **Step 7: Commit**

```bash
git add crates/extract Cargo.lock
git commit -m "extract: XLSX and ODS extraction via calamine

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: TXT·CSV 추출 (인코딩 감지)

**Files:**
- Create: `crates/extract/src/text.rs`
- Modify: `crates/extract/Cargo.toml`, `crates/extract/src/lib.rs`, `crates/extract/tests/formats.rs`

**Interfaces:**
- Produces: `pub(crate) fn decode(bytes: &[u8]) -> String`(BOM → BOM 없는 UTF-16 → UTF-8 → chardetng(kr 기준) 순), `pub(crate) fn txt(bytes: &[u8]) -> Document`(빈 줄로 문단 나눔), `pub(crate) fn csv(bytes: &[u8]) -> Result<Document, FailKind>`(첫 표 머리글은 `push_table` 규칙)

- [ ] **Step 1: 실패하는 시험** — `formats.rs` 에 추가

```rust
#[test]
fn txt_encodings_are_detected() {
    let ko = "한국마사회 경마 시행 계획\n렛츠런파크 서울 운영 방안을 보고합니다.";
    for f in ["utf8.txt", "utf8_bom.txt", "cp949.txt", "utf16le_bom.txt", "utf16be_bom.txt", "utf16le_nobom.txt"] {
        let (_, b) = blocks(extract_one(&office(f)));
        assert_eq!(b, vec![kt(doc::BlockKind::Paragraph, ko)], "{f}");
    }
    assert_eq!(blocks(extract_one(&office("cp949_short.txt"))).1, vec![kt(doc::BlockKind::Paragraph, "회의록")]);
    assert_eq!(blocks(extract_one(&office("cp949_ext.txt"))).1, vec![kt(doc::BlockKind::Paragraph, "똠방각하 펲뷁 샾")]);
    assert_eq!(blocks(extract_one(&office("ascii.txt"))).1, vec![kt(doc::BlockKind::Paragraph, "plain ascii only")]);
}

#[test]
fn csv_quotes_and_multiline_cells() {
    use doc::BlockKind::*;
    let (_, b) = blocks(extract_one(&office("cp949.csv")));
    assert_eq!(b, vec![
        kt(TableHeader, "구분 | 금액 | 비고"),
        kt(TableRow, "구분: 경마 시행 | 금액: 1,200 | 비고: 따옴표 \"안\" 글"),
        kt(TableRow, "구분: 시설 개선 | 금액: 300"),
        kt(TableRow, "구분: 여러 줄 칸 | 금액: 5 | 비고: 끝"),
    ]);
}

#[test]
fn blank_lines_split_txt_paragraphs() {
    let (_d, p) = temp_file("메모.txt", "첫 문단 첫 줄\n첫 문단 둘째 줄\n\n\n둘째 문단".as_bytes());
    let (_, b) = blocks(extract_one(&p));
    assert_eq!(b, vec![kt(doc::BlockKind::Paragraph, "첫 문단 첫 줄\n첫 문단 둘째 줄"), kt(doc::BlockKind::Paragraph, "둘째 문단")]);
}
```

- [ ] **Step 2: 실패 확인** — Run: `cargo test -p extract --release --test formats txt_ csv_ blank_` → FAIL

- [ ] **Step 3: 의존성** — `crates/extract/Cargo.toml`:

```toml
chardetng = "1.0.0"
encoding_rs = "0.8.42"
csv = "1.4.0"
```

- [ ] **Step 4: `crates/extract/src/text.rs`**

```rust
//! TXT / CSV: 인코딩 감지(BOM → BOM 없는 UTF-16 → UTF-8 → 레거시 추정) + csv 파싱.

use crate::norm::push;
use crate::table::push_table;
use crate::FailKind;
use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use doc::{BlockKind, Document};
use encoding_rs::Encoding;

/// 바이트 → 문자열. 실패해도 대체 문자(U+FFFD)로 채워 돌려준다.
pub(crate) fn decode(bytes: &[u8]) -> String {
    // 1) BOM(UTF-8 / UTF-16LE / UTF-16BE)
    if let Some((enc, bom_len)) = Encoding::for_bom(bytes) {
        return enc.decode_without_bom_handling(&bytes[bom_len..]).0.into_owned();
    }
    // 2) BOM 없는 UTF-16: 텍스트에는 NUL 바이트가 없어야 정상 → 있으면 NUL 위치(짝/홀)로 LE/BE 를 정한다.
    //    (ASCII 만 든 UTF-16 은 올바른 UTF-8 이기도 해서 UTF-8 검사보다 먼저 한다.
    //     공백·숫자 없이 한글만 있는 UTF-16 은 NUL 이 없어 여기서 못 잡는다 — 메모장은 항상 BOM 을 쓴다.)
    let zeros_even = bytes.iter().step_by(2).filter(|&&b| b == 0).count();
    let zeros_odd = bytes.iter().skip(1).step_by(2).filter(|&&b| b == 0).count();
    if bytes.len() % 2 == 0 && zeros_even + zeros_odd > 0 {
        let enc = if zeros_odd >= zeros_even { encoding_rs::UTF_16LE } else { encoding_rs::UTF_16BE };
        let (s, had_errors) = enc.decode_without_bom_handling(bytes);
        if !had_errors {
            return s.into_owned();
        }
    }
    // 3) 올바른 UTF-8 이면 그대로(ASCII 포함)
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_owned();
    }
    // 4) 레거시 인코딩 추정. tld "kr" 은 EUC-KR(=CP949) 쪽으로 기운다. tld 는 반드시 소문자 ASCII 상수(대문자면 chardetng 패닉).
    let mut det = EncodingDetector::new(Iso2022JpDetection::Deny);
    det.feed(bytes, true);
    let enc = det.guess(Some(b"kr"), Utf8Detection::Allow);
    enc.decode_without_bom_handling(bytes).0.into_owned()
}

/// 빈 줄로 문단을 나눈다.
pub(crate) fn txt(bytes: &[u8]) -> Document {
    let s = decode(bytes).replace("\r\n", "\n");
    let mut out = Vec::new();
    let mut para = String::new();
    for line in s.split('\n') {
        if line.trim().is_empty() {
            push(&mut out, BlockKind::Paragraph, &para);
            para.clear();
        } else {
            para.push_str(line);
            para.push('\n');
        }
    }
    push(&mut out, BlockKind::Paragraph, &para);
    Document { title: None, blocks: out }
}

pub(crate) fn csv(bytes: &[u8]) -> Result<Document, FailKind> {
    let s = decode(bytes);
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false) // 머리글 판단은 push_table 이 한다
        .flexible(true) // 행마다 칸 수가 달라도 허용
        .from_reader(s.as_bytes());
    let mut rows = Vec::new();
    for rec in rdr.records().take(crate::sheet::MAX_ROWS_PER_SHEET) {
        let rec = rec.map_err(|_| FailKind::Corrupt)?;
        rows.push(rec.iter().map(str::to_owned).collect::<Vec<_>>());
    }
    let mut out = Vec::new();
    push_table(&mut out, &rows);
    Ok(Document { title: None, blocks: out })
}
```

- [ ] **Step 5: `lib.rs` 연결** — `mod text;`, `match format` 에 `Format::Txt => plain(Ok(text::txt(bytes))),`, `Format::Csv => plain(text::csv(bytes)),`

- [ ] **Step 6: 통과 확인** — Run: `cargo test -p extract --release` → PASS

- [ ] **Step 7: Commit**

```bash
git add crates/extract Cargo.lock
git commit -m "extract: TXT and CSV extraction with UTF-8/CP949/UTF-16 detection

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: PDF 추출 (pdfium, 명시 경로 로드, 글자 깨짐 감지) (KI-014)

**Files:**
- Create: `tools/fetch-pdfium.sh`, `tools/fixtures/make_pdf.py`, `crates/extract/tests/fixtures/pdf/{korean.pdf, mixed.pdf, garbled.pdf, image_only.pdf, no_title.pdf}`, `crates/extract/src/pdf.rs`
- Modify: `crates/extract/Cargo.toml`, `crates/extract/src/lib.rs`, `crates/extract/tests/formats.rs`, `crates/extract/tests/fixtures/README.md`, `.github/workflows/ci.yml`, `CLAUDE.md`(개발 명령), `docs/engineering/known-issues.md`

**Interfaces:**
- Produces:

```rust
// extract::pdf
/// pdfium 라이브러리를 **절대 경로**로 불러온다. 프로세스당 한 번(두 번째부터는 Ok). 오류 문장에 경로를 넣지 않는다.
pub fn init(lib: &std::path::Path) -> Result<(), String>;
/// 이 플랫폼의 pdfium 파일 이름(pdfium.dll / libpdfium.dylib / libpdfium.so).
pub const LIB_NAME: &str;
pub(crate) fn pdf(bytes: &[u8]) -> Result<(doc::Document, crate::Stats), FailKind>;  // init 전이면 Unsupported
```

- 쪽마다 글자가 충분하고 깨지지 않았으면 `Paragraph`(줄은 `\n` 유지). 비었거나 깨진 쪽은 버리고 `Stats.pdf_bad_pages` 에 센다. 남은 쪽이 없으면 `FailKind::NoText`(파일명만 색인, 스펙 §6.5 `no_text_layer`).

- [ ] **Step 1: `tools/fetch-pdfium.sh`**

```bash
#!/usr/bin/env bash
# 개발·CI·릴리즈 전용: bblanchon/pdfium-binaries 의 고정 태그를 third_party/pdfium/<플랫폼>/ 에 받는다.
# 실행 시(제품)에는 네트워크를 쓰지 않는다 — 이 스크립트가 받은 라이브러리를 실행 파일 옆에 동봉한다.
# 사용: bash tools/fetch-pdfium.sh [플랫폼...]   (생략 시 현재 호스트 플랫폼; all = 전부)
# 태그를 바꾸면 SHA-256 4개를 같이 바꾸고 pdfium-render 의 pdfium_XXXX 기능도 맞춘다(runbooks/upgrade-lindera-pdfium.md).
set -euo pipefail
export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL="*"
TAG="chromium/7881"   # pdfium-render 0.9.4 의 pdfium_7881 과 맞춤
ROOT="${PDFIUM_DIR:-$(cd "$(dirname "$0")/.." && pwd)/third_party/pdfium}"

sha_of() { (sha256sum "$1" 2>/dev/null || shasum -a 256 "$1") | cut -d' ' -f1; }

asset_sha() {
  case "$1" in
    mac-arm64) echo 52e94ca5aa8847934330daf3f8150c190682c5ca93831468794f8b90d4392e40 ;;
    mac-x64)   echo 6dedf83990e0e3d6b7c93c9e7589c5a126b0ae14b7464d76120cff7a26afb18b ;;
    win-x64)   echo 73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08 ;;
    linux-x64) echo 1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d ;;
    *) echo "알 수 없는 플랫폼: $1" >&2; exit 1 ;;
  esac
}

lib_path() {
  case "$1" in
    mac-*)   echo lib/libpdfium.dylib ;;
    win-*)   echo bin/pdfium.dll ;;
    linux-*) echo lib/libpdfium.so ;;
  esac
}

host_platform() {
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) echo mac-arm64 ;;
    Darwin-x86_64) echo mac-x64 ;;
    MINGW*-x86_64|MSYS*-x86_64|CYGWIN*-x86_64) echo win-x64 ;;
    Linux-x86_64) echo linux-x64 ;;
    *) echo "지원하지 않는 호스트: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
  esac
}

fetch() {
  local p="$1" want dir tgz got
  want="$(asset_sha "$p")"
  dir="$ROOT/$p"
  if [ -f "$dir/.tag" ] && [ "$(cat "$dir/.tag")" = "$TAG $want" ] && [ -f "$dir/$(lib_path "$p")" ]; then
    echo "pdfium $TAG $p already present"; return
  fi
  rm -rf "$dir"; mkdir -p "$dir"
  tgz="$dir/pdfium-$p.tgz"
  curl -sSfL -o "$tgz.part" "https://github.com/bblanchon/pdfium-binaries/releases/download/$TAG/pdfium-$p.tgz"
  mv "$tgz.part" "$tgz"
  got="$(sha_of "$tgz")"
  if [ "$got" != "$want" ]; then
    echo "SHA-256 불일치: pdfium-$p.tgz ($got, 기대 $want)" >&2
    rm -f "$tgz"; exit 1
  fi
  # 라이브러리·라이선스만 푼다(헤더·cmake 설정은 쓰지 않음).
  tar -xzf "$tgz" -C "$dir" "$(lib_path "$p")" LICENSE VERSION licenses
  rm -f "$tgz"
  echo "$TAG $want" > "$dir/.tag"
  echo "$want  pdfium-$p.tgz → $dir/$(lib_path "$p")"
}

PLATFORMS=("$@")
[ ${#PLATFORMS[@]} -eq 0 ] && PLATFORMS=("$(host_platform)")
[ "${PLATFORMS[0]}" = all ] && PLATFORMS=(mac-arm64 mac-x64 win-x64 linux-x64)
for p in "${PLATFORMS[@]}"; do fetch "$p"; done
```

Run: `bash tools/fetch-pdfium.sh` → `third_party/pdfium/<호스트>/lib/libpdfium.dylib`(또는 `bin/pdfium.dll`) 생성, SHA 일치 출력.

- [ ] **Step 2: PDF 시험 자료** — `tools/fixtures/make_pdf.py` (상용 글꼴을 임베드하지 않는다: 한글은 비임베드 CID 글꼴, 깨짐 흉내는 Helvetica)

```python
# 시험용 PDF 생성기 — 내용은 모두 합성. 글꼴을 임베드하지 않는다(라이선스 문제 없음).
# 사용: uv run --with reportlab==5.0.1 --with pillow python -I tools/fixtures/make_pdf.py crates/extract/tests/fixtures/pdf
import os, sys
from reportlab.pdfgen import canvas
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.cidfonts import UnicodeCIDFont
from reportlab.lib.utils import ImageReader
from PIL import Image

out = sys.argv[1]
os.makedirs(out, exist_ok=True)
pdfmetrics.registerFont(UnicodeCIDFont("HYGothic-Medium"))  # 비임베드 CID 글꼴(Adobe-Korea1) — pdfium 이 유니코드로 풀어 준다

KOREAN = [
    ["한국마사회 경마 시행 계획", "2026년 렛츠런파크 서울 운영 방안을 보고합니다.", "Mixed English 텍스트 123"],
    ["제2장 예산", "총 사업비는 1,234백만원이며 세부 내역은 다음과 같다."],
]
# ToUnicode 없는 한글 글꼴을 pdfium 이 읽으면 이런 Latin-1 글자가 나온다(조사 2026-10-14) — 같은 모양을 Helvetica 로 흉내.
GARBLED = ["ÄÅÇÉ ÑÖÜá àâäãåç éèêëíìîï", "ñóòôöõ úùûü ÀÁÂÃ ÈÉÊË ÌÍÎÏ"]

def page(c, font, lines):
    y = 780
    for ln in lines:
        c.setFont(font, 14); c.drawString(72, y, ln); y -= 24
    c.showPage()

def new(name, title):
    c = canvas.Canvas(os.path.join(out, name), invariant=1)  # invariant: 시각·ID 고정 → 같은 바이트
    if title:
        c.setTitle(title)
    return c

c = new("korean.pdf", "마사회 검색 시험")
for lines in KOREAN:
    page(c, "HYGothic-Medium", lines)
c.save()

c = new("mixed.pdf", "섞인 문서")
page(c, "HYGothic-Medium", KOREAN[0])
page(c, "Helvetica", GARBLED)
c.save()

c = new("garbled.pdf", "깨진 문서")
page(c, "Helvetica", GARBLED)
c.save()

img = Image.new("RGB", (200, 100), (255, 255, 255))
for x in range(200):
    img.putpixel((x, 50), (0, 0, 0))
c = new("image_only.pdf", "스캔 문서")
c.drawImage(ImageReader(img), 72, 600, 400, 200)
c.showPage(); c.save()

c = new("no_title.pdf", None)  # reportlab 은 제목 자리에 "untitled" 를 쓴다
page(c, "HYGothic-Medium", ["제목 없는 문서 본문"])
c.save()
print("ok")
```

```bash
uv run --with reportlab==5.0.1 --with pillow python -I tools/fixtures/make_pdf.py crates/extract/tests/fixtures/pdf
ls -la crates/extract/tests/fixtures/pdf   # 5개, 각각 수 KB
```

`fixtures/README.md` 끝에 추가:

```markdown
## PDF (`pdf/`)
`tools/fixtures/make_pdf.py`(reportlab 5.0.1) 로 만든 합성 문서. 글꼴을 임베드하지 않는다 — 한글은 비임베드 CID 글꼴 HYGothic-Medium, "깨진 글자"는 Helvetica 로 Latin-1 문자를 찍어 흉내 냈다.
```

- [ ] **Step 3: 실패하는 시험** — `formats.rs` 에 추가

```rust
/// 이 호스트의 pdfium(tools/fetch-pdfium.sh 가 받은 것)을 한 번 불러온다.
fn pdfium() {
    let plat = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "mac-arm64/lib",
        ("macos", "x86_64") => "mac-x64/lib",
        ("windows", "x86_64") => "win-x64/bin",
        ("linux", "x86_64") => "linux-x64/lib",
        other => panic!("pdfium 플랫폼 없음: {other:?}"),
    };
    let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/pdfium").join(plat).join(extract::pdf::LIB_NAME);
    assert!(lib.exists(), "먼저 `bash tools/fetch-pdfium.sh` 실행: {}", lib.display());
    extract::pdf::init(&lib.canonicalize().unwrap()).unwrap();
}

fn pdf_fx(name: &str) -> PathBuf {
    fx(&format!("pdf/{name}"))
}

#[test]
fn pdf_korean_pages_and_title() {
    pdfium();
    match extract_one(&pdf_fx("korean.pdf")) {
        Outcome::Ok { doc, stats, .. } => {
            assert_eq!(doc.title.as_deref(), Some("마사회 검색 시험"));
            assert_eq!((stats.pdf_pages, stats.pdf_bad_pages), (2, 0));
            assert_eq!(doc.blocks.len(), 2);
            assert!(doc.blocks[0].text.contains("렛츠런파크 서울 운영 방안"), "{:?}", doc.blocks);
            assert!(doc.blocks[1].text.contains("1,234백만원"), "{:?}", doc.blocks);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn pdf_garbled_and_image_only_are_no_text() {
    pdfium();
    assert_eq!(fail_kind(&extract_one(&pdf_fx("garbled.pdf"))), Some(FailKind::NoText));
    assert_eq!(fail_kind(&extract_one(&pdf_fx("image_only.pdf"))), Some(FailKind::NoText));
}

#[test]
fn pdf_bad_pages_are_dropped_and_counted() {
    pdfium();
    match extract_one(&pdf_fx("mixed.pdf")) {
        Outcome::Ok { doc, stats, .. } => {
            assert_eq!((stats.pdf_pages, stats.pdf_bad_pages), (2, 1));
            assert_eq!(doc.blocks.len(), 1);
            assert!(!doc.blocks[0].text.contains('Ä'));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn pdf_placeholder_title_is_ignored() {
    pdfium();
    match extract_one(&pdf_fx("no_title.pdf")) {
        Outcome::Ok { doc, .. } => assert_eq!(doc.title, None),
        other => panic!("{other:?}"),
    }
}
```

- [ ] **Step 4: 실패 확인** — Run: `cargo test -p extract --release --test formats pdf_` → 컴파일 실패(`extract::pdf` 없음)

- [ ] **Step 5: 의존성** — `crates/extract/Cargo.toml`:

```toml
# 동적 로드(실행 파일 옆 DLL 을 절대 경로로) — `static`·`bindings`·`pdfium_use_win32` 기능은 켜지 않는다.
# 기본 기능을 끄면 image 크레이트가 빠진다. thread_safe 는 Pdfium 이 Send 가 되려면 필요(병렬 처리는 하지 않는다).
pdfium-render = { version = "=0.9.4", default-features = false, features = ["pdfium_7881", "thread_safe"] }
```

- [ ] **Step 6: `crates/extract/src/pdf.rs`**

```rust
//! PDF 추출 — pdfium(bblanchon/pdfium-binaries)을 실행 파일 옆에서 **절대 경로**로 불러온다(스펙 §7.6).
//! pdfium 은 여러 스레드에서 동시에 쓰면 문서 열기가 무작위로 실패한다(조사 2026-10-14) → 문서 하나 전체를 뮤텍스로 감싼다.
//! 워커는 어차피 한 번에 파일 하나만 처리한다.

use crate::norm::{clean, clean_lines};
use crate::{FailKind, Stats};
use doc::{Block, BlockKind, Document};
use pdfium_render::prelude::*;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

#[cfg(windows)]
pub const LIB_NAME: &str = "pdfium.dll";
#[cfg(target_os = "macos")]
pub const LIB_NAME: &str = "libpdfium.dylib";
#[cfg(all(unix, not(target_os = "macos")))]
pub const LIB_NAME: &str = "libpdfium.so";

static PDFIUM: OnceLock<Mutex<Pdfium>> = OnceLock::new();

pub fn init(lib: &Path) -> Result<(), String> {
    if PDFIUM.get().is_some() {
        return Ok(());
    }
    if !lib.is_absolute() {
        return Err("pdfium 경로는 절대 경로여야 합니다".into());
    }
    // 오류 값(LoadLibraryError)에는 경로가 들어 있다 — 진단 보고서로 새지 않게 고정 문장만 돌려준다(INC-010).
    let bindings = Pdfium::bind_to_library(lib).map_err(|_| "pdfium 을 불러오지 못했습니다".to_string())?;
    let _ = PDFIUM.set(Mutex::new(Pdfium::new(bindings)));
    Ok(())
}

/// 쪽 글자 품질(공백 제외 글자 수 기준).
#[derive(Default)]
struct Quality {
    chars: usize,
    hangul: usize,
    garbage: usize,        // U+FFFD, 사설 영역, 제어 문자
    latin1_letters: usize, // U+00C0~U+00FF(× ÷ 제외) — ToUnicode 없는 한글 글꼴이 흔히 이렇게 깨진다
}

fn quality(s: &str) -> Quality {
    let mut q = Quality::default();
    for c in s.chars().filter(|c| !c.is_whitespace()) {
        q.chars += 1;
        let u = c as u32;
        if (0xAC00..=0xD7A3).contains(&u) || (0x1100..=0x11FF).contains(&u) || (0x3130..=0x318F).contains(&u) {
            q.hangul += 1;
        }
        if c == '\u{FFFD}' || (0xE000..=0xF8FF).contains(&u) || c.is_control() {
            q.garbage += 1;
        }
        if (0xC0..=0xFF).contains(&u) && u != 0xD7 && u != 0xF7 {
            q.latin1_letters += 1;
        }
    }
    q
}

/// 쓸 만한 쪽인가: 글자가 있고, 깨진 글자가 10% 이하이며, 한글 없이 Latin-1 글자가 30% 넘지 않을 것.
/// 가운뎃점(·)·①②·‘’ 같은 공문서 기호는 의심 글자로 세지 않는다(실제 공문 PDF 136쪽에서 오판 0 확인).
fn page_ok(q: &Quality) -> bool {
    if q.chars == 0 {
        return false;
    }
    let r = |n: usize| n as f64 / q.chars as f64;
    !(q.chars >= 10 && (r(q.garbage) > 0.10 || (q.hangul == 0 && r(q.latin1_letters) > 0.30)))
}

/// 메타데이터 제목이 믿을 만한가. "untitled"(reportlab 기본값), 원본 파일 이름(…hwp), 깨진 Latin-1 은 버린다.
fn usable_title(raw: &str) -> Option<String> {
    if raw.contains('\0') {
        return None;
    }
    let t = clean(raw);
    if t.is_empty() || t.eq_ignore_ascii_case("untitled") {
        return None;
    }
    let lower = t.to_lowercase();
    if [".hwp", ".hwpx", ".doc", ".docx", ".pdf", ".xls", ".xlsx", ".ppt", ".pptx"].iter().any(|e| lower.ends_with(e)) {
        return None;
    }
    let n = t.chars().count();
    let latin1 = t.chars().filter(|c| ('\u{80}'..='\u{FF}').contains(c)).count();
    let hangul = t.chars().filter(|c| ('\u{AC00}'..='\u{D7A3}').contains(c)).count();
    if hangul == 0 && latin1 * 2 > n {
        return None;
    }
    Some(t)
}

pub(crate) fn pdf(bytes: &[u8]) -> Result<(Document, Stats), FailKind> {
    let Some(lock) = PDFIUM.get() else { return Err(FailKind::Unsupported) };
    let pdfium = lock.lock().unwrap_or_else(|e| e.into_inner());
    let doc = pdfium.load_pdf_from_byte_slice(bytes, None).map_err(|e| match e {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => FailKind::Encrypted,
        _ => FailKind::Corrupt,
    })?;
    let title = doc.metadata().get(PdfDocumentMetadataTagType::Title).and_then(|t| usable_title(t.value()));
    let mut stats = Stats::default();
    let mut blocks: Vec<Block> = Vec::new();
    for page in doc.pages().iter() {
        stats.pdf_pages += 1;
        let text = page.text().map(|t| t.all()).unwrap_or_default();
        if page_ok(&quality(&text)) {
            let t = clean_lines(&text.replace("\r\n", "\n"));
            if !t.is_empty() {
                blocks.push(Block { kind: BlockKind::Paragraph, text: t });
                continue;
            }
        }
        stats.pdf_bad_pages += 1;
    }
    if blocks.is_empty() {
        return Err(FailKind::NoText);
    }
    Ok((Document { title, blocks }, stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn korean_punctuation_is_not_garbage() {
        assert!(page_ok(&quality("Ⅲ. 추진 계획 · 세부 ① ‘가’ 항목 2026. 10.")));
        assert!(!page_ok(&quality("ÄÅÇÉ ÑÖÜá àâäãåç éèêëíìîï")));
        assert!(!page_ok(&quality("\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}\u{7}\u{8}\u{e}\u{f}")));
        assert!(!page_ok(&quality("   ")));
    }

    #[test]
    fn placeholder_titles_are_dropped() {
        assert_eq!(usable_title("Untitled"), None);
        assert_eq!(usable_title("2026년 계획(안).hwp"), None);
        assert_eq!(usable_title("Ã«Â¬Â¸Ã¬Â\u{84}Â\u{9c}"), None);
        assert_eq!(usable_title(" 2026년 더비온 운영계획 ").as_deref(), Some("2026년 더비온 운영계획"));
    }
}
```

> pdfium-render 오류 변형 이름(`PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)`)이 다르면 `~/.cargo/registry/src/*/pdfium-render-0.9.4/src/error.rs` 를 보고 맞춘다 — 비밀번호 오류만 Encrypted, 나머지는 Corrupt.

- [ ] **Step 7: `lib.rs` 연결** — `pub mod pdf;` 추가, `match format` 에:

```rust
        Format::Pdf => pdf::pdf(bytes)
            .map(|(doc, stats)| Extracted { doc, distribution: false, stats })
            .map_err(|k| (k, magic::magic_hex(bytes))),
```

- [ ] **Step 8: 통과 확인**

Run: `cargo test -p extract --release`
Expected: PASS(pdf 시험 4개 + 단위 시험 2개 포함).

- [ ] **Step 9: CI 에 pdfium 받기** — `.github/workflows/ci.yml` `test` 작업의 `fetch rhwp sources` 단계 바로 아래에 추가:

```yaml
      - name: fetch pdfium (pinned, SHA-256)
        shell: bash
        run: bash tools/fetch-pdfium.sh
```

`CLAUDE.md` "개발 명령" 블록의 `bash tools/fetch-rhwp.sh` 아래에 `bash tools/fetch-pdfium.sh        # PDF 추출 엔진(third_party/pdfium, 시험에 필요)` 추가.

- [ ] **Step 10: KI-014 해결** — `known-issues.md` KI-014 상태 → `해결: pdfium(M1 Task 8). 스캔·글자 깨진 PDF 는 NoText(파일명만)`

- [ ] **Step 11: Commit**

```bash
git add tools/fetch-pdfium.sh tools/fixtures/make_pdf.py crates/extract .github/workflows/ci.yml CLAUDE.md docs/engineering/known-issues.md Cargo.lock
git commit -m "extract: PDF via pdfium loaded by absolute path, garbled/scan pages detected (KI-014)

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---
### Task 9: 상주 워커 — 요청·응답 한 줄 JSON, 시간 초과·비정상 종료 시 재시작

M0 는 파일마다 프로세스를 새로 띄웠다(DEC-025 범위 밖 항목). 제품은 워커 하나를 띄워 두고 파일을 하나씩 보낸다. 워커가 멈추면(30초) 앱이 죽이고 다시 띄우며, 그 파일은 "시간 초과"로 기록한다(스펙 §5.1).

**Files:**
- Create: `crates/extract/src/worker.rs`, `crates/extract/src/bin/extract-worker.rs`(시험용 워커), `crates/extract/tests/worker.rs`
- Modify: `crates/extract/src/lib.rs`

**Interfaces:**
- Consumes: `cli::extract_one`, `runner::Outcome`(Task 2), `pdf::init`(Task 8)
- Produces:

```rust
// extract::worker
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Request { pub id: u64, pub path: std::path::PathBuf }
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Response { pub id: u64, pub outcome: crate::runner::Outcome }
/// 응답 줄 앞에 붙이는 표지 — 파서 라이브러리가 표준 출력에 찍는 잡음과 구분한다.
pub const MARKER: &str = "\u{1}KRA ";
/// 워커 본체: 요청 한 줄 → `extract(path)` → 표지 + 응답 JSON 한 줄. 입력이 닫히면 끝.
pub fn serve(input: impl std::io::BufRead, output: impl std::io::Write, extract: impl Fn(&std::path::Path) -> Outcome) -> std::io::Result<()>;
/// 앱 쪽: 워커 프로세스 하나를 관리한다.
pub struct Worker { /* … */ }
impl Worker {
    pub fn new(make_cmd: impl Fn() -> std::process::Command + Send + 'static) -> Worker;
    /// 응답이 `timeout` 안에 안 오면 워커를 죽이고 `Outcome::Timeout`, 워커가 죽으면 `Outcome::Crashed`. 다음 요청 때 다시 띄운다.
    pub fn extract(&mut self, path: &std::path::Path, timeout: std::time::Duration) -> Outcome;
    pub fn restarts(&self) -> u32;
}
```

- [ ] **Step 1: 시험용 워커 바이너리** — `crates/extract/src/bin/extract-worker.rs` (개발·시험 전용, 출시 안 함)

```rust
//! 시험용 상주 워커. 파일 이름 `__hang__`/`__crash__`/`__noise__` 로 멈춤·비정상 종료·표준 출력 잡음을 흉내 낸다.
//! 제품 워커는 `kra-search worker`(app 크레이트)이며 이런 훅이 없다.

use std::io::Write;
use std::path::Path;

fn main() {
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        if a == "--pdfium" {
            let p = args.next().expect("--pdfium <경로>");
            extract::pdf::init(Path::new(&p)).expect("pdfium");
        }
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    extract::worker::serve(stdin.lock(), stdout.lock(), |p| {
        match p.file_name().and_then(|n| n.to_str()) {
            Some("__hang__") => std::thread::sleep(std::time::Duration::from_secs(600)),
            Some("__crash__") => std::process::abort(),
            Some("__noise__") => {
                // 파서가 줄바꿈 없이 표준 출력에 찍는 경우를 흉내
                print!("garbage from a parser");
                let _ = std::io::stdout().flush();
            }
            _ => {}
        }
        extract::cli::extract_one(p)
    })
    .expect("serve");
}
```

- [ ] **Step 2: 실패하는 시험** — `crates/extract/tests/worker.rs`

```rust
use extract::runner::Outcome;
use extract::worker::Worker;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

fn fx(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn worker() -> Worker {
    Worker::new(|| Command::new(env!("CARGO_BIN_EXE_extract-worker")))
}

fn is_ok(o: &Outcome) -> bool {
    matches!(o, Outcome::Ok { .. })
}

#[test]
fn worker_handles_many_files_in_one_process() {
    let mut w = worker();
    for _ in 0..3 {
        assert!(is_ok(&w.extract(&fx("schedule.hwp"), Duration::from_secs(30))));
        assert!(is_ok(&w.extract(&fx("office/sample.docx"), Duration::from_secs(30))));
    }
    assert_eq!(w.restarts(), 0);
}

#[test]
fn worker_survives_hang_and_crash() {
    let mut w = worker();
    let t = std::time::Instant::now();
    assert!(matches!(w.extract(Path::new("/x/__hang__"), Duration::from_secs(2)), Outcome::Timeout));
    assert!(t.elapsed() < Duration::from_secs(10), "시간 초과가 바로 처리돼야 함: {:?}", t.elapsed());
    assert!(is_ok(&w.extract(&fx("schedule.hwp"), Duration::from_secs(30))), "멈춘 뒤 다음 파일 정상");
    assert!(matches!(w.extract(Path::new("/x/__crash__"), Duration::from_secs(30)), Outcome::Crashed { .. }));
    assert!(is_ok(&w.extract(&fx("schedule.hwp"), Duration::from_secs(30))), "죽은 뒤 다음 파일 정상");
    assert_eq!(w.restarts(), 2);
}

#[test]
fn parser_stdout_noise_does_not_break_protocol() {
    let mut w = worker();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("__noise__");
    std::fs::write(&p, b"x").unwrap();
    // 확장자가 없어 Unsupported 로 실패하지만, 응답 자체는 제대로 받아야 한다(시간 초과 아님).
    assert!(matches!(w.extract(&p, Duration::from_secs(5)), Outcome::Failed { .. }));
    assert_eq!(w.restarts(), 0);
}
```

- [ ] **Step 3: 실패 확인** — Run: `cargo test -p extract --release --test worker` → 컴파일 실패(`extract::worker` 없음)

- [ ] **Step 4: `crates/extract/src/worker.rs`**

```rust
//! 상주 워커: 앱이 워커 프로세스 하나를 띄워 두고 파일 경로를 한 줄씩 보낸다(스펙 §5.1).
//! 워커가 멈추거나 죽어도 앱은 영향받지 않는다 — 그 파일만 시간 초과/비정상 종료로 기록하고 워커를 다시 띄운다.
//! 응답 줄은 MARKER 로 시작한다. 파서(rhwp·pdfium)가 표준 출력에 찍는 잡음이 섞여도 표지 뒤만 읽는다.

use crate::runner::Outcome;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Request {
    pub id: u64,
    pub path: PathBuf,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Response {
    pub id: u64,
    pub outcome: Outcome,
}

pub const MARKER: &str = "\u{1}KRA ";

pub fn serve(input: impl BufRead, mut output: impl Write, extract: impl Fn(&Path) -> Outcome) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        let Ok(req) = serde_json::from_str::<Request>(&line) else { continue };
        let outcome = extract(&req.path);
        let json = serde_json::to_string(&Response { id: req.id, outcome }).expect("응답 직렬화");
        // 앞에 줄바꿈을 넣어 파서가 남긴 미완성 줄과 섞이지 않게 한다.
        writeln!(output, "\n{MARKER}{json}")?;
        output.flush()?;
    }
    Ok(())
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
}

pub struct Worker {
    make_cmd: Box<dyn Fn() -> Command + Send>,
    running: Option<Running>,
    next_id: u64,
    restarts: u32,
}

impl Worker {
    pub fn new(make_cmd: impl Fn() -> Command + Send + 'static) -> Worker {
        Worker { make_cmd: Box::new(make_cmd), running: None, next_id: 1, restarts: 0 }
    }

    pub fn restarts(&self) -> u32 {
        self.restarts
    }

    fn spawn(&self) -> std::io::Result<Running> {
        let mut cmd = (self.make_cmd)();
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(i) = line.find(MARKER) {
                    if tx.send(line[i + MARKER.len()..].to_string()).is_err() {
                        break;
                    }
                }
            }
        });
        Ok(Running { child, stdin, rx })
    }

    /// 지금 워커를 죽이고 다음 요청 때 새로 띄우게 한다.
    fn discard(&mut self) -> Option<i32> {
        let mut r = self.running.take()?;
        let _ = r.child.kill();
        let code = r.child.wait().ok().and_then(|s| s.code());
        self.restarts += 1;
        code
    }

    pub fn extract(&mut self, path: &Path, timeout: Duration) -> Outcome {
        let id = self.next_id;
        self.next_id += 1;
        let req = serde_json::to_string(&Request { id, path: path.to_path_buf() }).expect("요청 직렬화");
        // 쓰기가 실패하면(이전에 죽은 워커) 한 번 다시 띄워 보낸다.
        let mut sent = false;
        for _ in 0..2 {
            if self.running.is_none() {
                match self.spawn() {
                    Ok(r) => self.running = Some(r),
                    Err(_) => return Outcome::Crashed { code: None },
                }
            }
            let r = self.running.as_mut().expect("running");
            if writeln!(r.stdin, "{req}").and_then(|_| r.stdin.flush()).is_ok() {
                sent = true;
                break;
            }
            self.discard();
        }
        if !sent {
            return Outcome::Crashed { code: None };
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let r = self.running.as_mut().expect("running");
            match r.rx.recv_timeout(left) {
                Ok(json) => match serde_json::from_str::<Response>(&json) {
                    Ok(resp) if resp.id == id => return resp.outcome,
                    _ => continue, // 이전 요청의 늦은 응답·깨진 줄은 버린다
                },
                Err(RecvTimeoutError::Timeout) => {
                    self.discard();
                    return Outcome::Timeout;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let code = self.discard();
                    return Outcome::Crashed { code };
                }
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(mut r) = self.running.take() {
            let _ = r.child.kill();
            let _ = r.child.wait();
        }
    }
}
```

`lib.rs` 에 `pub mod worker;` 추가.

- [ ] **Step 5: 통과 확인** — Run: `cargo test -p extract --release --test worker` → PASS(3개). Windows CI 에서도 `abort()` 가 `Crashed` 로 잡히는지 다음 CI 실행에서 확인.

- [ ] **Step 6: Commit**

```bash
git add crates/extract
git commit -m "extract: resident worker protocol with timeout and crash restart

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 10: `platform` 크레이트 — 데이터 위치·키 생성·DPAPI 보호

**Files:**
- Create: `crates/platform/Cargo.toml`, `crates/platform/src/lib.rs`, `crates/platform/src/paths.rs`, `crates/platform/src/keys.rs`

**Interfaces:**
- Produces:

```rust
// platform::paths
pub struct Home { root: PathBuf }
impl Home {
    /// Windows: %LOCALAPPDATA%\KraSearch, 그 밖(개발용): $HOME/.kra-search
    pub fn default_root() -> PathBuf;
    pub fn new(root: impl Into<PathBuf>) -> Home;
    pub fn root(&self) -> &Path;
    pub fn data(&self) -> PathBuf;          // root/data   — 색인 전체(나중에 EFS 적용 대상, M5)
    pub fn keys(&self) -> PathBuf;          // root/keys
    pub fn keyword_dir(&self) -> PathBuf;   // data/tantivy
    pub fn meta_db(&self) -> PathBuf;       // data/meta.db
    pub fn meta_key(&self) -> PathBuf;      // keys/meta.key
}
// platform::keys
pub fn new_key() -> [u8; 32];
pub fn protect(data: &[u8]) -> std::io::Result<Vec<u8>>;    // Windows: DPAPI(CurrentUser). 그 밖: 개발용 표지만 붙임(보호 없음)
pub fn unprotect(blob: &[u8]) -> std::io::Result<Vec<u8>>;
#[derive(Debug)] pub enum KeyError { Io(std::io::Error), Unreadable }
/// 키 파일이 있으면 풀어 돌려주고, 없으면 새로 만들어 보호해 저장한다. 풀 수 없으면 Unreadable(→ 앱이 data 를 지우고 다시 색인).
pub fn load_or_create(path: &Path) -> Result<[u8; 32], KeyError>;
```

- [ ] **Step 1: `crates/platform/Cargo.toml`**

```toml
[package]
name = "platform"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
getrandom = "0.4"

[target.'cfg(windows)'.dependencies]
windows = { version = "0.62", features = ["Win32_Foundation", "Win32_Security_Cryptography"] }

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: 실패하는 시험** — `crates/platform/src/keys.rs` 아래쪽 `mod tests`(Step 4 의 파일에 포함)와 `paths.rs` 시험. 먼저 빈 `lib.rs` 로 컴파일 실패를 확인한다:

```rust
//! Windows 전용 기능(DPAPI·데이터 위치). macOS 는 개발용 스텁(스펙 §5.2 platform).
pub mod keys;
pub mod paths;
```

Run: `cargo test -p platform --release` → 컴파일 실패(모듈 파일 없음)

- [ ] **Step 3: `crates/platform/src/paths.rs`**

```rust
//! 데이터 위치(스펙 §5.3): 사용자별 `%LOCALAPPDATA%\KraSearch\{data,keys}`.

use std::path::{Path, PathBuf};

pub struct Home {
    root: PathBuf,
}

impl Home {
    pub fn default_root() -> PathBuf {
        if cfg!(windows) {
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                return PathBuf::from(local).join("KraSearch");
            }
        }
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        home.join(".kra-search")
    }

    pub fn new(root: impl Into<PathBuf>) -> Home {
        Home { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn data(&self) -> PathBuf {
        self.root.join("data")
    }
    pub fn keys(&self) -> PathBuf {
        self.root.join("keys")
    }
    pub fn keyword_dir(&self) -> PathBuf {
        self.data().join("tantivy")
    }
    pub fn meta_db(&self) -> PathBuf {
        self.data().join("meta.db")
    }
    pub fn meta_key(&self) -> PathBuf {
        self.keys().join("meta.key")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn layout_is_under_root() {
        let h = super::Home::new("/r");
        assert!(h.meta_db().starts_with(h.data()));
        assert!(h.keyword_dir().starts_with(h.data()));
        assert!(h.meta_key().starts_with(h.keys()));
        assert!(!h.keys().starts_with(h.data()), "키는 data 밖(data 를 지워도 남음)");
    }
}
```

- [ ] **Step 4: `crates/platform/src/keys.rs`**

```rust
//! 메타 DB 키: 32바이트 OS 난수를 Windows DPAPI(CurrentUser)로 보호해 파일에 둔다(스펙 §4·§7.1).
//! 다른 사용자·다른 PC 에서는 풀 수 없다. 관리자가 로컬 계정 비밀번호를 재설정하면 풀 수 없게 되므로
//! 그때는 Unreadable 을 돌려주고, 앱이 data 를 지우고 다시 색인한다(스펙 §5.3).

use std::io;
use std::path::Path;

pub fn new_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    getrandom::fill(&mut k).expect("OS 난수");
    k
}

#[derive(Debug)]
pub enum KeyError {
    Io(io::Error),
    Unreadable,
}

pub fn load_or_create(path: &Path) -> Result<[u8; 32], KeyError> {
    match std::fs::read(path) {
        Ok(blob) => {
            let raw = unprotect(&blob).map_err(|_| KeyError::Unreadable)?;
            raw.try_into().map_err(|_| KeyError::Unreadable)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let key = new_key();
            let blob = protect(&key).map_err(KeyError::Io)?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(KeyError::Io)?;
            }
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, &blob).map_err(KeyError::Io)?;
            std::fs::rename(&tmp, path).map_err(KeyError::Io)?;
            Ok(key)
        }
        Err(e) => Err(KeyError::Io(e)),
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};

    /// DPAPI 가 LocalAlloc 으로 준 출력 버퍼를 Vec 으로 복사하고 LocalFree 한다. `wipe` 면 해제 전에 0 으로 지운다.
    unsafe fn take_blob(out: CRYPT_INTEGER_BLOB, wipe: bool) -> Vec<u8> {
        if out.pbData.is_null() {
            return Vec::new();
        }
        let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        if wipe {
            unsafe { std::ptr::write_bytes(out.pbData, 0, out.cbData as usize) };
        }
        unsafe { LocalFree(Some(HLOCAL(out.pbData.cast()))) };
        v
    }

    pub fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB::default();
        unsafe {
            // 설명 문자열은 blob 에 평문으로 들어간다 — 일반 명칭만.
            CryptProtectData(&input, windows::core::w!("kra-search key"), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
                .map_err(|e| io::Error::other(e.to_string()))?;
            Ok(take_blob(out, false))
        }
    }

    pub fn unprotect(blob: &[u8]) -> io::Result<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB { cbData: blob.len() as u32, pbData: blob.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB::default();
        unsafe {
            CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).map_err(|e| io::Error::other(e.to_string()))?;
            Ok(take_blob(out, true))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    //! 개발용(macOS): 보호하지 않는다. 제품은 Windows 전용이다.
    use std::io;
    const DEV: &[u8] = b"KRA-DEV-UNPROTECTED:";

    pub fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
        Ok([DEV, data].concat())
    }

    pub fn unprotect(blob: &[u8]) -> io::Result<Vec<u8>> {
        blob.strip_prefix(DEV).map(<[u8]>::to_vec).ok_or_else(|| io::Error::other("개발용 키 형식이 아님"))
    }
}

pub use imp::{protect, unprotect};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protect_roundtrip_and_tamper_fails() {
        let key = new_key();
        let blob = protect(&key).unwrap();
        assert_eq!(unprotect(&blob).unwrap(), key);
        let mut bad = blob.clone();
        bad[0] ^= 0xFF;
        assert!(unprotect(&bad).is_err());
    }

    #[test]
    fn load_or_create_is_stable_and_detects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("keys/meta.key");
        let a = load_or_create(&p).unwrap();
        let b = load_or_create(&p).unwrap();
        assert_eq!(a, b);
        std::fs::write(&p, b"garbage").unwrap();
        assert!(matches!(load_or_create(&p), Err(KeyError::Unreadable)));
    }

    #[test]
    fn keys_are_random() {
        assert_ne!(new_key(), new_key());
    }
}
```

- [ ] **Step 5: 통과 확인** — Run: `cargo test -p platform --release && cargo check -p platform --release --target x86_64-pc-windows-msvc`
Expected: macOS 시험 PASS, Windows 대상 타입 검사 통과(실제 DPAPI 시험은 Windows CI 에서 돈다).

- [ ] **Step 6: Commit**

```bash
git add crates/platform Cargo.lock
git commit -m "platform: data locations and DPAPI-protected random key

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 11: 메타 DB (`index::meta`, SQLCipher)

**Files:**
- Create: `crates/index/src/meta.rs`
- Modify: `crates/index/Cargo.toml`, `crates/index/src/lib.rs`, `.github/workflows/ci.yml`, `.github/workflows/release-m0.yml`, `.github/workflows/perf-probe.yml`(있으면)

**Interfaces:**
- Consumes: `doc::Document`
- Produces:

```rust
// index::meta
#[derive(Debug)] pub enum MetaError { WrongKeyOrCorrupt, Io(std::io::Error), Sql(rusqlite::Error) }
pub type Result<T> = std::result::Result<T, MetaError>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum State { Pending, Indexed }
pub struct ContentData { pub fail: Option<String>, pub title: Option<String>, pub distribution: bool, pub doc: Option<doc::Document> }
pub struct Content { pub id: i64, pub hash: u128, pub state: State, pub fail: Option<String>, pub title: Option<String>, pub distribution: bool, pub retries: u32 }
pub struct FileRow { pub path: String, pub size: u64, pub mtime_ns: i64, pub content_id: Option<i64> }
#[derive(Debug, Default, PartialEq)] pub struct Counts { pub files: u64, pub contents: u64, pub failed: u64, pub pending: u64 }
pub struct Meta { /* rusqlite::Connection */ }
impl Meta {
    pub fn open(path: &Path, key: &[u8; 32]) -> Result<Meta>;                 // 키가 틀리거나 파일이 깨지면 WrongKeyOrCorrupt
    pub fn roots(&self) -> Result<Vec<String>>;
    pub fn set_roots(&mut self, roots: &[String]) -> Result<()>;
    pub fn known_files(&self) -> Result<HashMap<String, (u64, i64)>>;          // 경로 → (크기, 수정 시각 ns)
    pub fn content_by_hash(&self, hash: u128) -> Result<Option<Content>>;
    pub fn insert_content(&mut self, hash: u128, d: &ContentData) -> Result<i64>; // 상태 Pending
    pub fn retry_content(&mut self, id: i64, d: &ContentData) -> Result<()>;      // 재시도 결과로 교체, retries += 1, Pending
    pub fn upsert_file(&mut self, path: &str, size: u64, mtime_ns: i64, content_id: i64) -> Result<Option<i64>>; // 이전 content id
    pub fn remove_file(&mut self, path: &str) -> Result<Option<i64>>;            // 이전 content id
    pub fn mark_pending(&mut self, content_id: i64) -> Result<()>;
    pub fn mark_all_pending(&mut self) -> Result<()>;                            // 키워드 색인을 잃었을 때 전체 다시 쓰기
    pub fn pending(&self, limit: usize) -> Result<Vec<i64>>;
    pub fn orphans(&self) -> Result<Vec<i64>>;                                   // 파일이 하나도 없는 내용
    pub fn doc(&self, content_id: i64) -> Result<Option<doc::Document>>;
    pub fn content(&self, id: i64) -> Result<Option<Content>>;
    pub fn files_of(&self, content_id: i64) -> Result<Vec<FileRow>>;             // 수정 시각 최근 순
    pub fn finish_flush(&mut self, indexed: &[i64], deleted: &[i64], opstamp: u64) -> Result<()>; // 한 트랜잭션
    pub fn keyword_opstamp(&self) -> Result<u64>;
    pub fn counts(&self) -> Result<Counts>;
}
```

- [ ] **Step 1: 의존성** — `crates/index/Cargo.toml`:

```toml
[dependencies]
doc = { path = "../doc" }
serde_json.workspace = true
# SQLCipher + OpenSSL 동봉 빌드. 기본 기능을 끄면 wasm 관련 의존이 빠진다.
rusqlite = { version = "0.40", default-features = false, features = ["bundled-sqlcipher-vendored-openssl", "cache"] }

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: CI — Windows 에서 OpenSSL 빌드용 Perl** — `.github/workflows/ci.yml` 의 `libclang (windows)` 단계 아래, `release-m0.yml`(Task 17 에서 `release-diag.yml` 로 이름 변경)과 `perf-probe.yml` 의 빌드 전 단계에 추가:

```yaml
      - name: Strawberry Perl for vendored OpenSSL (windows)
        if: runner.os == 'Windows'
        run: echo "OPENSSL_SRC_PERL=C:\Strawberry\perl\bin\perl.exe" >> $env:GITHUB_ENV
```

(`release-m0.yml` 은 Windows 전용이라 `if:` 없이 `run:` 만 넣는다. `OPENSSL_DIR` 은 설정하지 않는다 — 설정하면 동봉 빌드 대신 그 경로를 쓴다.)

- [ ] **Step 3: 실패하는 시험** — `crates/index/src/meta.rs` 의 `mod tests`(Step 4 파일 끝)를 먼저 쓰고 `lib.rs` 에 `pub mod meta;` 를 넣어 컴파일 실패를 확인한다.

Run: `cargo test -p index --release meta` → 컴파일 실패

- [ ] **Step 4: `crates/index/src/meta.rs`**

```rust
//! 메타 DB(meta.db) — SQLCipher(키는 platform::keys). 앱만 쓴다(스펙 §5.1).
//! 내용(content) = 같은 바이트의 파일 묶음(xxh3-128, 스펙 §6.4). 추출·색인은 내용 단위, 경로(file)는 여러 개.
//! 키워드 색인과의 일관성(스펙 §6.8): 내용을 Pending 으로 두고 → 키워드 색인에 쓰고 커밋 → finish_flush 로 Indexed.
//! 중간에 죽으면 Pending 이 남아 다음 실행에서 다시 쓴다(키워드 쪽은 delete+add 라 여러 번 써도 같다).

use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug)]
pub enum MetaError {
    /// 키가 틀렸거나 파일이 깨짐 — 둘을 구분할 수 없다. 앱은 data 를 지우고 다시 색인한다(스펙 §5.3).
    WrongKeyOrCorrupt,
    Io(std::io::Error),
    Sql(rusqlite::Error),
}

impl From<rusqlite::Error> for MetaError {
    fn from(e: rusqlite::Error) -> Self {
        MetaError::Sql(e)
    }
}

impl std::fmt::Display for MetaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetaError::WrongKeyOrCorrupt => write!(f, "메타 DB 를 열 수 없음(키 불일치 또는 손상)"),
            MetaError::Io(e) => write!(f, "메타 DB 폴더 오류: {e}"),
            MetaError::Sql(e) => write!(f, "메타 DB 오류: {e}"),
        }
    }
}

pub type Result<T> = std::result::Result<T, MetaError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Indexed,
}

pub struct ContentData {
    pub fail: Option<String>,
    pub title: Option<String>,
    pub distribution: bool,
    pub doc: Option<doc::Document>,
}

#[derive(Debug, Clone)]
pub struct Content {
    pub id: i64,
    pub hash: u128,
    pub state: State,
    pub fail: Option<String>,
    pub title: Option<String>,
    pub distribution: bool,
    pub retries: u32,
}

#[derive(Debug, Clone)]
pub struct FileRow {
    pub path: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub content_id: Option<i64>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Counts {
    pub files: u64,
    pub contents: u64,
    pub failed: u64,
    pub pending: u64,
}

const SCHEMA_VERSION: &str = "1";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS kv(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS root(path TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS content(
  id INTEGER PRIMARY KEY,
  hash BLOB NOT NULL UNIQUE,
  state INTEGER NOT NULL,
  fail TEXT,
  title TEXT,
  distribution INTEGER NOT NULL DEFAULT 0,
  doc TEXT,
  retries INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS content_state ON content(state);
CREATE TABLE IF NOT EXISTS file(
  path TEXT PRIMARY KEY,
  size INTEGER NOT NULL,
  mtime_ns INTEGER NOT NULL,
  content_id INTEGER REFERENCES content(id)
);
CREATE INDEX IF NOT EXISTS file_content ON file(content_id);
";

pub struct Meta {
    conn: Connection,
}

fn hex(key: &[u8]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

fn state_of(v: i64) -> State {
    if v == 1 { State::Indexed } else { State::Pending }
}

fn content_row(r: &rusqlite::Row) -> rusqlite::Result<Content> {
    let hash: Vec<u8> = r.get(1)?;
    let mut h = [0u8; 16];
    h.copy_from_slice(&hash[..16.min(hash.len())]);
    Ok(Content {
        id: r.get(0)?,
        hash: u128::from_le_bytes(h),
        state: state_of(r.get(2)?),
        fail: r.get(3)?,
        title: r.get(4)?,
        distribution: r.get::<_, i64>(5)? != 0,
        retries: r.get::<_, i64>(6)? as u32,
    })
}

const CONTENT_COLS: &str = "id, hash, state, fail, title, distribution, retries";

impl Meta {
    pub fn open(path: &Path, key: &[u8; 32]) -> Result<Meta> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(MetaError::Io)?;
        }
        let conn = Connection::open(path)?;
        // 키는 연결에서 가장 먼저. 원시 키 형식 x'…' 은 키 유도(PBKDF2)를 건너뛴다.
        conn.pragma_update(None, "key", format!("x'{}'", hex(key)))?;
        // 키가 틀리면 여기(첫 읽기)에서 NotADatabase 가 난다.
        if let Err(e) = conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)) {
            return match e.sqlite_error_code() {
                Some(rusqlite::ErrorCode::NotADatabase) => Err(MetaError::WrongKeyOrCorrupt),
                _ => Err(e.into()),
            };
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        let m = Meta { conn };
        let v: Option<String> = m.conn.query_row("SELECT value FROM kv WHERE key='schema'", [], |r| r.get(0)).optional()?;
        match v.as_deref() {
            None => {
                m.conn.execute("INSERT INTO kv(key, value) VALUES('schema', ?1)", [SCHEMA_VERSION])?;
            }
            Some(SCHEMA_VERSION) => {}
            // 다른 스키마 = 다른 버전의 앱이 만든 색인 → 다시 색인(스펙 §6.8)
            Some(_) => return Err(MetaError::WrongKeyOrCorrupt),
        }
        Ok(m)
    }

    pub fn roots(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare_cached("SELECT path FROM root ORDER BY path")?;
        let v = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(v)
    }

    pub fn set_roots(&mut self, roots: &[String]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM root", [])?;
        for r in roots {
            tx.execute("INSERT OR IGNORE INTO root(path) VALUES(?1)", [r])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn known_files(&self) -> Result<HashMap<String, (u64, i64)>> {
        let mut st = self.conn.prepare_cached("SELECT path, size, mtime_ns FROM file")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)?))))?;
        Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
    }

    pub fn content_by_hash(&self, hash: u128) -> Result<Option<Content>> {
        let sql = format!("SELECT {CONTENT_COLS} FROM content WHERE hash = ?1");
        Ok(self.conn.query_row(&sql, [hash.to_le_bytes().to_vec()], content_row).optional()?)
    }

    pub fn content(&self, id: i64) -> Result<Option<Content>> {
        let sql = format!("SELECT {CONTENT_COLS} FROM content WHERE id = ?1");
        Ok(self.conn.query_row(&sql, [id], content_row).optional()?)
    }

    fn doc_json(d: &ContentData) -> Option<String> {
        d.doc.as_ref().map(|doc| serde_json::to_string(doc).expect("문서 직렬화"))
    }

    pub fn insert_content(&mut self, hash: u128, d: &ContentData) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO content(hash, state, fail, title, distribution, doc) VALUES(?1, 0, ?2, ?3, ?4, ?5)",
            params![hash.to_le_bytes().to_vec(), d.fail, d.title, d.distribution as i64, Self::doc_json(d)],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn retry_content(&mut self, id: i64, d: &ContentData) -> Result<()> {
        self.conn.execute(
            "UPDATE content SET state = 0, fail = ?2, title = ?3, distribution = ?4, doc = ?5, retries = retries + 1 WHERE id = ?1",
            params![id, d.fail, d.title, d.distribution as i64, Self::doc_json(d)],
        )?;
        Ok(())
    }

    pub fn upsert_file(&mut self, path: &str, size: u64, mtime_ns: i64, content_id: i64) -> Result<Option<i64>> {
        let tx = self.conn.transaction()?;
        let old: Option<Option<i64>> = tx.query_row("SELECT content_id FROM file WHERE path = ?1", [path], |r| r.get(0)).optional()?;
        tx.execute(
            "INSERT INTO file(path, size, mtime_ns, content_id) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET size = excluded.size, mtime_ns = excluded.mtime_ns, content_id = excluded.content_id",
            params![path, size as i64, mtime_ns, content_id],
        )?;
        // 파일 이름 목록이 바뀌었으니 새 내용과 이전 내용 모두 키워드 색인을 다시 쓴다.
        tx.execute("UPDATE content SET state = 0 WHERE id = ?1", [content_id])?;
        if let Some(Some(o)) = old {
            tx.execute("UPDATE content SET state = 0 WHERE id = ?1", [o])?;
        }
        tx.commit()?;
        Ok(old.flatten())
    }

    pub fn remove_file(&mut self, path: &str) -> Result<Option<i64>> {
        let tx = self.conn.transaction()?;
        let old: Option<Option<i64>> = tx.query_row("SELECT content_id FROM file WHERE path = ?1", [path], |r| r.get(0)).optional()?;
        tx.execute("DELETE FROM file WHERE path = ?1", [path])?;
        if let Some(Some(o)) = old {
            tx.execute("UPDATE content SET state = 0 WHERE id = ?1", [o])?;
        }
        tx.commit()?;
        Ok(old.flatten())
    }

    pub fn mark_pending(&mut self, content_id: i64) -> Result<()> {
        self.conn.execute("UPDATE content SET state = 0 WHERE id = ?1", [content_id])?;
        Ok(())
    }

    pub fn mark_all_pending(&mut self) -> Result<()> {
        self.conn.execute("UPDATE content SET state = 0", [])?;
        Ok(())
    }

    pub fn pending(&self, limit: usize) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare_cached("SELECT id FROM content WHERE state = 0 ORDER BY id LIMIT ?1")?;
        let v = st.query_map([limit as i64], |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(v)
    }

    pub fn orphans(&self) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare_cached("SELECT id FROM content c WHERE NOT EXISTS (SELECT 1 FROM file f WHERE f.content_id = c.id)")?;
        let v = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(v)
    }

    pub fn doc(&self, content_id: i64) -> Result<Option<doc::Document>> {
        let s: Option<Option<String>> = self.conn.query_row("SELECT doc FROM content WHERE id = ?1", [content_id], |r| r.get(0)).optional()?;
        Ok(s.flatten().and_then(|j| serde_json::from_str(&j).ok()))
    }

    pub fn files_of(&self, content_id: i64) -> Result<Vec<FileRow>> {
        let mut st = self.conn.prepare_cached("SELECT path, size, mtime_ns, content_id FROM file WHERE content_id = ?1 ORDER BY mtime_ns DESC, path")?;
        let v = st
            .query_map([content_id], |r| Ok(FileRow { path: r.get(0)?, size: r.get::<_, i64>(1)? as u64, mtime_ns: r.get(2)?, content_id: r.get(3)? }))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn finish_flush(&mut self, indexed: &[i64], deleted: &[i64], opstamp: u64) -> Result<()> {
        let tx = self.conn.transaction()?;
        for id in indexed {
            tx.execute("UPDATE content SET state = 1 WHERE id = ?1", [id])?;
        }
        for id in deleted {
            tx.execute("DELETE FROM content WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM file WHERE content_id = ?1)", [id])?;
        }
        tx.execute("INSERT INTO kv(key, value) VALUES('keyword_opstamp', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [opstamp.to_string()])?;
        tx.commit()?;
        Ok(())
    }

    pub fn keyword_opstamp(&self) -> Result<u64> {
        let v: Option<String> = self.conn.query_row("SELECT value FROM kv WHERE key='keyword_opstamp'", [], |r| r.get(0)).optional()?;
        Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
    }

    pub fn counts(&self) -> Result<Counts> {
        let one = |sql: &str| -> Result<u64> { Ok(self.conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as u64) };
        Ok(Counts {
            files: one("SELECT count(*) FROM file")?,
            contents: one("SELECT count(*) FROM content")?,
            failed: one("SELECT count(*) FROM content WHERE fail IS NOT NULL")?,
            pending: one("SELECT count(*) FROM content WHERE state = 0")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(title: &str) -> ContentData {
        ContentData {
            fail: None,
            title: Some(title.into()),
            distribution: false,
            doc: Some(doc::Document { title: Some(title.into()), blocks: vec![doc::Block { kind: doc::BlockKind::Paragraph, text: "본문 비밀표지".into() }] }),
        }
    }

    #[test]
    fn wrong_key_is_detected_and_file_is_encrypted() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("meta.db");
        let key = [7u8; 32];
        {
            let mut m = Meta::open(&p, &key).unwrap();
            let id = m.insert_content(42, &data("내부통제규정")).unwrap();
            m.upsert_file("C:/문서/규정.hwp", 10, 1, id).unwrap();
        }
        assert!(matches!(Meta::open(&p, &[8u8; 32]), Err(MetaError::WrongKeyOrCorrupt)));
        let raw = std::fs::read(&p).unwrap();
        assert!(!raw.windows("비밀표지".len()).any(|w| w == "비밀표지".as_bytes()), "평문이 디스크에 남으면 안 됨");
        assert!(!raw.starts_with(b"SQLite format 3"));
        let m = Meta::open(&p, &key).unwrap();
        assert_eq!(m.counts().unwrap(), Counts { files: 1, contents: 1, failed: 0, pending: 1 });
    }

    #[test]
    fn flush_cycle_and_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Meta::open(&dir.path().join("meta.db"), &[1u8; 32]).unwrap();
        let a = m.insert_content(1, &data("가")).unwrap();
        m.upsert_file("/d/a.hwp", 1, 10, a).unwrap();
        m.upsert_file("/d/a-복사본.hwp", 1, 20, a).unwrap();
        assert_eq!(m.pending(10).unwrap(), vec![a]);
        m.finish_flush(&[a], &[], 5).unwrap();
        assert!(m.pending(10).unwrap().is_empty());
        assert_eq!(m.keyword_opstamp().unwrap(), 5);
        assert_eq!(m.files_of(a).unwrap().iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["/d/a-복사본.hwp", "/d/a.hwp"]);

        // 경로 하나 삭제 → 다시 Pending(파일 이름 목록 변경), 아직 고아 아님
        assert_eq!(m.remove_file("/d/a.hwp").unwrap(), Some(a));
        assert_eq!(m.pending(10).unwrap(), vec![a]);
        assert!(m.orphans().unwrap().is_empty());
        // 마지막 경로 삭제 → 고아
        m.remove_file("/d/a-복사본.hwp").unwrap();
        assert_eq!(m.orphans().unwrap(), vec![a]);
        m.finish_flush(&[], &[a], 6).unwrap();
        assert!(m.content(a).unwrap().is_none());
        assert_eq!(m.content_by_hash(1).unwrap().map(|c| c.id), None);
    }

    #[test]
    fn doc_roundtrip_and_retry() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Meta::open(&dir.path().join("meta.db"), &[2u8; 32]).unwrap();
        let failed = ContentData { fail: Some("Timeout".into()), title: None, distribution: false, doc: None };
        let id = m.insert_content(u128::MAX, &failed).unwrap();
        assert_eq!(m.content_by_hash(u128::MAX).unwrap().unwrap().fail.as_deref(), Some("Timeout"));
        assert!(m.doc(id).unwrap().is_none());
        m.retry_content(id, &data("다시")).unwrap();
        let c = m.content(id).unwrap().unwrap();
        assert_eq!((c.retries, c.fail, c.state), (1, None, State::Pending));
        assert_eq!(m.doc(id).unwrap().unwrap().blocks[0].text, "본문 비밀표지");
    }

    #[test]
    fn roots_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Meta::open(&dir.path().join("meta.db"), &[3u8; 32]).unwrap();
        m.set_roots(&["/b".into(), "/a".into(), "/a".into()]).unwrap();
        assert_eq!(m.roots().unwrap(), vec!["/a".to_string(), "/b".to_string()]);
    }
}
```

`crates/index/src/lib.rs`:

```rust
//! 키워드 색인(tantivy)·메타 DB(SQLCipher). 벡터 연산·평면 저장소는 `vector` 크레이트.
pub mod meta;
```

- [ ] **Step 5: 통과 확인** — Run: `cargo test -p index --release` → PASS(4개). 첫 빌드는 OpenSSL·SQLCipher 컴파일로 1분 남짓 걸린다.

- [ ] **Step 6: Commit 후 Windows CI 확인**

```bash
git add crates/index .github/workflows Cargo.lock
git commit -m "index: SQLCipher metadata store with flush protocol

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
git push -u origin m1
```

Windows 러너에서 `index` 시험이 통과하는지 확인한다(OpenSSL 을 nmake·Strawberry Perl 로 빌드하는 첫 실행 — rusqlite 자체 CI 는 이 조합을 시험하지 않는다). 실패하면 로그의 openssl-src 단계를 보고 고친 뒤 INC 로 남긴다.

---
### Task 12: 한국어 토크나이저 — 형태소(lindera ko-dic) + 한글 bigram + 사전 고정 받기

**Files:**
- Create: `crates/index/src/tokenize.rs`, `tools/fetch-ko-dic.sh`, `tools/check-no-net.sh`
- Modify: `crates/index/Cargo.toml`, `crates/index/src/lib.rs`, `.cargo/config.toml`, `deny.toml`, `.github/workflows/ci.yml`, `CLAUDE.md`(개발 명령)

**Interfaces:**
- Produces:

```rust
// index::tokenize
pub const MORPH: &str = "ko_morph";
pub const BIGRAM: &str = "ko_bigram";
/// 토큰 최대 길이(글자). 더 긴 덩어리(base64 등)는 색인하지 않는다(위치는 차지).
pub const MAX_TOKEN_CHARS: usize = 40;
#[derive(Clone)] pub struct KoMorphTokenizer { /* Arc<lindera Segmenter> */ }
#[derive(Clone, Default)] pub struct HangulBigramTokenizer;
impl KoMorphTokenizer { pub fn analyze(&self, text: &str) -> Vec<tantivy::tokenizer::Token>; }
impl HangulBigramTokenizer { pub fn analyze(text: &str) -> Vec<tantivy::tokenizer::Token>; }
/// 프로세스당 한 번 만든다(사전 불러오기 약 50~70ms, 상주 메모리 약 43MB — KI 참조).
#[derive(Clone)] pub struct Analyzers { pub morph: KoMorphTokenizer }
impl Analyzers { pub fn load() -> Result<Analyzers, String>; }
/// 색인을 열거나 만들 때마다 호출해야 한다(tantivy 는 토크나이저 등록을 저장하지 않는다).
pub fn register(index: &tantivy::Index, a: &Analyzers);
```

두 토크나이저 모두 **NFKC 정규화 → 소문자**로 만든다(전각 `ＫＲＡ` = `kra`). 토큰 오프셋은 정규화한 글자 기준이다(강조 표시를 만들 때 주의 — M3).

- [ ] **Step 1: 사전 원본 고정 받기** — `tools/fetch-ko-dic.sh`

```bash
#!/usr/bin/env bash
# 개발·CI·릴리즈 전용: lindera 한국어 사전(mecab-ko-dic, Apache-2.0) 원본을 lindera 빌드 캐시 위치에 받아 둔다.
# lindera-ko-dic 빌드 스크립트는 캐시에 이 파일이 있으면(MD5 일치) 내려받지 않는다 → 빌드가 네트워크 없이 재현된다.
# 캐시 위치는 .cargo/config.toml 의 LINDERA_BUILD_DICTIONARY_CACHE_DIR(= third_party/ko-dic).
# lindera 를 올리면 LINDERA_VER(폴더 이름)를 바꾼다 — runbooks/upgrade-lindera-pdfium.md.
set -euo pipefail
LINDERA_VER="6.2.0"
FMT="2"   # lindera-dictionary 의 DICTIONARY_FORMAT_VERSION(캐시 폴더 이름의 일부)
FILE="mecab-ko-dic-2.1.1-20180720.tar.gz"
URL="https://Lindera.dev/$FILE"
SHA="702ced21c6167e9d9aebc674ab5ee54af58d4443975f2940d37d0567c020591a"
DIR="$(cd "$(dirname "$0")/.." && pwd)/third_party/ko-dic/$LINDERA_VER-fmt$FMT"

sha_of() { (sha256sum "$1" 2>/dev/null || shasum -a 256 "$1") | cut -d' ' -f1; }

mkdir -p "$DIR"
if [ -f "$DIR/$FILE" ] && [ "$(sha_of "$DIR/$FILE")" = "$SHA" ]; then
  echo "ko-dic $FILE already present"; exit 0
fi
curl -sSfL -o "$DIR/$FILE.part" "$URL"
got="$(sha_of "$DIR/$FILE.part")"
if [ "$got" != "$SHA" ]; then
  echo "SHA-256 불일치: $FILE ($got, 기대 $SHA)" >&2
  rm -f "$DIR/$FILE.part"; exit 1
fi
mv "$DIR/$FILE.part" "$DIR/$FILE"
echo "$SHA  $FILE → $DIR"
```

`.cargo/config.toml` 끝에 추가:

```toml
[env]
# lindera 한국어 사전 빌드 캐시(tools/fetch-ko-dic.sh 가 채움). 저장소 루트 기준 경로.
LINDERA_BUILD_DICTIONARY_CACHE_DIR = { value = "third_party/ko-dic", relative = true }
```

Run: `bash tools/fetch-ko-dic.sh` → `third_party/ko-dic/6.2.0-fmt2/mecab-ko-dic-2.1.1-20180720.tar.gz`(약 50MB, SHA 일치)

- [ ] **Step 2: cargo-deny 예외와 "실행 중 네트워크 0" 검사**

`deny.toml` 의 `{ name = "ureq" }` 를 다음으로 바꾼다:

```toml
  # lindera 사전 빌드 스크립트(build_rs 기능)만 쓴다 — 실행 의존 그래프에는 없다(tools/check-no-net.sh 가 확인).
  { name = "ureq", wrappers = ["lindera-dictionary"] },
```

`[licenses]` 표에 추가:

```toml
exceptions = [
  # ureq(빌드 전용)의 TLS 루트 인증서 목록. 실행 파일에 들어가지 않는다.
  { allow = ["CDLA-Permissive-2.0"], crate = "webpki-roots" },
]
```

`tools/check-no-net.sh`:

```bash
#!/usr/bin/env bash
# 실행 의존(normal) 그래프에 HTTP 크레이트가 없는지 확인한다 — 실행 중 네트워크 0(CLAUDE.md).
# cargo-deny 는 빌드 의존까지 함께 보므로 lindera 사전 빌드 스크립트의 ureq 를 wrappers 로 허용했다. 이 검사가 그 예외를 메운다.
set -euo pipefail
bad=0
for c in reqwest hyper ureq isahc curl attohttpc; do
  if cargo tree --workspace -e normal --target all -i "$c" 2>/dev/null | grep -q "^$c v"; then
    echo "실행 의존 그래프에 $c 가 있음:" >&2
    cargo tree --workspace -e normal --target all -i "$c" >&2 || true
    bad=1
  fi
done
[ $bad -eq 0 ] && echo "실행 의존에 HTTP 크레이트 없음"
exit $bad
```

`.github/workflows/ci.yml`:
- `test` 작업, `fetch pdfium` 단계 아래에:

```yaml
      - name: cache ko-dic (lindera dictionary source + build cache)
        uses: actions/cache@v4
        with:
          path: third_party/ko-dic
          key: ko-dic-${{ runner.os }}-${{ hashFiles('tools/fetch-ko-dic.sh', 'Cargo.lock') }}
      - name: fetch ko-dic (pinned, SHA-256)
        shell: bash
        run: bash tools/fetch-ko-dic.sh
```

- `deny` 작업, `cargo-deny-action` 단계 아래에:

```yaml
      - uses: dtolnay/rust-toolchain@1.97.0
      - name: no HTTP crates in runtime dependency graph
        run: bash tools/check-no-net.sh
```

`CLAUDE.md` 개발 명령에 `bash tools/fetch-ko-dic.sh         # 한국어 형태소 사전 원본(third_party/ko-dic, 빌드에 필요)` 추가.

- [ ] **Step 3: 의존성** — `crates/index/Cargo.toml` `[dependencies]` 에 추가:

```toml
tantivy = "=0.26.2"
# embed-ko-dic: 사전을 실행 파일에 넣는다(+87MB). 사전 원본은 tools/fetch-ko-dic.sh 가 미리 받아 둔다.
lindera = { version = "=6.2.0", features = ["embed-ko-dic"] }
unicode-normalization = "0.1"
```

- [ ] **Step 4: 실패하는 시험** — `tokenize.rs` 의 `mod tests`(Step 5 파일 끝)를 쓰고 `lib.rs` 에 `pub mod tokenize;` 추가 → Run: `cargo test -p index --release tokenize` → 컴파일 실패 확인

- [ ] **Step 5: `crates/index/src/tokenize.rs`**

```rust
//! tantivy 용 한국어 토크나이저 두 개(스펙 §4·§8.2):
//! - 형태소(lindera + mecab-ko-dic): 조사·어미·기호를 빼고 내용 형태소만. 용언은 어간("달렸고" → "달리").
//! - 한글 bigram: 한글 구간에서만 2글자씩, **실제 위치**를 기록해 구절 검색이 된다(tantivy NgramTokenizer 는 위치가 0).
//! 둘 다 NFKC 정규화 + 소문자. 오프셋은 정규화한 글자 기준.

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use std::borrow::Cow;
use std::sync::Arc;
use tantivy::tokenizer::{Token, TokenStream, Tokenizer};
use unicode_normalization::UnicodeNormalization;

pub const MORPH: &str = "ko_morph";
pub const BIGRAM: &str = "ko_bigram";
pub const MAX_TOKEN_CHARS: usize = 40;

fn normalize(text: &str) -> String {
    text.nfkc().collect()
}

/// 미리 계산한 토큰을 흘려보내는 TokenStream.
pub struct VecTokenStream {
    tokens: Vec<Token>,
    cursor: usize,
}

impl TokenStream for VecTokenStream {
    fn advance(&mut self) -> bool {
        if self.cursor < self.tokens.len() {
            self.cursor += 1;
            true
        } else {
            false
        }
    }
    fn token(&self) -> &Token {
        &self.tokens[self.cursor - 1]
    }
    fn token_mut(&mut self) -> &mut Token {
        &mut self.tokens[self.cursor - 1]
    }
}

fn token(from: usize, to: usize, position: usize, text: String) -> Token {
    Token { offset_from: from, offset_to: to, position, text, position_length: 1 }
}

// ------------------------------------------------------------------ 형태소

/// 남길 품사(mecab-ko-dic). 나머지(조사 J*, 어미 E*, 접사 X[PS]*, 기호 S[FPSOCEY], 의존명사 NNB/NNBC,
/// 대명사 NP, 관형사 MM, 부사 MA*, 감탄사 IC, 지정사 VC*, 보조용언 VX)는 버린다. 사전에 없는 말(UNKNOWN)은 남긴다.
const KEEP_POS: &[&str] = &["NNG", "NNP", "NR", "VV", "VA", "XR", "SL", "SH", "SN", "UNKNOWN"];

fn keep(tag: &str) -> bool {
    KEEP_POS.contains(&tag)
}

#[derive(Clone)]
pub struct KoMorphTokenizer {
    segmenter: Arc<Segmenter>,
}

impl KoMorphTokenizer {
    /// 위치: 형태소마다 1씩 올린다(활용형은 분해된 형태소마다). 버린 형태소도 위치를 차지한다(구절이 조사를 건너뛰어 맞지 않게).
    pub fn analyze(&self, text: &str) -> Vec<Token> {
        let text = normalize(text);
        let mut out = Vec::new();
        let Ok(mut ltoks) = self.segmenter.segment(Cow::Borrowed(text.as_str())) else {
            return out;
        };
        let mut pos = 0usize;
        for lt in ltoks.iter_mut() {
            let (from, to) = (lt.byte_start, lt.byte_end);
            let surface = lt.surface.to_string();
            let tag = lt.get_detail(0).unwrap_or("UNKNOWN").to_string();
            let emit = |form: &str, out: &mut Vec<Token>, pos: usize| {
                if !form.is_empty() && form.chars().count() <= MAX_TOKEN_CHARS {
                    out.push(token(from, to, pos, form.to_lowercase()));
                }
            };
            if tag.contains('+') {
                // 예: "했" = XSV+EP, 8번째 필드 = "하/XSV/*+았/EP/*"
                let expr = lt.get_detail(7).unwrap_or("*").to_string();
                for part in expr.split('+') {
                    let mut it = part.split('/');
                    let (form, ptag) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
                    if keep(ptag) {
                        emit(form, &mut out, pos);
                    }
                    pos += 1;
                }
            } else {
                if keep(&tag) {
                    emit(&surface, &mut out, pos);
                }
                pos += 1;
            }
        }
        out
    }
}

impl Tokenizer for KoMorphTokenizer {
    type TokenStream<'a> = VecTokenStream;
    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        VecTokenStream { tokens: self.analyze(text), cursor: 0 }
    }
}

// ------------------------------------------------------------------ 한글 bigram

fn is_hangul(c: char) -> bool {
    matches!(c, '\u{AC00}'..='\u{D7A3}' | '\u{1100}'..='\u{11FF}' | '\u{3130}'..='\u{318F}')
}

#[derive(Clone, Copy, PartialEq)]
enum Class {
    Hangul,
    Other, // 한글이 아닌 글자·숫자(한자 포함)
    Sep,
}

fn class(c: char) -> Class {
    if is_hangul(c) {
        Class::Hangul
    } else if c.is_alphanumeric() {
        Class::Other
    } else {
        Class::Sep
    }
}

/// 구간(run) 사이 위치 간격. 다른 어절의 bigram 이 이어져 구절로 잘못 맞는 것을 막는다("가나 나다" ≠ "가나다").
pub const RUN_GAP: usize = 1;

#[derive(Clone, Default)]
pub struct HangulBigramTokenizer;

impl HangulBigramTokenizer {
    pub fn analyze(text: &str) -> Vec<Token> {
        let text = normalize(text);
        let text = text.as_str();
        let mut out = Vec::new();
        let mut pos = 0usize;
        let mut first_run = true;
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let end_of = |i: usize| chars.get(i).map(|&(b, _)| b).unwrap_or(text.len());
        let mut i = 0;
        while i < chars.len() {
            let k = class(chars[i].1);
            if k == Class::Sep {
                i += 1;
                continue;
            }
            let start = i;
            while i < chars.len() && class(chars[i].1) == k {
                i += 1;
            }
            if !first_run {
                pos += RUN_GAP;
            }
            first_run = false;
            if k == Class::Hangul && i - start >= 2 {
                for j in start..i - 1 {
                    let (from, to) = (chars[j].0, end_of(j + 2));
                    out.push(token(from, to, pos, text[from..to].to_string()));
                    pos += 1;
                }
            } else {
                // 1글자 한글 구간, 또는 한글 아닌 글자·숫자 구간 → 구간 전체가 토큰 하나
                let (from, to) = (chars[start].0, end_of(i));
                if i - start <= MAX_TOKEN_CHARS {
                    out.push(token(from, to, pos, text[from..to].to_lowercase()));
                }
                pos += 1;
            }
        }
        out
    }
}

impl Tokenizer for HangulBigramTokenizer {
    type TokenStream<'a> = VecTokenStream;
    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        VecTokenStream { tokens: Self::analyze(text), cursor: 0 }
    }
}

// ------------------------------------------------------------------ 등록

#[derive(Clone)]
pub struct Analyzers {
    pub morph: KoMorphTokenizer,
}

impl Analyzers {
    pub fn load() -> Result<Analyzers, String> {
        let dictionary = load_dictionary("embedded://ko-dic").map_err(|e| format!("형태소 사전: {e}"))?;
        let segmenter = Arc::new(Segmenter::new(Mode::Normal, dictionary, None));
        Ok(Analyzers { morph: KoMorphTokenizer { segmenter } })
    }
}

pub fn register(index: &tantivy::Index, a: &Analyzers) {
    index.tokenizers().register(MORPH, a.morph.clone());
    index.tokenizers().register(BIGRAM, HangulBigramTokenizer);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bigrams(s: &str) -> Vec<(String, usize)> {
        HangulBigramTokenizer::analyze(s).into_iter().map(|t| (t.text, t.position)).collect()
    }

    fn morphs(s: &str) -> Vec<(String, usize)> {
        let a = Analyzers::load().unwrap();
        a.morph.analyze(s).into_iter().map(|t| (t.text, t.position)).collect()
    }

    fn v(x: &[(&str, usize)]) -> Vec<(String, usize)> {
        x.iter().map(|(t, p)| (t.to_string(), *p)).collect()
    }

    #[test]
    fn bigram_positions_and_runs() {
        assert_eq!(
            bigrams("내부통제규정 KRA-2026 가"),
            v(&[("내부", 0), ("부통", 1), ("통제", 2), ("제규", 3), ("규정", 4), ("kra", 6), ("2026", 8), ("가", 10)])
        );
        assert_eq!(bigrams("2026년에"), v(&[("2026", 0), ("년에", 2)]));
    }

    #[test]
    fn fullwidth_is_normalized_and_long_runs_dropped() {
        assert_eq!(bigrams("ＫＲＡ２０２６"), v(&[("kra2026", 0)]));
        let long = "a".repeat(MAX_TOKEN_CHARS + 1);
        assert_eq!(bigrams(&format!("{long} 규정")), v(&[("규정", 2)]));
    }

    #[test]
    fn morph_drops_particles_and_endings() {
        assert_eq!(
            morphs("한국마사회는 내부통제규정을 2026년에 개정했다. KRA-2026 문서"),
            v(&[("한국", 0), ("마사회", 1), ("내부", 3), ("통제", 4), ("규정", 5), ("2026", 7), ("개정", 10), ("kra", 15), ("2026", 17), ("문서", 18)])
        );
    }

    #[test]
    fn morph_inflected_verbs_become_stems() {
        let texts: Vec<String> = morphs("말들이 달렸고 빠른 말이 이겼습니다").into_iter().map(|x| x.0).collect();
        assert_eq!(texts, vec!["말", "달리", "빠르", "말", "이기"]);
    }
}
```

> 형태소 시험의 위치 값은 lindera 6.2.0 + mecab-ko-dic 2.1.1 에서 확인한 값이다(조사 2026-10-14). lindera·사전을 올리면 다시 확인한다.

- [ ] **Step 6: 통과 확인**

Run: `cargo test -p index --release && bash tools/check-no-net.sh`
Expected: PASS(토크나이저 4개 + meta 4개), `실행 의존에 HTTP 크레이트 없음`. 빌드 로그에 사전 다운로드가 없어야 한다(캐시 사용).

`cargo deny check` 가 설치돼 있으면 함께 돌린다(CI 의 deny 작업이 최종 확인).

- [ ] **Step 7: Commit**

```bash
git add crates/index tools/fetch-ko-dic.sh tools/check-no-net.sh .cargo/config.toml deny.toml .github/workflows/ci.yml CLAUDE.md Cargo.lock
git commit -m "index: Korean morpheme and Hangul bigram tokenizers, pinned ko-dic source

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 13: 키워드 색인 (`index::keyword`, tantivy BM25)

**Files:**
- Create: `crates/index/src/keyword.rs`
- Modify: `crates/index/src/lib.rs`

**Interfaces:**
- Consumes: `tokenize::{Analyzers, register, MORPH, BIGRAM}`(Task 12), `doc::Document`
- Produces:

```rust
// index::keyword
pub const BODY_MAX_CHARS: usize = 5_000_000;
pub struct KeywordDoc { pub id: u64, pub title: String, pub filenames: String, pub headings: String, pub body: String }
impl KeywordDoc {
    /// 제목은 문서 제목, 없으면 첫 파일 이름(확장자 뺀 것). 파일 이름은 확장자를 뺀 이름들을 줄바꿈으로 잇는다.
    /// 소제목 = Heading 블록, 본문 = 나머지 블록(최대 BODY_MAX_CHARS 글자).
    pub fn new(id: u64, doc: Option<&doc::Document>, file_names: &[String]) -> KeywordDoc;
}
pub struct KeywordIndex { /* tantivy Index, reader, writer(필요할 때만) */ }
impl KeywordIndex {
    pub fn open(dir: &Path, a: &Analyzers) -> tantivy::Result<KeywordIndex>;   // 없으면 만든다
    pub fn in_ram(a: &Analyzers) -> tantivy::Result<KeywordIndex>;            // 평가·시험
    pub fn upsert(&mut self, d: &KeywordDoc) -> tantivy::Result<()>;           // 같은 id 는 지우고 새로
    pub fn delete(&mut self, id: u64) -> tantivy::Result<()>;
    pub fn commit(&mut self) -> tantivy::Result<u64>;                          // opstamp
    pub fn opstamp(&self) -> u64;                                             // 마지막 커밋 opstamp(없으면 0)
    pub fn search(&self, query: &str, limit: usize) -> tantivy::Result<Vec<(u64, f32)>>;
    pub fn num_docs(&self) -> u64;
}
```

- 필드: `doc_id`(u64, FAST|INDEXED|STORED) + `title/filename/heading/body` 각각 `_m`(형태소)·`_b`(bigram), 모두 위치 포함.
- 검색어: 공백으로 나눈 낱말(최대 32개, 검색어 최대 1,000글자). 낱말마다 `*_m` 형태소 TermQuery + `*_b` bigram 구절(PhraseQuery, 토큰 1개면 TermQuery)을 Should 로 묶고, 필드 가중치 제목 3.0 · 파일명 2.5 · 소제목 2.0 · 본문 1.0(bigram 은 0.5배). 낱말끼리는 Should(많이 맞을수록 위).

- [ ] **Step 1: 실패하는 시험** — `keyword.rs` 의 `mod tests`(Step 3 파일 끝)를 쓰고 `lib.rs` 에 `pub mod keyword;` 추가 → Run: `cargo test -p index --release keyword` → 컴파일 실패

- [ ] **Step 2: (없음 — Step 3 에서 구현과 시험을 한 파일에 둔다)**

- [ ] **Step 3: `crates/index/src/keyword.rs`**

```rust
//! 키워드 색인(tantivy BM25). 문서 = 내용(content) 하나, doc_id = meta.db 의 content id.
//! 필드 가중치(스펙 §8.2): 제목 > 파일명 > 소제목 > 본문. 본문은 형태소 필드 + 한글 bigram 필드(다글자 낱말은 bigram 구절 일치).

use crate::tokenize::{register, Analyzers, BIGRAM, MORPH};
use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, BoostQuery, Occur, PhraseQuery, Query, TermQuery};
use tantivy::schema::{Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, FAST, INDEXED, STORED};
use tantivy::tokenizer::TokenStream;
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};

pub const BODY_MAX_CHARS: usize = 5_000_000;
const MAX_QUERY_CHARS: usize = 1_000;
const MAX_QUERY_WORDS: usize = 32;
/// 색인 쓰기 메모리 — 저사양 PC 를 생각해 작게(tantivy 최소 15MB/스레드).
const WRITER_BYTES: usize = 50_000_000;

pub struct KeywordDoc {
    pub id: u64,
    pub title: String,
    pub filenames: String,
    pub headings: String,
    pub body: String,
}

fn stem(name: &str) -> &str {
    Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name)
}

impl KeywordDoc {
    pub fn new(id: u64, doc: Option<&doc::Document>, file_names: &[String]) -> KeywordDoc {
        let filenames = file_names.iter().map(|n| stem(n)).collect::<Vec<_>>().join("\n");
        let title = doc
            .and_then(|d| d.title.clone())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| file_names.first().map(|n| stem(n).to_string()).unwrap_or_default());
        let (mut headings, mut body) = (String::new(), String::new());
        if let Some(d) = doc {
            for b in &d.blocks {
                let dst = if b.kind == doc::BlockKind::Heading { &mut headings } else { &mut body };
                if dst.len() < BODY_MAX_CHARS * 4 {
                    dst.push_str(&b.text);
                    dst.push('\n');
                }
            }
        }
        if body.chars().count() > BODY_MAX_CHARS {
            body = body.chars().take(BODY_MAX_CHARS).collect();
        }
        KeywordDoc { id, title, filenames, headings, body }
    }
}

struct Fields {
    doc_id: Field,
    /// (형태소 필드, bigram 필드, 가중치) — 제목, 파일명, 소제목, 본문 순
    pairs: [(Field, Field, f32); 4],
}

fn schema() -> (Schema, Fields) {
    let mut sb = Schema::builder();
    let doc_id = sb.add_u64_field("doc_id", FAST | INDEXED | STORED);
    let text = |tok: &str| {
        TextOptions::default().set_indexing_options(
            TextFieldIndexing::default().set_tokenizer(tok).set_index_option(IndexRecordOption::WithFreqsAndPositions),
        )
    };
    let mut pair = |name: &str, w: f32| (sb.add_text_field(&format!("{name}_m"), text(MORPH)), sb.add_text_field(&format!("{name}_b"), text(BIGRAM)), w);
    let pairs = [pair("title", 3.0), pair("filename", 2.5), pair("heading", 2.0), pair("body", 1.0)];
    (sb.build(), Fields { doc_id, pairs })
}

pub struct KeywordIndex {
    index: Index,
    reader: IndexReader,
    writer: Option<IndexWriter>,
    f: Fields,
}

impl KeywordIndex {
    pub fn open(dir: &Path, a: &Analyzers) -> tantivy::Result<KeywordIndex> {
        let (schema, f) = schema();
        std::fs::create_dir_all(dir)?;
        let index = if dir.join("meta.json").exists() { Index::open_in_dir(dir)? } else { Index::create_in_dir(dir, schema)? };
        Self::wrap(index, f, a)
    }

    pub fn in_ram(a: &Analyzers) -> tantivy::Result<KeywordIndex> {
        let (schema, f) = schema();
        Self::wrap(Index::create_in_ram(schema), f, a)
    }

    fn wrap(index: Index, f: Fields, a: &Analyzers) -> tantivy::Result<KeywordIndex> {
        register(&index, a);
        let reader = index.reader_builder().reload_policy(ReloadPolicy::Manual).try_into()?;
        Ok(KeywordIndex { index, reader, writer: None, f })
    }

    /// 쓰기 잠금은 실제로 쓸 때만 잡는다(검색만 하는 프로세스가 잠그지 않도록).
    fn writer(&mut self) -> tantivy::Result<&mut IndexWriter> {
        if self.writer.is_none() {
            self.writer = Some(self.index.writer_with_num_threads(1, WRITER_BYTES)?);
        }
        Ok(self.writer.as_mut().expect("writer"))
    }

    pub fn upsert(&mut self, d: &KeywordDoc) -> tantivy::Result<()> {
        let [(tm, tb, _), (fm, fb, _), (hm, hb, _), (bm, bb, _)] = self.f.pairs;
        let doc_id = self.f.doc_id;
        let mut doc = TantivyDocument::default();
        doc.add_u64(doc_id, d.id);
        for (m, b, text) in [(tm, tb, &d.title), (fm, fb, &d.filenames), (hm, hb, &d.headings), (bm, bb, &d.body)] {
            if !text.is_empty() {
                doc.add_text(m, text);
                doc.add_text(b, text);
            }
        }
        let w = self.writer()?;
        w.delete_term(Term::from_field_u64(doc_id, d.id));
        w.add_document(doc)?;
        Ok(())
    }

    pub fn delete(&mut self, id: u64) -> tantivy::Result<()> {
        let doc_id = self.f.doc_id;
        self.writer()?.delete_term(Term::from_field_u64(doc_id, id));
        Ok(())
    }

    pub fn commit(&mut self) -> tantivy::Result<u64> {
        let op = self.writer()?.commit()?;
        self.reader.reload()?;
        Ok(op)
    }

    pub fn opstamp(&self) -> u64 {
        self.index.load_metas().map(|m| m.opstamp).unwrap_or(0)
    }

    pub fn num_docs(&self) -> u64 {
        self.reader.searcher().num_docs()
    }

    fn build_query(&self, query: &str) -> tantivy::Result<Option<Box<dyn Query>>> {
        let query: String = query.chars().take(MAX_QUERY_CHARS).collect();
        let mut words: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for word in query.split_whitespace().take(MAX_QUERY_WORDS) {
            let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
            for &(mf, bf, w) in &self.f.pairs {
                let mut morph = self.index.tokenizer_for_field(mf)?;
                let mut ts = morph.token_stream(word);
                while let Some(tok) = ts.next() {
                    let tq = TermQuery::new(Term::from_field_text(mf, &tok.text), IndexRecordOption::WithFreqs);
                    clauses.push((Occur::Should, Box::new(BoostQuery::new(Box::new(tq), w))));
                }
                let mut bigram = self.index.tokenizer_for_field(bf)?;
                let mut ts = bigram.token_stream(word);
                let mut terms: Vec<(usize, Term)> = Vec::new();
                while let Some(tok) = ts.next() {
                    terms.push((tok.position, Term::from_field_text(bf, &tok.text)));
                }
                let bq: Box<dyn Query> = match terms.len() {
                    0 => continue,
                    1 => Box::new(TermQuery::new(terms.pop().expect("1개").1, IndexRecordOption::WithFreqs)),
                    _ => Box::new(PhraseQuery::new_with_offset(terms)), // 2개 미만이면 패닉하므로 위에서 나눔
                };
                clauses.push((Occur::Should, Box::new(BoostQuery::new(bq, w * 0.5))));
            }
            if !clauses.is_empty() {
                words.push((Occur::Should, Box::new(BooleanQuery::new(clauses))));
            }
        }
        Ok((!words.is_empty()).then(|| Box::new(BooleanQuery::new(words)) as Box<dyn Query>))
    }

    pub fn search(&self, query: &str, limit: usize) -> tantivy::Result<Vec<(u64, f32)>> {
        let Some(q) = self.build_query(query)? else { return Ok(Vec::new()) };
        let searcher = self.reader.searcher();
        let hits = searcher.search(&q, &TopDocs::with_limit(limit.max(1)).order_by_score())?;
        let mut out = Vec::with_capacity(hits.len());
        for (score, addr) in hits {
            let d: TantivyDocument = searcher.doc(addr)?;
            if let Some(id) = d.get_first(self.f.doc_id).and_then(|v| v.as_u64()) {
                out.push((id, score));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::{Block, BlockKind, Document};

    fn analyzers() -> Analyzers {
        Analyzers::load().unwrap()
    }

    fn d(title: Option<&str>, heads: &[&str], body: &[&str]) -> Document {
        let mut blocks: Vec<Block> = heads.iter().map(|h| Block { kind: BlockKind::Heading, text: h.to_string() }).collect();
        blocks.extend(body.iter().map(|b| Block { kind: BlockKind::Paragraph, text: b.to_string() }));
        Document { title: title.map(String::from), blocks }
    }

    fn ids(kw: &KeywordIndex, q: &str) -> Vec<u64> {
        kw.search(q, 10).unwrap().into_iter().map(|x| x.0).collect()
    }

    fn index_of(docs: &[(u64, Document, &str)]) -> KeywordIndex {
        let mut kw = KeywordIndex::in_ram(&analyzers()).unwrap();
        for (id, doc, name) in docs {
            kw.upsert(&KeywordDoc::new(*id, Some(doc), &[name.to_string()])).unwrap();
        }
        kw.commit().unwrap();
        kw
    }

    #[test]
    fn title_beats_body() {
        let kw = index_of(&[
            (1, d(None, &[], &["이 문서는 내부통제 관련 회의 결과를 정리했다."]), "회의록.hwp"),
            (2, d(Some("내부통제규정 개정안"), &[], &["개정 이유"]), "개정안.hwp"),
        ]);
        assert_eq!(ids(&kw, "내부통제"), vec![2, 1]);
    }

    #[test]
    fn compound_and_spaced_queries_match() {
        let kw = index_of(&[
            (1, d(None, &[], &["더비온 행사를 운영한다.", "내부통제규정을 개정했다."]), "a.hwp"),
            (2, d(None, &[], &["경마 실적 보고"]), "b.hwp"),
        ]);
        assert_eq!(ids(&kw, "더비온"), vec![1], "사전에 없는 합성어는 bigram 이 찾는다");
        assert_eq!(ids(&kw, "통제 규정"), vec![1]);
        assert_eq!(ids(&kw, "달렸다"), Vec::<u64>::new());
    }

    #[test]
    fn filename_without_extension_is_searchable() {
        let kw = index_of(&[(7, d(None, &[], &[]), "2026 렛츠런파크 안전점검 결과.hwp")]);
        assert_eq!(ids(&kw, "안전점검"), vec![7]);
        assert_eq!(ids(&kw, "hwp"), Vec::<u64>::new(), "확장자는 색인하지 않는다");
    }

    #[test]
    fn upsert_replaces_and_delete_removes() {
        let mut kw = index_of(&[(1, d(None, &[], &["사과 배"]), "a.txt")]);
        kw.upsert(&KeywordDoc::new(1, Some(&d(None, &[], &["포도"])), &["a.txt".into()])).unwrap();
        kw.commit().unwrap();
        assert_eq!(ids(&kw, "사과"), Vec::<u64>::new());
        assert_eq!(ids(&kw, "포도"), vec![1]);
        assert_eq!(kw.num_docs(), 1);
        kw.delete(1).unwrap();
        kw.commit().unwrap();
        assert_eq!(kw.num_docs(), 0);
    }

    #[test]
    fn reopen_keeps_documents_and_opstamp() {
        let dir = tempfile::tempdir().unwrap();
        let a = analyzers();
        let op = {
            let mut kw = KeywordIndex::open(dir.path(), &a).unwrap();
            kw.upsert(&KeywordDoc::new(3, Some(&d(None, &[], &["경마 시행 계획"])), &["x.hwp".into()])).unwrap();
            kw.commit().unwrap()
        };
        let kw = KeywordIndex::open(dir.path(), &a).unwrap();
        assert_eq!(ids(&kw, "경마"), vec![3]);
        assert_eq!(kw.opstamp(), op);
    }

    #[test]
    fn fullwidth_query_matches_halfwidth_text() {
        let kw = index_of(&[(1, d(None, &[], &["KRA-2026 문서번호"]), "a.hwp")]);
        assert_eq!(ids(&kw, "ＫＲＡ"), vec![1]);
        assert_eq!(ids(&kw, "kra"), vec![1]);
    }

    #[test]
    fn odd_queries_do_not_panic() {
        let kw = index_of(&[(1, d(Some("가"), &[], &["가나다 라마바"]), "a.hwp")]);
        let long = "가".repeat(5_000);
        for q in ["", "   ", "!!!", "\"", "'\"(){}[]", "가", long.as_str(), "가 나 다 라 마 바 사 아 자 차 카 타 파 하 가 나 다 라 마 바 사 아 자 차 카 타 파 하 가 나 다 라 마"] {
            let _ = kw.search(q, 10).unwrap();
        }
        assert!(ids(&kw, "").is_empty());
    }
}
```

`crates/index/Cargo.toml` 의 `[dev-dependencies]` 에 `tempfile = "3"` 이 이미 있는지 확인(Task 11).

- [ ] **Step 4: 통과 확인** — Run: `cargo test -p index --release` → PASS(keyword 7개 포함)

  기대와 다르면(예: `title_beats_body` 순서) 가중치를 바꾸지 말고 먼저 토큰을 찍어 원인을 본다 — 가중치 조정은 M2 `--eval` 의 일이다.

- [ ] **Step 5: Commit**

```bash
git add crates/index
git commit -m "index: tantivy keyword index with weighted morpheme and bigram fields

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---
### Task 14: 폴더 훑기 (`crawler`) — 제외 규칙·차이 계산·내용 해시

**Files:**
- Create: `crates/crawler/Cargo.toml`, `crates/crawler/src/lib.rs`

**Interfaces:**
- Consumes: `extract::format::Format::from_path`(대상 확장자)
- Produces:

```rust
pub struct Scanned { pub path: String, pub size: u64, pub mtime_ns: i64 }
pub struct ScanOptions { pub excluded_dirs: Vec<String>, pub excluded_prefixes: Vec<PathBuf> }   // 이름은 소문자 비교
impl Default for ScanOptions  // 스펙 §6.3 기본 제외 + %TEMP%/%TMP%
pub struct ScanReport { pub files: Vec<Scanned>, pub missing_roots: Vec<PathBuf>, pub skipped_non_utf8: usize, pub unreadable: usize }
pub fn scan(roots: &[PathBuf], opt: &ScanOptions) -> ScanReport;
pub struct Diff { pub changed: Vec<Scanned>, pub removed: Vec<String> }   // changed: 새 파일·크기/시각 바뀐 파일(최근 수정 먼저)
/// `known` 중 이번에 안 보인 경로는 지운 것으로 본다. 단 사라진 루트(USB 빠짐 등) 아래 경로는 남긴다.
pub fn diff(scanned: &[Scanned], known: &HashMap<String, (u64, i64)>, missing_roots: &[PathBuf]) -> Diff;
pub fn hash_file(path: &Path) -> std::io::Result<u128>;   // xxh3-128, 64KiB 씩
```

- [ ] **Step 1: `crates/crawler/Cargo.toml`**

```toml
[package]
name = "crawler"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
extract = { path = "../extract" }
walkdir = "2"
xxhash-rust = { version = "0.8", features = ["xxh3"] }

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: 실패하는 시험 + 구현** — `crates/crawler/src/lib.rs` (시험을 먼저 쓰고 `cargo test -p crawler --release` 로 실패를 본 뒤 함수를 채운다)

```rust
//! 폴더 훑기: 대상 확장자 파일의 경로·크기·수정 시각을 모으고, 지난번과 비교해 바뀐 것만 고른다.
//! 폴더 감시(USN·ReadDirectoryChangesW)는 M5·M6 — M1 은 `kra-search index` 실행 때마다 훑는다.

use extract::format::Format;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq)]
pub struct Scanned {
    pub path: String,
    pub size: u64,
    pub mtime_ns: i64,
}

/// 스펙 §6.3 기본 제외(이름, 대소문자 무시) + 개발 폴더.
pub const DEFAULT_EXCLUDED_DIRS: &[&str] = &[
    "windows", "program files", "program files (x86)", "appdata", "$recycle.bin",
    "system volume information", ".git", "node_modules",
];

pub struct ScanOptions {
    pub excluded_dirs: Vec<String>,
    pub excluded_prefixes: Vec<PathBuf>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        let excluded_prefixes = ["TEMP", "TMP"].iter().filter_map(|v| std::env::var_os(v)).map(PathBuf::from).collect();
        ScanOptions { excluded_dirs: DEFAULT_EXCLUDED_DIRS.iter().map(|s| s.to_string()).collect(), excluded_prefixes }
    }
}

#[derive(Debug, Default)]
pub struct ScanReport {
    pub files: Vec<Scanned>,
    pub missing_roots: Vec<PathBuf>,
    pub skipped_non_utf8: usize,
    pub unreadable: usize,
}

/// 대상 파일인가: 지원 확장자이고 Office 임시 파일(`~$…`)이 아님.
fn is_target(p: &Path) -> bool {
    let office_temp = p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("~$"));
    !office_temp && Format::from_path(p).is_some()
}

fn mtime_ns(m: &std::fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos().min(i64::MAX as u128) as i64).unwrap_or(0)
}

pub fn scan(roots: &[PathBuf], opt: &ScanOptions) -> ScanReport {
    let mut r = ScanReport::default();
    let excluded: HashSet<String> = opt.excluded_dirs.iter().map(|s| s.to_lowercase()).collect();
    for root in roots {
        if !root.is_dir() {
            r.missing_roots.push(root.clone());
            continue;
        }
        let walker = WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| {
            if e.depth() == 0 || !e.file_type().is_dir() {
                return true;
            }
            let name_ok = e.file_name().to_str().map_or(true, |n| !excluded.contains(&n.to_lowercase()));
            name_ok && !opt.excluded_prefixes.iter().any(|p| e.path().starts_with(p))
        });
        for entry in walker {
            let e = match entry {
                Ok(e) => e,
                Err(_) => {
                    r.unreadable += 1; // 권한 없는 폴더·사라진 파일 — 그것만 건너뛴다
                    continue;
                }
            };
            if !e.file_type().is_file() || !is_target(e.path()) {
                continue;
            }
            let Some(path) = e.path().to_str() else {
                r.skipped_non_utf8 += 1;
                continue;
            };
            match e.metadata() {
                Ok(m) => r.files.push(Scanned { path: path.to_string(), size: m.len(), mtime_ns: mtime_ns(&m) }),
                Err(_) => r.unreadable += 1,
            }
        }
    }
    r
}

pub struct Diff {
    pub changed: Vec<Scanned>,
    pub removed: Vec<String>,
}

pub fn diff(scanned: &[Scanned], known: &HashMap<String, (u64, i64)>, missing_roots: &[PathBuf]) -> Diff {
    let mut changed: Vec<Scanned> = scanned
        .iter()
        .filter(|s| known.get(&s.path) != Some(&(s.size, s.mtime_ns)))
        .cloned()
        .collect();
    changed.sort_by(|a, b| b.mtime_ns.cmp(&a.mtime_ns)); // 최근 수정 먼저
    let seen: HashSet<&str> = scanned.iter().map(|s| s.path.as_str()).collect();
    let mut removed: Vec<String> = known
        .keys()
        .filter(|p| !seen.contains(p.as_str()))
        .filter(|p| !missing_roots.iter().any(|r| Path::new(p.as_str()).starts_with(r)))
        .cloned()
        .collect();
    removed.sort();
    Diff { changed, removed }
}

pub fn hash_file(path: &Path) -> std::io::Result<u128> {
    let mut f = std::fs::File::open(path)?;
    let mut h = xxhash_rust::xxh3::Xxh3Default::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.digest128())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    fn names(r: &ScanReport) -> Vec<String> {
        let mut v: Vec<String> = r.files.iter().map(|s| Path::new(&s.path).file_name().unwrap().to_str().unwrap().to_string()).collect();
        v.sort();
        v
    }

    #[test]
    fn scans_targets_and_skips_excluded() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        touch(&root.join("보고서.hwp"), b"a");
        touch(&root.join("하위/표.XLSX"), b"b");
        touch(&root.join("하위/~$임시.docx"), b"c");
        touch(&root.join("하위/메모.doc"), b"d");
        touch(&root.join("AppData/설정.txt"), b"e");
        touch(&root.join("node_modules/x.txt"), b"f");
        touch(&root.join("임시폴더/y.txt"), b"g");
        let opt = ScanOptions { excluded_prefixes: vec![root.join("임시폴더")], ..ScanOptions::default() };
        let r = scan(&[root.to_path_buf()], &opt);
        assert_eq!(names(&r), vec!["보고서.hwp", "표.XLSX"]);
    }

    #[test]
    fn long_paths_are_scanned() {
        let d = tempfile::tempdir().unwrap();
        let mut p = d.path().to_path_buf();
        while p.as_os_str().len() < 300 {
            p.push("아주긴폴더이름입니다");
        }
        touch(&p.join("깊은 문서.txt"), b"x");
        let r = scan(&[d.path().to_path_buf()], &ScanOptions::default());
        assert_eq!(names(&r), vec!["깊은 문서.txt"]);
    }

    #[cfg(unix)]
    #[test]
    fn skips_unreadable_and_non_utf8_without_aborting() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("ok.txt"), b"x");
        let locked = d.path().join("잠긴폴더");
        touch(&locked.join("숨은.txt"), b"y");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let r = scan(&[d.path().to_path_buf()], &ScanOptions::default());
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(names(&r), vec!["ok.txt"]);
        assert!(r.unreadable >= 1);
    }

    #[cfg(windows)]
    #[test]
    fn skips_unreadable_and_non_utf8_without_aborting() {
        use std::os::windows::ffi::OsStringExt;
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("ok.txt"), b"x");
        // 짝 없는 서로게이트(0xD800) — UTF-8 로 바꿀 수 없는 Windows 파일 이름
        let bad: Vec<u16> = "깨진".encode_utf16().chain([0xD800]).chain(".txt".encode_utf16()).collect();
        std::fs::write(d.path().join(std::ffi::OsString::from_wide(&bad)), b"y").unwrap();
        let r = scan(&[d.path().to_path_buf()], &ScanOptions::default());
        assert_eq!(names(&r), vec!["ok.txt"]);
        assert_eq!(r.skipped_non_utf8, 1);
    }

    #[test]
    fn diff_finds_changes_and_keeps_missing_roots() {
        let s = |p: &str, size: u64, t: i64| Scanned { path: p.into(), size, mtime_ns: t };
        let known: HashMap<String, (u64, i64)> = [
            ("/a/same.hwp".to_string(), (1, 1)),
            ("/a/edited.hwp".to_string(), (1, 1)),
            ("/a/gone.hwp".to_string(), (1, 1)),
            ("/usb/kept.hwp".to_string(), (1, 1)),
        ]
        .into();
        let scanned = vec![s("/a/same.hwp", 1, 1), s("/a/edited.hwp", 2, 5), s("/a/new.hwp", 1, 9)];
        let d = diff(&scanned, &known, &[PathBuf::from("/usb")]);
        assert_eq!(d.changed.iter().map(|x| x.path.as_str()).collect::<Vec<_>>(), vec!["/a/new.hwp", "/a/edited.hwp"]);
        assert_eq!(d.removed, vec!["/a/gone.hwp".to_string()]);
    }

    #[test]
    fn missing_root_is_reported() {
        let r = scan(&[PathBuf::from("/없는/루트")], &ScanOptions::default());
        assert_eq!(r.missing_roots, vec![PathBuf::from("/없는/루트")]);
    }

    #[test]
    fn hash_matches_one_shot_and_detects_change() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.bin");
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        assert_eq!(hash_file(&p).unwrap(), xxhash_rust::xxh3::xxh3_128(&data));
        std::fs::write(&p, &data[1..]).unwrap();
        assert_ne!(hash_file(&p).unwrap(), xxhash_rust::xxh3::xxh3_128(&data));
    }
}
```

- [ ] **Step 3: 통과 확인** — Run: `cargo test -p crawler --release` → PASS(macOS: 6개, Windows CI: 비-UTF-8 시험 포함 6개)

- [ ] **Step 4: Commit**

```bash
git add crates/crawler Cargo.lock
git commit -m "crawler: folder scan with default excludes, diff and xxh3-128 hashing

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 15: `kra-search` 명령 — 색인 파이프라인·검색·상태·워커

**Files:**
- Create: `crates/app/Cargo.toml`, `crates/app/src/lib.rs`, `crates/app/src/store.rs`, `crates/app/src/pipeline.rs`, `crates/app/src/search.rs`, `crates/app/src/main.rs`, `crates/app/tests/cli.rs`, `crates/app/tests/pipeline.rs`

**Interfaces:**
- Consumes: `extract::{worker::{Worker, serve}, cli::extract_one, pdf, runner::Outcome, format::MAX_FILE_BYTES}`, `index::{meta::*, keyword::*, tokenize::Analyzers}`, `crawler::*`, `platform::{paths::Home, keys}`
- Produces (명령줄 — 사용자·이후 마일스톤이 씀):

```
kra-search [--home <폴더>] [--pdfium <절대 경로>] <명령>
  index [--root <폴더>]...      # 루트를 주면 저장된 루트를 바꾼다. 없으면 저장된 루트(처음이면 문서·바탕화면·다운로드)
  search <검색어> [--limit 20]
  status
  worker                        # (숨김) 상주 워커 — 앱이 띄운다
```

```rust
// app (lib)
pub struct Store { pub meta: Meta, pub keyword: KeywordIndex }
pub fn open_store(home: &Home, a: &Analyzers) -> Result<Store, String>;          // 키 분실·DB 손상·색인 유실을 스스로 복구(스펙 §5.3·§6.8)
pub struct IndexSummary { pub scanned: usize, pub changed: usize, pub removed: usize, pub extracted: usize, pub reused: usize, pub failed: usize, pub unreadable: usize }
pub fn run_index(store: &mut Store, worker: &mut Worker, roots: &[PathBuf], timeout: Duration) -> Result<IndexSummary, String>;
pub fn flush(store: &mut Store) -> Result<usize, String>;                        // Pending·고아를 키워드 색인에 반영, 반영 개수
pub struct Hit { pub content_id: i64, pub score: f32, pub title: String, pub paths: Vec<String>, pub fail: Option<String> }
pub fn search(store: &Store, query: &str, limit: usize) -> Result<Vec<Hit>, String>;
pub fn fail_label(fail: &str) -> &'static str;                                   // 사용자용 쉬운 말
```

- [ ] **Step 1: `crates/app/Cargo.toml`**

```toml
[package]
name = "app"
version.workspace = true
edition.workspace = true
license.workspace = true

[[bin]]
name = "kra-search"
path = "src/main.rs"

[dependencies]
doc = { path = "../doc" }
extract = { path = "../extract" }
index = { path = "../index" }
crawler = { path = "../crawler" }
platform = { path = "../platform" }
clap = { version = "4", features = ["derive"] }

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: 실패하는 시험** — `crates/app/tests/cli.rs` (명령을 실제로 실행하는 종단 시험)

```rust
//! kra-search 종단 시험: 임시 홈·임시 문서 폴더에서 index → search.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../extract/tests/fixtures")
}

fn pdfium() -> PathBuf {
    let plat = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "mac-arm64/lib",
        ("macos", "x86_64") => "mac-x64/lib",
        ("windows", "x86_64") => "win-x64/bin",
        _ => "linux-x64/lib",
    };
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/pdfium").join(plat).join(extract::pdf::LIB_NAME).canonicalize().expect("bash tools/fetch-pdfium.sh 먼저")
}

struct Env {
    home: tempfile::TempDir,
    docs: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        let docs = tempfile::tempdir().unwrap();
        for (src, dst) in [
            ("office/sample.docx", "운영계획.docx"),
            ("office/sample.docx", "운영계획 - 복사본.docx"),
            ("schedule.hwp", "추진일정.hwp"),
            ("pdf/korean.pdf", "시행계획.pdf"),
            ("office/cp949.csv", "예산.csv"),
        ] {
            std::fs::copy(fixtures().join(src), docs.path().join(dst)).unwrap();
        }
        std::fs::write(docs.path().join("깨진문서.hwp"), b"garbage bytes").unwrap();
        Env { home: tempfile::tempdir().unwrap(), docs }
    }

    fn run(&self, args: &[&str]) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_kra-search"))
            .arg("--home").arg(self.home.path())
            .arg("--pdfium").arg(pdfium())
            .args(args)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(out.status.success(), "{args:?} 실패\nstdout:\n{stdout}\nstderr:\n{stderr}");
        // 안내 문장(복구 등)은 stderr 로 나온다 — 시험은 둘 다 본다.
        format!("{stdout}{stderr}")
    }

    fn index(&self) -> String {
        self.run(&["index", "--root", self.docs.path().to_str().unwrap()])
    }
}

#[test]
fn index_and_search_all_formats() {
    let e = Env::new();
    let s = e.index();
    assert!(s.contains("파일 6개"), "{s}");
    let r = e.run(&["search", "렛츠런파크"]);
    assert!(r.contains("운영계획") && r.contains("시행계획.pdf"), "{r}");
    assert!(r.contains("같은 내용 1개 더"), "복사본은 결과 한 줄로 묶인다:\n{r}");
    let r = e.run(&["search", "경마 시행"]);
    assert!(r.contains("예산.csv"), "{r}");
    let r = e.run(&["search", "깨진문서"]);
    assert!(r.contains("깨진문서.hwp") && r.contains("내용을 읽지 못함"), "실패한 파일도 이름으로 찾힌다:\n{r}");
    let st = e.run(&["status"]);
    assert!(st.contains("파일 6") && st.contains("내용 5"), "{st}");
}

#[test]
fn reindex_without_changes_does_nothing() {
    let e = Env::new();
    e.index();
    let s = e.index();
    assert!(s.contains("바뀐 파일 0개"), "{s}");
}

#[test]
fn duplicates_rename_and_delete() {
    let e = Env::new();
    e.index();
    let d = e.docs.path();
    std::fs::rename(d.join("운영계획.docx"), d.join("2026 사업계획.docx")).unwrap();
    e.index();
    let r = e.run(&["search", "렛츠런파크"]);
    // 같은 내용의 경로는 모두 출력된다. 옛 이름("…/운영계획.docx")은 없어야 한다("운영계획 - 복사본.docx" 는 남음).
    assert!(r.contains("2026 사업계획.docx") && !r.contains("운영계획.docx"), "{r}");
    let r = e.run(&["search", "사업계획"]);
    assert!(r.contains("2026 사업계획.docx"), "새 이름으로 찾힌다:\n{r}");
    std::fs::remove_file(d.join("2026 사업계획.docx")).unwrap();
    std::fs::remove_file(d.join("운영계획 - 복사본.docx")).unwrap();
    e.index();
    let r = e.run(&["search", "글상자"]);
    assert!(!r.contains(".docx"), "마지막 경로가 지워지면 결과에서 사라진다:\n{r}");
}

#[test]
fn oversized_file_is_filename_only() {
    let e = Env::new();
    let f = std::fs::File::create(e.docs.path().join("대용량보고서.pdf")).unwrap();
    f.set_len(extract::format::MAX_FILE_BYTES + 1).unwrap();
    e.index();
    let r = e.run(&["search", "대용량보고서"]);
    assert!(r.contains("대용량보고서.pdf") && r.contains("100MB 초과"), "{r}");
}

#[test]
fn key_loss_rebuilds_index() {
    let e = Env::new();
    e.index();
    std::fs::write(e.home.path().join("keys/meta.key"), b"broken").unwrap();
    let s = e.index();
    assert!(s.contains("다시 만듭니다"), "{s}");
    let r = e.run(&["search", "렛츠런파크"]);
    assert!(r.contains("운영계획"), "{r}");
}

#[test]
fn keyword_index_loss_is_rebuilt() {
    let e = Env::new();
    e.index();
    std::fs::remove_dir_all(e.home.path().join("data/tantivy")).unwrap();
    e.index();
    let r = e.run(&["search", "렛츠런파크"]);
    assert!(r.contains("운영계획"), "{r}");
}
```

`crates/app/tests/pipeline.rs` (라이브러리 함수로 중단 상황을 흉내):

```rust
use app::{flush, open_store, run_index, search};
use extract::worker::Worker;
use index::tokenize::Analyzers;
use platform::paths::Home;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

#[test]
fn resume_after_interrupted_flush_has_no_duplicates() {
    let docs = tempfile::tempdir().unwrap();
    let fx = Path::new(env!("CARGO_MANIFEST_DIR")).join("../extract/tests/fixtures");
    std::fs::copy(fx.join("office/sample.docx"), docs.path().join("a.docx")).unwrap();
    std::fs::copy(fx.join("office/sample.odt"), docs.path().join("b.odt")).unwrap();
    let home_dir = tempfile::tempdir().unwrap();
    let home = Home::new(home_dir.path());
    let a = Analyzers::load().unwrap();
    let mut store = open_store(&home, &a).unwrap();
    let mut w = Worker::new(|| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kra-search"));
        c.arg("worker");
        c
    });
    run_index(&mut store, &mut w, &[docs.path().to_path_buf()], Duration::from_secs(30)).unwrap();
    assert_eq!(store.keyword.num_docs(), 2);

    // 키워드 커밋 뒤 meta 갱신 전에 죽은 상황: meta 는 모두 Pending 으로 남아 있다.
    store.meta.mark_all_pending().unwrap();
    drop(store);
    let mut store = open_store(&home, &a).unwrap();
    flush(&mut store).unwrap();
    assert_eq!(store.keyword.num_docs(), 2, "다시 써도 문서가 늘지 않는다");
    let hits = search(&store, "렛츠런파크", 10).unwrap();
    assert_eq!(hits.len(), 1, "{:?}", hits.iter().map(|h| &h.paths).collect::<Vec<_>>());
    assert_eq!(store.meta.counts().unwrap().pending, 0);
}
```

Run: `cargo test -p app --release` → 컴파일 실패(크레이트 비어 있음)

- [ ] **Step 3: `crates/app/src/store.rs`** — 열기와 스스로 복구

```rust
//! 색인 저장소 열기. 키를 못 풀거나 DB 가 깨졌으면 data 를 지우고 새로 만들고(스펙 §5.3),
//! 키워드 색인을 잃었으면 meta 의 내용을 모두 Pending 으로 돌려 다시 쓴다(스펙 §6.8). 사용자에게 대화상자를 띄우지 않는다(§3.3).

use index::keyword::KeywordIndex;
use index::meta::{Meta, MetaError};
use index::tokenize::Analyzers;
use platform::keys::{self, KeyError};
use platform::paths::Home;

pub struct Store {
    pub meta: Meta,
    pub keyword: KeywordIndex,
}

fn reset(home: &Home) -> Result<(), String> {
    eprintln!("색인을 열 수 없어 처음부터 다시 만듭니다.");
    match std::fs::remove_dir_all(home.data()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("색인 폴더를 지우지 못함: {e}")),
    }
    let _ = std::fs::remove_file(home.meta_key());
    Ok(())
}

fn load_key(home: &Home) -> Result<[u8; 32], String> {
    keys::load_or_create(&home.meta_key()).map_err(|e| format!("키 파일 오류: {e:?}"))
}

pub fn open_store(home: &Home, a: &Analyzers) -> Result<Store, String> {
    let key = match keys::load_or_create(&home.meta_key()) {
        Ok(k) => k,
        Err(KeyError::Unreadable) => {
            reset(home)?;
            load_key(home)?
        }
        Err(e) => return Err(format!("키 파일 오류: {e:?}")),
    };
    let mut meta = match Meta::open(&home.meta_db(), &key) {
        Ok(m) => m,
        Err(MetaError::WrongKeyOrCorrupt) => {
            reset(home)?;
            let key = load_key(home)?;
            Meta::open(&home.meta_db(), &key).map_err(|e| e.to_string())?
        }
        Err(e) => return Err(e.to_string()),
    };
    let keyword = match KeywordIndex::open(&home.keyword_dir(), a) {
        Ok(k) => k,
        Err(_) => {
            let _ = std::fs::remove_dir_all(home.keyword_dir());
            meta.mark_all_pending().map_err(|e| e.to_string())?;
            KeywordIndex::open(&home.keyword_dir(), a).map_err(|e| e.to_string())?
        }
    };
    // 키워드 색인이 meta 가 아는 마지막 커밋보다 뒤처졌으면(색인 폴더 유실·복원) 전부 다시 쓴다.
    if keyword.opstamp() < meta.keyword_opstamp().map_err(|e| e.to_string())? {
        meta.mark_all_pending().map_err(|e| e.to_string())?;
    }
    Ok(Store { meta, keyword })
}
```

- [ ] **Step 4: `crates/app/src/pipeline.rs`**

```rust
//! 색인 파이프라인(스펙 §6.1 의 1단계 "키워드"): 훑기 → 바뀐 파일만 해시 → 같은 내용이면 재사용, 아니면 워커로 추출
//! → meta 에 기록(Pending) → 주기적으로 키워드 색인에 반영(flush).

use crate::store::Store;
use extract::format::MAX_FILE_BYTES;
use extract::runner::Outcome;
use extract::worker::Worker;
use index::keyword::KeywordDoc;
use index::meta::ContentData;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 이만큼 처리할 때마다 키워드 색인에 반영한다(중간에 꺼져도 잃는 작업이 적도록).
const FLUSH_EVERY: usize = 200;
/// flush 한 번에 다시 쓰는 내용 수.
const FLUSH_BATCH: usize = 500;

#[derive(Debug, Default)]
pub struct IndexSummary {
    pub scanned: usize,
    pub changed: usize,
    pub removed: usize,
    pub extracted: usize,
    pub reused: usize,
    pub failed: usize,
    pub unreadable: usize,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn content_data(o: Outcome) -> ContentData {
    match o {
        Outcome::Ok { doc, distribution, .. } => ContentData { fail: None, title: doc.title.clone(), distribution, doc: Some(doc) },
        Outcome::Failed { kind, .. } => ContentData { fail: Some(format!("{kind:?}")), title: None, distribution: false, doc: None },
        Outcome::Timeout => ContentData { fail: Some("Timeout".into()), title: None, distribution: false, doc: None },
        Outcome::Crashed { .. } => ContentData { fail: Some("Crashed".into()), title: None, distribution: false, doc: None },
    }
}

fn extract(worker: &mut Worker, path: &Path, size: u64, timeout: Duration) -> ContentData {
    if size > MAX_FILE_BYTES {
        // 100MB 초과: 워커에 보내지 않는다(스펙 §6.5).
        return ContentData { fail: Some("TooLarge".into()), title: None, distribution: false, doc: None };
    }
    content_data(worker.extract(path, timeout))
}

fn file_name(p: &str) -> String {
    Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or(p).to_string()
}

pub fn run_index(store: &mut Store, worker: &mut Worker, roots: &[PathBuf], timeout: Duration) -> Result<IndexSummary, String> {
    let report = crawler::scan(roots, &crawler::ScanOptions::default());
    let known = store.meta.known_files().map_err(err)?;
    let diff = crawler::diff(&report.files, &known, &report.missing_roots);
    let mut s = IndexSummary { scanned: report.files.len(), changed: diff.changed.len(), removed: diff.removed.len(), unreadable: report.unreadable, ..Default::default() };

    for path in &diff.removed {
        store.meta.remove_file(path).map_err(err)?;
    }
    for (i, f) in diff.changed.iter().enumerate() {
        eprint!("\r문서 읽는 중 {}/{}", i + 1, diff.changed.len());
        let Ok(hash) = crawler::hash_file(Path::new(&f.path)) else {
            s.unreadable += 1;
            continue;
        };
        let id = match store.meta.content_by_hash(hash).map_err(err)? {
            Some(c) => {
                // 시간 초과·비정상 종료는 내용이 같아도 한 번 더 시도한다(스펙 §6.9 "1회 재시도").
                let retryable = matches!(c.fail.as_deref(), Some("Timeout") | Some("Crashed"));
                if retryable && c.retries == 0 {
                    let d = extract(worker, Path::new(&f.path), f.size, timeout);
                    s.failed += d.fail.is_some() as usize;
                    store.meta.retry_content(c.id, &d).map_err(err)?;
                    s.extracted += 1;
                } else {
                    s.reused += 1;
                }
                c.id
            }
            None => {
                let d = extract(worker, Path::new(&f.path), f.size, timeout);
                s.failed += d.fail.is_some() as usize;
                s.extracted += 1;
                store.meta.insert_content(hash, &d).map_err(err)?
            }
        };
        store.meta.upsert_file(&f.path, f.size, f.mtime_ns, id).map_err(err)?;
        if (i + 1) % FLUSH_EVERY == 0 {
            flush(store)?;
        }
    }
    if !diff.changed.is_empty() {
        eprintln!();
    }
    flush(store)?;
    Ok(s)
}

/// Pending 내용을 키워드 색인에 다시 쓰고, 경로가 하나도 없는 내용은 지운다.
/// 순서: 키워드 색인 쓰기 → 커밋 → meta 에 Indexed 기록(finish_flush). 그 사이에 꺼져도 다음 실행에서 같은 일을 다시 한다.
pub fn flush(store: &mut Store) -> Result<usize, String> {
    let mut total = 0;
    loop {
        let pending = store.meta.pending(FLUSH_BATCH).map_err(err)?;
        let orphans = store.meta.orphans().map_err(err)?;
        if pending.is_empty() && orphans.is_empty() {
            return Ok(total);
        }
        let mut indexed = Vec::new();
        for &id in &pending {
            if orphans.contains(&id) {
                continue;
            }
            let names: Vec<String> = store.meta.files_of(id).map_err(err)?.iter().map(|f| file_name(&f.path)).collect();
            let doc = store.meta.doc(id).map_err(err)?;
            store.keyword.upsert(&KeywordDoc::new(id as u64, doc.as_ref(), &names)).map_err(err)?;
            indexed.push(id);
        }
        for &id in &orphans {
            store.keyword.delete(id as u64).map_err(err)?;
        }
        let op = store.keyword.commit().map_err(err)?;
        store.meta.finish_flush(&indexed, &orphans, op).map_err(err)?;
        total += indexed.len() + orphans.len();
    }
}
```

- [ ] **Step 5: `crates/app/src/search.rs`**

```rust
//! 검색: 키워드 색인 → 내용 id → meta 의 제목·경로.

use crate::store::Store;
use std::path::Path;

#[derive(Debug)]
pub struct Hit {
    pub content_id: i64,
    pub score: f32,
    pub title: String,
    /// 같은 내용의 경로들(최근 수정 먼저)
    pub paths: Vec<String>,
    pub fail: Option<String>,
}

/// 실패 유형 → 화면에 보일 쉬운 말(스펙 §3.3·UI D7).
pub fn fail_label(fail: &str) -> &'static str {
    match fail {
        "Drm" => "보안 문서(DRM)라 파일 이름만 찾을 수 있음",
        "Encrypted" => "암호가 걸려 파일 이름만 찾을 수 있음",
        "TooLarge" => "100MB 초과라 파일 이름만 찾을 수 있음",
        "NoText" => "글자가 없는 문서(스캔 등)라 파일 이름만 찾을 수 있음",
        "Timeout" => "읽는 데 너무 오래 걸려 파일 이름만 찾을 수 있음",
        _ => "내용을 읽지 못함 — 파일 이름만 찾을 수 있음",
    }
}

pub fn search(store: &Store, query: &str, limit: usize) -> Result<Vec<Hit>, String> {
    let hits = store.keyword.search(query, limit).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (id, score) in hits {
        let id = id as i64;
        let Some(c) = store.meta.content(id).map_err(|e| e.to_string())? else { continue };
        let paths: Vec<String> = store.meta.files_of(id).map_err(|e| e.to_string())?.into_iter().map(|f| f.path).collect();
        if paths.is_empty() {
            continue; // 아직 반영 안 된 삭제
        }
        let title = c.title.clone().unwrap_or_else(|| {
            Path::new(&paths[0]).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string()
        });
        out.push(Hit { content_id: id, score, title, paths, fail: c.fail });
    }
    Ok(out)
}
```

- [ ] **Step 6: `crates/app/src/lib.rs`**

```rust
//! kra-search 앱(M1: 명령줄). 색인의 유일한 쓰기 주체(스펙 §5.1). 화면(Tauri)은 M3.
mod pipeline;
mod search;
mod store;

pub use pipeline::{flush, run_index, IndexSummary};
pub use search::{fail_label, search, Hit};
pub use store::{open_store, Store};
```

- [ ] **Step 7: `crates/app/src/main.rs`**

```rust
//! kra-search 명령줄. `kra-search index`, `kra-search search "검색어"`, `kra-search status`.

use app::{fail_label, open_store, run_index, search};
use clap::{Parser, Subcommand};
use extract::worker::Worker;
use index::tokenize::Analyzers;
use platform::paths::Home;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "kra-search", version, about = "KRA 문서검색 (M1 명령줄)")]
struct Cli {
    /// 색인 저장 위치(기본: %LOCALAPPDATA%\KraSearch)
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    /// pdfium 라이브러리 절대 경로(기본: 실행 파일 옆)
    #[arg(long, global = true, hide = true)]
    pdfium: Option<PathBuf>,
    /// 파일당 제한 시간(초)
    #[arg(long, global = true, hide = true, default_value_t = 30)]
    timeout_secs: u64,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 문서를 읽어 색인한다(바뀐 파일만)
    Index {
        /// 색인할 폴더(여러 번 지정 가능). 지정하면 저장된 폴더 목록을 바꾼다.
        #[arg(long)]
        root: Vec<PathBuf>,
    },
    /// 색인에서 찾는다
    Search {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// 색인 현황
    Status,
    /// (내부용) 상주 추출 워커
    #[command(hide = true)]
    Worker,
}

/// pdfium 은 명시 경로만 쓴다: --pdfium, 없으면 실행 파일과 같은 폴더(스펙 §7.6).
fn pdfium_path(arg: &Option<PathBuf>) -> Option<PathBuf> {
    arg.clone().or_else(|| {
        let p = std::env::current_exe().ok()?.parent()?.join(extract::pdf::LIB_NAME);
        p.exists().then_some(p)
    })
}

fn default_roots() -> Vec<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    home.map(|h| ["Documents", "Desktop", "Downloads"].iter().map(|d| h.join(d)).filter(|p| p.is_dir()).collect()).unwrap_or_default()
}

fn worker_cmd(pdfium: Option<PathBuf>) -> impl Fn() -> Command + Send + 'static {
    move || {
        let mut c = Command::new(std::env::current_exe().expect("실행 파일 경로"));
        c.arg("worker");
        if let Some(p) = &pdfium {
            c.arg("--pdfium").arg(p);
        }
        c
    }
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("오류: {e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let pdfium = pdfium_path(&cli.pdfium);
    if let Cmd::Worker = cli.cmd {
        if let Some(p) = &pdfium {
            // 실패해도 워커는 돈다 — PDF 만 "읽지 못함"이 된다.
            let _ = extract::pdf::init(p);
        }
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        return extract::worker::serve(stdin.lock(), stdout.lock(), extract::cli::extract_one).map_err(|e| e.to_string());
    }
    let home = Home::new(cli.home.clone().unwrap_or_else(Home::default_root));
    let analyzers = Analyzers::load()?;
    let mut store = open_store(&home, &analyzers)?;
    match cli.cmd {
        Cmd::Index { root } => {
            if !root.is_empty() {
                let roots: Vec<String> = root.iter().map(|r| r.canonicalize().unwrap_or_else(|_| r.clone()).to_string_lossy().into_owned()).collect();
                store.meta.set_roots(&roots).map_err(|e| e.to_string())?;
            }
            let mut roots: Vec<PathBuf> = store.meta.roots().map_err(|e| e.to_string())?.into_iter().map(PathBuf::from).collect();
            if roots.is_empty() {
                roots = default_roots();
                let rs: Vec<String> = roots.iter().map(|r| r.to_string_lossy().into_owned()).collect();
                store.meta.set_roots(&rs).map_err(|e| e.to_string())?;
            }
            let mut worker = Worker::new(worker_cmd(pdfium));
            let s = run_index(&mut store, &mut worker, &roots, Duration::from_secs(cli.timeout_secs))?;
            println!(
                "파일 {}개 확인 · 바뀐 파일 {}개 · 지운 파일 {}개 · 새로 읽음 {}개(못 읽음 {}개) · 같은 내용 재사용 {}개 · 접근 못 함 {}개",
                s.scanned, s.changed, s.removed, s.extracted, s.failed, s.reused, s.unreadable
            );
        }
        Cmd::Search { query, limit } => {
            let hits = search(&store, &query, limit)?;
            if hits.is_empty() {
                println!("결과 없음");
            }
            for (i, h) in hits.iter().enumerate() {
                println!("{:>3}. {}  [{:.2}]", i + 1, h.title, h.score);
                println!("     {}", h.paths[0]);
                if h.paths.len() > 1 {
                    println!("     같은 내용 {}개 더: {}", h.paths.len() - 1, h.paths[1..].join(" | "));
                }
                if let Some(f) = &h.fail {
                    println!("     ※ {}", fail_label(f));
                }
            }
        }
        Cmd::Status => {
            let c = store.meta.counts().map_err(|e| e.to_string())?;
            let roots = store.meta.roots().map_err(|e| e.to_string())?;
            println!("색인 폴더: {}", if roots.is_empty() { "(없음)".to_string() } else { roots.join(", ") });
            println!("파일 {} · 내용 {} · 내용을 못 읽은 것 {} · 반영 대기 {} · 키워드 색인 문서 {}", c.files, c.contents, c.failed, c.pending, store.keyword.num_docs());
            println!("저장 위치: {}", Path::new(home.root()).display());
        }
        Cmd::Worker => unreachable!(),
    }
    Ok(())
}
```

> `kra-search` 는 개발 명령줄이다. 경로를 터미널에 출력하는 것은 사용자 자신의 화면이라 괜찮지만, **진단 보고서(kra-diag)에는 이 출력이 들어가지 않는다.**

- [ ] **Step 8: 통과 확인**

Run: `bash tools/fetch-pdfium.sh && cargo test -p app --release`
Expected: PASS(cli 6개 + pipeline 1개). 실패 시 출력에 stdout/stderr 가 함께 나온다.

수동 확인(개발 PC):

```bash
cargo run -p app --release -- --home /tmp/kra-home index --root eval/public/docs
cargo run -p app --release -- --home /tmp/kra-home search "고령친화도시"
cargo run -p app --release -- --home /tmp/kra-home status
```

`eval/public/docs` 1,000건 색인 시간과 검색 지연을 `docs/m1/results.md`(Task 18)에 적는다.

- [ ] **Step 9: CI** — `.github/workflows/ci.yml` 의 Windows `put llama DLLs next to binaries` 단계 아래에 pdfium 도 실행 파일 옆에 두는 단계를 추가(제품과 같은 배치로 시험):

```yaml
      - name: put pdfium next to binaries (windows)
        if: runner.os == 'Windows'
        shell: bash
        run: cp third_party/pdfium/win-x64/bin/pdfium.dll target/release/
```

(시험은 `--pdfium` 으로 경로를 직접 주므로 이 단계 없이도 통과해야 한다. 실행 파일 옆 기본 경로 동작은 Task 17 릴리즈 스모크 시험이 확인한다.)

- [ ] **Step 10: Commit**

```bash
git add crates/app .github/workflows/ci.yml Cargo.lock
git commit -m "app: kra-search CLI with incremental keyword indexing, dedupe and self-recovery

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---
### Task 16: 키워드 검색 평가 (`m0eval --keyword`)

M1 완료 기준(스펙 §14: "공개 평가 세트 키워드 검색 동작")을 숫자로 남기고, M2 "하이브리드 > 키워드 단독"의 기준점을 만든다.

**Files:**
- Create: `crates/m0eval/src/keyword.rs`, `docs/m1/eval-keyword.md`(생성물), `docs/m1/results.md`
- Modify: `crates/m0eval/Cargo.toml`, `crates/m0eval/src/main.rs`, `crates/m0eval/src/corpus.rs`

**Interfaces:**
- Consumes: `index::{keyword::{KeywordIndex, KeywordDoc}, tokenize::Analyzers}`, `metrics::{ndcg_at_k, recall_at_k, first_relevant_rank}`(기존)
- Produces: `corpus::EvalDoc.name: String`(매니페스트 제목 + 확장자 — 실제 PC 의 파일 이름 역할), `keyword::report(docs, queries) -> String`(마크다운), 명령 `cargo run -p m0eval --release -- --eval eval/public --eval eval/kra --keyword --out docs/m1/eval-keyword.md`

- [ ] **Step 1: 의존성·말뭉치** — `crates/m0eval/Cargo.toml` 에 `index = { path = "../index" }` 추가.

`corpus.rs`: `EvalDoc` 에 필드 추가

```rust
pub struct EvalDoc {
    pub id: String,
    /// 매니페스트 제목 + 확장자 — 실제 PC 의 파일 이름 역할(키워드 파일명 필드)
    pub name: String,
    pub doc: Document,
}
```

`load_docs` 의 `ok.push(EvalDoc { id: e.id, doc })` 를 `ok.push(EvalDoc { id: e.id, name: format!("{}.{}", e.title, e.ext), doc })` 로 바꾼다(`e.title` 을 위에서 clone 해 쓰고 있으므로 `name` 을 먼저 만든 뒤 `doc.title` 대체에 `e.title.clone()` 을 쓴다).

- [ ] **Step 2: `crates/m0eval/src/keyword.rs`**

```rust
//! 키워드 검색 단독 평가(M1). 같은 질의·정답으로 M0 의미 검색(eval-dev.md)과 비교한다.

use crate::corpus::{EvalDoc, Query};
use crate::metrics::{first_relevant_rank, ndcg_at_k, recall_at_k};
use index::keyword::{KeywordDoc, KeywordIndex};
use index::tokenize::Analyzers;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::time::Instant;

pub fn report(docs: &[EvalDoc], queries: &[Query]) -> String {
    let a = Analyzers::load().expect("형태소 사전");
    let mut kw = KeywordIndex::in_ram(&a).expect("색인");
    let t = Instant::now();
    for (i, d) in docs.iter().enumerate() {
        kw.upsert(&KeywordDoc::new(i as u64, Some(&d.doc), &[d.name.clone()])).expect("색인 쓰기");
    }
    kw.commit().expect("커밋");
    let index_secs = t.elapsed().as_secs_f64();

    let mut kinds: BTreeMap<String, (usize, f64, f64)> = BTreeMap::new();
    let mut lat = Vec::new();
    let mut ranks = Vec::new();
    for q in queries {
        let t = Instant::now();
        let hits = kw.search(&q.text, 100).expect("검색");
        lat.push(t.elapsed().as_secs_f64() * 1000.0);
        let ranked: Vec<String> = hits.iter().map(|(id, _)| docs[*id as usize].id.clone()).collect();
        let (n, r) = (ndcg_at_k(&ranked, &q.relevant, 10), recall_at_k(&ranked, &q.relevant, 20));
        for k in [q.kind.as_str(), "전체"] {
            let e = kinds.entry(k.to_string()).or_default();
            e.0 += 1;
            e.1 += n;
            e.2 += r;
        }
        ranks.push((q, first_relevant_rank(&ranked, &q.relevant)));
    }
    lat.sort_by(f64::total_cmp);
    let pct = |p: f64| lat.get(((lat.len().max(1) - 1) as f64 * p).round() as usize).copied().unwrap_or(0.0);

    let mut s = String::new();
    writeln!(s, "# M1 키워드 검색 평가 (자동 생성: `cargo run -p m0eval --release -- --keyword`)\n").unwrap();
    writeln!(s, "- 문서 {}건 · 질의 {}개 · 색인 {:.1}초 · 검색 p50 {:.1}ms / p95 {:.1}ms(이 PC)\n", docs.len(), queries.len(), index_secs, pct(0.5), pct(0.95)).unwrap();
    writeln!(s, "## 1. 질의 유형별\n\n| 유형 | 질의 수 | nDCG@10 | Recall@20 |\n|---|---|---|---|").unwrap();
    for (k, (n, nd, rc)) in &kinds {
        writeln!(s, "| {k} | {n} | {:.4} | {:.4} |", nd / *n as f64, rc / *n as f64).unwrap();
    }
    writeln!(s, "\n## 2. 마사회 질의별 첫 정답 순위 (100위 밖은 -, 10위 밖은 굵게)\n\n| 질의 | 유형 | 검색어 | 순위 |\n|---|---|---|---|").unwrap();
    for (q, r) in ranks.iter().filter(|(q, _)| q.kind.starts_with("kra")) {
        let rank = match r {
            Some(r) if *r > 10 => format!("**{r}**"),
            Some(r) => r.to_string(),
            None => "-".into(),
        };
        writeln!(s, "| {} | {} | {} | {} |", q.qid, q.kind, q.text, rank).unwrap();
    }
    s
}
```

- [ ] **Step 3: `main.rs`** — 플래그와 분기

`Args` 에 추가:

```rust
    /// 키워드 검색만 평가한다(모델 불필요). --out 으로 저장 위치를 준다.
    #[arg(long)]
    keyword: bool,
```

`mod keyword;` 를 추가하고, `eprintln!("문서 {} …")` 바로 다음에:

```rust
    if a.keyword {
        std::fs::write(&a.out, keyword::report(&docs, &queries)).unwrap();
        eprintln!("저장: {}", a.out.display());
        return;
    }
```

- [ ] **Step 4: 실행**

```bash
mkdir -p docs/m1
cargo run -p m0eval --release -- --eval eval/public --eval eval/kra --keyword --out docs/m1/eval-keyword.md
```

Expected: `docs/m1/eval-keyword.md` 생성, 질의 202개, 유형 9개 + 전체. (평가 문서 원본 `eval/*/docs` 는 개발 PC 에만 있다 — 없으면 `python3 tools/eval/fetch_all.py && python3 tools/eval/import_local.py eval/kra`.)

- [ ] **Step 5: `docs/m1/results.md` 작성** — 생성된 숫자를 옮겨 적는다(아래 의미 검색 열은 `docs/m0/eval-dev.md` §1 의 Q8·512·300·청크 행 그대로).

```markdown
# M1 결과 (추출·키워드 검색)

- 상태: 진행 중 — 폐쇄망 m1-diag-1 결과 대기
- 근거: [eval-keyword.md](eval-keyword.md) · M0 의미 검색 [../m0/eval-dev.md](../m0/eval-dev.md)

## 1. 키워드 검색 vs M0 의미 검색 (같은 질의 202개, nDCG@10)

| 유형 | 질의 수 | 키워드(M1) | 의미(M0, Q8·512·청크 300·청크 점수) |
|---|---|---|---|
| 전체 | 202 | (eval-keyword.md) | 0.9122 |
| semantic | | | 0.9941 |
| keyword | | | 0.9754 |
| abbrev | | | 1.0000 |
| temporal | | | 0.9695 |
| deep | | | 0.9254 |
| kra | | | 0.9602 |
| kra-deep | | | 0.8164 |
| kra-plain | | | 0.6202 |

## 2. 의미 검색만으로 못 찾던 질의(results.md 발견 7) — 키워드 순위
| 질의 | 의미(M0) | 키워드(M1) |
|---|---|---|
| D012 변화관리협의체 고위험 모니터링단 | 22~23위 | (eval-keyword.md §2) |
| D028 손해배상 책임보험금 상향 금액 | 64~85위 | (eval-keyword.md §2) |

## 3. 해석
(숫자를 채운 뒤 2~4줄: 키워드가 앞서는 유형·뒤지는 유형, M2 결합에서 기대하는 것)
```

표의 빈 칸을 `eval-keyword.md` 숫자로 모두 채우고 "(eval-keyword.md …)" 표시는 지운다. "3. 해석"도 실제 숫자로 쓴다(예: "kra-plain 은 키워드 X — 질의에 문서 낱말이 없어 둘 다 약함").

- [ ] **Step 6: Commit**

```bash
git add crates/m0eval docs/m1 Cargo.lock
git commit -m "m0eval: keyword-only evaluation and M1 baseline numbers

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 17: 진단 도구 M1 (형식별 추출·PDF 품질·자동 번호·스레드 수·카드 길이) + 릴리즈 `m1-diag-1` 준비

**Files:**
- Modify: `crates/diag/src/{main.rs, extract_stats.rs, report.rs, bench.rs}`, `crates/diag/Cargo.toml`(필요 시), `crates/diag/tests/report.rs`
- Rename: `.github/workflows/release-m0.yml` → `.github/workflows/release-diag.yml`, `docs/m0/README-diag.txt` → `docs/diag/README-diag.txt`
- Modify: `docs/engineering/runbooks/release-diag.md`, `docs/engineering/known-issues.md`(KI-017)

**Interfaces:**
- Consumes: `extract::{format::Format, pdf, Stats, runner::Outcome}`, `embed::chunk::build_card`, `embed::profile::ModelProfile`
- Produces (보고서 schema 2, 모두 집계 숫자 — INC-010):

```rust
// report.rs
#[derive(Serialize, serde::Deserialize, Default, Clone)]
pub struct FormatCount { pub ext: String, pub total: usize, pub ok: usize, pub no_text: usize, pub drm: usize, pub encrypted: usize, pub other_fail: usize, pub timeout: usize }
// ExtractInfo 추가 필드(모두 #[serde(default)])
pub by_format: Vec<FormatCount>,
pub pdf_pages: u64, pub pdf_bad_pages: u64,
pub article_numbered_docs: usize, pub article_numbered_paras: u64,
pub card_tokens_mean: Option<f64>, pub card_tokens_p50: Option<f64>, pub card_tokens_p90: Option<f64>,
```

- 요약 줄 추가: `형식별 읽기 성공: hwp 480/539 · pdf 90/120(글자 없음 25) · …`, `PDF 쪽: 전체 N · 비었거나 깨져 버린 쪽 M`, `"제N조" 자동 번호: 문서 a개 · 문단 b개`, `문서 카드 길이(토큰): 평균 x · 중앙값 y · 90% z`, 모델 줄에 `스레드별 초당 토큰 1→… · 2→… · 4→… · 8→…`.

- [ ] **Step 1: 실패하는 시험** — `crates/diag/tests/report.rs` 에 추가

```rust
#[test]
fn per_format_stats_and_no_paths() {
    let dir = std::env::temp_dir().join("kra diag 형식 홍길동");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../extract/tests/fixtures");
    std::fs::copy(fx.join("office/sample.docx"), dir.join("홍길동_인사.docx")).unwrap();
    std::fs::copy(fx.join("office/cp949.csv"), dir.join("명단.csv")).unwrap();
    std::fs::copy(fx.join("schedule.hwp"), dir.join("일정.hwp")).unwrap();
    std::fs::write(dir.join("~$임시.docx"), b"x").unwrap();
    let out_dir = std::env::temp_dir().join("kra-diag-out3");
    std::fs::create_dir_all(&out_dir).unwrap();
    let st = Command::new(env!("CARGO_BIN_EXE_kra-diag")).args(["extract-stats", "--root"]).arg(&dir).current_dir(&out_dir).status().unwrap();
    assert!(st.success());
    let txt = std::fs::read_to_string(out_dir.join("kra-diag-summary.txt")).unwrap();
    let json = std::fs::read_to_string(out_dir.join("kra-diag-report.json")).unwrap();
    assert!(txt.contains("docx 1/1") && txt.contains("csv 1/1") && txt.contains("hwp 1/1"), "{txt}");
    for needle in ["홍길동", "인사", "명단", "일정", "kra diag"] {
        assert!(!json.contains(needle) && !txt.contains(needle), "leaked {needle:?}");
    }
}
```

Run: `cargo test -p diag --release --test report per_format` → FAIL(HWP 만 훑음)

- [ ] **Step 2: 대상 파일·형식별 집계** — `extract_stats.rs`

`walk` 의 확장자 조건을 다음으로 바꾼다:

```rust
            } else if extract::format::Format::from_path(&p).is_some()
                && !p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("~$"))
            {
                out.push(p);
            }
```

`Acc` 에 `by_format: std::collections::BTreeMap<String, FormatCount>`, `card_tokens: Vec<f64>` 를 추가하고, 루프 안에서 확장자(`Format::from_path(f).map(|x| x.ext())`)별로 `total`·`ok`·`no_text`·`drm`·`encrypted`·`other_fail`·`timeout` 을 센다. `Outcome::Ok` 에서:

```rust
            Outcome::Ok { doc, distribution, coverage, stats } => {
                a.x.pdf_pages += stats.pdf_pages as u64;
                a.x.pdf_bad_pages += stats.pdf_bad_pages as u64;
                if stats.article_numbered_paras > 0 {
                    a.x.article_numbered_docs += 1;
                    a.x.article_numbered_paras += stats.article_numbered_paras as u64;
                }
                // (기존 ok·coverage·distribution·토큰 비율·청크 수 처리 그대로)
                if let Some(tk) = tokenizer {
                    let count = |s: &str| tk.count(s).unwrap_or_else(|_| s.chars().count());
                    if let Some(card) = embed::chunk::build_card(&doc, 256, &count) {
                        let input = embed::profile::ModelProfile::embeddinggemma2().render_doc(doc.title.as_deref(), &card);
                        a.card_tokens.push(count(&input) as f64);
                    }
                }
            }
```

`snapshot` 에서 `by_format` 을 `Vec` 으로(확장자 순), `card_tokens_mean/p50/p90` 을 채운다(`percentile` 재사용, 평균은 합/개수).
`Failed { kind, .. }` 의 `FailKind::Drm`·`Encrypted`·`NoText` 는 해당 칸, 나머지는 `other_fail`. `Timeout`·`Crashed` 는 `timeout`(형식별)·기존 전체 칸.

`clone_counts` 에 새 숫자 필드를 모두 옮긴다(`by_format` 은 snapshot 이 다시 만든다).

- [ ] **Step 3: 자식 추출에 pdfium 전달** — `main.rs`

`Cmd::ExtractOne` 에 인자 추가:

```rust
        /// pdfium 절대 경로(부모가 실행 파일 옆 것을 넘긴다)
        #[arg(long)]
        pdfium: Option<PathBuf>,
```

처리:

```rust
        Cmd::ExtractOne { file, coverage, pdfium } => {
            if let Some(p) = pdfium {
                let _ = extract::pdf::init(p);
            }
            extract::cli::extract_one_main(file, *coverage);
            return;
        }
```

`extract_stats::run` 의 자식 명령에 `exe` 폴더의 pdfium 이 있으면 `--pdfium <절대 경로>` 를 붙인다:

```rust
    let pdfium = exe.parent().map(|d| d.join(extract::pdf::LIB_NAME)).filter(|p| p.exists());
    let extract = |f: &Path, coverage: bool| {
        let mut cmd = Command::new(&exe);
        cmd.arg("extract-one").arg(f);
        if coverage {
            cmd.arg("--coverage");
        }
        if let Some(p) = &pdfium {
            cmd.arg("--pdfium").arg(p);
        }
        run_command(cmd, Duration::from_secs(30))
    };
```

- [ ] **Step 4: 스레드 수 측정(KI-017)** — `bench.rs`

논리 코어 수를 측정 자식까지 넘긴다:
- `bench::run`: `let logical = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(physical);` 를 구하고 `bench_in_child(m, physical, logical)` 로 호출.
- `bench_in_child(model, threads, logical)`: 자식 인자에 `"--logical", &logical.to_string()` 추가.
- `main.rs` `Cmd::BenchOne` 에 `#[arg(long, default_value_t = 0)] logical: usize` 추가, `bench::bench_model(model, Some(&dir), *threads, (*logical).max(*threads))` 로 호출.
- `bench_model(path, backend_dir, physical, logical)` 시그니처에 `logical: usize` 추가.

`bench_model` 의 `let mut threads = vec![1, 2, physical.max(1)];` 를 다음으로 바꾼다(뒤의 `threads.sort(); threads.dedup();` 는 그대로):

```rust
    // 하이브리드 CPU(성능+효율 코어)에서는 물리 코어 수가 최적이 아닐 수 있다(KI-017) — 여러 값을 잰다.
    let mut threads: Vec<usize> = [1, 2, 4, 6, 8, 12, 16, physical, logical].into_iter().filter(|&t| t >= 1 && t <= logical.max(1)).collect();
```

`report.rs` 모델 요약 줄 끝에 `· 스레드별 {}` 를 붙인다: `m.tokens_per_sec.iter().map(|(t, v)| format!("{t}→{v:.0}")).collect::<Vec<_>>().join(" · ")`.

- [ ] **Step 5: 요약 줄** — `report.rs` `summarize` 의 추출 줄 다음에:

```rust
        if !x.by_format.is_empty() {
            let parts: Vec<String> = x
                .by_format
                .iter()
                .map(|f| if f.no_text > 0 { format!("{} {}/{}(글자 없음 {})", f.ext, f.ok, f.total, f.no_text) } else { format!("{} {}/{}", f.ext, f.ok, f.total) })
                .collect();
            s.push(format!("형식별 읽기 성공: {}", parts.join(" · ")));
        }
        if x.pdf_pages > 0 {
            s.push(format!("PDF 쪽: 전체 {} · 비었거나 깨져 버린 쪽 {}", thousands(x.pdf_pages as f64), thousands(x.pdf_bad_pages as f64)));
        }
        s.push(format!("\"제N조\" 자동 번호: 문서 {}개 · 문단 {}개", x.article_numbered_docs, x.article_numbered_paras));
        if let (Some(m), Some(p50), Some(p90)) = (x.card_tokens_mean, x.card_tokens_p50, x.card_tokens_p90) {
            s.push(format!("문서 카드 길이(토큰): 평균 {m:.0} · 중앙값 {p50:.0} · 90% {p90:.0}"));
        }
```

기존 추출 줄의 머리 `"HWP {}개{}: …"` 는 `"문서 {}개{}: …"` 로, `HWP아님` 은 `형식 아님` 으로 바꾼다. `Report::new` 의 `schema: 1` → `schema: 2`.

- [ ] **Step 6: 시험 통과** — Run: `cargo test -p diag --release` → PASS(새 시험 포함, 기존 `report_never_contains_paths` 통과 유지)

- [ ] **Step 7: 릴리즈 워크플로·안내문**

```bash
git mv .github/workflows/release-m0.yml .github/workflows/release-diag.yml
mkdir -p docs/diag && git mv docs/m0/README-diag.txt docs/diag/README-diag.txt
```

`release-diag.yml` 수정:
- `name: release-diag`, `tags: ["m0-diag-*", "m1-diag-*"]`
- `fetch rhwp sources` 아래에 `- run: bash tools/fetch-pdfium.sh win-x64` (shell: bash)
- package 단계: `cp docs/m0/README-diag.txt` → `cp docs/diag/README-diag.txt $OUT/`, `manual-checks` 복사 줄 삭제, 추가:

```bash
          cp third_party/pdfium/win-x64/bin/pdfium.dll $OUT/
          mkdir -p $OUT/licenses/pdfium && cp -r third_party/pdfium/win-x64/LICENSE third_party/pdfium/win-x64/licenses/* $OUT/licenses/pdfium/
```

- 스모크 단계 끝에 추가(실행 파일 옆 pdfium 이 실제로 불러와지는지 — 시험 자료 PDF 5개 중 글자 있는 3개):

```bash
          ./kra-diag.exe extract-stats --root "$GITHUB_WORKSPACE/crates/extract/tests/fixtures"
          grep -q "pdf 3/5" kra-diag-summary.txt
```

- release 단계 `--title "M0 진단 도구 $GITHUB_REF_NAME"` → `--title "진단 도구 $GITHUB_REF_NAME"`, `--notes-file docs/diag/README-diag.txt`

`docs/diag/README-diag.txt` 전체를 다음으로 바꾼다:

```text
KRA 문서검색 진단 도구 (M1)

이 도구는 KRA 문서검색 개발에 필요한 수치를 측정합니다.
 - 이 PC의 처리 속도(스레드 수별)
 - 업무 문서(한글·PDF·워드·엑셀·파워포인트·ODF·TXT·CSV)를 형식별로 몇 개나 읽을 수 있는지
네트워크를 사용하지 않으며, 설치·관리자 권한이 필요 없습니다.
이 PC의 설정이나 파일을 바꾸지 않습니다(결과 파일 2개만 만듭니다).

1. zip을 아무 폴더에나 풀어 주세요(pdfium.dll 은 PDF를 읽는 부품입니다 — 같은 폴더에 두세요).
2. kra-diag.exe 를 더블클릭하면 측정이 시작됩니다.
   (또는 그 폴더에서 명령 프롬프트를 열고: kra-diag.exe all)
   - 처리 속도 측정이 지난번보다 몇 분 더 걸립니다(스레드 수를 여러 가지로 잽니다).
   - 업무 문서 폴더(문서·바탕화면·다운로드, D:)의 문서를 읽어 형식별 "읽기 성공/실패 개수",
     "글자가 없는 PDF(스캔 등) 개수", "문서 카드 길이" 같은 숫자만 냅니다.
   - 문서 내용·파일 이름·폴더 경로는 결과에 남지 않습니다.
   - 문서 수에 따라 10분~수 시간 걸릴 수 있습니다. 중간에 멈추려면 창을 닫거나 Ctrl+C.
3. 끝나면 같은 폴더에 두 파일이 생깁니다(그 폴더에 쓸 수 없으면 kra-diag.exe 가 있는 폴더).
   측정 도중에도 단계마다 저장되므로, 중간에 멈춰도 그때까지의 결과가 남습니다. 이 두 파일을 반출해 주세요.
     kra-diag-summary.txt  (요약 몇 줄 — 반출이 어려우면 이 내용만 옮겨 적어도 됩니다)
     kra-diag-report.json  (상세)
4. 반출 전에 두 파일을 메모장으로 열어 개인정보·문서 내용이 없는지 직접 확인할 수 있습니다.
```

`docs/engineering/runbooks/release-diag.md`: 워크플로 이름(`release-diag.yml`)·태그(`m1-diag-N`)·zip 구성(`pdfium.dll`, `licenses/pdfium/`, `README-diag.txt`, 수동 점검표 없음)·스모크 확인 항목(`pdf 3/5`)을 고친다. 이력 표에 `| m1-diag-1 | (게시 후 채움) | 형식별 추출·PDF·스레드 수별 속도·카드 길이·"제N조" 자동 번호 |` 행 추가.

`known-issues.md` KI-017 상태 → `측정 추가(m1-diag-1). 앱 스레드 정책은 M2`.

- [ ] **Step 8: Commit**

```bash
git add -A crates/diag .github/workflows docs/diag docs/m0 docs/engineering
git commit -m "diag: per-format extraction stats, PDF quality, thread sweep, card length (m1-diag)

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 18: 마무리 — 기록(DEC·KI)·스펙·문서, 전체 검증, 진단 릴리즈

**Files:**
- Modify: `docs/engineering/decisions.md`, `docs/engineering/known-issues.md`, `docs/superpowers/specs/2026-10-12-kra-search-design.md`(§4·§5.2·§12.2·§18), `CLAUDE.md`, `docs/m1/results.md`
- Create: `docs/engineering/runbooks/upgrade-lindera-pdfium.md`

- [ ] **Step 1: DEC 추가** — `decisions.md` 에 새 절 `## M1 추출·키워드 색인` 과 행:

```markdown
## M1 추출·키워드 색인
| ID | 판단 | 이유 | 틀렸을 때 비용 | 상태 |
|---|---|---|---|---|
| DEC-033 | 상주 워커 1개, 파일당 **벽시계** 30초. 응답 줄에 표지(`\u{1}KRA `)를 붙여 파서의 표준 출력 잡음과 구분 | 파일마다 프로세스 생성은 비쌈(DEC-025). Windows CPU 시간 측정은 상주 앱(M5)에서 | 느린 PC 에서 큰 문서가 시간 초과로 파일명만 색인될 수 있음(KI-019) | 유효 |
| DEC-034 | 형태소 사전은 lindera `embed-ko-dic`(실행 파일 +87MB). 사전 원본은 `tools/fetch-ko-dic.sh` 가 SHA-256 고정으로 받아 빌드 캐시에 둔다. `deny.toml` 은 빌드 전용 `ureq` 를 `wrappers` 로, `webpki-roots` 라이선스를 예외로 허용하고 `tools/check-no-net.sh` 가 실행 의존 그래프를 확인 | 사전 폴더 별도 배포보다 단순, 실행 중 네트워크 0 유지 | 설치 파일 크기(압축 시 약 14~21MB 증가) | 유효 |
| DEC-035 | pdfium = bblanchon/pdfium-binaries `chromium/7881` + pdfium-render 0.9.4(동적 로드, 절대 경로만). 문서 하나 전체를 전역 뮤텍스로 감싼다 | 여러 스레드 동시 사용 시 문서 열기가 무작위 실패(조사). 워커는 어차피 단일 스레드 | 없음 | 유효 |
| DEC-036 | 크기 상한: 압축 해제한 XML 파트 64MB(넘으면 TooLarge), 시트·CSV 50,000행, 키워드 본문 500만 자. PDF 는 쪽마다 문단 하나, 깨진 쪽(Latin-1 30%↑·쓰레기 10%↑)·빈 쪽은 버리고 남은 쪽이 없으면 NoText | 압축 폭탄·거대 데이터 시트가 워커·색인을 채우지 않도록 | 거대 시트 뒷부분은 키워드로 못 찾음 | 유효 |
| DEC-037 | 메타 DB: 내용(xxh3-128) 단위 행 + 경로 행. 키워드 반영 순서 = Pending → 키워드 커밋 → finish_flush(Indexed + opstamp). 열 때 키워드 opstamp 가 meta 보다 작으면 전부 다시 쓴다. 추출 결과 문서(JSON)를 meta 에 둔다 | 스펙 §6.8. 이름 바꾸기만으로 다시 추출하지 않으려면 본문이 있어야 함 | meta.db 크기(본문 텍스트) — M2 청크 텍스트와 함께 압축 검토 | 유효 |
| DEC-038 | 키워드 질의: 낱말마다 형태소 TermQuery + bigram 구절(1토큰이면 TermQuery)을 필드 가중치로 Should, 낱말끼리 Should. 필드 = 제목·파일명(확장자 제외)·소제목·본문 | 스펙 §8.2 의 초기값. 조정은 M2 `--eval` | 여러 낱말 질의에서 AND 가 필요한 경우 순위가 흐릴 수 있음 | 잠정 |
```

- [ ] **Step 2: KI 추가** — `known-issues.md` 표 끝에:

```markdown
| KI-018 | 형태소 사전이 메모리에 약 43MB 상주(사전 페이지) — 상주 앱 유휴 기준 50MB(스펙 §13)에 걸림 | `index::tokenize::Analyzers` | 상주 메모리 초과 | 검색창이 닫혀 있으면 사전 해제, 단축키를 누를 때 다시 불러오기(약 50~70ms) | M5 상주 앱 | 미해결 |
| KI-019 | 워커 제한 시간이 CPU 시간이 아니라 벽시계 30초 | `extract::worker` | 다른 프로그램이 바쁜 PC 에서 정상 문서가 시간 초과 | Windows `GetProcessTimes` 로 CPU 시간 측정 | M5 상주·성능 프로필 | 미해결 |
| KI-020 | 폴더 감시 없음 — `kra-search index` 를 실행할 때만 바뀐 파일을 찾음 | `crawler` | 새 문서가 바로 찾히지 않음 | ReadDirectoryChangesW(M5), USN(M6) | M5·M6 | 미해결(계획됨) |
| KI-021 | DOCX 머리말·꼬리말·각주·메모, PPTX 발표자 메모, 스타일 상속(basedOn)을 읽지 않음 | `extract/src/ooxml.rs` | 그 안에만 있는 글자 검색 불가 | 해당 파트 추가 | 실제 문서에서 필요가 보이면(m1-diag 이후) | 미해결 |
| KI-022 | calamine 0.36.1 이 quick-xml 0.41 을 따로 끌어옴(중복 빌드) | `Cargo.lock` | 빌드 시간 | calamine 이 0.42 로 올라가면 정리 | calamine 새 버전 | 미해결 |
| KI-023 | tantivy 쓰기 잠금 — `kra-search index` 두 개를 동시에 돌리면 뒤의 것이 실패 | `index::keyword` | 개발 명령줄 한정 | 상주 앱 단일 인스턴스(M5) | M5 | 미해결 |
| KI-024 | 키워드 토큰 오프셋이 NFKC 정규화한 글자 기준 | `index::tokenize` | 원문 강조 표시 위치가 어긋날 수 있음(전각 글자 등) | 정규화 전 오프셋 대응표 | M3 강조 표시 | 미해결 |
| KI-025 | 추출기 퍼징(cargo-fuzz, 스펙 §12.3) 아직 없음 — 압축 폭탄·반복 행·손상 파일은 개별 시험만 | `extract` | 알려지지 않은 입력에서 멈춤·과다 메모리(워커 격리로 앱은 안전) | HWP/HWPX/PDF/OOXML 퍼징 대상 추가 | M7 보안성 검토 전 | 미해결 |
```

- [ ] **Step 3: 스펙 갱신**
- §4 표: `HWP` 행의 "rhwp (git 의존, 리비전 고정)" → "rhwp (`tools/fetch-rhwp.sh` 부분 복제·리비전 고정, DEC-008)", `PDF` 행 비고에 "bblanchon/pdfium-binaries chromium/7881 동봉, pdfium-render 0.9.4(DEC-035)", `키워드 색인` 행 비고에 "lindera 6.2 embed-ko-dic(DEC-034)".
- §5.2 표: `index` 행 아래에 `| vector | 벡터 연산(MRL 축소·정규화·int8·이진)·평면 저장소 — 의존성 없음(KI-008) | ✔ |` 추가, `index` 행 책임에서 "벡터 저장소"를 빼고 "(벡터는 vector 크레이트)"로.
- §12.2 2번 보고서 항목의 "실제 문서 추출 통계(… — **M1 이후**)" 를 "실제 문서 추출 통계(형식별 성공/실패/DRM/글자 없음/시간 초과, PDF 쪽 품질, 카드 길이 — m1-diag 부터)" 로.
- §18 끝에 `| 26 | M1 범위 (2026-10-14) | 메타 DB SQLCipher·DPAPI 는 M1, 폴더 감시·EFS·개인정보·공문 구조는 M4~M6, 상주 워커 벽시계 30초, 자동 번호(KI-011)는 진단 결과로 판단. 계획 docs/superpowers/plans/2026-10-14-m1-extract-keyword.md |` 추가.

- [ ] **Step 4: 런북·CLAUDE.md**

`docs/engineering/runbooks/upgrade-lindera-pdfium.md`:

```markdown
# lindera(형태소 사전)·pdfium 올리기

## lindera
1. `crates/index/Cargo.toml` 의 `lindera = "=X"` 를 바꾼다.
2. `tools/fetch-ko-dic.sh` 의 `LINDERA_VER` 를 새 버전으로. 새 lindera-dictionary 의 `DICTIONARY_FORMAT_VERSION` 이 바뀌었으면 `FMT` 도(캐시 폴더 이름 `<버전>-fmt<형식>`).
3. lindera-ko-dic `build.rs` 의 사전 파일 이름·URL·MD5 가 바뀌었으면 스크립트의 FILE·URL·SHA-256 을 새로 계산해 넣는다.
4. `cargo deny check` — `ureq` 를 끌어오는 크레이트 이름이 바뀌면 `deny.toml` 의 `wrappers` 를 고친다. `bash tools/check-no-net.sh` 통과 확인.
5. `cargo test -p index --release` — 형태소 위치 시험(`morph_drops_particles_and_endings`)이 바뀌면 출력을 보고 의도한 변화인지 확인.
6. `cargo run -p m0eval --release -- --eval eval/public --eval eval/kra --keyword --out docs/m1/eval-keyword.md` 로 점수 변화를 확인.

## pdfium
1. `tools/fetch-pdfium.sh` 의 `TAG` 와 플랫폼별 SHA-256 4개를 새 릴리즈 값으로(`bash tools/fetch-pdfium.sh all` 로 받아 `shasum -a 256`).
2. pdfium-render 의 `pdfium_XXXX` 기능을 같은 번호로(없으면 가장 가까운 아래 번호 — 바인딩 시 모든 심볼을 확인하므로 맞지 않으면 init 이 실패한다).
3. `cargo test -p extract --release --test formats pdf_` 와 릴리즈 스모크(`pdf 3/5`).
```

`CLAUDE.md` "작업 전에 볼 문서" 표에 행 추가: `| lindera·pdfium 버전 변경, deny.toml 예외 | docs/engineering/runbooks/upgrade-lindera-pdfium.md — 사전 캐시 폴더·SHA·wrappers 같이 바꿀 것 |`. "개발 명령"에 `cargo run -p app --release -- --home /tmp/kra-home index --root <폴더>` / `… search "검색어"` 와 `cargo run -p m0eval --release -- --eval eval/public --eval eval/kra --keyword --out docs/m1/eval-keyword.md` 추가.

- [ ] **Step 5: 개발 PC 실측** — `docs/m1/results.md` 에 "## 4. 개발 PC 색인 속도" 표 추가:

```bash
rm -rf /tmp/kra-home
time cargo run -p app --release -- --home /tmp/kra-home index --root eval/public/docs
cargo run -p app --release -- --home /tmp/kra-home status
for q in 고령친화도시 "일자리 전담반" 렛츠런파크 산림항공; do /usr/bin/time -p target/release/kra-search --home /tmp/kra-home search "$q" > /dev/null; done
du -sh /tmp/kra-home/data/tantivy /tmp/kra-home/data/meta.db
```

표: 문서 수 · 색인 시간 · 검색 시간(명령 전체, 사전 로드 포함) · 키워드 색인 크기 · meta.db 크기. (검색 지연 기준 p95 ≤ 150ms 는 상주 앱 기준 — 명령줄은 사전 로드 시간이 더해진다고 적는다.)

- [ ] **Step 6: 전체 검증**

```bash
bash tools/fetch-rhwp.sh && bash tools/fetch-pdfium.sh && bash tools/fetch-ko-dic.sh
cargo build --workspace --release
cargo test --workspace --release
cargo clippy --workspace --release -- -D warnings
bash tools/check-no-net.sh
git push
```

Expected: 모두 통과, CI(macOS·Windows·deny) 통과 확인(`gh run watch`). Windows 에서만 도는 시험(DPAPI, 비-UTF-8 경로, OpenSSL 빌드, pdfium DLL)이 통과했는지 로그에서 확인한다.

- [ ] **Step 7: 전체 리뷰 요청** — superpowers:requesting-code-review 로 `main..m1` 전체 리뷰. 지적 사항은 고치거나 KI/DEC 로 남긴다.

- [ ] **Step 8: Commit**

```bash
git add -A docs CLAUDE.md
git commit -m "docs: M1 decisions, known issues, spec and runbook updates

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
git push
```

- [ ] **Step 9: 진단 릴리즈 — 사용자 확인 후**

사용자에게 릴리즈 태그(`m1-diag-1`)를 만들어도 되는지 묻는다(외부에 게시되는 일). 승인되면 `runbooks/release-diag.md` 절차대로:

```bash
git tag m1-diag-1 && git push origin m1-diag-1
gh run watch   # release-diag
```

스모크(Q4/Q8 초당 토큰 범위, `pdf 3/5`) 확인 후 릴리즈 링크·SHA-256·실행 방법을 사용자에게 전달하고, 이력 표를 채운다. `main` 병합은 사용자 결정.
