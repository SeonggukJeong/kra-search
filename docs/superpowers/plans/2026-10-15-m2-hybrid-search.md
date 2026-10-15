# M2 의미 검색 + 하이브리드 검색 구현 계획

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 문서 카드·본문 청크 임베딩(본문 3단 순서), 디스크 벡터 저장소, 날짜 표현 해석과 가산, 가중 RRF 하이브리드 검색을 명령줄 `kra-search` 에 넣고, 실험(m0eval)과 제품이 같은 검색 코어를 쓰는 것을 일치 검사로 증명하며, 폐쇄망 확인용 `eval --queries` 와 릴리즈까지 만든다.

**Architecture:** 순수 함수 크레이트 `query`(날짜·검색어 유형)와 검색 코어 `index::hybrid`(벡터 스캔 + RRF + 보정)를 m0eval 과 앱이 함께 부른다. 앱은 `index` 명령 안에서 키워드 → 카드 → 본문 ①②③ 순으로 임베딩해 `vector::store` 의 세대 파일에 쓰고 meta.db(v2)를 기준으로 정합성을 지킨다. 매개변수는 m0eval 이 조정용 질의로 고르고 검증용으로 판정한다.

**Tech Stack:** Rust 1.97.0(edition 2021), tantivy 0.26.2, rusqlite(SQLCipher), llama.cpp(llama-sys), EmbeddingGemma 2 Q8_0, Python 3(평가 도구, 표준 라이브러리만).

**Spec:** `docs/superpowers/specs/2026-10-15-m2-hybrid-search-design.md`(M2 설계) + 상위 `docs/superpowers/specs/2026-10-12-kra-search-design.md`(§6·§8·§13·§14·§18 #27·#28). 판단 기록 DEC-046~055, KI-042~046.

## Global Constraints

- **실행 중 네트워크 0**: 실행 의존 그래프에 HTTP 크레이트 금지(`deny.toml`, `tools/check-no-net.sh`). 새 의존성은 `toml`(이미 m0eval 사용)·`xxhash-rust`(이미 crawler 사용)·`serde` 뿐이다. 평가 도구(Python)만 개발 PC 에서 네트워크를 쓴다.
- **보고서 개인정보**: `kra-diag` 와 `kra-search eval --queries` 보고서에는 문서 내용·경로·파일명·질의 원문·사용자명·컴퓨터명을 남기지 않는다(INC-010). 시험으로 지킨다.
- 평가 원본(`eval/public/docs/`, `eval/kra/docs/`, `eval/downloads/`, `eval/data/docs/`)과 `*.gguf` 는 커밋하지 않는다.
- 화면에 나오는 말은 UI D7 용어: 임베딩·벡터 → "의미 검색", 카드 → "빠른 의미 검색", 본문 청크 → "정밀 의미 검색", 청크 → 쓰지 않음, BM25 → "키워드 검색", 유휴 → "PC를 쓰지 않을 때".
- 문서·주석은 한국어. 커밋 이메일은 저장소 설정(`seongguk.dev@gmail.com`). 커밋 끝줄 `Co-Authored-By:` 에는 **그 커밋을 실제로 쓴 모델**을 적는다(DEC-043).
- 고정값(M0·M1 결정): 저장 512차원 · 이진 768비트(`Layout::M0`) · 본문 청크 목표 300 / 최대 400토큰(`BODY_CHUNK`) · 문서당 상한 본문 2,000 / 표(xlsx·ods·csv) 200 · 이진 후보 max(1,000, 청크 수 × 2%) · 모델 Q8_0 · 입력 형식 `title: {제목|파일명} | text: {본문}` / `task: search result | query: {검색어}`.
- 카드 상한은 Task 16 판정 전까지 256(`embed::plan::CARD_TOKENS`).
- 기준 시점(오늘)은 주입한다: 앱 `--now YYYY-MM-DD`(숨김), 평가 기준일 **2026-10-15**. 날짜 계산은 한국 시각(UTC+9) 기준.
- 시험 명령: `cargo test --workspace --release`. 실모델 시험: `cargo test -p embed --release --test model --test reference -- --include-ignored --test-threads 1` 등(모델은 `bash tools/fetch-models.sh`).
- 기록 규칙: 고치면 `incidents.md`(INC), 스펙에 없는 것을 정하면 `decisions.md`(DEC), 미루면 `known-issues.md`(KI). 번호는 바꾸지 않는다(다음 번호 DEC-056, KI-047, INC-021).

## Review Focus

1. **일상어와 겹치는 날짜 낱말** — "전달 사항", "연말정산 서류", "지난 주민설명회", "올림픽"에서 날짜 칩이 생기면 안 된다(사람은 날짜로 읽지 않는다). → Task 4 `ordinary_words_are_not_dates`.
2. **날짜 표현만 있는 검색어** — "작년"만 쳐도 빈 검색이 되면 안 되고 원래 검색어로 찾는다. → Task 5 `date_only_query_keeps_text`.
3. **색인 뒤 모델 파일이 바뀐 PC** — 다른 모델의 벡터와 검색어 벡터를 섞으면 엉뚱한 결과가 나온다. 의미 검색을 끄고 키워드만 쓰며 `index` 를 다시 하라고 알린다. → Task 14 `model_change_disables_semantic_until_reindex`.
4. **임베딩 중 강제 종료(Ctrl+C·전원)** — 다음 `index` 가 이어서 하고 같은 문서가 두 번 들어가거나 엉뚱한 문서를 가리키면 안 된다. → Task 13 `crash_between_append_and_meta_resumes_cleanly`.
5. **색인 중 검색**(다른 창에서 `search`) — `index` 가 벡터를 덧붙이는 중에도 검색이 깨지지 않고 meta 에 기록된 만큼만 본다. → Task 12 `reader_ignores_unrecorded_tail`.

## 계획 검증 기록 (2026-10-15)

계획을 쓰는 동안 Task 4~15·17~19 의 코드를 버리는 작업 트리(macOS)에 그대로 적용해 컴파일·시험했다 — 실행자는 코드가 한 번 돌았다는 전제로 시작해도 된다. 그때 찾아 계획에 반영한 것:

- 검색이 meta 표 전체를 읽게 되면서 index 가 읽지 않는 쪽의 손상을 고치지 못하던 빈틈 → `Meta::check` + index 끝 검사(DEC-056, Task 11·13).
- 표뿐인 CSV·XLSX 는 카드가 없어 빠른 의미 검색에서 빠지던 빈틈 → 카드에 첫 표 머리글·첫 행(DEC-057, Task 9).
- 의미 검색 단계에서 드러난 meta 손상도 한 번 다시 만들도록 `index` 흐름을 고침(Task 13·17).
- 개발 PC 의 `target/release/models/` 때문에 CLI 시험이 PC 마다 다르게 돌던 것 → 시험이 없는 모델 경로를 준다(Task 13).
- 검색어 유형 기준(8글자)을 평가 질의로 정함, 기본값에 기대던 시험 2개, 릴리즈 스모크의 `! grep` 함정, clippy 지적.
- 확인한 값: 실모델 종단 시험 통과, 청크 50만 검색 코어 p95 3.8ms(개발 PC), m0eval `--hybrid` 작은 세트 끝까지 실행.
- 검증하지 못한 것: Windows 판 `platform::cpu::core_split`(CI 가 확인), Task 1~3 의 네트워크·사람 작업, Task 16·20 의 실행.

---

## 파일 구조

| 파일 | 책임 | 작업 |
|---|---|---|
| `tools/eval/fetch_dates.py` | 공개 세트 매니페스트에 korea.kr 게시일 `date` 채우기 | 1 |
| `tools/eval/check_sets.py` | 평가 세트 검사(날짜 형식·정답 id·유형·date 질의 기간) | 1 |
| `eval/kra/manifest.toml`, `eval/*/README.md` | 날짜 열, K0009~K0011 | 1 |
| `tools/eval/make_data_set.py`, `eval/data/*` | 데이터 형식 방해 문서·질의(결정적 생성) | 2 |
| `eval/public/queries.tsv`, `eval/kra/queries.tsv` | `date` 질의 30, 새 마사회 문서 질의 | 3 |
| `crates/query/` | `calendar`(날짜 계산) · `dates`(날짜 표 + 해석) · `kind`(유형) · `parse` | 4, 5 |
| `crates/vector/src/record.rs`, `store.rs` | 레코드(int8+배율+비트) · 세대 파일 | 6 |
| `crates/index/src/keyword.rs` | 가중치 매개변수 `KeywordWeights`, 구절 일치 `phrase_ids` | 7 |
| `crates/index/src/hybrid.rs` | 검색 코어(VectorSet·semantic·fuse·search·HybridParams) | 8 |
| `crates/embed/src/api.rs`, `fake.rs`, `plan.rs`, `chunk.rs`, `engine.rs` | 임베딩 trait·가짜 임베더·3단 순서·청크 표시·KI-001·KI-003 | 9 |
| `crates/platform/src/cpu.rs` | 성능 코어 수·기본 스레드 | 10 |
| `crates/index/src/meta.rs` | 스키마 v2(이전), 임베딩 테이블 API | 11 |
| `crates/app/src/vectors.rs` | 임베딩 지문·벡터 파일 열기/복구/압축·검색용 읽기 | 12 |
| `crates/app/src/embedding.rs` | 카드·본문 임베딩 단계 | 13 |
| `crates/app/src/search.rs`, `main.rs` | 하이브리드 검색(`Searcher`), CLI 플래그 | 13, 14 |
| `crates/m0eval/src/hybrid.rs` (+ `corpus.rs`) | 하이브리드 실험·결정·보고서·일치 검사 | 15, 17 |
| `docs/m2/eval-hybrid.md`, `docs/m2/params.json` | 실험 결과(자동 생성), 고른 매개변수 | 16 |
| `crates/app/src/evalset.rs`, `eval.rs` | 평가 세트 → 실제 경로, `eval --set`, `eval --queries` | 17, 18 |
| `crates/index/tests/bench_hybrid.rs` | 청크 50만 검색 지연 | 19 |
| `.github/workflows/ci.yml`, `release-check.yml`, `docs/check/README-check.txt`, `docs/engineering/runbooks/release-check.md` | 실모델 종단 시험, 폐쇄망 확인 릴리즈 | 20 |
| `docs/m2/results.md`, `docs/engineering/*` | 기록 정리 | 21 |

작업 순서: 1 → 2 → 3(평가 세트, 네트워크 필요) → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11 → 12 → 13 → 14 → 15 → 16 → 17 → 18 → 19 → 20 → 21. 4~11 은 서로의 인터페이스만 쓰므로 앞 작업이 끝나면 차례로 진행한다.

---

### Task 1: 평가 세트 — 문서 날짜와 새 마사회 문서

**Files:**
- Create: `tools/eval/fetch_dates.py`, `tools/eval/check_sets.py`
- Modify: `eval/public/manifest.toml`(스크립트가 `date` 줄 추가), `eval/kra/manifest.toml`, `eval/public/README.md`, `eval/kra/README.md`

**Interfaces:**
- Produces: 모든 매니페스트 `[[doc]]` 에 `date = "YYYY-MM-DD"`. 질의 파일 5번째 열(선택) `range` = `YYYY-MM-DD..YYYY-MM-DD` 또는 `future` — `date` 유형 질의만. `tools/eval/check_sets.py <세트 폴더>...` 가 0 으로 끝나면 세트가 맞다.

- [ ] **Step 1: 날짜 수집 스크립트를 쓴다**

`tools/eval/fetch_dates.py`:

```python
"""공개 평가 세트(eval/public) 매니페스트에 문서 날짜(date = korea.kr 게시일)를 채운다. 개발 PC 전용.

- date 가 없는 [[doc]] 만 보도자료 페이지를 받아 게시일을 읽는다(요청 사이 1초, robots.txt 전체 허용).
- 매니페스트 서식을 그대로 두려고 TOML 을 다시 쓰지 않고 `ext = ...` 줄 바로 뒤에 `date = "YYYY-MM-DD"` 줄을 끼운다.
- 50건마다 저장한다 — 중간에 끊겨도 다시 돌리면 남은 것만 받는다.

사용: python3 tools/eval/fetch_dates.py
"""
import re
import sys
import time
import tomllib
import urllib.request
from pathlib import Path

MAN = Path(__file__).resolve().parents[2] / "eval" / "public" / "manifest.toml"
UA = {"User-Agent": "Mozilla/5.0 (kra-search eval collector)"}
# 보도자료 본문 위 "info" 칸의 첫 날짜가 게시일이다(2026-10-15 확인: <div class="info"> <span>2026.10.12</span>).
DATE = re.compile(r'<div class="info">\s*<span>(\d{4})\.(\d{2})\.(\d{2})</span>')
SEP = "\n[[doc]]\n"


def posted(news_id: str) -> str:
    url = f"https://www.korea.kr/briefing/pressReleaseView.do?newsId={news_id}"
    with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=30) as r:
        html = r.read().decode("utf-8", "replace")
    m = DATE.search(html)
    if not m:
        raise ValueError(f"게시일을 찾지 못함: newsId={news_id}")
    return f"{m.group(1)}-{m.group(2)}-{m.group(3)}"


def save(blocks: list[str]) -> None:
    MAN.write_text(SEP.join(blocks), "utf-8")


def main() -> None:
    blocks = MAN.read_text("utf-8").split(SEP)
    done = failed = 0
    for i, b in enumerate(blocks):
        if re.search(r"^date = ", b, re.M):
            continue
        idm = re.search(r'^id = "(P\d{4})"', b, re.M)
        src = re.search(r'^source = "korea\.kr newsId=(\d+)"', b, re.M)
        if not (idm and src):
            continue
        time.sleep(1)
        try:
            d = posted(src.group(1))
        except Exception as e:  # noqa: BLE001 — 하나가 실패해도 나머지는 받는다
            print(f"실패 {idm.group(1)}: {e}", file=sys.stderr)
            failed += 1
            continue
        blocks[i] = re.sub(r'^(ext = "[^"]+")$', lambda x: f'{x.group(1)}\ndate = "{d}"', b, count=1, flags=re.M)
        done += 1
        if done % 50 == 0:
            save(blocks)
            print(f"{done}건 저장")
    save(blocks)
    # 다시 읽어 TOML 이 깨지지 않았는지 확인
    tomllib.loads(MAN.read_text("utf-8"))
    print(f"완료 {done}건, 실패 {failed}건")
    if failed:
        sys.exit(1)


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: 평가 세트 검사 스크립트를 쓴다**

`tools/eval/check_sets.py`:

```python
"""평가 세트 검사. 0 으로 끝나면 통과.

- 모든 [[doc]] 에 date(YYYY-MM-DD)가 있다.
- 질의 qid 는 모든 세트를 통틀어 겹치지 않는다. 유형은 KINDS 안이다.
- 정답 id 는 함께 검사하는 세트들의 문서 안에 있다(공개·마사회·데이터 세트를 합쳐 평가하므로).
- date 유형 질의는 5번째 열(range)이 있다: `YYYY-MM-DD..YYYY-MM-DD` 면 정답 문서 날짜가 모두 그 안이어야 하고, `future` 면 미래 표현 질의다.

사용: python3 tools/eval/check_sets.py eval/public eval/kra eval/data
"""
import datetime as dt
import sys
import tomllib
from pathlib import Path

KINDS = {"semantic", "keyword", "abbrev", "temporal", "deep", "kra", "kra-deep", "kra-plain", "date", "data-deep", "data-head"}


def day(s: str) -> dt.date:
    return dt.date.fromisoformat(s)


def main() -> None:
    dirs = [Path(a) for a in sys.argv[1:]]
    if not dirs:
        sys.exit("사용: check_sets.py <세트 폴더>...")
    errors: list[str] = []
    dates: dict[str, dt.date] = {}
    for d in dirs:
        for doc in tomllib.loads((d / "manifest.toml").read_text("utf-8"))["doc"]:
            if "date" not in doc:
                errors.append(f"{d}: {doc['id']} 에 date 없음")
                continue
            try:
                dates[doc["id"]] = day(doc["date"])
            except ValueError:
                errors.append(f"{d}: {doc['id']} date 형식 오류 {doc['date']!r}")
    seen: set[str] = set()
    for d in dirs:
        q = d / "queries.tsv"
        if not q.exists():
            continue
        for n, line in enumerate(q.read_text("utf-8").splitlines()[1:], start=2):
            if not line.strip():
                continue
            c = line.split("\t")
            if len(c) < 4:
                errors.append(f"{q}:{n} 칸이 4개 미만")
                continue
            qid, _text, kind, rel = c[0], c[1], c[2], c[3]
            if qid in seen:
                errors.append(f"{q}:{n} qid 중복 {qid}")
            seen.add(qid)
            if kind not in KINDS:
                errors.append(f"{q}:{n} 알 수 없는 유형 {kind}")
            ids = [x.strip() for x in rel.split(",") if x.strip()]
            for i in ids:
                if i not in dates:
                    errors.append(f"{q}:{n} 정답 id 없음 {i}")
            if kind == "date":
                rng = c[4].strip() if len(c) > 4 else ""
                if rng == "future":
                    continue
                if ".." not in rng:
                    errors.append(f"{q}:{n} date 질의에 기간 열(5번째) 없음")
                    continue
                a, b = (day(x) for x in rng.split(".."))
                for i in ids:
                    if i in dates and not (a <= dates[i] <= b):
                        errors.append(f"{q}:{n} {i} 날짜 {dates[i]} 가 기간 {rng} 밖")
    for e in errors:
        print(e)
    print(f"{'실패' if errors else '통과'}: 문서 {len(dates)} · 질의 {len(seen)}")
    sys.exit(1 if errors else 0)


if __name__ == "__main__":
    main()
```

- [ ] **Step 3: 검사가 지금 실패하는지 본다**

Run: `python3 tools/eval/check_sets.py eval/public eval/kra`
Expected: FAIL — `P0001 에 date 없음` 등 1,008줄, 마지막 줄 `실패: 문서 0 · 질의 202`.

- [ ] **Step 4: 공개 세트 게시일을 받는다(약 17분)**

Run: `python3 tools/eval/fetch_dates.py`
Expected: `완료 1000건, 실패 0건`. 실패가 있으면 다시 돌린다(남은 것만 받음). `git diff --stat eval/public/manifest.toml` 이 `1000 insertions` 이고 다른 줄은 바뀌지 않아야 한다.

- [ ] **Step 5: 마사회 매니페스트에 날짜와 새 문서를 넣는다**

`eval/kra/manifest.toml` 의 K0001~K0008 각 `ext = "hwp"` 줄 뒤에 출처의 시행일을 넣는다: K0001·K0002 `date = "2026-09-16"`, K0003·K0004 `date = "2026-05-29"`, K0005·K0006 `date = "2026-04-01"`, K0007·K0008 `date = "2026-02-27"`.

파일 끝에 덧붙인다(sha256 은 다음 단계에서 채운다):

```toml

[[doc]]
id = "K0009"
title = "(요약) 2026년 경마시행계획"
local = "2026년 경마시행계획(안)/(요약) 2026년 경마시행계획.hwp"
source = "open.go.kr 원문정보 · 한국마사회 경마기획부-1736(2025.12.21.) 붙임(요약)"
sha256 = "SHA_K0009"
ext = "hwp"
date = "2025-12-21"

[[doc]]
id = "K0010"
title = "(본문) 2026년 경마시행계획"
local = "2026년 경마시행계획(안)/(본문) 2026년 경마시행계획.hwp"
source = "open.go.kr 원문정보 · 한국마사회 경마기획부-1736(2025.12.21.) 붙임(본문)"
sha256 = "SHA_K0010"
ext = "hwp"
date = "2025-12-21"

[[doc]]
id = "K0011"
title = "2025년 직원 역량강화 프로젝트 미국 출장 계획(안)"
local = "2025년 직원 역량강화 프로젝트 출장계획(안)/2025년 직원 역량강화 프로젝트 미국 출장 계획(안).hwp"
source = "open.go.kr 원문정보 · 한국마사회 경마기획부-1432(2025.10.25.) 붙임"
sha256 = "SHA_K0011"
ext = "hwp"
date = "2025-10-25"
```

- [ ] **Step 6: SHA-256 을 계산해 자리표시자를 바꾸고 복사한다**

Run:
```bash
cd eval/downloads
for k in "K0009:2026년 경마시행계획(안)/(요약) 2026년 경마시행계획.hwp" \
         "K0010:2026년 경마시행계획(안)/(본문) 2026년 경마시행계획.hwp" \
         "K0011:2025년 직원 역량강화 프로젝트 출장계획(안)/2025년 직원 역량강화 프로젝트 미국 출장 계획(안).hwp"; do
  id=${k%%:*}; f=${k#*:}; h=$(shasum -a 256 "$f" | cut -d' ' -f1)
  sed -i '' "s/SHA_${id}/${h}/" ../kra/manifest.toml; echo "$id $h"
done
cd ../.. && python3 tools/eval/import_local.py eval/kra
```
Expected: 세 줄의 해시, 그리고 `ok: 11 documents verified`. `grep -c SHA_ eval/kra/manifest.toml` 이 0.

- [ ] **Step 7: 새 문서가 추출되는지 확인한다**

Run: `cargo run -p extract --release --bin extract-one -- eval/kra/docs/K0010.hwp | tail -c 400`
Expected: 마지막 줄이 `{"Ok":{"doc":{"title":...` 로 시작하는 JSON(실패 표시 `"Failed"` 가 아님). K0009·K0011 도 같은 방법으로 확인.

- [ ] **Step 8: 검사를 통과시킨다**

Run: `python3 tools/eval/check_sets.py eval/public eval/kra`
Expected: `통과: 문서 1011 · 질의 202`.

- [ ] **Step 9: README 를 고친다**

`eval/public/README.md` "## 문서" 목록 끝에 한 줄 추가:
```markdown
- **문서 날짜**(`date`, 2026-10-15 추가): korea.kr 보도자료 게시일. `python3 tools/eval/fetch_dates.py` 가 채운다. 평가에서는 이 날짜를 파일 수정일로 둔다(M2 날짜 가산 평가, DEC-055). 게시일 범위는 2026년 9~10월이라 "작년" 같은 표현은 마사회 세트에서만 시험된다.
```
`eval/kra/README.md` 문서 표에 세 줄을 더하고 "## 검토 이력" 끝에 한 항목을 더한다:
```markdown
| K0009 | (요약) 2026년 경마시행계획 | HWP | 2025-12-21 시행(작년에 만든 올해 계획 — "작년 경마시행계획" 날짜 함정) |
| K0010 | (본문) 2026년 경마시행계획 | HWP | 같은 건 본문 |
| K0011 | 2025년 직원 역량강화 프로젝트 미국 출장 계획(안) | HWP | 2025-10-25 시행 |
```
```markdown
- 2026-10-15 문서 추가(M2 날짜 평가용): 사용자가 2025년에 시행된 문서 2건(경마시행계획 요약·본문, 직원 역량강화 출장계획)을 추가. 모든 문서에 `date`(갑지 시행일)를 넣었다. 갑지 PDF 는 M4(공문 구조)에서 넣는다 — 지금 넣으면 기존 질의 정답을 다시 판정해야 한다(DEC-055).
```

- [ ] **Step 10: 커밋**

```bash
git add tools/eval/fetch_dates.py tools/eval/check_sets.py eval/public/manifest.toml eval/kra/manifest.toml eval/public/README.md eval/kra/README.md
git commit -m "eval: document dates for all eval docs, KRA K0009-K0011, set checker"
```

---

### Task 2: 평가 세트 — 데이터 형식 방해 문서(결정적 생성)

공공데이터포털은 내려받기에 자동등록 방지 문자(캡차)와 횟수 제한이 있어 스크립트로 다시 받을 수 없다(2026-10-15 확인). 그래서 설계 §5 의 "공공데이터포털 파일"을 **고정 시드 생성기**로 바꾼다 — 같은 바이트가 다시 나오고, 정답이 몇 번째 행에 있는지 정확히 안다. 이 변경은 Task 21 에서 DEC-055 상태 칸에 적는다.

**Files:**
- Create: `tools/eval/make_data_set.py`, `eval/data/manifest.toml`(생성), `eval/data/queries.tsv`(생성), `eval/data/README.md`
- Modify: `.gitignore`

**Interfaces:**
- Produces: 세트 폴더 `eval/data`(문서 id `D0001`~`D0048`, 유형 `data-deep` 8개·`data-head` 4개). 다른 세트와 같은 형식이라 `check_sets.py`·m0eval·`kra-search eval --set` 이 그대로 읽는다.

- [ ] **Step 1: gitignore 에 생성 문서 폴더를 넣는다**

`.gitignore` 끝에 `/eval/data/docs/` 한 줄 추가.

- [ ] **Step 2: 생성기를 쓴다**

`tools/eval/make_data_set.py`:

```python
"""데이터 형식(CSV·XLSX·TXT) 방해 문서와 그 질의를 만든다 — 고정 시드, 원본은 저장소에 넣지 않는다.
설계: docs/superpowers/specs/2026-10-15-m2-hybrid-search-design.md §5, DEC-055.

- 48개(CSV 16 · XLSX 16 · TXT 16), 각 600행. 행 하나 ≈ 50~70자 → 본문 청크 하나(300토큰 ≈ 480자)에 약 8행.
- 정답이 앞 20청크(약 160행) 뒤 — 420~560행 — 에 있는 질의 8개(data-deep), 앞 30행 안에 있는 질의 4개(data-head).
- 다시 돌려도 같은 바이트가 나온다(--check 가 매니페스트 SHA 와 비교). 다르면 생성기가 바뀐 것이다 — 매니페스트·질의를 새로 쓰고 이유를 README 에 적는다.

사용: python3 tools/eval/make_data_set.py          # 문서·매니페스트·질의 생성
      python3 tools/eval/make_data_set.py --check  # 문서만 다시 만들어 매니페스트 SHA 와 비교
"""
import datetime as dt
import hashlib
import io
import random
import sys
import zipfile
from pathlib import Path
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parents[2] / "eval" / "data"
SEED = 20261015
ROWS = 600
SITES = ["렛츠런파크 서울", "렛츠런파크 부산경남", "렛츠런파크 제주", "렛츠런CCC 강남", "장외발매소 분당",
         "장외발매소 일산", "장외발매소 대전", "장외발매소 광주", "장외발매소 수원", "장외발매소 인천"]
FACILITIES = ["관람대", "주차장", "마사", "발매창구", "화장실", "엘리베이터", "에스컬레이터", "식당", "전광판", "경주로"]
PROBLEMS = ["청소 상태", "조명 밝기", "안내 표지", "냉난방", "소음", "대기 시간", "직원 응대", "좌석 파손", "출입 통제", "물품 분실"]
STATUS = ["접수", "처리중", "완료", "이관", "보류"]
CHECKS = ["소화기 압력", "비상구 표시등", "누전 차단기", "배수 상태", "난간 고정", "CCTV 작동", "방송 설비", "급수 설비"]
RESULTS = ["양호", "양호", "양호", "주의", "불량"]
LOGS = ["발매기 #{n} 정상 동작 확인", "발매 서버 {n}번 응답 시간 {ms}ms", "현장 단말 {n} 재부팅", "회선 점검 완료 구간 {n}",
        "출력 용지 교체 단말 {n}", "정산 배치 {n}건 처리", "백업 작업 {n}번 정상 종료", "단말 {n} 접속 지연 경보 해제"]

# (형식, 같은 형식 안의 파일 순번, 행 위치 범위, 심는 행 문구, 질의, 유형)
PLANTED = [
    ("csv", 0, (420, 560), "렛츠런파크 부산경남 주차장 야간 조명이 꺼져 있다는 민원", "부경 주차장 밤에 불 꺼진 민원", "data-deep"),
    ("csv", 5, (420, 560), "장외발매소 분당 엘리베이터 문에 손님 손이 끼인 사고 신고", "분당 지점 승강기 문에 끼인 사고", "data-deep"),
    ("csv", 11, (420, 560), "은퇴한 경주마를 개인이 입양하는 절차 문의", "경주 끝난 말 데려가는 방법 문의", "data-deep"),
    ("xlsx", 2, (420, 560), "제주 마사 지붕 누수로 방수 공사 실시", "제주 마구간 비 새서 공사한 기록", "data-deep"),
    ("xlsx", 9, (420, 560), "서울 관람대 3층 소화기 압력 미달 12개 교체", "서울 관람석 소화기 바꾼 점검", "data-deep"),
    ("xlsx", 14, (420, 560), "부산경남 마필 수송차량 냉방장치 고장 수리", "말 실어 나르는 트럭 에어컨 고장", "data-deep"),
    ("txt", 1, (420, 560), "발매기 #4471 지폐 인식 모듈 고장으로 현장 교체 완료", "지폐 투입 안 되는 발매기 수리", "data-deep"),
    ("txt", 12, (420, 560), "제주 장외발매 네트워크 회선 이중화 장비 전원 장애 복구", "제주 발매 통신 끊긴 장애 처리", "data-deep"),
    ("csv", 3, (5, 25), "렛츠런CCC 강남 대형 전광판 글자 깨짐 민원", "강남 지점 큰 화면 글씨 깨짐", "data-head"),
    ("xlsx", 6, (5, 25), "광주 장외발매소 급수 설비 녹물 발생으로 배관 교체", "광주 지점 수돗물 녹물 배관", "data-head"),
    ("txt", 7, (5, 25), "일산 장외발매소 출력 용지 걸림 반복으로 프린터 교체", "일산 프린터 종이 걸려서 바꿈", "data-head"),
    ("csv", 14, (5, 25), "대전 장외발매소 식당 위생 불량 신고", "대전 지점 식당 더럽다는 신고", "data-head"),
]
KINDS = {"csv": ("민원 접수 현황", ["접수번호", "접수일", "사업장", "유형", "처리상태", "요지"]),
         "xlsx": ("시설 점검 기록", ["점검일", "사업장", "시설", "점검 항목", "결과", "조치 내용"]),
         "txt": ("발매 시스템 운영 일지", None)}
XML = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
ZIP_TIME = (2026, 1, 1, 0, 0, 0)


def rows_for(kind: str, rng: random.Random, start: dt.date) -> list[list[str] | str]:
    out: list[list[str] | str] = []
    for r in range(ROWS):
        day = start + dt.timedelta(days=r // 3)
        site, fac = rng.choice(SITES), rng.choice(FACILITIES)
        if kind == "csv":
            out.append([f"{day.year}-{r + 1:05d}", day.isoformat(), site, rng.choice(["불편", "건의", "칭찬", "문의"]),
                        rng.choice(STATUS), f"{fac} {rng.choice(PROBLEMS)} 관련 {rng.choice(['문의', '불편 신고', '개선 요청'])}"])
        elif kind == "xlsx":
            res = rng.choice(RESULTS)
            out.append([day.isoformat(), site, fac, rng.choice(CHECKS), res,
                        "이상 없음" if res == "양호" else f"{fac} {rng.choice(['보수 예정', '부품 교체', '재점검 요청'])}"])
        else:
            t = f"{rng.randrange(8, 20):02d}:{rng.randrange(60):02d}:{rng.randrange(60):02d}"
            msg = rng.choice(LOGS).format(n=rng.randrange(1000, 9999), ms=rng.randrange(20, 900))
            out.append(f"{day.isoformat()} {t} [{rng.choice(['정보', '정보', '경고'])}] {site} {msg}")
    return out


def plant(kind: str, rows: list, text: str, at: int) -> None:
    if kind == "csv":
        rows[at][5] = text
    elif kind == "xlsx":
        rows[at][5] = text
    else:
        head = rows[at].split("] ", 1)[0]
        rows[at] = f"{head}] {text}"


def csv_bytes(header: list[str], rows: list[list[str]], cp949: bool) -> bytes:
    def q(v: str) -> str:
        return f'"{v}"' if ("," in v or '"' in v) else v
    text = "\n".join(",".join(q(c) for c in line) for line in [header, *rows]) + "\n"
    return text.encode("cp949" if cp949 else "utf-8")


def col(i: int) -> str:
    s = ""
    i += 1
    while i:
        i, r = divmod(i - 1, 26)
        s = chr(65 + r) + s
    return s


def xlsx_bytes(title: str, header: list[str], rows: list[list[str]]) -> bytes:
    def row_xml(n: int, cells: list[str]) -> str:
        return f'<row r="{n}">' + "".join(
            f'<c r="{col(i)}{n}" t="inlineStr"><is><t>{escape(v)}</t></is></c>' for i, v in enumerate(cells)) + "</row>"
    sheet = (XML + '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>'
             + "".join(row_xml(n + 1, r) for n, r in enumerate([header, *rows])) + "</sheetData></worksheet>")
    parts = [
        ("[Content_Types].xml", XML + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
         '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
         '<Default Extension="xml" ContentType="application/xml"/>'
         '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
         '<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
         '<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/></Types>'),
        ("_rels/.rels", XML + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
         '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>'),
        ("xl/workbook.xml", XML + '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
         'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>'
         '<sheet name="기록" sheetId="1" r:id="rId1"/></sheets></workbook>'),
        ("xl/_rels/workbook.xml.rels", XML + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
         '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>'),
        ("xl/worksheets/sheet1.xml", sheet),
        ("docProps/core.xml", XML + '<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" '
         f'xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{escape(title)}</dc:title></cp:coreProperties>'),
    ]
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        for name, data in parts:
            info = zipfile.ZipInfo(name, date_time=ZIP_TIME)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            z.writestr(info, data.encode("utf-8"))
    return buf.getvalue()


def build() -> tuple[list[dict], list[tuple[str, str, str, str]]]:
    rng = random.Random(SEED)
    docs: list[dict] = []
    queries: list[tuple[str, str, str, str]] = []
    n = 0
    for kind in ["csv", "xlsx", "txt"]:
        title_base, header = KINDS[kind]
        for k in range(16):
            n += 1
            did = f"D{n:04d}"
            start = dt.date(2024, 1, 1) + dt.timedelta(days=rng.randrange(0, 900))
            rows = rows_for(kind, rng, start)
            for (pk, pi, (lo, hi), text, query, qtype) in PLANTED:
                if pk == kind and pi == k:
                    plant(kind, rows, text, rng.randrange(lo, hi))
                    queries.append((f"X{len(queries) + 1:03d}", query, qtype, did))
            last = start + dt.timedelta(days=(ROWS - 1) // 3)
            title = f"{start.year}년 {title_base} {k + 1:02d}"
            if kind == "csv":
                data = csv_bytes(header, rows, cp949=(k % 2 == 0))
            elif kind == "xlsx":
                data = xlsx_bytes(title, header, rows)
            else:
                data = ("\n".join(rows) + "\n").encode("utf-8")
            docs.append({"id": did, "title": title, "ext": kind, "date": last.isoformat(), "data": data})
    return docs, queries


def manifest(docs: list[dict]) -> str:
    out = ["# 데이터 형식 방해 문서(합성). 원본은 저장소에 넣지 않는다 — `python3 tools/eval/make_data_set.py` 로 다시 만든다.\n"]
    for d in docs:
        out.append(f'[[doc]]\nid = "{d["id"]}"\ntitle = "{d["title"]}"\nsource = "합성(tools/eval/make_data_set.py, seed {SEED})"\n'
                   f'sha256 = "{hashlib.sha256(d["data"]).hexdigest()}"\next = "{d["ext"]}"\ndate = "{d["date"]}"\n')
    return "\n".join(out)


def main() -> None:
    docs, queries = build()
    out = ROOT / "docs"
    out.mkdir(parents=True, exist_ok=True)
    for d in docs:
        (out / f'{d["id"]}.{d["ext"]}').write_bytes(d["data"])
    if "--check" in sys.argv:
        want = (ROOT / "manifest.toml").read_text("utf-8")
        if want != manifest(docs):
            sys.exit("생성 결과가 매니페스트와 다름 — 생성기가 바뀌었다")
        print(f"ok: {len(docs)} documents match")
        return
    (ROOT / "manifest.toml").write_text(manifest(docs), "utf-8")
    lines = ["qid\tquery\ttype\trelevant"] + ["\t".join(q) for q in queries]
    (ROOT / "queries.tsv").write_text("\n".join(lines) + "\n", "utf-8")
    print(f"문서 {len(docs)} · 질의 {len(queries)}")


if __name__ == "__main__":
    main()
```

- [ ] **Step 3: 생성하고 결정성을 확인한다**

Run: `python3 tools/eval/make_data_set.py && python3 tools/eval/make_data_set.py --check`
Expected: `문서 48 · 질의 12` 뒤 `ok: 48 documents match`.

- [ ] **Step 4: 추출과 심은 행 위치를 확인한다**

`extract-one` 은 마지막 줄에 표시 `\u{1}KRA `(`extract::worker::MARKER`) 뒤 JSON 을 찍는다 — 마지막 줄에서 첫 `{` 부터 읽는다.

Run:
```bash
check() { # <파일> <심은 문구 일부>
  cargo run -q -p extract --release --bin extract-one -- "$1" | tail -1 > /tmp/kra-one.json
  python3 - "$2" <<'PY'
import json, sys
raw = open('/tmp/kra-one.json', encoding='utf-8').read()
out = json.loads(raw[raw.find('{'):])
doc = out['Ok']['doc']   # KeyError 면 추출 실패("Failed") — 생성기의 형식 구성을 고친다
text = '\n'.join(b['text'] for b in doc['blocks'])
print('블록', len(doc['blocks']), '· 글자', len(text), '· 심은 행 글자 위치', text.find(sys.argv[1]))
PY
}
check eval/data/docs/D0001.csv "야간 조명이 꺼져"
check eval/data/docs/D0019.xlsx "마사 지붕 누수"
check eval/data/docs/D0034.txt "지폐 인식 모듈"
```
Expected: 세 줄 모두 `심은 행 글자 위치` 가 20,000 이상(앞 20청크 ≈ 9,500자보다 훨씬 뒤). -1 이면 못 찾은 것 — 생성기를 고친다.

- [ ] **Step 5: README 를 쓰고 검사한다**

`eval/data/README.md`:
```markdown
# 데이터 형식 방해 문서 세트 (M2)

본문 3단 순서의 3(데이터 형식 뒷부분, 스펙 §6.6)이 검색을 돕는지·방해하는지 숫자로 재려고 만든 **합성** 세트(DEC-055).

- 문서: CSV 16(짝수 번째 CP949) · XLSX 16 · TXT 16, 각 600행. 민원 접수 현황 · 시설 점검 기록 · 발매 시스템 운영 일지 모양.
- 질의: `data-deep` 8개 — 정답 행이 420~560행(앞 20청크 뒤)에 있다. 3을 끄면 의미 검색으로는 못 찾아야 정상. `data-head` 4개 — 정답이 앞 30행 안(2에서 찾음).
- 생성: `python3 tools/eval/make_data_set.py`(고정 시드 20261015). `--check` 로 매니페스트 SHA 와 비교. 원본(`docs/`)은 커밋하지 않는다.
- 공공데이터포털 실제 파일을 쓰지 않은 이유: 내려받기에 자동등록 방지 문자와 횟수 제한이 있어 다시 받을 수 없다(2026-10-15).
- 한계: 실제 업무 PC 의 로그·내보내기 파일과 모양이 다르다 — 실제 영향은 폐쇄망 확인(M2 설계 §7)에서 본다.
```

Run: `python3 tools/eval/check_sets.py eval/public eval/kra eval/data`
Expected: `통과: 문서 1059 · 질의 214`.

- [ ] **Step 6: 커밋**

```bash
git add .gitignore tools/eval/make_data_set.py eval/data/manifest.toml eval/data/queries.tsv eval/data/README.md
git commit -m "eval: synthetic data-format distractor set with deep/head queries (deterministic)"
```

---

### Task 3: 평가 세트 — `date` 질의 30개와 새 마사회 문서 질의

**Files:**
- Modify: `eval/public/queries.tsv`, `eval/kra/queries.tsv`, `eval/public/README.md`, `eval/kra/README.md`

**Interfaces:**
- Consumes: Task 1 의 `date`, `check_sets.py`.
- Produces: 유형 `date` 질의 30개(qid `T001`~`T030`, 5번째 열 `range`), 새 문서용 질의(K021~, D034~, P020~ 이어 붙임).

질의는 사람이 판단해 쓰는 일이다 — 아래 규칙을 지키고, 검사 스크립트와 Task 4 의 해석 시험(`eval_date_queries_parse_to_their_range`)이 기계적으로 확인한다.

- [ ] **Step 1: 날짜별 문서 분포를 뽑아 주제 묶음을 찾는다**

Run:
```bash
python3 - <<'PY'
import tomllib, collections
docs = tomllib.loads(open('eval/public/manifest.toml', encoding='utf-8').read())['doc']
by = collections.Counter(d['date'][:7] for d in docs)
print(sorted(by.items()))
for d in sorted(docs, key=lambda d: d['date'])[:5] + sorted(docs, key=lambda d: d['date'])[-5:]:
    print(d['date'], d['id'], d['title'][:40])
PY
```
Expected: 월별 개수(2026-09·2026-10 중심)와 가장 이른·늦은 문서. 이 출력으로 아래 규칙의 "같은 주제, 다른 날짜" 묶음을 고른다: 제목 낱말(예: 전세사기·폭염·국정감사·추석·채용)로 `grep` 해 날짜가 다른 문서가 2개 이상인 주제를 고른다.

- [ ] **Step 2: 질의를 쓴다 — 규칙**

기준일 2026-10-15(목). 주는 월요일 시작: 이번 주 = 10-12~10-18, 지난주 = 10-05~10-11. 지난달 = 2026-09.

1. **각 질의 = 주제 + 날짜 표현**이고, 주제만으로는 날짜가 다른 문서가 **2개 이상** 걸려야 한다(날짜가 순위를 가른다). 정답 = 그 주제 문서 중 날짜가 기간 안인 것.
2. 5번째 열 `range` = 그 표현이 뜻하는 기간(`YYYY-MM-DD..YYYY-MM-DD`). 미래 표현 질의는 `future`, 정답은 주제만으로 고른다.
3. 표현은 아래 목록을 **각각 한 번 이상** 쓴다(30개 안에서): 작년 · 지난해 · 전년도 · 금년 · 올해 상반기 · 올 초 · 연말 · '25 · 25년 · 2025년 · 지난달 · 전월 · 저번 달 · 이번 달 · 이달 · 금월 · 9월 · 9월 말 · 10월 초 · 3분기 · 지난주 · 이번 주 · 금주 · 요즘 · 최근 · 얼마 전 · 최근 한 달 · 가을 · 지난 6개월 · 내년(future) · 다음 달(future).
4. 마사회 문서를 쓰는 질의를 6개 이상: 예) `작년 경마시행계획`(K0009,K0010 · 2025-01-01..2025-12-31 — 제목은 2026년이지만 작년에 시행), `지난해 직원 해외 출장 계획`(K0011), `올해 상반기 적극행정 계획`(K0003,K0004 · 2026-01-01..2026-06-30), `지난달 내부통제 규정 개정`(K0001,K0002 · 2026-09-01..2026-09-30), `금년 2월 경마 인식조사 결과`(K0007,K0008), `올 초 경마 국민 인식 조사`(K0007,K0008 · 2026-01-01..2026-02-28).
5. 질의 문장은 문서 문장을 베끼지 않는다. 일상 표현으로.
6. qid `T001`~`T030`, 유형 `date`. 공개 문서 정답 질의는 `eval/public/queries.tsv`, 마사회 문서 정답 질의는 `eval/kra/queries.tsv` 끝에 붙인다.

새 마사회 문서 일반 질의(날짜 표현 없음)도 붙인다(`eval/kra/queries.tsv`): `kra` 2개(K021~K022, 예 `경마시행계획 경주 일수`), `kra-deep` 3개(D034~D036, 예 `월요 공휴일 경마 처음 시행하는 곳` — K0009·K0010 본문을 `extract-one` 으로 읽고 답이 있는지 확인), `kra-plain` 2개(P020~P021, 예 `직원들 외국 견학 가는 일정`). 정답은 그 정보를 직접 담은 문서만(README 원칙).

- [ ] **Step 3: 검사를 통과시킨다**

Run: `python3 tools/eval/check_sets.py eval/public eval/kra eval/data`
Expected: `통과: 문서 1059 · 질의 251`(214 + date 30 + 새 일반 질의 7). 기간 밖 오류가 나면 정답이나 기간을 고친다.

- [ ] **Step 4: README 에 기록한다**

`eval/public/README.md` 질의 표에 한 줄: `| date | 30 | 오늘 기준(2026-10-15) 날짜 표현 + 주제. 5번째 열 = 표현의 기간. 주제만으로는 날짜가 다른 문서가 2개 이상 걸림. 한자어·구어·약식 표현을 섞음(M2 설계 §5) |` — 그리고 "공개·마사회 세트에 나눠 있다"를 덧붙인다. `eval/kra/README.md` 질의 표의 수(kra 22·kra-deep 36·kra-plain 21)를 고치고 검토 이력에 "2026-10-15 M2 질의 추가(date 중 마사회 문서 정답 N개, 새 문서 일반 질의 7개). 사용자 검토 대기 — 유형별 몇 개씩"을 적는다.

- [ ] **Step 5: 커밋하고 사용자 검토를 요청한다**

```bash
git add eval/public/queries.tsv eval/kra/queries.tsv eval/public/README.md eval/kra/README.md
git commit -m "eval: 30 date queries (relative/absolute/vague, hanja/colloquial forms) and queries for K0009-K0011"
```
사용자에게 date 질의 8개·새 일반 질의 4개를 보여 주고 검토를 부탁한다(M0 과 같은 방식). 검토를 기다리며 다음 작업을 진행한다 — 정답 수정은 Task 16 실행 전까지 반영한다.

---
### Task 4: `query` 크레이트 — 날짜 계산과 날짜 표현 해석

**Files:**
- Create: `crates/query/Cargo.toml`, `crates/query/src/lib.rs`, `crates/query/src/calendar.rs`, `crates/query/src/dates.rs`, `crates/query/src/dates.tsv`, `crates/query/tests/eval_dates.rs`

**Interfaces:**
- Produces (Task 5·8·14·15·17·18 이 쓴다):
  - `query::calendar::{Day, Range, from_ymd, to_ymd, days_in_month, weekday, add_months, parse_ymd, format_ymd, day_of_unix_ns, today, noon_kst_unix_secs, KST_OFFSET_SECS}` — `Day = i32`(1970-01-01 기준 일수), `Range { start: Day, end: Day }`(양 끝 포함).
  - `query::dates::{DateTable, DateSpan, Precision, Relativity}` — `DateTable::builtin()`, `DateTable::extend_tsv(&mut self, &str) -> Result<(), String>`, `DateTable::find(&self, q: &str, today: Day) -> Vec<DateSpan>`.
  - `DateSpan { start: usize, end: usize, strip_end: usize, range: Option<Range>, precision: Precision, relativity: Relativity }` — 바이트 위치. `range == None` 은 미래 표현.

- [ ] **Step 1: 크레이트를 만든다**

`crates/query/Cargo.toml`:
```toml
[package]
name = "query"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
```

`crates/query/src/lib.rs`(Task 5 에서 `parse`·`kind` 를 더한다):
```rust
//! 검색어 해석(스펙 §8.1): 날짜 표현 → 기간(칩), 검색어 유형. 기준 시점(오늘)은 바깥에서 받는다 — 평가·시험이 해가 바뀌어도 같게.
pub mod calendar;
pub mod dates;

pub use calendar::{Day, Range};
pub use dates::{DateSpan, DateTable, Precision, Relativity};
```

- [ ] **Step 2: 날짜 계산 시험을 쓴다**

`crates/query/src/calendar.rs` 의 끝에 들어갈 시험(먼저 파일을 이 시험만으로 만든다):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_roundtrip_and_known_days() {
        assert_eq!(from_ymd(1970, 1, 1), 0);
        assert_eq!(from_ymd(2026, 10, 15), 20_741);
        for d in [-800_000, -1, 0, 1, 11_016, 20_741, 60_000] {
            let (y, m, dd) = to_ymd(d);
            assert_eq!(from_ymd(y, m, dd), d);
        }
        assert_eq!(to_ymd(from_ymd(2028, 2, 29)), (2028, 2, 29));
    }

    #[test]
    fn weekday_monday_is_zero() {
        assert_eq!(weekday(from_ymd(2026, 10, 15)), 3, "2026-10-15 은 목요일");
        assert_eq!(weekday(from_ymd(2026, 10, 12)), 0, "월요일");
    }

    #[test]
    fn months_and_leap_years() {
        assert_eq!(days_in_month(2028, 2), 29);
        assert_eq!(days_in_month(2100, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(add_months(2026, 1, -1), (2025, 12));
        assert_eq!(add_months(2026, 12, 1), (2027, 1));
        assert_eq!(add_months(2026, 10, -22), (2024, 12));
        let r = Range::months(2025, 12, 3);
        assert_eq!((format_ymd(r.start), format_ymd(r.end)), ("2025-12-01".to_string(), "2026-02-28".to_string()));
    }

    #[test]
    fn parse_and_format() {
        assert_eq!(parse_ymd("2026-10-15"), Some(20_741));
        assert_eq!(parse_ymd("2026-02-30"), None);
        assert_eq!(parse_ymd("2026-13-01"), None);
        assert_eq!(format_ymd(20_741), "2026-10-15");
    }

    #[test]
    fn unix_ns_is_read_in_korean_time() {
        // 2026-10-14 15:00:00 UTC = 2026-10-15 00:00 한국 시각
        let ns = (from_ymd(2026, 10, 14) as i64 * 86_400 + 15 * 3_600) * 1_000_000_000;
        assert_eq!(day_of_unix_ns(ns), from_ymd(2026, 10, 15));
        assert_eq!(day_of_unix_ns(ns - 1), from_ymd(2026, 10, 14));
        assert_eq!(day_of_unix_ns(noon_kst_unix_secs(20_741) * 1_000_000_000), 20_741);
    }
}
```

- [ ] **Step 3: 시험이 실패하는지 본다**

Run: `cargo test -p query --release` (워크스페이스는 `members = ["crates/*"]` 라 자동 포함)
Expected: FAIL — `cannot find function from_ymd` 등.

- [ ] **Step 4: 날짜 계산을 구현한다**

`crates/query/src/calendar.rs` 시험 위에:
```rust
//! 그레고리력 날짜 계산(외부 크레이트 없이). 날짜는 1970-01-01 기준 일수(`Day`), 한국 시각(UTC+9) 기준.

use std::time::{SystemTime, UNIX_EPOCH};

pub type Day = i32;

pub const KST_OFFSET_SECS: i64 = 9 * 3_600;

/// Howard Hinnant 의 days_from_civil.
pub fn from_ymd(y: i32, m: u32, d: u32) -> Day {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let m = m as i32;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Howard Hinnant 의 civil_from_days.
pub fn to_ymd(z: Day) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
    }
}

/// 0 = 월요일 … 6 = 일요일(1970-01-01 은 목요일).
pub fn weekday(d: Day) -> u32 {
    (d + 3).rem_euclid(7) as u32
}

/// (y, m) 에서 k 개월 뒤(음수면 앞).
pub fn add_months(y: i32, m: u32, k: i32) -> (i32, u32) {
    let t = y * 12 + (m as i32 - 1) + k;
    (t.div_euclid(12), (t.rem_euclid(12) + 1) as u32)
}

/// "YYYY-MM-DD" → Day. 없는 날짜면 None.
pub fn parse_ymd(s: &str) -> Option<Day> {
    let mut it = s.trim().split('-');
    let y: i32 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    let d: u32 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
        return None;
    }
    Some(from_ymd(y, m, d))
}

pub fn format_ymd(d: Day) -> String {
    let (y, m, dd) = to_ymd(d);
    format!("{y:04}-{m:02}-{dd:02}")
}

/// 유닉스 시각(나노초) → 한국 날짜.
pub fn day_of_unix_ns(ns: i64) -> Day {
    (ns.div_euclid(1_000_000_000) + KST_OFFSET_SECS).div_euclid(86_400) as Day
}

/// 이 PC 의 오늘(한국 시각). 검색어 해석의 기준 시점 기본값 — 시험·평가는 고정 날짜를 주입한다.
pub fn today() -> Day {
    let ns = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(0);
    day_of_unix_ns(ns)
}

/// 그날 정오(한국 시각)의 유닉스 초 — 평가 문서의 수정 시각을 날짜 경계에서 떨어뜨리려고.
pub fn noon_kst_unix_secs(d: Day) -> i64 {
    d as i64 * 86_400 + 12 * 3_600 - KST_OFFSET_SECS
}

/// 날짜 기간(양 끝 포함).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub start: Day,
    pub end: Day,
}

impl Range {
    pub fn contains(&self, d: Day) -> bool {
        self.start <= d && d <= self.end
    }
    pub fn days(&self) -> i32 {
        self.end - self.start + 1
    }
    pub fn day(d: Day) -> Range {
        Range { start: d, end: d }
    }
    pub fn year(y: i32) -> Range {
        Range { start: from_ymd(y, 1, 1), end: from_ymd(y, 12, 31) }
    }
    pub fn month(y: i32, m: u32) -> Range {
        Range { start: from_ymd(y, m, 1), end: from_ymd(y, m, days_in_month(y, m)) }
    }
    /// (y, m) 부터 n 개월(해를 넘을 수 있음 — 겨울·연말연시).
    pub fn months(y: i32, m: u32, n: i32) -> Range {
        let (ey, em) = add_months(y, m, n - 1);
        Range { start: from_ymd(y, m, 1), end: from_ymd(ey, em, days_in_month(ey, em)) }
    }
}
```

- [ ] **Step 5: 시험이 통과하는지 본다**

Run: `cargo test -p query --release calendar`
Expected: PASS(5개).

- [ ] **Step 6: 내장 날짜 표를 쓴다**

`crates/query/src/dates.tsv`(탭으로 구분 — 편집기에서 탭이 공백으로 바뀌지 않게 주의):
```
# 날짜 표현 표(DEC-053). 형식: 표현<TAB>종류<TAB>값<TAB>정밀도(exact|vague). 표현 안의 공백은 무시하고 맞춘다("지난 달" = "지난달").
# 종류: year(해 오프셋) · year_bound(올 — 뒤에 시기가 붙을 때만) · month_rel · week_rel(월요일 시작) · day_rel · quarter_rel · quarter(n)
#       half(1|2) · year_part(early|mid|late|whole|turn) · season(spring|summer|autumn|winter)
#       month_part(early|mid|late — 달·해 바로 뒤에서만) · this_month_part(early|late) · window(며칠 전부터,며칠 전까지) · future(칩 아님)
# 넣지 않은 낱말(일상어와 겹침): 전달(전하다)·전주(지명)·전일(온종일)·당일(그날). 바깥 파일로 같은 형식의 줄을 덧붙일 수 있다.
올해	year	0	exact
금년	year	0	exact
금년도	year	0	exact
당해	year	0	exact
당해연도	year	0	exact
당해년도	year	0	exact
작년	year	-1	exact
지난해	year	-1	exact
전년	year	-1	exact
전년도	year	-1	exact
객년	year	-1	exact
재작년	year	-2	exact
지지난해	year	-2	exact
올	year_bound	0	exact
이번 달	month_rel	0	exact
이달	month_rel	0	exact
금월	month_rel	0	exact
당월	month_rel	0	exact
지난달	month_rel	-1	exact
저번 달	month_rel	-1	exact
전월	month_rel	-1	exact
지지난달	month_rel	-2	exact
이번 주	week_rel	0	exact
금주	week_rel	0	exact
지난주	week_rel	-1	exact
저번 주	week_rel	-1	exact
지지난주	week_rel	-2	exact
오늘	day_rel	0	exact
금일	day_rel	0	exact
어제	day_rel	-1	exact
어저께	day_rel	-1	exact
그제	day_rel	-2	exact
그저께	day_rel	-2	exact
엊그제	day_rel	-2	vague
이번 분기	quarter_rel	0	exact
지난 분기	quarter_rel	-1	exact
전분기	quarter_rel	-1	exact
일사분기	quarter	1	exact
이사분기	quarter	2	exact
삼사분기	quarter	3	exact
사사분기	quarter	4	exact
상반기	half	1	exact
전반기	half	1	exact
하반기	half	2	exact
후반기	half	2	exact
연초	year_part	early	exact
연말	year_part	late	exact
연중	year_part	whole	vague
연말연시	year_part	turn	exact
초	month_part	early	exact
상순	month_part	early	exact
중순	month_part	mid	exact
말	month_part	late	exact
하순	month_part	late	exact
월초	this_month_part	early	exact
월말	this_month_part	late	exact
봄	season	spring	vague
여름	season	summer	vague
가을	season	autumn	vague
겨울	season	winter	vague
최근	window	90,0	vague
요즘	window	90,0	vague
요새	window	90,0	vague
근래	window	180,0	vague
얼마 전	window	60,7	vague
몇 달 전	window	180,45	vague
몇 년 전	window	1825,365	vague
내년	future	0	exact
명년	future	0	exact
익년	future	0	exact
차년도	future	0	exact
다음 달	future	0	exact
내달	future	0	exact
다음 주	future	0	exact
내일	future	0	exact
모레	future	0	exact
```

- [ ] **Step 7: 해석 시험을 쓴다**

`crates/query/src/dates.rs` 의 끝에 들어갈 시험(먼저 이 시험과 `use` 만으로 파일을 만든다):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::{format_ymd, parse_ymd};

    fn day(s: &str) -> Day {
        parse_ymd(s).unwrap()
    }

    /// 검색어 → (표현, 시작, 끝). 표현이 하나라고 가정한다.
    fn one(q: &str, today: &str) -> (String, String, String) {
        let t = DateTable::builtin();
        let v = t.find(q, day(today));
        assert_eq!(v.len(), 1, "{q}: {v:?}");
        let s = &v[0];
        let r = s.range.expect("미래 표현 아님");
        (q[s.start..s.end].to_string(), format_ymd(r.start), format_ymd(r.end))
    }

    fn none(q: &str) {
        let v = DateTable::builtin().find(q, day("2026-10-15"));
        assert!(v.is_empty(), "{q}: {v:?}");
    }

    #[test]
    fn relative_words_on_reference_day() {
        let t = "2026-10-15";
        for (q, expr, a, b) in [
            ("작년 경마시행계획", "작년", "2025-01-01", "2025-12-31"),
            ("지난해 출장 계획", "지난해", "2025-01-01", "2025-12-31"),
            ("전년도 실적", "전년도", "2025-01-01", "2025-12-31"),
            ("재작년 예산", "재작년", "2024-01-01", "2024-12-31"),
            ("금년 사업", "금년", "2026-01-01", "2026-12-31"),
            ("지난달 규정 개정", "지난달", "2026-09-01", "2026-09-30"),
            ("저번 달 회의", "저번 달", "2026-09-01", "2026-09-30"),
            ("이달 행사", "이달", "2026-10-01", "2026-10-31"),
            ("지난주 보도자료", "지난주", "2026-10-05", "2026-10-11"),
            ("이번 주 일정", "이번 주", "2026-10-12", "2026-10-18"),
            ("어제 받은 공문", "어제", "2026-10-14", "2026-10-14"),
            ("지난 분기 실적", "지난 분기", "2026-07-01", "2026-09-30"),
            ("요즘 민원", "요즘", "2026-07-17", "2026-10-15"),
        ] {
            assert_eq!(one(q, t), (expr.to_string(), a.to_string(), b.to_string()), "{q}");
        }
    }

    #[test]
    fn composed_periods() {
        let t = "2026-10-15";
        for (q, expr, a, b) in [
            ("올해 상반기 적극행정 계획", "올해 상반기", "2026-01-01", "2026-06-30"),
            ("올 초 인식조사", "올 초", "2026-01-01", "2026-02-28"),
            ("올봄 행사", "올봄", "2026-03-01", "2026-05-31"),
            ("작년 말 실적", "작년 말", "2025-11-01", "2025-12-31"),
            ("작년 겨울 폭설", "작년 겨울", "2025-12-01", "2026-02-28"),
            ("2025년 9월 회의", "2025년 9월", "2025-09-01", "2025-09-30"),
            ("작년 9월 말 보고", "작년 9월 말", "2025-09-21", "2025-09-30"),
            ("2025년 3분기 매출", "2025년 3분기", "2025-07-01", "2025-09-30"),
            ("지난달 말 공지", "지난달 말", "2026-09-21", "2026-09-30"),
        ] {
            assert_eq!(one(q, t), (expr.to_string(), a.to_string(), b.to_string()), "{q}");
        }
    }

    #[test]
    fn numeric_forms() {
        let t = "2026-10-15";
        for (q, expr, a, b) in [
            ("2025년 사업계획", "2025년", "2025-01-01", "2025-12-31"),
            ("2025년도 예산", "2025년도", "2025-01-01", "2025-12-31"),
            ("2025 사업계획", "2025", "2025-01-01", "2025-12-31"),
            ("25년 결산", "25년", "2025-01-01", "2025-12-31"),
            ("'25 결산", "'25", "2025-01-01", "2025-12-31"),
            ("’25.5.30. 공문", "’25.5.30.", "2025-05-30", "2025-05-30"),
            ("25.9월 회의", "25.9월", "2025-09-01", "2025-09-30"),
            ("2025.9 보고", "2025.9", "2025-09-01", "2025-09-30"),
            ("2025-09-16 공문", "2025-09-16", "2025-09-16", "2025-09-16"),
            ("9월 회의록", "9월", "2026-09-01", "2026-09-30"),
            ("12월 송년회", "12월", "2025-12-01", "2025-12-31"),
            ("9월 15일 공문", "9월 15일", "2026-09-15", "2026-09-15"),
            ("9월 말 보고", "9월 말", "2026-09-21", "2026-09-30"),
            ("10월초 행사", "10월초", "2026-10-01", "2026-10-10"),
            ("3분기 실적", "3분기", "2026-07-01", "2026-09-30"),
            ("4분기 계획", "4분기", "2026-10-01", "2026-12-31"),
            ("3/4분기 실적", "3/4분기", "2026-07-01", "2026-09-30"),
            ("3사분기 실적", "3사분기", "2026-07-01", "2026-09-30"),
            ("삼사분기 실적", "삼사분기", "2026-07-01", "2026-09-30"),
            ("2Q 실적", "2Q", "2026-04-01", "2026-06-30"),
            ("최근 3개월 민원", "최근 3개월", "2026-07-15", "2026-10-15"),
            ("최근 한 달 공지", "최근 한 달", "2026-09-15", "2026-10-15"),
            ("지난 6개월 실적", "지난 6개월", "2026-04-15", "2026-10-15"),
            ("최근 2주 보도", "최근 2주", "2026-10-01", "2026-10-15"),
            ("2년 전 감사", "2년 전", "2024-01-01", "2024-12-31"),
            ("3개월 전 회의", "3개월 전", "2026-07-01", "2026-07-31"),
            ("한 달 전 공지", "한 달 전", "2026-09-01", "2026-09-30"),
        ] {
            assert_eq!(one(q, t), (expr.to_string(), a.to_string(), b.to_string()), "{q}");
        }
    }

    #[test]
    fn boundaries_new_year_leap_and_winter() {
        assert_eq!(one("지난달 회의", "2026-01-03").1, "2025-12-01");
        assert_eq!(one("작년 실적", "2026-01-01").1, "2025-01-01");
        assert_eq!(one("지난주 공지", "2026-10-12"), ("지난주".into(), "2026-10-05".into(), "2026-10-11".into()));
        assert_eq!(one("이번 주 공지", "2026-10-18"), ("이번 주".into(), "2026-10-12".into(), "2026-10-18".into()));
        assert_eq!(one("어제 공문", "2028-03-01").1, "2028-02-29");
        assert_eq!(one("2월 말 보고", "2028-03-10"), ("2월 말".into(), "2028-02-21".into(), "2028-02-29".into()));
        assert_eq!(one("겨울 폭설", "2026-01-10"), ("겨울".into(), "2025-12-01".into(), "2026-02-28".into()));
        assert_eq!(one("겨울 폭설", "2026-12-05"), ("겨울".into(), "2026-12-01".into(), "2027-02-28".into()));
        assert_eq!(one("연말연시 근무", "2026-10-15"), ("연말연시".into(), "2025-12-01".into(), "2026-01-31".into()));
        assert_eq!(one("하반기 계획", "2026-03-02").1, "2025-07-01", "아직 오지 않은 하반기는 작년 것");
    }

    #[test]
    fn future_words_are_spans_without_range() {
        let t = DateTable::builtin();
        for q in ["내년 사업계획", "다음 달 행사", "명년도 예산", "다음 주 회의"] {
            let v = t.find(q, day("2026-10-15"));
            assert_eq!(v.len(), 1, "{q}: {v:?}");
            assert!(v[0].range.is_none(), "{q}");
        }
    }

    /// Review Focus 1: 일상어와 겹치는 낱말은 날짜가 아니다.
    #[test]
    fn ordinary_words_are_not_dates() {
        for q in [
            "전달 사항 정리", "전주 출장 보고", "당일 출장", "전일 근무", "연말정산 서류", "지난 주민설명회 자료",
            "올림픽 지원", "봄철 산불 대책", "가을운동회", "말 복지 규정", "경주마 초과 출전", "이사회 의결",
            "10억 원 예산", "전화번호 010-1234-5678", "1.5배 증가",
        ] {
            none(q);
        }
    }

    #[test]
    fn particles_are_stripped_with_expression() {
        let t = DateTable::builtin();
        let q = "작년에 만든 출장 계획";
        let v = t.find(q, day("2026-10-15"));
        assert_eq!(&q[v[0].start..v[0].end], "작년");
        assert_eq!(&q[v[0].start..v[0].strip_end], "작년에");
        let q = "2025년도부터 바뀐 규정";
        let v = t.find(q, day("2026-10-15"));
        assert_eq!(&q[v[0].start..v[0].strip_end], "2025년도부터");
    }

    #[test]
    fn precision_and_relativity() {
        let t = DateTable::builtin();
        let s = &t.find("작년 계획", day("2026-10-15"))[0];
        assert_eq!((s.precision, s.relativity), (Precision::Exact, Relativity::Relative));
        let s = &t.find("2025년 계획", day("2026-10-15"))[0];
        assert_eq!(s.relativity, Relativity::Absolute);
        let s = &t.find("요즘 민원", day("2026-10-15"))[0];
        assert_eq!(s.precision, Precision::Vague);
        let s = &t.find("작년 가을 행사", day("2026-10-15"))[0];
        assert_eq!(s.precision, Precision::Vague, "막연한 낱말이 하나라도 섞이면 막연");
    }

    /// 표의 모든 줄이 실제로 해석된다(맥락이 필요한 year_bound·month_part 는 맥락과 함께).
    #[test]
    fn every_table_row_is_recognized() {
        let t = DateTable::builtin();
        for line in BUILTIN.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            let c: Vec<&str> = line.split('\t').collect();
            let q = match c[1] {
                "year_bound" => format!("{} 초 계획", c[0]),
                "month_part" => format!("9월 {} 보고", c[0]),
                _ => format!("{} 계획", c[0]),
            };
            let v = t.find(&q, day("2026-10-15"));
            assert_eq!(v.len(), 1, "{q}: {v:?}");
            assert_eq!(v[0].range.is_none(), c[1] == "future", "{q}");
        }
    }

    #[test]
    fn external_rows_extend_the_table() {
        let mut t = DateTable::builtin();
        assert!(t.find("올 시즌 경주", day("2026-10-15")).is_empty());
        t.extend_tsv("시즌\tyear\t0\tvague\n").unwrap();
        let v = t.find("시즌 경주", day("2026-10-15"));
        assert_eq!(v.len(), 1);
        assert!(t.extend_tsv("잘못된 줄\n").unwrap_err().contains("1번째 줄"));
        assert!(t.extend_tsv("x\tyear\t하나\texact\n").unwrap_err().contains("알 수 없는"));
    }
}
```

`crates/query/tests/eval_dates.rs`(평가 질의의 기간 열과 해석이 같은지 — 질의를 고치면 둘 중 하나가 틀렸다는 뜻):
```rust
//! eval/*/queries.tsv 의 date 질의: 5번째 열(기간)과 해석 결과가 같아야 한다(기준일 2026-10-15).

use query::calendar::{format_ymd, parse_ymd};
use query::DateTable;
use std::path::Path;

#[test]
fn eval_date_queries_parse_to_their_range() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../eval");
    let today = parse_ymd("2026-10-15").unwrap();
    let t = DateTable::builtin();
    let mut n = 0;
    for set in ["public", "kra"] {
        let text = std::fs::read_to_string(root.join(set).join("queries.tsv")).unwrap();
        for line in text.lines().skip(1) {
            let c: Vec<&str> = line.split('\t').collect();
            if c.len() < 5 || c[2] != "date" {
                continue;
            }
            n += 1;
            let spans = t.find(c[1], today);
            if c[4] == "future" {
                assert!(spans.iter().all(|s| s.range.is_none()) && !spans.is_empty(), "{}: {:?}", c[0], spans);
                continue;
            }
            let chips: Vec<_> = spans.iter().filter_map(|s| s.range).collect();
            assert_eq!(chips.len(), 1, "{} '{}': {:?}", c[0], c[1], spans);
            let got = format!("{}..{}", format_ymd(chips[0].start), format_ymd(chips[0].end));
            assert_eq!(got, c[4], "{} '{}'", c[0], c[1]);
        }
    }
    assert_eq!(n, 30, "date 질의 30개(Task 3)");
}
```

- [ ] **Step 8: 시험이 실패하는지 본다**

Run: `cargo test -p query --release`
Expected: FAIL — `cannot find type DateTable` 등(컴파일 오류).

- [ ] **Step 9: 해석기를 구현한다**

`crates/query/src/dates.rs` 시험 위에:
```rust
//! 날짜 표현 → 기간(스펙 §8.1, DEC-053). 낱말은 데이터 표(`dates.tsv`, 바깥 파일 덧붙이기 가능), 숫자 꼴은 코드에서 읽는다.
//! 규칙: 연도 없는 달·분기·반기·계절은 "기준 시점에 이미 시작한 가장 최근 것". 미래 표현은 칩이 아니다(range = None).
//! 경계: 표현 앞은 글자가 아니어야 하고(앞 표현에 바로 붙은 경우는 허용), 뒤는 글자가 아니거나 조사·이어지는 시기여야 한다
//! — "연말정산"·"지난 주민설명회"·"올림픽"을 날짜로 읽지 않으려고.

use crate::calendar::{add_months, days_in_month, from_ymd, to_ymd, weekday, Day, Range};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    Exact,
    Vague,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relativity {
    /// 기준 시점에 따라 바뀌는 표현(작년·지난달·9월)
    Relative,
    /// 숫자로 쓴 해가 든 표현(2025년·'25.5.30.)
    Absolute,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DateSpan {
    /// 원래 검색어 안 표현의 바이트 위치 [start, end).
    pub start: usize,
    pub end: usize,
    /// 검색어에서 뺄 때의 끝 — 바로 붙은 조사("작년에")까지.
    pub strip_end: usize,
    /// None = 미래 표현(내년·다음 달): 칩도 아니고 검색어에서 빼지도 않는다.
    pub range: Option<Range>,
    pub precision: Precision,
    pub relativity: Relativity,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Part {
    Early,
    Mid,
    Late,
    Whole,
    Turn,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Word {
    Year(i32),
    YearBound,
    MonthRel(i32),
    WeekRel(i32),
    DayRel(i32),
    QuarterRel(i32),
    Quarter(u32),
    Half(u32),
    YearPart(Part),
    Season(Season),
    MonthPart(Part),
    ThisMonthPart(Part),
    Window(i32, i32),
    Future,
}

struct Entry {
    /// 공백을 뺀 표현
    key: Vec<char>,
    word: Word,
    precision: Precision,
}

pub struct DateTable {
    /// 긴 표현부터(같은 자리에서 가장 긴 것을 고른다)
    entries: Vec<Entry>,
}

const BUILTIN: &str = include_str!("dates.tsv");

/// 표현 바로 뒤에 붙어도 되는 조사(검색어에서 뺄 때 함께 뺀다). 긴 것부터.
const PARTICLES: &[&str] = &[
    "에는", "에도", "에서", "부터", "까지", "동안", "이후", "이전", "에", "의", "도", "는", "은", "엔", "쯤", "경", "간", "중", "께", "분", "내",
];

fn parse_word(kind: &str, v: &str) -> Option<Word> {
    let int = || v.trim().parse::<i32>().ok();
    let part = || match v {
        "early" => Some(Part::Early),
        "mid" => Some(Part::Mid),
        "late" => Some(Part::Late),
        "whole" => Some(Part::Whole),
        "turn" => Some(Part::Turn),
        _ => None,
    };
    Some(match kind {
        "year" => Word::Year(int()?),
        "year_bound" => Word::YearBound,
        "month_rel" => Word::MonthRel(int()?),
        "week_rel" => Word::WeekRel(int()?),
        "day_rel" => Word::DayRel(int()?),
        "quarter_rel" => Word::QuarterRel(int()?),
        "quarter" => match int()? {
            q @ 1..=4 => Word::Quarter(q as u32),
            _ => return None,
        },
        "half" => match int()? {
            h @ 1..=2 => Word::Half(h as u32),
            _ => return None,
        },
        "year_part" => Word::YearPart(part()?),
        "season" => Word::Season(match v {
            "spring" => Season::Spring,
            "summer" => Season::Summer,
            "autumn" => Season::Autumn,
            "winter" => Season::Winter,
            _ => return None,
        }),
        "month_part" => Word::MonthPart(part()?),
        "this_month_part" => Word::ThisMonthPart(part()?),
        "window" => {
            let (a, b) = v.split_once(',')?;
            Word::Window(a.trim().parse().ok()?, b.trim().parse().ok()?)
        }
        "future" => Word::Future,
        _ => return None,
    })
}

/// 뒤에 붙어 하나의 기간을 이루는 낱말(앞 표현의 경계 검사에서 허용).
fn composable(w: Word) -> bool {
    matches!(w, Word::Half(_) | Word::Quarter(_) | Word::YearPart(_) | Word::Season(_) | Word::MonthPart(_))
}

impl DateTable {
    pub fn builtin() -> DateTable {
        let mut t = DateTable { entries: Vec::new() };
        t.extend_tsv(BUILTIN).expect("내장 날짜 표");
        t
    }

    /// 형식: `표현<TAB>종류<TAB>값<TAB>정밀도`. `#` 로 시작하는 줄과 빈 줄은 건너뛴다. 오류는 줄 번호와 함께.
    pub fn extend_tsv(&mut self, text: &str) -> Result<(), String> {
        let mut add = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let c: Vec<&str> = line.split('\t').collect();
            if c.len() != 4 {
                return Err(format!("{}번째 줄: 칸이 4개가 아님(탭으로 구분)", i + 1));
            }
            let word = parse_word(c[1].trim(), c[2].trim())
                .ok_or_else(|| format!("{}번째 줄: 알 수 없는 종류·값 '{}' '{}'", i + 1, c[1], c[2]))?;
            let precision = match c[3].trim() {
                "exact" => Precision::Exact,
                "vague" => Precision::Vague,
                x => return Err(format!("{}번째 줄: 정밀도는 exact 또는 vague('{x}')", i + 1)),
            };
            let key: Vec<char> = c[0].chars().filter(|ch| !ch.is_whitespace()).collect();
            if key.is_empty() {
                return Err(format!("{}번째 줄: 표현이 비었음", i + 1));
            }
            add.push(Entry { key, word, precision });
        }
        self.entries.extend(add);
        self.entries.sort_by_key(|e| std::cmp::Reverse(e.key.len()));
        Ok(())
    }

    /// 검색어 안의 날짜 표현(왼쪽부터, 겹치지 않음).
    pub fn find(&self, q: &str, today: Day) -> Vec<DateSpan> {
        let cs: Vec<(usize, char)> = q.char_indices().collect();
        let mut hits: Vec<Hit> = Vec::new();
        let mut i = 0;
        while i < cs.len() {
            let glued = hits.last().is_some_and(|h| h.j == i);
            let boundary = i == 0 || glued || !cs[i - 1].1.is_alphanumeric();
            if boundary && !cs[i].1.is_whitespace() {
                let prev = hits.last().filter(|h| only_space(&cs, h.j, i)).map(|h| h.atom);
                if let Some(h) = numeric(&cs, i, today).or_else(|| self.word(&cs, i, today, prev)) {
                    if self.after_ok(&cs, h.j) {
                        i = h.j;
                        hits.push(h);
                        continue;
                    }
                }
            }
            i += 1;
        }
        group(q, &cs, &hits, today)
    }

    fn word(&self, cs: &[(usize, char)], i: usize, today: Day, prev: Option<Atom>) -> Option<Hit> {
        for e in &self.entries {
            let Some(j) = match_key(cs, i, &e.key) else { continue };
            let atom = match e.word {
                // "초·말·중순"은 달이나 해 바로 뒤에서만 날짜다("말 복지", "초과" 방지).
                Word::MonthPart(p) => match prev {
                    Some(Atom::Month { .. } | Atom::Year { .. } | Atom::YearBound) => Atom::MonthPart(p),
                    _ => continue,
                },
                w => word_atom(w, today),
            };
            return Some(Hit { i, j, atom, precision: e.precision });
        }
        None
    }

    /// 표현 뒤 경계: 끝·글자 아님·숫자(“2025년3월”)·조사·이어지는 시기 낱말.
    fn after_ok(&self, cs: &[(usize, char)], j: usize) -> bool {
        let Some(&(_, c)) = cs.get(j) else { return true };
        if !c.is_alphanumeric() || c.is_ascii_digit() {
            return true;
        }
        PARTICLES.iter().any(|p| particle_at(cs, j, p))
            || self.entries.iter().any(|e| composable(e.word) && match_key(cs, j, &e.key).is_some())
    }
}

/// 조사 p 가 j 에서 시작하고, 그 뒤가 끝이거나 글자가 아니다.
fn particle_at(cs: &[(usize, char)], j: usize, p: &str) -> bool {
    let n = p.chars().count();
    j + n <= cs.len()
        && cs[j..j + n].iter().map(|x| x.1).eq(p.chars())
        && cs.get(j + n).is_none_or(|x| !x.1.is_alphanumeric())
}

/// 공백을 건너뛰며 key 를 맞춘다(첫 글자는 공백이 아니어야). 다음 위치를 돌려준다.
fn match_key(cs: &[(usize, char)], i: usize, key: &[char]) -> Option<usize> {
    let (mut j, mut k) = (i, 0);
    while k < key.len() {
        let c = cs.get(j)?.1;
        if c.is_whitespace() {
            if k == 0 {
                return None;
            }
            j += 1;
            continue;
        }
        if c != key[k] {
            return None;
        }
        j += 1;
        k += 1;
    }
    Some(j)
}

fn only_space(cs: &[(usize, char)], from: usize, to: usize) -> bool {
    from <= to && cs[from..to].iter().all(|c| c.1.is_whitespace())
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Atom {
    Year { y: i32, abs: bool },
    YearBound,
    Month { y: Option<i32>, m: u32, abs: bool },
    MonthDay { y: Option<i32>, m: u32, d: u32, abs: bool },
    Quarter(u32),
    Half(u32),
    YearPart(Part),
    Season(Season),
    MonthPart(Part),
    Range(Range, bool),
    Future,
}

#[derive(Debug, Clone, Copy)]
struct Hit {
    /// 글자 위치 [i, j)
    i: usize,
    j: usize,
    atom: Atom,
    precision: Precision,
}

fn word_atom(w: Word, today: Day) -> Atom {
    let (ty, tm, _) = to_ymd(today);
    match w {
        Word::Year(k) => Atom::Year { y: ty + k, abs: false },
        Word::YearBound => Atom::YearBound,
        Word::MonthRel(k) => {
            let (y, m) = add_months(ty, tm, k);
            Atom::Month { y: Some(y), m, abs: false }
        }
        Word::WeekRel(k) => {
            let monday = today - weekday(today) as i32 + 7 * k;
            Atom::Range(Range { start: monday, end: monday + 6 }, false)
        }
        Word::DayRel(k) => Atom::Range(Range::day(today + k), false),
        Word::QuarterRel(k) => {
            let t = ty * 4 + (tm as i32 - 1) / 3 + k;
            let (y, q) = (t.div_euclid(4), t.rem_euclid(4) as u32 + 1);
            Atom::Range(Range::months(y, 3 * q - 2, 3), false)
        }
        Word::Quarter(q) => Atom::Quarter(q),
        Word::Half(h) => Atom::Half(h),
        Word::YearPart(p) => Atom::YearPart(p),
        Word::Season(s) => Atom::Season(s),
        Word::MonthPart(p) => Atom::MonthPart(p),
        Word::ThisMonthPart(p) => Atom::Range(month_part(ty, tm, p), false),
        Word::Window(from, to) => Atom::Range(Range { start: today - from, end: today - to }, false),
        Word::Future => Atom::Future,
    }
}

// ---- 숫자 꼴 ----

/// i 에서 시작하는 숫자 묶음(1~4자리). 5자리 이상이면 날짜가 아니다(전화번호 등).
fn num(cs: &[(usize, char)], i: usize) -> Option<(u32, usize)> {
    let n = cs[i.min(cs.len())..].iter().take_while(|c| c.1.is_ascii_digit()).count();
    if n == 0 || n > 4 {
        return None;
    }
    Some((cs[i..i + n].iter().fold(0u32, |v, c| v * 10 + c.1.to_digit(10).unwrap()), n))
}

fn skip_sp(cs: &[(usize, char)], mut j: usize) -> usize {
    while cs.get(j).is_some_and(|c| c.1.is_whitespace()) {
        j += 1;
    }
    j
}

fn lit(cs: &[(usize, char)], j: usize, s: &str) -> Option<usize> {
    let n = s.chars().count();
    (j + n <= cs.len() && cs[j..j + n].iter().map(|c| c.1).eq(s.chars())).then_some(j + n)
}

fn lit_sp(cs: &[(usize, char)], j: usize, s: &str) -> Option<usize> {
    lit(cs, skip_sp(cs, j), s)
}

#[derive(Clone, Copy)]
enum Unit {
    Day,
    Week,
    Month,
    Year,
}

/// 개수: 숫자(1~3자리) 또는 한·두·세·네.
fn count_word(cs: &[(usize, char)], i: usize) -> Option<(i32, usize)> {
    if cs.get(i)?.1.is_ascii_digit() {
        let (v, n) = num(cs, i)?;
        return (n <= 3).then_some((v as i32, i + n));
    }
    for (w, v) in [("한", 1), ("두", 2), ("세", 3), ("네", 4)] {
        if let Some(e) = lit(cs, i, w) {
            return Some((v, e));
        }
    }
    None
}

fn unit_at(cs: &[(usize, char)], j: usize) -> Option<(Unit, usize)> {
    for (w, u) in [("주일", Unit::Week), ("개월", Unit::Month), ("일", Unit::Day), ("주", Unit::Week), ("달", Unit::Month), ("년", Unit::Year)] {
        if let Some(e) = lit(cs, j, w) {
            return Some((u, e));
        }
    }
    None
}

fn shift_back(today: Day, n: i32, u: Unit) -> Day {
    let (y, m, d) = to_ymd(today);
    let months = |k: i32| {
        let (yy, mm) = add_months(y, m, -k);
        from_ymd(yy, mm, d.min(days_in_month(yy, mm)))
    };
    match u {
        Unit::Day => today - n,
        Unit::Week => today - 7 * n,
        Unit::Month => months(n),
        Unit::Year => months(12 * n),
    }
}

fn ago(today: Day, n: i32, u: Unit) -> Range {
    let (y, m, _) = to_ymd(today);
    match u {
        Unit::Day => Range::day(today - n),
        Unit::Week => {
            let d = today - 7 * n;
            let monday = d - weekday(d) as i32;
            Range { start: monday, end: monday + 6 }
        }
        Unit::Month => {
            let (yy, mm) = add_months(y, m, -n);
            Range::month(yy, mm)
        }
        Unit::Year => Range::year(y - n),
    }
}

/// "." 또는 "-" 로 이은 달(.일). 반환: (달, 일?, 끝 위치).
fn dotted_md(cs: &[(usize, char)], j: usize) -> Option<(u32, Option<u32>, usize)> {
    let sep = cs.get(j)?.1;
    if sep != '.' && sep != '-' {
        return None;
    }
    let (m, mn) = num(cs, j + 1)?;
    if mn > 2 || !(1..=12).contains(&m) {
        return None;
    }
    let e = j + 1 + mn;
    if let Some(x) = lit(cs, e, &sep.to_string()) {
        if let Some((d, dn)) = num(cs, x) {
            if dn <= 2 {
                let end = x + dn;
                let end = lit(cs, end, ".").unwrap_or(end);
                return Some((m, Some(d), end));
            }
        }
    }
    Some((m, None, e))
}

fn md_hit(i: usize, j: usize, y: i32, m: u32, d: Option<u32>) -> Hit {
    let atom = match d {
        Some(d) => Atom::MonthDay { y: Some(y), m, d, abs: true },
        None => Atom::Month { y: Some(y), m, abs: true },
    };
    Hit { i, j, atom, precision: Precision::Exact }
}

fn exact(i: usize, j: usize, atom: Atom) -> Hit {
    Hit { i, j, atom, precision: Precision::Exact }
}

fn numeric(cs: &[(usize, char)], i: usize, today: Day) -> Option<Hit> {
    let c = cs[i].1;
    // '25 · '25.5 · '25.5.30.
    if matches!(c, '\'' | '’' | '‘') {
        let (yy, n) = num(cs, i + 1)?;
        if n != 2 {
            return None;
        }
        let y = 2000 + yy as i32;
        let j = i + 1 + n;
        if let Some((m, d, e)) = dotted_md(cs, j) {
            return Some(md_hit(i, e, y, m, d));
        }
        let e = lit_sp(cs, j, "년").map(|x| lit(cs, x, "도").unwrap_or(x)).unwrap_or(j);
        return Some(exact(i, e, Atom::Year { y, abs: true }));
    }
    // 최근/지난 N 단위 → [오늘 - N 단위, 오늘]
    for head in ["최근", "지난"] {
        if let Some(e) = lit(cs, i, head) {
            if let Some((n, e2)) = count_word(cs, skip_sp(cs, e)) {
                if let Some((u, e3)) = unit_at(cs, skip_sp(cs, e2)) {
                    return Some(exact(i, e3, Atom::Range(Range { start: shift_back(today, n, u), end: today }, false)));
                }
            }
        }
    }
    // N 단위 전 → 그 시점이 든 날·주·달·해(막연)
    if let Some((n, e)) = count_word(cs, i) {
        if let Some((u, e2)) = unit_at(cs, skip_sp(cs, e)) {
            if let Some(e3) = lit_sp(cs, e2, "전") {
                return Some(Hit { i, j: e3, atom: Atom::Range(ago(today, n, u), false), precision: Precision::Vague });
            }
        }
    }
    if !c.is_ascii_digit() {
        return None;
    }
    let (v, n) = num(cs, i)?;
    let j = i + n;
    if n == 4 {
        if !(1900..=2099).contains(&v) {
            return None;
        }
        let y = v as i32;
        if let Some((m, d, e)) = dotted_md(cs, j) {
            return Some(md_hit(i, e, y, m, d));
        }
        let e = lit_sp(cs, j, "년").map(|x| lit(cs, x, "도").unwrap_or(x)).unwrap_or(j);
        return Some(exact(i, e, Atom::Year { y, abs: true }));
    }
    if n > 2 {
        return None;
    }
    if n == 2 {
        // 25.9월
        if let Some(x) = lit(cs, j, ".") {
            if let Some((m, mn)) = num(cs, x) {
                if mn <= 2 && (1..=12).contains(&m) {
                    if let Some(e) = lit_sp(cs, x + mn, "월") {
                        return Some(exact(i, e, Atom::Month { y: Some(2000 + v as i32), m, abs: true }));
                    }
                }
            }
        }
        // 25년(도)
        if let Some(x) = lit_sp(cs, j, "년") {
            let e = lit(cs, x, "도").unwrap_or(x);
            return Some(exact(i, e, Atom::Year { y: 2000 + v as i32, abs: true }));
        }
    }
    // 9월 (15일)
    if (1..=12).contains(&v) {
        if let Some(e) = lit_sp(cs, j, "월") {
            let x = skip_sp(cs, e);
            if let Some((d, dn)) = num(cs, x) {
                if dn <= 2 {
                    if let Some(e2) = lit_sp(cs, x + dn, "일") {
                        return Some(exact(i, e2, Atom::MonthDay { y: None, m: v, d, abs: false }));
                    }
                }
            }
            return Some(exact(i, e, Atom::Month { y: None, m: v, abs: false }));
        }
    }
    // 3분기 · 3/4분기 · 3사분기 · 3Q
    if (1..=4).contains(&v) {
        for tail in ["/4분기", "사분기", "분기"] {
            if let Some(e) = lit_sp(cs, j, tail) {
                return Some(exact(i, e, Atom::Quarter(v)));
            }
        }
        if let Some(e) = lit(cs, j, "Q").or_else(|| lit(cs, j, "q")) {
            if cs.get(e).is_none_or(|c| !c.1.is_ascii_alphabetic()) {
                return Some(exact(i, e, Atom::Quarter(v)));
            }
        }
    }
    None
}

// ---- 묶기·해석 ----

/// 앞 표현에 이어 붙을 수 있는가(해 + 시기, 달 + 초·말).
fn can_follow(last: Atom, next: Atom) -> bool {
    matches!(
        (last, next),
        (
            Atom::Year { .. } | Atom::YearBound,
            Atom::Half(_) | Atom::Quarter(_) | Atom::Season(_) | Atom::YearPart(_) | Atom::Month { y: None, .. } | Atom::MonthDay { y: None, .. } | Atom::MonthPart(_),
        ) | (Atom::Month { .. }, Atom::MonthPart(_))
    )
}

fn group(q: &str, cs: &[(usize, char)], hits: &[Hit], today: Day) -> Vec<DateSpan> {
    let byte = |i: usize| cs.get(i).map(|c| c.0).unwrap_or(q.len());
    let mut out = Vec::new();
    let mut k = 0;
    while k < hits.len() {
        let mut n = 1;
        while k + n < hits.len() && only_space(cs, hits[k + n - 1].j, hits[k + n].i) && can_follow(hits[k + n - 1].atom, hits[k + n].atom) {
            n += 1;
        }
        let g = &hits[k..k + n];
        k += n;
        let Some((range, relativity, precision)) = resolve(g, today) else { continue };
        let j = g[n - 1].j;
        let strip = PARTICLES.iter().find(|p| particle_at(cs, j, p)).map(|p| j + p.chars().count()).unwrap_or(j);
        out.push(DateSpan { start: byte(g[0].i), end: byte(j), strip_end: byte(strip), range, precision, relativity });
    }
    out
}

/// 연도가 없으면 기준 시점에 이미 시작한 가장 최근 것.
fn anchored(anchor: Option<i32>, today: Day, f: impl Fn(i32) -> Range) -> Range {
    match anchor {
        Some(y) => f(y),
        None => {
            let ty = to_ymd(today).0;
            let r = f(ty);
            if r.start <= today {
                r
            } else {
                f(ty - 1)
            }
        }
    }
}

fn year_part(y: i32, p: Part) -> Range {
    match p {
        Part::Early => Range::months(y, 1, 2),
        Part::Mid => Range::months(y, 5, 4),
        Part::Late => Range::months(y, 11, 2),
        Part::Whole => Range::year(y),
        Part::Turn => Range::months(y, 12, 2),
    }
}

fn month_part(y: i32, m: u32, p: Part) -> Range {
    let last = days_in_month(y, m);
    let (a, b) = match p {
        Part::Early => (1, 10),
        Part::Mid => (11, 20),
        Part::Late => (21, last),
        Part::Whole | Part::Turn => (1, last),
    };
    Range { start: from_ymd(y, m, a), end: from_ymd(y, m, b) }
}

type Resolved = (Option<Range>, Relativity, Precision);

fn resolve(g: &[Hit], today: Day) -> Option<Resolved> {
    let vague = g.iter().any(|h| h.precision == Precision::Vague);
    let (mut abs, mut anchor, mut range, mut month) = (false, None::<i32>, None::<Range>, None::<(i32, u32)>);
    let latest_month = |m: u32| -> (i32, u32) {
        let (ty, tm, _) = to_ymd(today);
        if m <= tm {
            (ty, m)
        } else {
            (ty - 1, m)
        }
    };
    for h in g {
        match h.atom {
            Atom::Future => return Some((None, Relativity::Relative, Precision::Exact)),
            Atom::Year { y, abs: a } => {
                abs |= a;
                anchor = Some(y);
                range = Some(Range::year(y));
            }
            Atom::YearBound => anchor = Some(to_ymd(today).0),
            Atom::Month { y, m, abs: a } => {
                abs |= a;
                let (yy, mm) = match y.or(anchor) {
                    Some(y) => (y, m),
                    None => latest_month(m),
                };
                month = Some((yy, mm));
                range = Some(Range::month(yy, mm));
            }
            Atom::MonthDay { y, m, d, abs: a } => {
                abs |= a;
                let yy = y.or(anchor).unwrap_or_else(|| latest_month(m).0);
                if d == 0 || d > days_in_month(yy, m) {
                    return None;
                }
                range = Some(Range::day(from_ymd(yy, m, d)));
            }
            Atom::Quarter(q) => range = Some(anchored(anchor, today, |y| Range::months(y, 3 * q - 2, 3))),
            Atom::Half(hh) => range = Some(anchored(anchor, today, |y| Range::months(y, 6 * hh - 5, 6))),
            Atom::Season(s) => {
                let m0 = match s {
                    Season::Spring => 3,
                    Season::Summer => 6,
                    Season::Autumn => 9,
                    Season::Winter => 12,
                };
                range = Some(anchored(anchor, today, |y| Range::months(y, m0, 3)));
            }
            Atom::YearPart(p) => range = Some(anchored(anchor, today, |y| year_part(y, p))),
            Atom::MonthPart(p) => match (month, anchor) {
                (Some((y, m)), _) => range = Some(month_part(y, m, p)),
                (None, Some(y)) => range = Some(year_part(y, p)),
                (None, None) => return None,
            },
            Atom::Range(r, a) => {
                abs |= a;
                range = Some(r);
            }
        }
    }
    let range = range?;
    Some((Some(range), if abs { Relativity::Absolute } else { Relativity::Relative }, if vague { Precision::Vague } else { Precision::Exact }))
}
```

참고 — 이 구현에서 일부러 정한 것(시험이 지킨다):
- "올"(year_bound)만 있고 시기가 붙지 않으면 버린다("올림픽"은 뒤 경계에서도 걸러진다).
- 한 묶음에 해 + 달 + 초·말까지 붙는다("작년 9월 말"). "지난달 말"은 `month_rel` 이 연도 있는 달을 만들므로 `can_follow(Month, MonthPart)` 로 붙는다.
- `MonthPart` 를 해 뒤에 쓰면 해의 시기로 읽는다("작년 말" = 11~12월, "올 초" = 1~2월).

- [ ] **Step 10: 시험이 통과하는지 본다**

Run: `cargo test -p query --release`
Expected: PASS. `eval_date_queries_parse_to_their_range` 가 실패하면 Task 3 의 질의 기간 열이나 표현을 고친다 — 해석기를 질의에 맞춰 고치지 말고, 사람이 읽는 뜻과 다른 쪽을 고친다(어느 쪽인지 커밋 메시지에 적는다).

- [ ] **Step 11: 커밋**

```bash
git add crates/query Cargo.lock
git commit -m "query: calendar and data-table date expression parser (hanja, colloquial, abbreviated, future)"
```

---

### Task 5: `query` — 검색어 유형 분류와 `parse`

**Files:**
- Create: `crates/query/src/kind.rs`
- Modify: `crates/query/src/lib.rs`

**Interfaces:**
- Consumes: Task 4 `DateTable::find`, `DateSpan`.
- Produces: `query::QueryKind { Keyword, Semantic }`, `query::kind::classify(&str) -> QueryKind`, `query::ParsedQuery { original: String, dates: Vec<DateSpan>, kind: QueryKind }`, `ParsedQuery::text_without(&self, drop: impl Fn(&DateSpan) -> bool) -> String`, `ParsedQuery::chips(&self) -> impl Iterator<Item = &DateSpan>`, `query::parse(q: &str, today: Day, table: &DateTable) -> ParsedQuery`.

- [ ] **Step 1: 시험을 쓴다**

`crates/query/src/kind.rs`(시험만 먼저):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_nouns_numbers_and_file_names_are_keyword() {
        for q in ["더비온", "과기정통부 예산", "경마기획부-1736", "내부통제규정_개정전문.hwp", "보고서.pdf", "K-water"] {
            assert_eq!(classify(q), QueryKind::Keyword, "{q}");
        }
    }

    #[test]
    fn sentences_are_semantic() {
        for q in ["일 열심히 하다 실수했을 때 징계 안 받는 경우", "새 사업 시작하기 전에 미리 확인해야 하는 것", "부경 주차장 밤에 불 꺼진 민원"] {
            assert_eq!(classify(q), QueryKind::Semantic, "{q}");
        }
    }
}
```
`crates/query/src/lib.rs` 끝에 붙일 시험:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::parse_ymd;

    fn p(q: &str) -> ParsedQuery {
        parse(q, parse_ymd("2026-10-15").unwrap(), &DateTable::builtin())
    }

    #[test]
    fn kind_is_decided_without_date_words() {
        let q = p("작년 더비온");
        assert_eq!(q.kind, QueryKind::Keyword, "날짜를 빼면 '더비온' 한 낱말");
        assert_eq!(q.text_without(|_| true), "더비온");
        assert_eq!(q.text_without(|_| false), "작년 더비온");
    }

    #[test]
    fn particles_go_with_the_date_and_spaces_collapse() {
        assert_eq!(p("작년에 만든  출장 계획").text_without(|_| true), "만든 출장 계획");
    }

    /// Review Focus 2: 날짜만 있는 검색어는 빼고 나면 비므로 원래 검색어를 쓴다.
    #[test]
    fn date_only_query_keeps_text() {
        let q = p("작년");
        assert_eq!(q.text_without(|_| true), "작년");
        assert_eq!(q.chips().count(), 1);
    }

    #[test]
    fn future_words_are_not_chips_and_stay_in_text() {
        let q = p("내년 사업계획");
        assert_eq!(q.chips().count(), 0);
        assert_eq!(q.text_without(|s| s.range.is_some()), "내년 사업계획");
    }
}
```

- [ ] **Step 2: 시험이 실패하는지 본다**

Run: `cargo test -p query --release`
Expected: FAIL — `cannot find function classify`, `parse`.

- [ ] **Step 3: 구현한다**

`crates/query/src/kind.rs` 시험 위에:
```rust
//! 검색어 유형(스펙 §8.1): 짧은 명사·문서번호·파일명 꼴 → 키워드형, 문장 → 의미형. 결합 가중치에 쓴다(§8.3).
//! 사람 이름 인식은 없다(KI-044) — 이름 두세 글자는 "짧은 낱말"로 키워드형이 된다.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    Keyword,
    Semantic,
}

const EXTS: &[&str] = &[".hwp", ".hwpx", ".pdf", ".docx", ".pptx", ".xlsx", ".odt", ".odp", ".ods", ".txt", ".csv"];

/// 문서번호 꼴: 글자들-숫자(예: 경마기획부-1736).
fn doc_number(w: &str) -> bool {
    let Some((a, b)) = w.split_once('-') else { return false };
    !a.is_empty() && a.chars().all(char::is_alphabetic) && b.chars().next().is_some_and(|c| c.is_ascii_digit())
}

fn file_like(w: &str) -> bool {
    let l = w.to_lowercase();
    EXTS.iter().any(|e| l.ends_with(e)) || w.contains('_')
}

pub fn classify(text: &str) -> QueryKind {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() || words.iter().any(|w| doc_number(w) || file_like(w)) {
        return QueryKind::Keyword;
    }
    // 2026-10-15 평가 질의로 정함: 낱말 2개 이하·8글자 이하면 keyword 유형 30/30, kra 유형 19/20 을 맞힌다(6글자면 keyword 28/30).
    let chars: usize = words.iter().map(|w| w.chars().count()).sum();
    if words.len() <= 2 && chars <= 8 {
        QueryKind::Keyword
    } else {
        QueryKind::Semantic
    }
}
```
(`K-water` 는 `doc_number` 가 아니지만 한 낱말 7자라 짧은 낱말로 키워드형. 정확도는 Task 15 보고서가 잰다 — `abbrev`(약칭 + 주제, 예 "행안부 저출산 대응 공모사업")와 `kra` 는 성격이 섞여 정확도 계산에서 뺀다.)

`crates/query/src/lib.rs` 를 다음으로 바꾼다(시험 블록은 그대로 끝에):
```rust
//! 검색어 해석(스펙 §8.1): 날짜 표현 → 기간(칩), 검색어 유형. 기준 시점(오늘)은 바깥에서 받는다 — 평가·시험이 해가 바뀌어도 같게.
pub mod calendar;
pub mod dates;
pub mod kind;

pub use calendar::{Day, Range};
pub use dates::{DateSpan, DateTable, Precision, Relativity};
pub use kind::QueryKind;

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedQuery {
    pub original: String,
    pub dates: Vec<DateSpan>,
    /// 날짜 표현을 모두 뺀 글로 정한 유형
    pub kind: QueryKind,
}

impl ParsedQuery {
    /// `drop` 이 참인 날짜 표현(바로 붙은 조사 포함)을 뺀 검색어. 빼고 나면 비면 원래 검색어(빈 검색 방지).
    pub fn text_without(&self, drop: impl Fn(&DateSpan) -> bool) -> String {
        let q = &self.original;
        let mut out = String::new();
        let mut at = 0;
        for s in &self.dates {
            if drop(s) {
                out.push_str(&q[at..s.start]);
                out.push(' ');
                at = s.strip_end;
            }
        }
        out.push_str(&q[at..]);
        let t = out.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.is_empty() {
            q.split_whitespace().collect::<Vec<_>>().join(" ")
        } else {
            t
        }
    }

    /// 날짜 칩(가산할 기간). 미래 표현은 칩이 아니다.
    pub fn chips(&self) -> impl Iterator<Item = &DateSpan> {
        self.dates.iter().filter(|d| d.range.is_some())
    }
}

pub fn parse(q: &str, today: Day, table: &DateTable) -> ParsedQuery {
    let dates = table.find(q, today);
    let mut pq = ParsedQuery { original: q.to_string(), dates, kind: QueryKind::Keyword };
    pq.kind = kind::classify(&pq.text_without(|_| true));
    pq
}
```

- [ ] **Step 4: 시험이 통과하는지 본다**

Run: `cargo test -p query --release`
Expected: PASS.

- [ ] **Step 5: 커밋**

```bash
git add crates/query
git commit -m "query: query kind classification and parse() with date stripping"
```

---
### Task 6: `vector` — 레코드와 세대 파일 저장소

**Files:**
- Create: `crates/vector/src/record.rs`, `crates/vector/src/store.rs`
- Modify: `crates/vector/src/lib.rs`(모듈 2개 추가), `crates/vector/src/flat.rs`(`push_record`, `push` 가 레코드를 거치게)
- Test: 각 파일 안 `#[cfg(test)]`. `crates/vector/Cargo.toml` 에 `[dev-dependencies] tempfile = "3"`.

**Interfaces:**
- Produces:
  - `vector::record::Layout { dim: usize, bit_dim: usize }`, `Layout::M0`(512/768), `Layout::words()`, `Layout::record_bytes()`.
  - `vector::record::Record { codes: Vec<i8>, inv_scale: f32, bits: Vec<u64> }`, `Record::from_full(&[f32], Layout) -> Option<Record>`, `Record::write_to(&self, &mut Vec<u8>)`, `Record::read_from(&[u8], Layout) -> Record`.
  - `vector::flat::FlatIndex::push_record(&mut self, &Record)`.
  - `vector::store::{VecFile, StoreError, read_each, HEADER}` — `VecFile::create(&Path, Layout) -> io::Result<VecFile>`, `VecFile::open(&Path, Layout, expect_len: u64) -> Result<VecFile, StoreError>`(meta 보다 긴 꼬리는 잘라냄), `VecFile::append(&mut self, &[Record]) -> io::Result<u64>`(첫 슬롯), `VecFile::len()`, `read_each(&Path, Layout, len, FnMut(u64, Record) -> io::Result<()>) -> Result<(), StoreError>`. `StoreError { Io(io::Error), BadHeader, Short { have, want } }`.

- [ ] **Step 1: 시험을 쓴다**

`crates/vector/src/record.rs`(시험만 먼저 — 아래 Step 3 의 구현을 그 위에 둔다):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::flat::FlatIndex;

    fn v(seed: u64, n: usize) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((s >> 33) as f32 / (1u64 << 31) as f32) - 0.5
            })
            .collect()
    }

    #[test]
    fn bytes_roundtrip() {
        let l = Layout { dim: 32, bit_dim: 64 };
        let r = Record::from_full(&v(1, 64), l).unwrap();
        let mut b = Vec::new();
        r.write_to(&mut b);
        assert_eq!(b.len(), l.record_bytes());
        assert_eq!(Record::read_from(&b, l), r);
    }

    #[test]
    fn m0_layout_size() {
        assert_eq!(Layout::M0.record_bytes(), 512 + 4 + 96);
    }

    /// 평가(f32 캐시 → 레코드)와 제품(디스크에서 읽은 레코드)이 같은 점수를 내야 한다 — 일치 검사의 바탕.
    #[test]
    fn push_and_push_record_search_identically() {
        let l = Layout { dim: 32, bit_dim: 64 };
        let vs: Vec<Vec<f32>> = (0..50).map(|i| v(i, 64)).collect();
        let (mut a, mut b) = (FlatIndex::new(l.dim, l.bit_dim), FlatIndex::new(l.dim, l.bit_dim));
        for x in &vs {
            a.push(x);
            let mut bytes = Vec::new();
            Record::from_full(x, l).unwrap().write_to(&mut bytes);
            b.push_record(&Record::read_from(&bytes, l));
        }
        assert_eq!(a.search_hamming(&vs[3], 20, 10), b.search_hamming(&vs[3], 20, 10));
        assert_eq!(a.search_exact(&vs[7], 10), b.search_exact(&vs[7], 10));
    }

    #[test]
    fn short_or_zero_vectors_have_no_record() {
        let l = Layout { dim: 4, bit_dim: 8 };
        assert!(Record::from_full(&[1.0; 4], l).is_none());
        assert!(Record::from_full(&[0.0; 8], l).is_none());
    }
}
```

`crates/vector/src/store.rs`(시험만 먼저):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{Layout, Record};

    const L: Layout = Layout { dim: 4, bit_dim: 8 };

    fn rec(x: f32) -> Record {
        Record::from_full(&[x, 1.0, -0.5, 0.25, 0.1, -0.2, 0.3, -0.4], L).unwrap()
    }

    fn all(p: &Path, len: u64) -> Result<Vec<(u64, Record)>, StoreError> {
        let mut out = Vec::new();
        read_each(p, L, len, |s, r| {
            out.push((s, r));
            Ok(())
        })?;
        Ok(out)
    }

    #[test]
    fn append_returns_slots_and_reads_back() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("cards.0.vec");
        let mut f = VecFile::create(&p, L).unwrap();
        assert_eq!(f.append(&[rec(1.0), rec(2.0)]).unwrap(), 0);
        assert_eq!(f.append(&[rec(3.0)]).unwrap(), 2);
        assert_eq!(f.len(), 3);
        drop(f);
        let got = all(&p, 3).unwrap();
        assert_eq!(got.iter().map(|x| x.0).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(got[2].1, rec(3.0));
    }

    /// meta 에 기록되기 전에 꺼져 파일에만 남은 꼬리는 열 때 잘라낸다(DEC-048).
    #[test]
    fn open_truncates_unrecorded_tail() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.vec");
        let mut f = VecFile::create(&p, L).unwrap();
        f.append(&[rec(1.0), rec(2.0), rec(3.0)]).unwrap();
        drop(f);
        let mut f = VecFile::open(&p, L, 2).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(std::fs::metadata(&p).unwrap().len(), HEADER + 2 * L.record_bytes() as u64);
        assert_eq!(f.append(&[rec(9.0)]).unwrap(), 2, "잘라낸 자리부터 다시 쓴다");
    }

    #[test]
    fn short_file_and_bad_header_are_errors() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.vec");
        let mut f = VecFile::create(&p, L).unwrap();
        f.append(&[rec(1.0)]).unwrap();
        drop(f);
        assert!(matches!(VecFile::open(&p, L, 2), Err(StoreError::Short { have: 1, want: 2 })));
        assert!(matches!(all(&p, 2), Err(StoreError::Short { have: 1, want: 2 })));
        assert!(matches!(VecFile::open(&p, Layout { dim: 8, bit_dim: 8 }, 0), Err(StoreError::BadHeader)), "다른 차원의 파일");
        std::fs::write(&p, b"garbage").unwrap();
        assert!(matches!(VecFile::open(&p, L, 0), Err(StoreError::BadHeader)));
    }

    /// Review Focus 5 바탕: 쓰는 쪽이 더 덧붙여도 읽는 쪽은 주어진 길이만 읽는다.
    #[test]
    fn reader_sees_only_given_len() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.vec");
        let mut f = VecFile::create(&p, L).unwrap();
        f.append(&[rec(1.0), rec(2.0), rec(3.0)]).unwrap();
        assert_eq!(all(&p, 2).unwrap().len(), 2);
    }
}
```

- [ ] **Step 2: 시험이 실패하는지 본다**

`crates/vector/src/lib.rs` 의 `pub mod flat;` 아래에 `pub mod record;`, `pub mod store;` 를 더하고 `crates/vector/Cargo.toml` 에 dev-dependency 를 더한 뒤:

Run: `cargo test -p vector --release`
Expected: FAIL — `cannot find type Layout`, `VecFile` 등.

- [ ] **Step 3: 레코드를 구현한다**

`crates/vector/src/record.rs` 시험 위에:
```rust
//! 저장 단위 레코드: 앞 `dim` 차원 int8 코드 + 역스케일 + 앞 `bit_dim` 차원 부호 비트(M0 결정 512/768).
//! 평가(m0eval, f32 캐시)와 제품(디스크)이 같은 레코드에서 점수를 내도록 계산은 여기 한 곳에서만 한다.

use crate::{quantize_i8, sign_bits, truncate_normalize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub dim: usize,
    pub bit_dim: usize,
}

impl Layout {
    /// M0 결정: int8 512차원 + 이진 768비트(스펙 §6.7·§8.2).
    pub const M0: Layout = Layout { dim: 512, bit_dim: 768 };

    pub fn words(&self) -> usize {
        self.bit_dim.div_ceil(64)
    }

    pub fn record_bytes(&self) -> usize {
        self.dim + 4 + self.words() * 8
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub codes: Vec<i8>,
    pub inv_scale: f32,
    pub bits: Vec<u64>,
}

impl Record {
    /// 원본 차원 벡터 → 레코드. 차원이 모자라거나 0벡터·비유한값이면 None.
    pub fn from_full(full: &[f32], l: Layout) -> Option<Record> {
        let v = truncate_normalize(full, l.dim)?;
        let b = truncate_normalize(full, l.bit_dim)?;
        let (codes, inv_scale) = quantize_i8(&v);
        Some(Record { codes, inv_scale, bits: sign_bits(&b) })
    }

    /// 리틀 엔디언: 코드(dim 바이트) · 역스케일(f32) · 비트(u64 × words).
    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.extend(self.codes.iter().map(|c| *c as u8));
        out.extend_from_slice(&self.inv_scale.to_le_bytes());
        for w in &self.bits {
            out.extend_from_slice(&w.to_le_bytes());
        }
    }

    pub fn read_from(b: &[u8], l: Layout) -> Record {
        let codes = b[..l.dim].iter().map(|x| *x as i8).collect();
        let inv_scale = f32::from_le_bytes(b[l.dim..l.dim + 4].try_into().expect("4바이트"));
        let bits = b[l.dim + 4..l.record_bytes()].chunks_exact(8).map(|w| u64::from_le_bytes(w.try_into().expect("8바이트"))).collect();
        Record { codes, inv_scale, bits }
    }
}
```

`crates/vector/src/flat.rs` 의 `push` 를 바꾸고 `push_record` 를 더한다:
```rust
    /// 원본 차원 벡터를 추가한다. 차원 부족·0벡터면 추가하지 않고 false.
    pub fn push(&mut self, full: &[f32]) -> bool {
        match Record::from_full(full, Layout { dim: self.dim, bit_dim: self.bit_dim }) {
            Some(r) => {
                self.push_record(&r);
                true
            }
            None => false,
        }
    }

    /// 이미 만든 레코드(디스크에서 읽은 것)를 추가한다. 차원이 다르면 패닉 — 부르는 쪽 Layout 오류.
    pub fn push_record(&mut self, r: &Record) {
        assert_eq!((r.codes.len(), r.bits.len()), (self.dim, self.words), "레코드 차원이 색인과 다름");
        self.codes.extend_from_slice(&r.codes);
        self.inv_scales.push(r.inv_scale);
        self.bits.extend_from_slice(&r.bits);
    }
```
그리고 `flat.rs` 맨 위 `use` 를 `use crate::record::{Layout, Record};` 와 `use crate::{asym_score, dot_i8, hamming, quantize_i8, sign_bits, truncate_normalize};` 로 둔다(`quantize_i8`·`sign_bits` 를 더 쓰지 않게 되면 컴파일러 경고대로 지운다).

- [ ] **Step 4: 세대 파일을 구현한다**

`crates/vector/src/store.rs` 시험 위에:
```rust
//! 벡터 파일(DEC-048): 머리 32바이트 + 고정 길이 레코드. 슬롯 = 레코드 순번. 추가만 한다.
//! 몇 개가 유효한지는 meta.db 가 기억한다(쓰기 순서: 레코드 추가 → fsync → meta 기록). 열 때 meta 보다 긴 꼬리는 잘라낸다.

use crate::record::{Layout, Record};
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const MAGIC: [u8; 8] = *b"KRAVEC01";
pub const VERSION: u32 = 1;
pub const HEADER: u64 = 32;

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    /// 형식 표시·버전·차원이 다름(다른 프로그램의 파일이거나 깨짐)
    BadHeader,
    /// 파일에 레코드가 meta 가 아는 것보다 적다(유실)
    Short { have: u64, want: u64 },
}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        StoreError::Io(e)
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "벡터 파일 오류: {e}"),
            StoreError::BadHeader => write!(f, "벡터 파일 형식이 다름"),
            StoreError::Short { have, want } => write!(f, "벡터 파일이 짧음({have}/{want})"),
        }
    }
}

fn header(l: Layout) -> [u8; HEADER as usize] {
    let mut h = [0u8; HEADER as usize];
    h[..8].copy_from_slice(&MAGIC);
    h[8..12].copy_from_slice(&VERSION.to_le_bytes());
    h[12..16].copy_from_slice(&(l.dim as u32).to_le_bytes());
    h[16..20].copy_from_slice(&(l.bit_dim as u32).to_le_bytes());
    h
}

/// 머리를 읽어 확인한다. 끝나면 읽기 위치는 첫 레코드.
fn check_header(f: &mut File, l: Layout) -> Result<(), StoreError> {
    let mut h = [0u8; HEADER as usize];
    f.seek(SeekFrom::Start(0))?;
    if f.read_exact(&mut h).is_err() || h != header(l) {
        return Err(StoreError::BadHeader);
    }
    Ok(())
}

pub struct VecFile {
    file: File,
    layout: Layout,
    len: u64,
}

impl VecFile {
    /// 새 파일(있으면 비운다).
    pub fn create(path: &Path, l: Layout) -> io::Result<VecFile> {
        let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path)?;
        file.write_all(&header(l))?;
        file.sync_all()?;
        Ok(VecFile { file, layout: l, len: 0 })
    }

    /// meta 가 아는 길이(`expect`)로 연다. 더 길면 꼬리를 자르고, 짧으면 `Short`.
    pub fn open(path: &Path, l: Layout, expect: u64) -> Result<VecFile, StoreError> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        check_header(&mut file, l)?;
        let size = file.metadata()?.len();
        let rec = l.record_bytes() as u64;
        let have = (size - HEADER) / rec;
        if have < expect {
            return Err(StoreError::Short { have, want: expect });
        }
        let want = HEADER + expect * rec;
        if size != want {
            file.set_len(want)?;
            file.sync_all()?;
        }
        Ok(VecFile { file, layout: l, len: expect })
    }

    /// 레코드를 덧붙이고 디스크에 내린 뒤 첫 슬롯 번호를 돌려준다.
    pub fn append(&mut self, recs: &[Record]) -> io::Result<u64> {
        let first = self.len;
        let mut buf = Vec::with_capacity(recs.len() * self.layout.record_bytes());
        for r in recs {
            r.write_to(&mut buf);
        }
        self.file.seek(SeekFrom::Start(HEADER + self.len * self.layout.record_bytes() as u64))?;
        self.file.write_all(&buf)?;
        self.file.sync_data()?;
        self.len += recs.len() as u64;
        Ok(first)
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// 앞 `len` 개 레코드를 차례로 넘긴다(검색용 읽기·압축). 파일이 그보다 짧으면 `Short`.
pub fn read_each(path: &Path, l: Layout, len: u64, mut f: impl FnMut(u64, Record) -> io::Result<()>) -> Result<(), StoreError> {
    let mut file = File::open(path)?;
    check_header(&mut file, l)?;
    let rec = l.record_bytes() as u64;
    let have = file.metadata()?.len().saturating_sub(HEADER) / rec;
    if have < len {
        return Err(StoreError::Short { have, want: len });
    }
    let mut r = BufReader::with_capacity(1 << 20, file);
    let mut buf = vec![0u8; rec as usize];
    for slot in 0..len {
        r.read_exact(&mut buf)?;
        f(slot, Record::read_from(&buf, l))?;
    }
    Ok(())
}
```

- [ ] **Step 5: 시험이 통과하는지 본다**

Run: `cargo test -p vector --release`
Expected: PASS(기존 flat 시험 포함).

- [ ] **Step 6: 커밋**

```bash
git add crates/vector Cargo.lock
git commit -m "vector: record layout shared by eval and product, generation vector files with tail truncation"
```

---

### Task 7: `index::keyword` — 가중치 매개변수와 구절 일치

**Files:**
- Modify: `crates/index/src/keyword.rs`, `crates/index/Cargo.toml`(serde)

**Interfaces:**
- Produces: `index::keyword::KeywordWeights { title, filename, heading, body, bigram: f32 }`(Default = 3.0/2.5/2.0/1.0/0.5 — DEC-038 의 기존 값, serde), `KeywordIndex::search_with(&self, &str, limit, &KeywordWeights) -> tantivy::Result<Vec<(u64, f32)>>`, `KeywordIndex::phrase_ids(&self, &str, limit) -> tantivy::Result<Vec<u64>>`. `search(q, limit)` 는 `search_with(q, limit, &KeywordWeights::default())` 와 같다.

가중치는 원래 스키마가 아니라 검색할 때 `BoostQuery` 로만 쓰였다 — 스키마·`KEYWORD_VERSION` 은 바꾸지 않는다(기존 색인을 다시 만들 필요 없음).

- [ ] **Step 1: 시험을 쓴다**

`keyword.rs` 의 `mod tests` 안에 더한다:
```rust
    fn ids_w(kw: &KeywordIndex, q: &str, w: &KeywordWeights) -> Vec<u64> {
        kw.search_with(q, 10, w).unwrap().into_iter().map(|x| x.0).collect()
    }

    #[test]
    fn weights_change_field_priority() {
        let kw = index_of(&[
            (1, d(None, &[], &["이 문서는 내부통제 관련 회의 결과를 정리했다."]), "회의록.hwp"),
            (2, d(Some("내부통제규정 개정안"), &[], &["개정 이유"]), "개정안.hwp"),
        ]);
        assert_eq!(ids_w(&kw, "내부통제", &KeywordWeights::default()), ids(&kw, "내부통제"), "기본값 = 기존 search");
        assert_eq!(ids(&kw, "내부통제"), vec![2, 1]);
        let body_first = KeywordWeights { title: 0.1, filename: 0.1, heading: 0.1, body: 5.0, bigram: 0.5 };
        assert_eq!(ids_w(&kw, "내부통제", &body_first), vec![1, 2]);
    }

    #[test]
    fn phrase_ids_need_morphemes_in_order() {
        let kw = index_of(&[
            (1, d(None, &[], &["내부통제 규정을 개정했다"]), "a.hwp"),
            (2, d(None, &[], &["규정과 내부통제를 따로 본다"]), "b.hwp"),
            (3, d(None, &["내부통제 규정"], &["본문"]), "c.hwp"),
        ]);
        let mut got = kw.phrase_ids("내부통제 규정", 10).unwrap();
        got.sort();
        assert_eq!(got, vec![1, 3]);
        assert!(kw.phrase_ids("규정", 10).unwrap().is_empty(), "형태소 하나는 구절이 아니다");
    }
```

- [ ] **Step 2: 시험이 실패하는지 본다**

Run: `cargo test -p index --release keyword`
Expected: FAIL — `cannot find type KeywordWeights`.

- [ ] **Step 3: 구현한다**

`crates/index/Cargo.toml` `[dependencies]` 에 `serde.workspace = true` 를 더한다.

`keyword.rs`:
1. `Fields.pairs` 의 타입을 `[(Field, Field); 4]` 로 바꾸고 주석을 `/// (형태소 필드, bigram 필드) — 제목, 파일명, 소제목, 본문 순. 가중치는 검색할 때 KeywordWeights 로 준다.` 로.
2. `schema()` 의 `pair` 를 `let mut pair = |name: &str| (sb.add_text_field(&format!("{name}_m"), text(MORPH)), sb.add_text_field(&format!("{name}_b"), text(BIGRAM)));` 로, `let pairs = [pair("title"), pair("filename"), pair("heading"), pair("body")];` 로(필드 이름·순서가 같으므로 스키마는 그대로).
3. `upsert` 의 분해를 `let [(tm, tb), (fm, fb), (hm, hb), (bm, bb)] = self.f.pairs;` 로.
4. 가중치 타입과 검색을 더한다(`KeywordDoc` 정의 위):
```rust
/// 키워드 필드 가중치(스펙 §8.2, DEC-038 초기값). M2 실험(m0eval)이 조정한다 — 검색할 때만 쓰므로 바꿔도 다시 색인할 필요 없다.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KeywordWeights {
    pub title: f32,
    pub filename: f32,
    pub heading: f32,
    pub body: f32,
    /// bigram 구절 일치에 곱하는 배수
    pub bigram: f32,
}

impl Default for KeywordWeights {
    fn default() -> Self {
        KeywordWeights { title: 3.0, filename: 2.5, heading: 2.0, body: 1.0, bigram: 0.5 }
    }
}
```
5. `build_query` 를 `fn build_query(&self, query: &str, w: &KeywordWeights)` 로 바꾸고 필드 반복을 다음으로:
```rust
            let ws = [w.title, w.filename, w.heading, w.body];
            for (&(mf, bf), &fw) in self.f.pairs.iter().zip(&ws) {
```
그 안에서 `BoostQuery::new(Box::new(tq), w)` → `BoostQuery::new(Box::new(tq), fw)`, `BoostQuery::new(bq, w * 0.5)` → `BoostQuery::new(bq, fw * w.bigram)`.
6. `search` 를 둘로:
```rust
    pub fn search(&self, query: &str, limit: usize) -> tantivy::Result<Vec<(u64, f32)>> {
        self.search_with(query, limit, &KeywordWeights::default())
    }

    pub fn search_with(&self, query: &str, limit: usize, w: &KeywordWeights) -> tantivy::Result<Vec<(u64, f32)>> {
        let Some(q) = self.build_query(query, w)? else { return Ok(Vec::new()) };
        self.top_ids(&*q, limit)
    }

    /// 검색어 전체의 형태소가 순서대로 나오는 문서(제목·소제목·본문). 정확 구절 일치 가산용(스펙 §8.3). 형태소가 2개 미만이면 빈 목록.
    pub fn phrase_ids(&self, query: &str, limit: usize) -> tantivy::Result<Vec<u64>> {
        let query: String = query.chars().take(MAX_QUERY_CHARS).collect();
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for &(mf, _) in [self.f.pairs[0], self.f.pairs[2], self.f.pairs[3]].iter() {
            let mut tok = self.index.tokenizer_for_field(mf)?;
            let mut ts = tok.token_stream(&query);
            let mut terms: Vec<(usize, Term)> = Vec::new();
            while let Some(t) = ts.next() {
                terms.push((t.position, Term::from_field_text(mf, &t.text)));
            }
            if terms.len() < 2 {
                return Ok(Vec::new());
            }
            clauses.push((Occur::Should, Box::new(PhraseQuery::new_with_offset(terms))));
        }
        Ok(self.top_ids(&BooleanQuery::new(clauses), limit)?.into_iter().map(|x| x.0).collect())
    }

    fn top_ids(&self, q: &dyn Query, limit: usize) -> tantivy::Result<Vec<(u64, f32)>> {
        let searcher = self.reader.searcher();
        let hits = searcher.search(q, &TopDocs::with_limit(limit.max(1)).order_by_score())?;
        let mut out = Vec::with_capacity(hits.len());
        for (score, addr) in hits {
            let d: TantivyDocument = searcher.doc(addr)?;
            if let Some(id) = d.get_first(self.f.doc_id).and_then(|v| v.as_u64()) {
                out.push((id, score));
            }
        }
        Ok(out)
    }
```

- [ ] **Step 4: 시험이 통과하는지 본다**

Run: `cargo test -p index --release keyword`
Expected: PASS(기존 키워드 시험 전부 포함 — 기본 가중치로 결과가 같아야 한다).

- [ ] **Step 5: 커밋**

```bash
git add crates/index Cargo.lock
git commit -m "index: keyword field weights as search-time parameters, phrase match ids"
```

---

### Task 8: `index::hybrid` — 실험과 제품이 함께 쓰는 검색 코어

**Files:**
- Create: `crates/index/src/hybrid.rs`
- Modify: `crates/index/src/lib.rs`(`pub mod hybrid;`), `crates/index/Cargo.toml`(`query`, `vector` 경로 의존)

**Interfaces:**
- Consumes: `query::{ParsedQuery, QueryKind, DateSpan, Precision, Relativity, Day}`, `vector::{flat::FlatIndex, record::{Layout, Record}}`, `KeywordIndex::{search_with, phrase_ids}`, `KeywordWeights`.
- Produces (Task 14·15·17·18):
  - `Combine { ChunkElseCard, Max, Separate, CardOnly }`(serde).
  - `HybridParams { rrf_k: f64, w_keyword: [f64; 2], w_semantic: [f64; 2], combine: Combine, floor: f32, keyword: KeywordWeights, date_exact: f64, date_vague: f64, strip_relative: bool, strip_absolute: bool, recency: f64, phrase: f64, keyword_limit: usize, card_limit: usize, chunk_top: usize, chunk_min: usize, chunk_frac: f64 }` — `[0]` = 키워드형 질의, `[1]` = 의미형 질의. `Default`, serde.
  - `VectorSet { index: FlatIndex, owners: Vec<i64> }` — `VectorSet::new(Layout)`, `push(&mut self, owner: i64, &Record)`, `len()`, `is_empty()`.
  - `trait DocMeta { fn date(&self, id: i64) -> Option<Day>; fn has_chunks(&self, id: i64) -> bool; }`, `MapMeta { dates: HashMap<i64, Day>, chunked: HashSet<i64> }`(구현).
  - `QueryTexts { keyword: String, semantic: String }`, `query_texts(&ParsedQuery, &HybridParams) -> QueryTexts`.
  - `Semantic { cards: Vec<(i64, f32)>, chunks: Vec<(i64, f32)> }`(문서별 최고, 점수 내림차순), `semantic(&VectorSet, &VectorSet, &[f32], &HybridParams) -> Semantic`.
  - `Hit { id: i64, score: f64, keyword_rank: Option<u32>, card: Option<(u32, f32)>, chunk: Option<(u32, f32)>, date_boost: f64 }`.
  - `fuse(&[(i64, f32)], &Semantic, &ParsedQuery, &HybridParams, Day, &dyn DocMeta, &HashSet<i64>) -> Vec<Hit>`.
  - `search(&KeywordIndex, &VectorSet, &VectorSet, &ParsedQuery, Option<&[f32]>, &HybridParams, Day, &dyn DocMeta, limit) -> tantivy::Result<Vec<Hit>>` — `qv` 는 `query_texts(..).semantic` 을 모델 프로필로 감싸 임베딩한 원본 벡터(부르는 쪽 책임).

- [ ] **Step 1: 시험을 쓴다**

`crates/index/src/hybrid.rs`(시험만 먼저):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyword::{KeywordDoc, KeywordIndex};
    use crate::tokenize::Analyzers;
    use query::calendar::parse_ymd;
    use query::{parse, DateTable};

    fn today() -> Day {
        parse_ymd("2026-10-15").unwrap()
    }
    fn pq(q: &str) -> ParsedQuery {
        parse(q, today(), &DateTable::builtin())
    }
    fn meta(dates: &[(i64, &str)], chunked: &[i64]) -> MapMeta {
        MapMeta { dates: dates.iter().map(|(i, d)| (*i, parse_ymd(d).unwrap())).collect(), chunked: chunked.iter().copied().collect() }
    }
    fn ids(h: &[Hit]) -> Vec<i64> {
        h.iter().map(|x| x.id).collect()
    }
    fn none() -> HashSet<i64> {
        HashSet::new()
    }

    #[test]
    fn rrf_scores_follow_formula() {
        let p = HybridParams { rrf_k: 10.0, ..Default::default() };
        let q = pq("부경 주차장 밤에 불 꺼진 민원"); // 의미형
        let h = fuse(&[(7, 3.0), (8, 2.0)], &Semantic::default(), &q, &p, today(), &meta(&[], &[]), &none());
        assert_eq!(ids(&h), vec![7, 8]);
        assert!((h[0].score - p.w_keyword[1] / 11.0).abs() < 1e-12);
        assert!((h[1].score - p.w_keyword[1] / 12.0).abs() < 1e-12);
        assert_eq!(h[1].keyword_rank, Some(2));
    }

    #[test]
    fn query_kind_picks_weights() {
        let p = HybridParams { w_keyword: [5.0, 1.0], ..Default::default() };
        let kw = fuse(&[(1, 1.0)], &Semantic::default(), &pq("더비온"), &p, today(), &meta(&[], &[]), &none());
        let se = fuse(&[(1, 1.0)], &Semantic::default(), &pq("새 사업 시작하기 전에 확인할 것"), &p, today(), &meta(&[], &[]), &none());
        assert!((kw[0].score / se[0].score - 5.0).abs() < 1e-9);
    }

    #[test]
    fn chunk_else_card_uses_card_only_for_documents_without_chunks() {
        let p = HybridParams { combine: Combine::ChunkElseCard, ..Default::default() };
        let s = Semantic { cards: vec![(1, 0.9), (2, 0.8)], chunks: vec![(3, 0.7)] };
        // 1 은 본문 청크가 있지만 상위 청크에 없음 → 카드로 대신하지 않는다. 2 는 청크가 아직 없음 → 카드.
        let h = fuse(&[], &s, &pq("새 사업 시작하기 전에 확인할 것"), &p, today(), &meta(&[], &[1, 3]), &none());
        assert_eq!(ids(&h), vec![2, 3]);
        assert_eq!(h[0].card, Some((2, 0.8)));
    }

    #[test]
    fn max_and_separate_and_card_only() {
        let s = Semantic { cards: vec![(1, 0.9), (2, 0.5)], chunks: vec![(2, 0.95), (1, 0.6)] };
        let q = pq("새 사업 시작하기 전에 확인할 것");
        let m = meta(&[], &[1, 2]);
        let max = fuse(&[], &s, &q, &HybridParams { combine: Combine::Max, ..Default::default() }, today(), &m, &none());
        assert_eq!(ids(&max), vec![2, 1], "max(0.5, 0.95) > max(0.9, 0.6)");
        let sep = fuse(&[], &s, &q, &HybridParams { combine: Combine::Separate, rrf_k: 10.0, ..Default::default() }, today(), &m, &none());
        assert!((sep[0].score - sep[1].score).abs() < 1e-12, "카드 1위+청크 2위 = 카드 2위+청크 1위");
        let card = fuse(&[], &s, &q, &HybridParams { combine: Combine::CardOnly, ..Default::default() }, today(), &m, &none());
        assert_eq!(ids(&card), vec![1, 2]);
    }

    #[test]
    fn floor_drops_weak_semantic_results() {
        let s = Semantic { cards: vec![], chunks: vec![(1, 0.6), (2, 0.2)] };
        let h = fuse(&[], &s, &pq("새 사업 시작하기 전에 확인할 것"), &HybridParams { floor: 0.3, ..Default::default() }, today(), &meta(&[], &[1, 2]), &none());
        assert_eq!(ids(&h), vec![1]);
    }

    #[test]
    fn date_boost_inside_full_near_half_far_none() {
        let p = HybridParams { rrf_k: 10.0, date_exact: 2.0, ..Default::default() };
        // 작년 = 2025-01-01..12-31, 여유 = max(365/10, 7) = 36일
        let m = meta(&[(1, "2025-06-01"), (2, "2026-01-20"), (3, "2026-09-01")], &[]);
        let h = fuse(&[(3, 1.0), (2, 1.0), (1, 1.0)], &Semantic::default(), &pq("작년 출장 계획"), &p, today(), &m, &none());
        let by: HashMap<i64, &Hit> = h.iter().map(|x| (x.id, x)).collect();
        let top = 1.0 / 11.0;
        assert!((by[&1].date_boost - 2.0 * top).abs() < 1e-12);
        assert!((by[&2].date_boost - 1.0 * top).abs() < 1e-12);
        assert_eq!(by[&3].date_boost, 0.0);
        assert_eq!(ids(&h)[0], 1);
    }

    #[test]
    fn future_and_vague_dates() {
        let p = HybridParams { date_exact: 2.0, date_vague: 0.5, ..Default::default() };
        let m = meta(&[(1, "2026-10-01")], &[]);
        let h = fuse(&[(1, 1.0)], &Semantic::default(), &pq("내년 사업계획"), &p, today(), &m, &none());
        assert_eq!(h[0].date_boost, 0.0, "미래 표현은 가산하지 않는다");
        let h = fuse(&[(1, 1.0)], &Semantic::default(), &pq("요즘 민원"), &p, today(), &m, &none());
        assert!((h[0].date_boost - 0.5 / (p.rrf_k + 1.0)).abs() < 1e-12);
    }

    #[test]
    fn query_texts_strip_by_relativity() {
        let q = pq("작년 2025년 출장 계획");
        let both = HybridParams { strip_relative: true, strip_absolute: true, ..Default::default() };
        assert_eq!(query_texts(&q, &both).keyword, "출장 계획");
        let rel = HybridParams { strip_relative: true, strip_absolute: false, ..Default::default() };
        assert_eq!(query_texts(&q, &rel).keyword, "2025년 출장 계획");
        let off = HybridParams { strip_relative: false, strip_absolute: false, ..Default::default() };
        assert_eq!(query_texts(&q, &off).semantic, "작년 2025년 출장 계획");
    }

    #[test]
    fn recency_and_phrase_boosts_only_listed_docs() {
        let p = HybridParams { recency: 1.0, phrase: 1.0, rrf_k: 10.0, ..Default::default() };
        let m = meta(&[(1, "2026-10-15"), (2, "2016-10-15"), (9, "2026-10-15")], &[]);
        let phrase: HashSet<i64> = [2, 9].into_iter().collect();
        let h = fuse(&[(2, 1.0), (1, 1.0)], &Semantic::default(), &pq("내부통제 규정 개정 이유"), &p, today(), &m, &phrase);
        assert_eq!(ids(&h).len(), 2, "목록에 없는 9 는 가산만으로 들어오지 않는다");
        let by: HashMap<i64, &Hit> = h.iter().map(|x| (x.id, x)).collect();
        let top = 1.0 / 11.0;
        assert!((by[&1].score - (p.w_keyword[1] / 12.0 + top)).abs() < 1e-9, "오늘 문서 최근 가산 = 1위 기여");
        assert!(by[&2].score > p.w_keyword[1] / 11.0 + top * 0.99, "구절 가산");
    }

    fn vecs(n: usize, seed: u64) -> Vec<Vec<f32>> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                (0..768)
                    .map(|_| {
                        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                        ((s >> 33) as f32 / (1u64 << 31) as f32) - 0.5
                    })
                    .collect()
            })
            .collect()
    }

    /// 일치 검사의 단위 판: f32 → 레코드(평가) 와 f32 → 레코드 → 바이트 → 레코드(제품 디스크)가 같은 순위를 낸다.
    #[test]
    fn same_vectors_give_same_ranking_through_disk_bytes() {
        let a = Analyzers::load().unwrap();
        let mut kw = KeywordIndex::in_ram(&a).unwrap();
        let docs = ["내부통제 규정 개정", "더비온 운영 계획", "경마 시행 계획", "출장 계획"];
        for (i, t) in docs.iter().enumerate() {
            let d = doc::Document { title: Some(t.to_string()), blocks: vec![] };
            kw.upsert(&KeywordDoc::new(i as u64, Some(&d), &[format!("{t}.hwp")])).unwrap();
        }
        kw.commit().unwrap();
        let vs = vecs(12, 3);
        let (mut c1, mut k1, mut c2, mut k2) = (VectorSet::new(Layout::M0), VectorSet::new(Layout::M0), VectorSet::new(Layout::M0), VectorSet::new(Layout::M0));
        for (i, v) in vs.iter().enumerate() {
            let r = Record::from_full(v, Layout::M0).unwrap();
            let mut b = Vec::new();
            r.write_to(&mut b);
            let r2 = Record::read_from(&b, Layout::M0);
            let owner = (i % 4) as i64;
            if i < 4 {
                c1.push(owner, &r);
                c2.push(owner, &r2);
            } else {
                k1.push(owner, &r);
                k2.push(owner, &r2);
            }
        }
        let p = HybridParams::default();
        let q = pq("작년 출장 계획");
        let m = meta(&[(3, "2025-03-01")], &[0, 1, 2, 3]);
        let h1 = search(&kw, &c1, &k1, &q, Some(&vs[5]), &p, today(), &m, 10).unwrap();
        let h2 = search(&kw, &c2, &k2, &q, Some(&vs[5]), &p, today(), &m, 10).unwrap();
        assert_eq!(h1, h2);
        assert!(!h1.is_empty());
    }

    #[test]
    fn keyword_only_without_query_vector() {
        let a = Analyzers::load().unwrap();
        let mut kw = KeywordIndex::in_ram(&a).unwrap();
        let d = doc::Document { title: Some("더비온 운영 계획".into()), blocks: vec![] };
        kw.upsert(&KeywordDoc::new(4, Some(&d), &["a.hwp".into()])).unwrap();
        kw.commit().unwrap();
        let empty = VectorSet::new(Layout::M0);
        let h = search(&kw, &empty, &empty, &pq("더비온"), None, &HybridParams::default(), today(), &meta(&[], &[]), 10).unwrap();
        assert_eq!(ids(&h), vec![4]);
    }
}
```

- [ ] **Step 2: 시험이 실패하는지 본다**

`crates/index/src/lib.rs` 에 `pub mod hybrid;`, `crates/index/Cargo.toml` `[dependencies]` 에 `query = { path = "../query" }`, `vector = { path = "../vector" }` 를 더하고:

Run: `cargo test -p index --release hybrid`
Expected: FAIL — `cannot find type HybridParams` 등.

- [ ] **Step 3: 구현한다**

`crates/index/src/hybrid.rs` 시험 위에:
```rust
//! 하이브리드 검색 코어(스펙 §8.2·§8.3, M2 설계 §4.2): 키워드 + 문서 카드 + 본문 청크 → 문서 단위 가중 RRF + 날짜·최근·구절 보정.
//! m0eval(실험)과 앱(제품)이 **이 함수들을 그대로** 부른다 — 바깥이 다른 것은 벡터를 어디서 읽는지뿐이다(DEC-052).
//! 모든 값은 `HybridParams`. 기본값은 Task 16 실험이 정한 값이다.

use crate::keyword::{KeywordIndex, KeywordWeights};
use query::{DateSpan, Day, ParsedQuery, Precision, QueryKind, Relativity};
use std::collections::{HashMap, HashSet};
use vector::flat::FlatIndex;
use vector::record::{Layout, Record};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Combine {
    /// 최고 본문 청크, 청크가 아직 없는 문서만 카드(스펙 §8.3 기본)
    ChunkElseCard,
    /// max(카드, 청크)
    Max,
    /// 카드 목록과 청크 목록을 따로 RRF 에 넣는다
    Separate,
    /// 카드만(비교 기준)
    CardOnly,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HybridParams {
    pub rrf_k: f64,
    /// [키워드형 질의, 의미형 질의]
    pub w_keyword: [f64; 2],
    pub w_semantic: [f64; 2],
    pub combine: Combine,
    /// 코사인 하한 — 미만인 의미 결과는 결합에서 뺀다. 0 이하면 끔.
    pub floor: f32,
    pub keyword: KeywordWeights,
    /// 날짜 칩 기간 안 문서 가산(1위 RRF 기여 1/(k+1) 의 배수). 경계 근처는 절반.
    pub date_exact: f64,
    pub date_vague: f64,
    /// 상대 표현(작년·지난달)을 키워드·의미 검색어에서 뺀다
    pub strip_relative: bool,
    /// 숫자로 쓴 해(2025년)를 뺀다
    pub strip_absolute: bool,
    /// 최근 문서 약한 가산 세기(0 = 끔): 세기 × exp(−경과일/365) × 1/(k+1)
    pub recency: f64,
    /// 정확 구절 일치 가산(0 = 끔), 1/(k+1) 의 배수
    pub phrase: f64,
    pub keyword_limit: usize,
    pub card_limit: usize,
    /// 재계산 뒤 남길 청크 수(문서로 묶기 전)
    pub chunk_top: usize,
    /// 이진 후보 수 = max(chunk_min, 청크 수 × chunk_frac)(스펙 §8.2)
    pub chunk_min: usize,
    pub chunk_frac: f64,
}

impl Default for HybridParams {
    fn default() -> Self {
        HybridParams {
            rrf_k: 60.0,
            w_keyword: [1.0, 1.0],
            w_semantic: [1.0, 1.0],
            combine: Combine::ChunkElseCard,
            floor: 0.0,
            keyword: KeywordWeights::default(),
            date_exact: 1.0,
            date_vague: 0.5,
            strip_relative: true,
            strip_absolute: false,
            recency: 0.0,
            phrase: 0.0,
            keyword_limit: 200,
            card_limit: 200,
            chunk_top: 200,
            chunk_min: 1_000,
            chunk_frac: 0.02,
        }
    }
}

/// 슬롯 순서 그대로의 벡터와 그 주인(내용 id).
pub struct VectorSet {
    pub index: FlatIndex,
    pub owners: Vec<i64>,
}

impl VectorSet {
    pub fn new(l: Layout) -> VectorSet {
        VectorSet { index: FlatIndex::new(l.dim, l.bit_dim), owners: Vec::new() }
    }
    pub fn push(&mut self, owner: i64, r: &Record) {
        self.index.push_record(r);
        self.owners.push(owner);
    }
    pub fn len(&self) -> usize {
        self.owners.len()
    }
    pub fn is_empty(&self) -> bool {
        self.owners.is_empty()
    }
}

/// 결합에 필요한 문서 정보.
pub trait DocMeta {
    /// 문서 날짜(M2: 가장 최근 경로의 수정일, 한국 날짜)
    fn date(&self, id: i64) -> Option<Day>;
    /// 본문 청크가 하나라도 있는가(ChunkElseCard 에서 카드로 대신할지)
    fn has_chunks(&self, id: i64) -> bool;
}

#[derive(Debug, Default, Clone)]
pub struct MapMeta {
    pub dates: HashMap<i64, Day>,
    pub chunked: HashSet<i64>,
}

impl DocMeta for MapMeta {
    fn date(&self, id: i64) -> Option<Day> {
        self.dates.get(&id).copied()
    }
    fn has_chunks(&self, id: i64) -> bool {
        self.chunked.contains(&id)
    }
}

pub struct QueryTexts {
    pub keyword: String,
    /// 모델 프로필의 검색어 접두어를 붙이기 전
    pub semantic: String,
}

pub fn query_texts(pq: &ParsedQuery, p: &HybridParams) -> QueryTexts {
    let drop = |s: &DateSpan| {
        s.range.is_some()
            && match s.relativity {
                Relativity::Relative => p.strip_relative,
                Relativity::Absolute => p.strip_absolute,
            }
    };
    let t = pq.text_without(drop);
    QueryTexts { keyword: t.clone(), semantic: t }
}

#[derive(Debug, Default, Clone)]
pub struct Semantic {
    pub cards: Vec<(i64, f32)>,
    pub chunks: Vec<(i64, f32)>,
}

fn sort_desc(v: &mut [(i64, f32)]) {
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
}

/// 카드는 int8 전체 스캔, 청크는 이진 후보 → int8 재계산 → 상위 chunk_top 청크 → 문서별 최고.
pub fn semantic(cards: &VectorSet, chunks: &VectorSet, qv: &[f32], p: &HybridParams) -> Semantic {
    let card_hits: Vec<(i64, f32)> = cards.index.search_exact(qv, p.card_limit).into_iter().map(|(s, c)| (cards.owners[s as usize], c)).collect();
    let cand = p.chunk_min.max((chunks.len() as f64 * p.chunk_frac).round() as usize);
    let mut best: HashMap<i64, f32> = HashMap::new();
    for (s, c) in chunks.index.search_hamming(qv, cand, p.chunk_top) {
        let e = best.entry(chunks.owners[s as usize]).or_insert(f32::MIN);
        if c > *e {
            *e = c;
        }
    }
    let mut chunk_hits: Vec<(i64, f32)> = best.into_iter().collect();
    sort_desc(&mut chunk_hits);
    Semantic { cards: card_hits, chunks: chunk_hits }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub id: i64,
    pub score: f64,
    pub keyword_rank: Option<u32>,
    /// (순위, 코사인)
    pub card: Option<(u32, f32)>,
    pub chunk: Option<(u32, f32)>,
    pub date_boost: f64,
}

fn hit(acc: &mut HashMap<i64, Hit>, id: i64) -> &mut Hit {
    acc.entry(id).or_insert(Hit { id, score: 0.0, keyword_rank: None, card: None, chunk: None, date_boost: 0.0 })
}

pub fn fuse(keyword: &[(i64, f32)], sem: &Semantic, pq: &ParsedQuery, p: &HybridParams, today: Day, meta: &dyn DocMeta, phrase: &HashSet<i64>) -> Vec<Hit> {
    let ki = match pq.kind {
        QueryKind::Keyword => 0,
        QueryKind::Semantic => 1,
    };
    let k = p.rrf_k;
    let rrf = |r: usize| 1.0 / (k + r as f64 + 1.0);
    let mut acc: HashMap<i64, Hit> = HashMap::new();
    let mut listed: HashSet<i64> = HashSet::new();
    for (r, (id, _)) in keyword.iter().enumerate() {
        let h = hit(&mut acc, *id);
        h.keyword_rank = Some(r as u32 + 1);
        h.score += p.w_keyword[ki] * rrf(r);
        listed.insert(*id);
    }
    let keep = |v: &[(i64, f32)]| -> Vec<(i64, f32)> { v.iter().copied().filter(|x| p.floor <= 0.0 || x.1 >= p.floor).collect() };
    let (cards, chunks) = (keep(&sem.cards), keep(&sem.chunks));
    for (r, (id, c)) in cards.iter().enumerate() {
        hit(&mut acc, *id).card = Some((r as u32 + 1, *c));
    }
    for (r, (id, c)) in chunks.iter().enumerate() {
        hit(&mut acc, *id).chunk = Some((r as u32 + 1, *c));
    }
    let ws = p.w_semantic[ki];
    let mut add_list = |acc: &mut HashMap<i64, Hit>, list: &[(i64, f32)]| {
        for (r, (id, _)) in list.iter().enumerate() {
            hit(acc, *id).score += ws * rrf(r);
            listed.insert(*id);
        }
    };
    match p.combine {
        Combine::Separate => {
            add_list(&mut acc, &cards);
            add_list(&mut acc, &chunks);
        }
        Combine::CardOnly => add_list(&mut acc, &cards),
        Combine::ChunkElseCard | Combine::Max => {
            let mut docs: HashMap<i64, f32> = chunks.iter().copied().collect();
            for (id, c) in &cards {
                if p.combine == Combine::Max {
                    let e = docs.entry(*id).or_insert(*c);
                    if *c > *e {
                        *e = *c;
                    }
                } else if !docs.contains_key(id) && !meta.has_chunks(*id) {
                    docs.insert(*id, *c);
                }
            }
            let mut list: Vec<(i64, f32)> = docs.into_iter().collect();
            sort_desc(&mut list);
            add_list(&mut acc, &list);
        }
    }
    // 보정은 어느 목록에든 오른 문서에만(가산만으로 새 문서가 들어오지 않게)
    let top = rrf(0);
    let chips: Vec<&DateSpan> = pq.chips().collect();
    acc.retain(|id, _| listed.contains(id));
    for h in acc.values_mut() {
        if let Some(d) = meta.date(h.id) {
            let mut b: f64 = 0.0;
            for s in &chips {
                let r = s.range.expect("칩은 기간이 있다");
                let w = match s.precision {
                    Precision::Exact => p.date_exact,
                    Precision::Vague => p.date_vague,
                };
                let margin = (r.days() / 10).max(7);
                let v = if r.contains(d) {
                    w
                } else if d >= r.start - margin && d <= r.end + margin {
                    w / 2.0
                } else {
                    0.0
                };
                b = b.max(v);
            }
            h.date_boost = b * top;
            h.score += h.date_boost;
            if p.recency > 0.0 {
                let age = (today - d).max(0) as f64;
                h.score += p.recency * (-age / 365.0).exp() * top;
            }
        }
        if p.phrase > 0.0 && phrase.contains(&h.id) {
            h.score += p.phrase * top;
        }
    }
    let mut v: Vec<Hit> = acc.into_values().collect();
    v.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.id.cmp(&b.id)));
    v
}

/// 검색 한 번. `qv` 가 없으면(모델 없음·벡터 없음) 키워드만.
#[allow(clippy::too_many_arguments)]
pub fn search(
    kw: &KeywordIndex,
    cards: &VectorSet,
    chunks: &VectorSet,
    pq: &ParsedQuery,
    qv: Option<&[f32]>,
    p: &HybridParams,
    today: Day,
    meta: &dyn DocMeta,
    limit: usize,
) -> tantivy::Result<Vec<Hit>> {
    let t = query_texts(pq, p);
    let keyword: Vec<(i64, f32)> = kw.search_with(&t.keyword, p.keyword_limit, &p.keyword)?.into_iter().map(|(id, s)| (id as i64, s)).collect();
    let phrase: HashSet<i64> = if p.phrase > 0.0 {
        kw.phrase_ids(&t.keyword, p.keyword_limit)?.into_iter().map(|x| x as i64).collect()
    } else {
        HashSet::new()
    };
    let sem = match qv {
        Some(v) if !(cards.is_empty() && chunks.is_empty()) => semantic(cards, chunks, v, p),
        _ => Semantic::default(),
    };
    let mut hits = fuse(&keyword, &sem, pq, p, today, meta, &phrase);
    hits.truncate(limit);
    Ok(hits)
}
```

- [ ] **Step 4: 시험이 통과하는지 본다**

Run: `cargo test -p index --release hybrid`
Expected: PASS.

- [ ] **Step 5: 워크스페이스 전체 시험**

Run: `cargo test --workspace --release`
Expected: PASS(diag·m0eval 도 `vector`·`index` 변경 뒤 컴파일된다).

- [ ] **Step 6: 커밋**

```bash
git add crates/index Cargo.lock
git commit -m "index: hybrid search core shared by eval and product (RRF, card/chunk combine, floor, date/recency/phrase boosts)"
```

---
### Task 9: `embed` — 임베딩 trait, 시험용 가짜, 본문 3단 순서, KI-001·KI-003

**Files:**
- Create: `crates/embed/src/api.rs`, `crates/embed/src/fake.rs`, `crates/embed/src/plan.rs`
- Modify: `crates/embed/src/lib.rs`, `crates/embed/src/chunk.rs`(청크 표시, KI-001), `crates/embed/src/engine.rs`(KI-003)

**Interfaces:**
- Produces:
  - `embed::api::Embed` trait: `fn embed(&mut self, inputs: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>`(원본 차원, 입력 순서), `fn count_tokens(&self, text: &str) -> usize`. `Embedder` 가 구현.
  - `embed::fake::{FakeEmbedder, fake_vector}` — `FakeEmbedder { calls: usize, fail_on: Option<String> }`(Default), 768차원.
  - `embed::chunk::{Chunk { text: String, heading: bool }, build_chunks_marked}` — `build_chunks` 는 그대로(진단이 씀).
  - `embed::plan::{FormatClass { Document, Data }, class_of_ext(&str) -> FormatClass, CARD_TOKENS: usize = 256, DATA_HEAD: usize = 20, PIECE: usize = 50, chunk_params(ext: &str) -> ChunkParams, priority_order(&[Chunk]) -> Vec<usize>, CardDoc { id, class, mtime_ns }, card_order(&mut [CardDoc]), BodyDoc { id, class, mtime_ns, total }, schedule(&[BodyDoc], data_tail: bool) -> Vec<(i64, Range<usize>)>, tag() -> String}`.
  - 청크 "위치(pos)" = `priority_order` 결과 안의 순번. 데이터 형식 앞부분(②) = pos < `DATA_HEAD`.

- [ ] **Step 1: 시험을 쓴다**

`crates/embed/src/plan.rs`(시험만 먼저):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_and_caps() {
        for e in ["hwp", "HWPX", "pdf", "docx", "pptx", "odt", "odp"] {
            assert_eq!(class_of_ext(e), FormatClass::Document, "{e}");
        }
        for e in ["txt", "csv", "xlsx", "ods"] {
            assert_eq!(class_of_ext(e), FormatClass::Data, "{e}");
        }
        assert_eq!(chunk_params("xlsx").max_chunks, 200);
        assert_eq!(chunk_params("csv").max_chunks, 200);
        assert_eq!(chunk_params("txt").max_chunks, 2000);
        assert_eq!(chunk_params("hwp").max_chunks, 2000);
        assert_eq!((chunk_params("hwp").target_tokens, chunk_params("hwp").max_tokens), (300, 400));
    }

    #[test]
    fn heading_chunks_first_and_stable() {
        let c = |t: &str, h: bool| Chunk { text: t.into(), heading: h };
        assert_eq!(priority_order(&[c("a", false), c("b", true), c("c", false), c("d", true)]), vec![1, 3, 0, 2]);
    }

    #[test]
    fn cards_documents_first_then_recent() {
        let mut v = vec![
            CardDoc { id: 1, class: FormatClass::Data, mtime_ns: 900 },
            CardDoc { id: 2, class: FormatClass::Document, mtime_ns: 100 },
            CardDoc { id: 3, class: FormatClass::Document, mtime_ns: 500 },
            CardDoc { id: 4, class: FormatClass::Document, mtime_ns: 500 },
        ];
        card_order(&mut v);
        assert_eq!(v.iter().map(|d| d.id).collect::<Vec<_>>(), vec![3, 4, 2, 1]);
    }

    #[test]
    fn schedule_three_tiers() {
        let docs = [
            BodyDoc { id: 1, class: FormatClass::Document, mtime_ns: 10, total: 120 },
            BodyDoc { id: 2, class: FormatClass::Document, mtime_ns: 20, total: 30 },
            BodyDoc { id: 3, class: FormatClass::Data, mtime_ns: 30, total: 75 },
            BodyDoc { id: 4, class: FormatClass::Data, mtime_ns: 5, total: 10 },
            BodyDoc { id: 5, class: FormatClass::Document, mtime_ns: 99, total: 0 },
        ];
        assert_eq!(
            schedule(&docs, true),
            vec![(2, 0..30), (1, 0..50), (1, 50..100), (1, 100..120), (3, 0..20), (4, 0..10), (3, 20..70), (3, 70..75)]
        );
        assert_eq!(schedule(&docs, false), vec![(2, 0..30), (1, 0..50), (1, 50..100), (1, 100..120), (3, 0..20), (4, 0..10)]);
    }

    #[test]
    fn tag_names_every_value_that_changes_vectors() {
        let t = tag();
        for x in ["card=256", "card_v=2", "chunk=300/400", "doc_cap=2000", "table_cap=200", "head=20"] {
            assert!(t.contains(x), "{t}");
        }
    }
}
```

`crates/embed/src/fake.rs`(시험만 먼저):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::cosine;

    #[test]
    fn deterministic_and_text_sensitive() {
        assert_eq!(fake_vector("내부통제 규정"), fake_vector("내부통제 규정"));
        let a = fake_vector("내부통제 규정 개정 이유");
        let b = fake_vector("내부통제 규정을 개정한 이유");
        let c = fake_vector("경주마 수송 차량 냉방");
        assert!(cosine(&a, &b) > cosine(&a, &c));
        assert_eq!(a.len(), 768);
    }

    #[test]
    fn fail_on_marks_inputs() {
        let mut f = FakeEmbedder { fail_on: Some("고장".into()), ..Default::default() };
        assert!(f.embed(&["정상".into()]).is_ok());
        assert!(f.embed(&["정상".into(), "고장 입력".into()]).is_err());
        assert_eq!(f.calls, 2);
        assert_eq!(f.count_tokens("가나"), 4);
    }
}
```

`crates/embed/src/chunk.rs` 의 `mod tests` 에 더한다:
```rust
    #[test]
    fn marked_chunks_know_heading_starts() {
        let d = Document { title: None, blocks: vec![b(Paragraph, "앞"), b(Heading, "Ⅱ. 원인"), b(Paragraph, "뒤")] };
        let c = build_chunks_marked(&d, &ChunkParams { target_tokens: 100, max_tokens: 120, max_chunks: 10 }, &chars);
        assert_eq!(c, vec![Chunk { text: "앞".into(), heading: false }, Chunk { text: "Ⅱ. 원인\n뒤".into(), heading: true }]);
        assert_eq!(build_chunks(&d, &ChunkParams { target_tokens: 100, max_tokens: 120, max_chunks: 10 }, &chars), vec!["앞", "Ⅱ. 원인\n뒤"]);
    }

    /// KI-001: 15만 자 한 문단을 자를 때 매번 글 전체를 세지 않는다(창 안에서만 센다).
    #[test]
    fn huge_paragraph_is_cut_inside_a_window() {
        let text = "가나다라마바사아자차 ".repeat(15_000);
        let d = Document { title: None, blocks: vec![b(Paragraph, &text)] };
        let longest = std::cell::Cell::new(0usize);
        let count = |s: &str| {
            longest.set(longest.get().max(s.len()));
            s.chars().count() + 2
        };
        let t = std::time::Instant::now();
        let c = build_chunks(&d, &BODY_CHUNK, &count);
        assert!(c.len() > 300, "{}", c.len());
        assert!(longest.get() < 20_000, "센 글이 너무 김: {}바이트", longest.get());
        assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
    }
```

- [ ] **Step 2: 시험이 실패하는지 본다**

`crates/embed/src/lib.rs` 를 다음으로 바꾸고:
```rust
pub mod api;
pub mod chunk;
pub mod engine;
pub mod fake;
pub mod plan;
pub mod profile;
/// 벡터 연산은 `vector` 크레이트로 옮겼다(KI-008). 기존 경로 `embed::vector::*` 를 유지하려고 재수출한다.
pub use ::vector;
```
Run: `cargo test -p embed --release --lib`
Expected: FAIL — `cannot find type Chunk`, `FakeEmbedder`, `class_of_ext` 등.

- [ ] **Step 3: 임베딩 trait 과 가짜를 구현한다**

`crates/embed/src/api.rs`:
```rust
//! 임베딩 엔진 추상화: 앱 파이프라인이 실제 모델(`Embedder`)과 시험용 가짜(`FakeEmbedder`)를 같은 길로 쓴다.

use crate::engine::{EmbedError, Embedder};

pub trait Embed {
    /// 원본 차원(정규화 전) 벡터, 입력 순서대로. 공백뿐인 입력은 넣지 않는다(부르는 쪽 책임).
    fn embed(&mut self, inputs: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;
    /// 카드·청크 상한을 재는 토큰 수(실제: 모델 토크나이저).
    fn count_tokens(&self, text: &str) -> usize;
}

impl Embed for Embedder {
    fn embed(&mut self, inputs: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(Embedder::embed(self, inputs)?.into_iter().map(|e| e.vector).collect())
    }

    fn count_tokens(&self, text: &str) -> usize {
        self.token_count(text).unwrap_or_else(|_| text.chars().count())
    }
}
```

`crates/embed/src/fake.rs` 시험 위에:
```rust
//! 시험용 임베더 — CI 가 모델 없이 색인·검색 파이프라인 전체를 돌리려고. 글자 bigram 을 해시해 768차원에 더한다:
//! 결정적이고, 글자가 겹치는 글끼리 비슷하다. 품질 평가에는 쓰지 않는다.

use crate::api::Embed;
use crate::engine::EmbedError;

#[derive(Debug, Default)]
pub struct FakeEmbedder {
    /// embed 호출 횟수(시험이 "다시 임베딩하지 않았다"를 확인)
    pub calls: usize,
    /// 이 글자를 담은 입력이 있으면 그 묶음을 실패시킨다(실패 처리 시험)
    pub fail_on: Option<String>,
}

pub fn fake_vector(t: &str) -> Vec<f32> {
    let cs: Vec<char> = t.chars().filter(|c| !c.is_whitespace()).collect();
    let mut v = vec![0f32; 768];
    v[0] = 1e-3; // 빈 글도 0벡터가 되지 않게
    for w in cs.windows(2) {
        let mut h: u64 = 0xcbf29ce484222325;
        let mut buf = [0u8; 8];
        for c in w {
            for b in c.encode_utf8(&mut buf).bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
        }
        let sign = if (h >> 40) & 1 == 1 { -1.0 } else { 1.0 };
        v[(h % 768) as usize] += sign;
    }
    v
}

impl Embed for FakeEmbedder {
    fn embed(&mut self, inputs: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        self.calls += 1;
        if let Some(f) = &self.fail_on {
            if inputs.iter().any(|t| t.contains(f.as_str())) {
                return Err(EmbedError::Decode(-9));
            }
        }
        Ok(inputs.iter().map(|t| fake_vector(t)).collect())
    }

    fn count_tokens(&self, text: &str) -> usize {
        text.chars().count() + 2
    }
}
```

- [ ] **Step 4: 청크 표시와 KI-001 을 구현한다**

`crates/embed/src/chunk.rs`:
1. `cut_to` 를 바꾼다:
```rust
/// `text`를 토큰 한도 이하가 되도록 글자 경계에서 자른다. 앞에서부터 창(한도 × 4글자, 모자라면 두 배씩)만 세고
/// 그 안에서 이진 탐색한다 — 15만 자 한 문단에서 매번 글 전체를 토큰화하던 문제(KI-001).
fn cut_to(text: &str, max_tokens: usize, count: &dyn Fn(&str) -> usize) -> String {
    let idx: Vec<usize> = text.char_indices().map(|(i, _)| i).chain([text.len()]).collect();
    let n = idx.len() - 1;
    let mut w = (max_tokens.max(1) * 4).min(n);
    loop {
        if count(&text[..idx[w]]) > max_tokens {
            break;
        }
        if w == n {
            return text.to_string();
        }
        w = (w * 2).min(n);
    }
    let (mut lo, mut hi) = (0usize, w);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if count(&text[..idx[mid]]) <= max_tokens {
            lo = mid
        } else {
            hi = mid - 1
        }
    }
    text[..idx[lo]].to_string()
}
```
2. `split_long` 의 반복 머리를 바꾼다(남은 글 전체를 세지 않게):
```rust
    while !rest.is_empty() {
        let head = cut_to(rest, max, count);
        if head.len() == rest.len() {
            out.push(rest.to_string());
            break;
        }
        let mut cut = head.rfind(['.', '\n']).map(|i| i + 1).filter(|i| *i > head.len() / 2).unwrap_or(head.len());
```
(그 아래 `if cut == 0 { … }` 부터는 그대로.)
3. `build_chunks` 를 `build_chunks_marked` 로 옮기고 감싼다:
```rust
/// 본문 청크와 "소제목으로 시작하는가" 표시(목차·소제목 청크를 먼저 임베딩, 스펙 §6.6).
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub text: String,
    pub heading: bool,
}

/// 구조 기반 청크. 표가 이어지는 청크에는 표 머리글을 다시 붙인다. 최대 max_chunks개.
pub fn build_chunks(doc: &Document, p: &ChunkParams, count: &dyn Fn(&str) -> usize) -> Vec<String> {
    build_chunks_marked(doc, p, count).into_iter().map(|c| c.text).collect()
}

pub fn build_chunks_marked(doc: &Document, p: &ChunkParams, count: &dyn Fn(&str) -> usize) -> Vec<Chunk> {
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut cur_heading = false;
    let mut table_header: Option<String> = None;
    let flush = |cur: &mut Vec<String>, cur_heading: &mut bool, chunks: &mut Vec<Chunk>| {
        if cur.iter().any(|s| !s.trim().is_empty()) {
            chunks.push(Chunk { text: cur.join("\n"), heading: *cur_heading });
        }
        cur.clear();
        *cur_heading = false;
    };
    for b in &doc.blocks {
        let text = b.text.trim();
        if text.is_empty() {
            continue;
        }
        match b.kind {
            BlockKind::Heading => {
                flush(&mut cur, &mut cur_heading, &mut chunks);
                table_header = None;
                cur.push(text.to_string());
                cur_heading = true;
            }
            BlockKind::TableHeader => {
                flush(&mut cur, &mut cur_heading, &mut chunks);
                // 반복해서 붙일 머리글은 한도의 1/3 이내로 줄인다. 원래 머리글이 길면 그 전문은 따로 청크로 남긴다.
                let short = cut_to(text, p.max_tokens / 3, count);
                if short.len() < text.len() {
                    chunks.extend(split_long(text, p.max_tokens, count).into_iter().map(|t| Chunk { text: t, heading: false }));
                }
                if short.trim().is_empty() {
                    table_header = None;
                } else {
                    cur.push(short.clone());
                    table_header = Some(short);
                }
            }
            BlockKind::Paragraph | BlockKind::TableRow => {
                if b.kind == BlockKind::Paragraph {
                    table_header = None;
                }
                let reserve = table_header.as_deref().map(count).unwrap_or(0) + 1;
                for piece in split_long(text, p.max_tokens.saturating_sub(reserve).max(1), count) {
                    let candidate = if cur.is_empty() { piece.clone() } else { format!("{}\n{}", cur.join("\n"), piece) };
                    if !cur.is_empty() && count(&candidate) > p.target_tokens {
                        flush(&mut cur, &mut cur_heading, &mut chunks);
                        if let (BlockKind::TableRow, Some(h)) = (&b.kind, &table_header) {
                            cur.push(h.clone());
                        }
                    }
                    cur.push(piece);
                }
            }
        }
        if chunks.len() >= p.max_chunks {
            break;
        }
    }
    flush(&mut cur, &mut cur_heading, &mut chunks);
    chunks.truncate(p.max_chunks);
    chunks
}
```

4. `build_card` 의 첫 문단 부분을 바꾼다 — 문단이 없는 문서(CSV·XLSX 처럼 표뿐인 것)는 카드가 없어 빠른 의미 검색에서 통째로 빠지던 빈틈(계획 검증 중 발견, DEC-057):
```rust
    match doc.blocks.iter().find(|b| b.kind == BlockKind::Paragraph && !b.text.trim().is_empty()) {
        Some(p) => parts.push(p.text.trim().to_string()),
        // 문단이 없으면(표·시트) 첫 표 머리글과 첫 행으로 대신한다(DEC-057).
        None => {
            for kind in [BlockKind::TableHeader, BlockKind::TableRow] {
                if let Some(b) = doc.blocks.iter().find(|b| b.kind == kind && !b.text.trim().is_empty()) {
                    parts.push(b.text.trim().to_string());
                }
            }
        }
    }
```
(바꾸는 곳: `if let Some(p) = doc.blocks.iter().find(|b| b.kind == BlockKind::Paragraph …) { parts.push(…); }` 블록. 함수 위 주석은 `/// 제목 + 소제목들 + 첫 문단(없으면 첫 표 머리글·첫 행), 토큰 ≤ max_tokens. 내용이 없으면 None.` 로.)
그리고 `chunk.rs` 시험에 더한다:
```rust
    #[test]
    fn card_uses_table_header_and_first_row_without_paragraphs() {
        let d = Document { title: None, blocks: vec![b(TableHeader, "번호 | 사업장"), b(TableRow, "번호: 1 | 사업장: 서울"), b(TableRow, "번호: 2 | 사업장: 제주")] };
        assert_eq!(build_card(&d, 1000, &chars).unwrap(), "번호 | 사업장\n번호: 1 | 사업장: 서울");
    }
```

- [ ] **Step 5: 3단 순서를 구현한다**

`crates/embed/src/plan.rs` 시험 위에:
```rust
//! 임베딩 순서와 범위(결정 #27·#28, 스펙 §6.6): 카드는 문서 형식·최근 수정 먼저,
//! 본문은 ① 문서 형식 전체(50청크 조각을 문서끼리 번갈아) → ② 데이터 형식 앞 20청크 → ③ 데이터 형식 나머지(끌 수 있음).
//! 청크 "위치(pos)" = `priority_order` 결과 안의 순번(소제목 청크가 앞). 이 값들이 바뀌면 벡터를 다시 만들어야 하므로 `tag()` 에 넣는다.

use crate::chunk::{Chunk, ChunkParams, BODY_CHUNK};
use std::ops::Range;

/// 문서 카드 상한(토큰). 결정 #27 — M2 평가(Task 16)가 128 로 바꿀지 정한다.
pub const CARD_TOKENS: usize = 256;
/// 데이터 형식 앞부분(본문 ②) 청크 수
pub const DATA_HEAD: usize = 20;
/// 공정 스케줄링 조각(스펙 §6.6)
pub const PIECE: usize = 50;
/// 표 데이터(xlsx·ods·csv) 문서당 청크 상한(스펙 §6.6)
pub const TABLE_CAP: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FormatClass {
    Document,
    Data,
}

/// 확장자 → 묶음. 모르는 확장자는 문서 형식으로 본다.
pub fn class_of_ext(ext: &str) -> FormatClass {
    match ext.to_ascii_lowercase().as_str() {
        "txt" | "csv" | "xlsx" | "ods" => FormatClass::Data,
        _ => FormatClass::Document,
    }
}

/// 형식별 본문 청크 설정(크기는 M0 결정, 상한은 표 200 · 나머지 2,000).
pub fn chunk_params(ext: &str) -> ChunkParams {
    let max_chunks = match ext.to_ascii_lowercase().as_str() {
        "xlsx" | "ods" | "csv" => TABLE_CAP,
        _ => BODY_CHUNK.max_chunks,
    };
    ChunkParams { target_tokens: BODY_CHUNK.target_tokens, max_tokens: BODY_CHUNK.max_tokens, max_chunks }
}

/// 임베딩할 순서: 소제목으로 시작하는 청크 먼저(각 묶음 안에서는 원래 순서).
pub fn priority_order(chunks: &[Chunk]) -> Vec<usize> {
    let mut v: Vec<usize> = (0..chunks.len()).collect();
    v.sort_by_key(|&i| !chunks[i].heading);
    v
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardDoc {
    pub id: i64,
    pub class: FormatClass,
    pub mtime_ns: i64,
}

/// 문서 형식 먼저, 그 안에서 최근 수정 먼저(같으면 id).
pub fn card_order(v: &mut [CardDoc]) {
    v.sort_by(|a, b| a.class.cmp(&b.class).then(b.mtime_ns.cmp(&a.mtime_ns)).then(a.id.cmp(&b.id)));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyDoc {
    pub id: i64,
    pub class: FormatClass,
    pub mtime_ns: i64,
    /// 상한을 적용한 청크 수
    pub total: usize,
}

fn rounds(out: &mut Vec<(i64, Range<usize>)>, docs: &[&BodyDoc], from: usize) {
    let mut r = 0;
    loop {
        let mut any = false;
        for d in docs {
            let a = from + r * PIECE;
            if a < d.total {
                out.push((d.id, a..(a + PIECE).min(d.total)));
                any = true;
            }
        }
        if !any {
            break;
        }
        r += 1;
    }
}

/// 본문 임베딩 순서: (내용 id, 위치 범위). 이미 한 위치는 부르는 쪽이 건너뛴다.
pub fn schedule(docs: &[BodyDoc], data_tail: bool) -> Vec<(i64, Range<usize>)> {
    let mut sorted: Vec<&BodyDoc> = docs.iter().collect();
    sorted.sort_by(|a, b| b.mtime_ns.cmp(&a.mtime_ns).then(a.id.cmp(&b.id)));
    let (doc, data): (Vec<&BodyDoc>, Vec<&BodyDoc>) = sorted.into_iter().partition(|d| d.class == FormatClass::Document);
    let mut out = Vec::new();
    rounds(&mut out, &doc, 0);
    for d in &data {
        if d.total > 0 {
            out.push((d.id, 0..d.total.min(DATA_HEAD)));
        }
    }
    if data_tail {
        rounds(&mut out, &data, DATA_HEAD);
    }
    out
}

/// 벡터 내용을 바꾸는 값들 — 임베딩 지문에 넣는다(DEC-047).
pub fn tag() -> String {
    format!(
        "card={CARD_TOKENS};card_v=2;chunk={}/{};doc_cap={};table_cap={TABLE_CAP};head={DATA_HEAD};order=heading-first",
        BODY_CHUNK.target_tokens, BODY_CHUNK.max_tokens, BODY_CHUNK.max_chunks
    )
}
```
(`rounds` 는 첫 조각이 끝 위치를 넘는 문서를 건너뛰므로 `total` 0 인 문서는 나오지 않는다.)

- [ ] **Step 6: KI-003 — llama.cpp 로그를 끈다**

`crates/embed/src/engine.rs` 의 `init_backend` 안, `llama_backend_init();` 바로 앞에:
```rust
            // llama.cpp 로그(모델 경로 포함)를 끈다(KI-003). 문제를 볼 때는 KRA_LLAMA_LOG=1.
            if std::env::var_os("KRA_LLAMA_LOG").is_none() {
                llama_log_set(Some(quiet_log), std::ptr::null_mut());
            }
```
파일 아래쪽(함수 밖)에:
```rust
unsafe extern "C" fn quiet_log(_level: ggml_log_level, _text: *const c_char, _user: *mut std::ffi::c_void) {}
```
`llama_log_set`·`ggml_log_level` 은 `llama_sys::*` 에 있다(bindgen `llama_.*` 와 그 의존 타입). 확인: `cargo test -p embed --release --test model -- --include-ignored --test-threads 1 2>&1 | grep -c "llama_model_loader\|load_tensors"` 가 0.

- [ ] **Step 7: 시험이 통과하는지 본다**

Run: `cargo test -p embed --release --lib && cargo test -p diag --release`
Expected: PASS(진단의 청크 통계 시험도 그대로 — `build_chunks` 결과가 바뀌지 않았다).

- [ ] **Step 8: 커밋**

```bash
git add crates/embed
git commit -m "embed: Embed trait + fake embedder, chunk heading marks, 3-tier body schedule, windowed cut_to (KI-001), quiet llama log (KI-003)"
```

---

### Task 10: `platform::cpu` — 성능 코어 수와 임베딩 스레드 기본값

**Files:**
- Create: `crates/platform/src/cpu.rs`
- Modify: `crates/platform/src/lib.rs`, `crates/platform/Cargo.toml`(Windows 기능 `Win32_System_SystemInformation`), `crates/diag/src/env.rs`, `crates/diag/src/env_windows.rs`, `crates/diag/Cargo.toml`(`platform` 의존)

**Interfaces:**
- Produces: `platform::cpu::core_split() -> Option<(usize, usize)>`(Windows: 성능·효율 코어, 그 밖 None), `platform::cpu::default_embed_threads() -> usize`(= min(4, 성능 코어 수), 판별 못 하면 논리 코어 수 기준, 1~4).

- [ ] **Step 1: 시험을 쓴다**

`crates/platform/src/cpu.rs`(시험만 먼저):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_policy() {
        assert_eq!(threads_for(Some(8), 24), 4, "i7-13700: 성능 8 → 4(results.md §5.2)");
        assert_eq!(threads_for(Some(2), 12), 2, "성능 2 + 효율 8 노트북");
        assert_eq!(threads_for(None, 1), 1);
        assert_eq!(threads_for(None, 12), 4);
        assert!((1..=4).contains(&default_embed_threads()));
    }

    #[cfg(windows)]
    #[test]
    fn core_split_is_known_on_windows() {
        let (perf, eff) = core_split().expect("GetLogicalProcessorInformationEx");
        assert!(perf >= 1);
        assert!(perf + eff <= std::thread::available_parallelism().unwrap().get());
    }
}
```

- [ ] **Step 2: 시험이 실패하는지 본다**

`crates/platform/src/lib.rs` 에 `pub mod cpu;` 를 더하고:
Run: `cargo test -p platform --release`
Expected: FAIL — `cannot find function threads_for`.

- [ ] **Step 3: 구현한다**

`crates/platform/src/cpu.rs` 시험 위에(Windows `core_split` 은 `crates/diag/src/env_windows.rs` 의 것을 옮긴 것 — 진단 쪽 사본은 Step 3 끝에서 지운다):
```rust
//! CPU 코어 정보. 임베딩 스레드 기본값 = min(4, 성능 코어 수)(DEC-050, results.md §5.2: Q8 4스레드가 최고의 97%,
//! 성능 코어 수(8)부터 느려지고 논리 스레드 전부는 7~12배 느림). 진단 도구도 같은 판별을 쓴다.

/// (성능 코어 수, 효율 코어 수). 하이브리드가 아니면 효율 0. Windows 에서만 잰다.
#[cfg(windows)]
pub fn core_split() -> Option<(usize, usize)> {
    use windows::Win32::System::SystemInformation::{GetLogicalProcessorInformationEx, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX};
    let mut len = 0u32;
    // 첫 호출은 필요한 크기만 알려 주고 실패한다(ERROR_INSUFFICIENT_BUFFER).
    let _ = unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, None, &mut len) };
    if len == 0 {
        return None;
    }
    // 구조체 정렬을 지키려고 u64 버퍼를 쓴다. 항목 크기(Size)는 공용체 전체 크기보다 작을 수 있어,
    // 마지막 항목을 구조체로 읽어도 버퍼 안에 있도록 구조체 하나만큼 더 잡는다.
    let entry = std::mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>();
    let mut buf = vec![0u64; (len as usize + entry).div_ceil(8)];
    unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, Some(buf.as_mut_ptr().cast()), &mut len) }.ok()?;
    let base = buf.as_ptr().cast::<u8>();
    let mut classes = Vec::new();
    let mut off = 0usize;
    while off + 8 <= len as usize {
        let info = unsafe { &*base.add(off).cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>() };
        if info.Size == 0 {
            break;
        }
        if info.Relationship == RelationProcessorCore {
            classes.push(unsafe { info.Anonymous.Processor.EfficiencyClass });
        }
        off += info.Size as usize;
    }
    let top = *classes.iter().max()?;
    let perf = classes.iter().filter(|&&c| c == top).count();
    Some((perf, classes.len() - perf))
}

#[cfg(not(windows))]
pub fn core_split() -> Option<(usize, usize)> {
    None
}

fn threads_for(perf: Option<usize>, logical: usize) -> usize {
    perf.unwrap_or(logical).clamp(1, 4)
}

pub fn default_embed_threads() -> usize {
    threads_for(core_split().map(|c| c.0), std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1))
}
```
`crates/platform/Cargo.toml` Windows 기능 목록에 `"Win32_System_SystemInformation"` 을 더한다.

진단 쪽: `crates/diag/Cargo.toml` `[dependencies]` 에 `platform = { path = "../platform" }`. `crates/diag/src/env.rs` 의 Windows `core_split` 본문을 `platform::cpu::core_split()` 로 바꾸고(비Windows 판은 그대로 None), `crates/diag/src/env_windows.rs` 에서 `core_split` 함수와 그것만 쓰던 `use` 를 지운다.

- [ ] **Step 4: 시험이 통과하는지 본다**

Run: `cargo test -p platform -p diag --release`
Expected: PASS. (Windows 판 컴파일은 CI 가 확인한다 — 로컬이 macOS 면 `cargo check --target x86_64-pc-windows-msvc -p platform` 는 생략한다.)

- [ ] **Step 5: 커밋**

```bash
git add crates/platform crates/diag Cargo.lock
git commit -m "platform: performance-core split moved from diag, embed thread default min(4, P cores) (DEC-050)"
```

---

### Task 11: meta.db 스키마 v2 — 이전과 임베딩 테이블

**Files:**
- Modify: `crates/index/src/meta.rs`

**Interfaces:**
- Produces(Task 12·13·14·18):
  - `index::meta::VecKind { Cards, Chunks }`.
  - `EmbedTodo { id: i64, path: String, mtime_ns: i64 }` — `path`·`mtime_ns` 는 가장 최근 경로.
  - `BodyRow { id: i64, total: usize, data: bool, mtime_ns: i64, path: String }`.
  - `EmbedCounts { cards_done, cards_total, main_done, main_total, tail_done, tail_total, unplanned }`(u64).
  - `Meta::card_todo() -> Result<Vec<EmbedTodo>>`(키워드 반영(Indexed)·본문 있음·카드 행 없음), `set_cards(&mut self, &[(i64, Option<u64>)], cards_len: u64)`, `body_todo() -> Result<Vec<EmbedTodo>>`, `set_bodies(&mut self, &[(i64, usize, bool)])`, `bodies() -> Result<Vec<BodyRow>>`(경로 있는 내용만), `chunk_positions(i64) -> Result<HashSet<usize>>`, `add_chunks(&mut self, &[(i64, usize, Option<u64>)], chunks_len: u64)`, `live_slots(VecKind) -> Result<Vec<(u64, i64)>>`, `vector_snapshot(VecKind) -> Result<(u64 gen, u64 len, Vec<(u64, i64)>)>`(한 읽기 트랜잭션), `chunked_contents() -> Result<HashSet<i64>>`, `newest_mtimes() -> Result<HashMap<i64, i64>>`, `kv(&str) -> Result<Option<String>>`, `set_kv(&mut self, &str, &str)`, `clear_vectors(&mut self)`, `remap_slots(&mut self, VecKind, &HashMap<u64, u64>, gen: u64, len: u64)`, `embed_counts(data_head: usize) -> Result<EmbedCounts>`.
  - `Meta::check(&self) -> Result<()>` — `PRAGMA quick_check` 로 모든 쪽을 읽어 본다. 손상이면 `WrongKeyOrCorrupt`(DEC-056).
  - 바뀐 동작: `retry_content` 는 그 내용의 card·body·chunk 행을 지운다(DEC-048). 내용 행이 지워지면(고아 정리) 외래 키로 함께 지워진다.
  - kv 키 이름: `vec_fp`, `cards_gen`, `cards_len`, `chunks_gen`, `chunks_len`, `model_hash`, `data_tail`, `stat_*`.

- [ ] **Step 1: 시험을 쓴다**

`meta.rs` 의 `mod tests` 에서 `other_schema_version_means_rebuild_before_schema_is_applied` 의 `'2'` 를 `'99'` 로 바꾸고(이제 2 는 현재 버전), 다음 시험들을 더한다:
```rust
    fn indexed(m: &mut Meta, hash: u128, path: &str, mtime: i64) -> i64 {
        let id = m.insert_content(hash, &data(path)).unwrap();
        m.upsert_file(path, 1, mtime, id).unwrap();
        m.finish_flush(&[id], &[], 1).unwrap();
        id
    }

    /// DEC-054: v1 색인은 다시 추출하지 않고 v2 로 이전한다.
    #[test]
    fn v1_database_migrates_without_losing_contents() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("meta.db");
        let key = [9u8; 32];
        {
            let mut m = Meta::open(&p, &key).unwrap();
            indexed(&mut m, 1, "/d/a.hwp", 10);
            m.conn.execute_batch("DROP TABLE chunk; DROP TABLE body; DROP TABLE card; UPDATE kv SET value = '1' WHERE key = 'schema';").unwrap();
        }
        let m = Meta::open(&p, &key).unwrap();
        assert_eq!(m.counts().unwrap().contents, 1);
        assert_eq!(m.kv("schema").unwrap().as_deref(), Some("2"));
        assert_eq!(m.card_todo().unwrap().len(), 1);
    }

    #[test]
    fn card_and_body_flow() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Meta::open(&dir.path().join("meta.db"), &[11u8; 32]).unwrap();
        let a = indexed(&mut m, 1, "/d/a.hwp", 10);
        let b = indexed(&mut m, 2, "/d/b.csv", 20);
        let pending = m.insert_content(3, &data("c")).unwrap(); // 키워드 반영 전 — 아직 대상 아님
        m.upsert_file("/d/c.hwp", 1, 30, pending).unwrap();
        let failed = m.insert_content(4, &ContentData { fail: Some("Drm".into()), title: None, distribution: false, doc: None }).unwrap();
        m.upsert_file("/d/drm.hwp", 1, 40, failed).unwrap();
        m.finish_flush(&[failed], &[], 2).unwrap();
        let todo: Vec<i64> = m.card_todo().unwrap().iter().map(|t| t.id).collect();
        assert_eq!(todo, vec![a, b], "본문 있고 키워드에 반영된 것만");
        assert_eq!(m.card_todo().unwrap()[1].path, "/d/b.csv");
        m.set_cards(&[(a, Some(0)), (b, None)], 1).unwrap();
        assert!(m.card_todo().unwrap().is_empty());
        assert_eq!(m.kv("cards_len").unwrap().as_deref(), Some("1"));
        assert_eq!(m.live_slots(VecKind::Cards).unwrap(), vec![(0, a)], "빈 카드(None)는 슬롯 없음");

        assert_eq!(m.body_todo().unwrap().len(), 2);
        m.set_bodies(&[(a, 3, false), (b, 25, true)]).unwrap();
        assert!(m.body_todo().unwrap().is_empty());
        m.add_chunks(&[(a, 0, Some(0)), (a, 1, Some(1)), (b, 0, Some(2)), (b, 21, Some(3)), (b, 1, None)], 4).unwrap();
        assert_eq!(m.chunk_positions(b).unwrap(), HashSet::from([0, 1, 21]));
        assert_eq!(m.chunked_contents().unwrap(), HashSet::from([a, b]));
        let c = m.embed_counts(20).unwrap();
        assert_eq!((c.cards_done, c.cards_total), (2, 3), "분모에는 키워드 반영 전인 c 도 든다(곧 대상)");
        assert_eq!((c.main_done, c.main_total), (4, 3 + 20), "a 2개 + b 앞부분 2개(0, 1) / a 3 + b 앞 20");
        assert_eq!((c.tail_done, c.tail_total), (1, 5));
        assert_eq!(c.unplanned, 1, "c 는 아직 청크 수를 세지 않았다");
        let rows = m.bodies().unwrap();
        assert_eq!(rows.iter().map(|r| (r.id, r.total, r.data)).collect::<Vec<_>>(), vec![(a, 3, false), (b, 25, true)]);
        assert_eq!(m.newest_mtimes().unwrap()[&b], 20);
    }

    #[test]
    fn retry_and_orphan_delete_drop_vector_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Meta::open(&dir.path().join("meta.db"), &[12u8; 32]).unwrap();
        let a = indexed(&mut m, 1, "/d/a.hwp", 10);
        m.set_cards(&[(a, Some(0))], 1).unwrap();
        m.set_bodies(&[(a, 1, false)]).unwrap();
        m.add_chunks(&[(a, 0, Some(0))], 1).unwrap();
        m.retry_content(a, &data("다시")).unwrap();
        assert!(m.live_slots(VecKind::Cards).unwrap().is_empty());
        assert!(m.live_slots(VecKind::Chunks).unwrap().is_empty());
        assert!(m.chunked_contents().unwrap().is_empty());

        m.finish_flush(&[a], &[], 2).unwrap();
        m.set_cards(&[(a, Some(1))], 2).unwrap();
        m.remove_file("/d/a.hwp").unwrap();
        m.finish_flush(&[], &[a], 3).unwrap(); // 고아 정리 → 외래 키로 card 행도
        assert!(m.live_slots(VecKind::Cards).unwrap().is_empty());
    }

    #[test]
    fn check_finds_damaged_pages() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("meta.db");
        let key = [14u8; 32];
        {
            let mut m = Meta::open(&p, &key).unwrap();
            for i in 0..200u128 {
                indexed(&mut m, i, &format!("/d/{i}.hwp"), i as i64);
            }
            m.check().unwrap();
            m.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
        }
        let mut bytes = std::fs::read(&p).unwrap();
        let last = bytes.len() / 4096 - 1;
        for b in &mut bytes[last * 4096..(last + 1) * 4096] {
            *b ^= 0x5A;
        }
        std::fs::write(&p, &bytes).unwrap();
        let r = Meta::open(&p, &key).and_then(|m| m.check());
        assert!(matches!(r, Err(MetaError::WrongKeyOrCorrupt)), "{r:?}");
    }

    #[test]
    fn remap_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = Meta::open(&dir.path().join("meta.db"), &[13u8; 32]).unwrap();
        let a = indexed(&mut m, 1, "/d/a.hwp", 10);
        let b = indexed(&mut m, 2, "/d/b.hwp", 20);
        m.set_cards(&[(a, Some(3)), (b, Some(7))], 8).unwrap();
        let map: HashMap<u64, u64> = [(3, 0), (7, 1)].into_iter().collect();
        m.remap_slots(VecKind::Cards, &map, 1, 2).unwrap();
        let mut live = m.live_slots(VecKind::Cards).unwrap();
        live.sort();
        assert_eq!(live, vec![(0, a), (1, b)]);
        assert_eq!(m.vector_snapshot(VecKind::Cards).unwrap().0, 1);
        assert_eq!(m.vector_snapshot(VecKind::Cards).unwrap().1, 2);
        m.clear_vectors().unwrap();
        assert!(m.live_slots(VecKind::Cards).unwrap().is_empty());
        assert_eq!(m.kv("cards_len").unwrap().as_deref(), Some("0"));
        assert_eq!(m.card_todo().unwrap().len(), 2);
    }
```

- [ ] **Step 2: 시험이 실패하는지 본다**

Run: `cargo test -p index --release meta`
Expected: FAIL — `no method named card_todo` 등.

- [ ] **Step 3: 구현한다**

`meta.rs`:
1. `const SCHEMA_VERSION: &str = "2";` 그리고 `SCHEMA` 끝(마지막 `CREATE INDEX … file_content …;` 뒤)에 붙인다:
```sql
CREATE TABLE IF NOT EXISTS card(
  content_id INTEGER PRIMARY KEY REFERENCES content(id) ON DELETE CASCADE,
  slot INTEGER
);
CREATE TABLE IF NOT EXISTS body(
  content_id INTEGER PRIMARY KEY REFERENCES content(id) ON DELETE CASCADE,
  total INTEGER NOT NULL,
  data INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS chunk(
  content_id INTEGER NOT NULL REFERENCES content(id) ON DELETE CASCADE,
  pos INTEGER NOT NULL,
  slot INTEGER,
  PRIMARY KEY(content_id, pos)
);
CREATE INDEX IF NOT EXISTS chunk_slot ON chunk(slot);
```
2. `open` 의 버전 검사를 바꾼다:
```rust
        match v.as_deref() {
            // v1(M1) → v2: 임베딩 테이블만 더한다 — 다시 추출하지 않는다(DEC-054).
            Some("1") => {
                conn.execute_batch(SCHEMA)?;
                conn.execute("UPDATE kv SET value = ?1 WHERE key = 'schema'", [SCHEMA_VERSION])?;
            }
            // 다른 스키마 = 다른 버전의 앱이 만든 색인 → 다시 색인(스펙 §6.8)
            Some(x) if x != SCHEMA_VERSION => return Err(MetaError::WrongKeyOrCorrupt),
            _ => {}
        }
```
3. 타입(파일 위쪽 `Counts` 아래):
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VecKind {
    Cards,
    Chunks,
}

impl VecKind {
    pub fn name(self) -> &'static str {
        match self {
            VecKind::Cards => "cards",
            VecKind::Chunks => "chunks",
        }
    }
    fn table(self) -> &'static str {
        match self {
            VecKind::Cards => "card",
            VecKind::Chunks => "chunk",
        }
    }
}

/// (세대, 길이, 살아 있는 슬롯 → 내용 id)
pub type VectorSnapshot = (u64, u64, Vec<(u64, i64)>);

#[derive(Debug, Clone, PartialEq)]
pub struct EmbedTodo {
    pub id: i64,
    pub path: String,
    pub mtime_ns: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BodyRow {
    pub id: i64,
    pub total: usize,
    pub data: bool,
    pub mtime_ns: i64,
    pub path: String,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct EmbedCounts {
    pub cards_done: u64,
    pub cards_total: u64,
    /// 본문 ①+② (화면의 "정밀 의미 검색 N%")
    pub main_done: u64,
    pub main_total: u64,
    /// 본문 ③ 데이터 형식 뒷부분
    pub tail_done: u64,
    pub tail_total: u64,
    /// 아직 청크 수를 세지 않은 내용
    pub unplanned: u64,
}

/// 가장 최근 경로(경로·수정 시각). 경로 없는 내용(고아)은 빠진다.
const NEWEST: &str = "JOIN file f ON f.path = (SELECT path FROM file WHERE content_id = c.id ORDER BY mtime_ns DESC, path LIMIT 1)";
```
4. `retry_content` 를 트랜잭션으로 바꾼다:
```rust
    pub fn retry_content(&mut self, id: i64, d: &ContentData) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE content SET state = 0, fail = ?2, title = ?3, distribution = ?4, doc = ?5, retries = retries + 1 WHERE id = ?1",
            params![id, d.fail, d.title, d.distribution as i64, Self::doc_json(d)],
        )?;
        // 문서가 바뀌었으니 의미 검색 벡터도 무효(DEC-048) — 다음 임베딩 단계가 다시 만든다.
        for t in ["card", "body", "chunk"] {
            tx.execute(&format!("DELETE FROM {t} WHERE content_id = ?1"), [id])?;
        }
        tx.commit()?;
        Ok(())
    }
```
5. 메서드(`counts` 아래):
```rust
    fn todo(&self, missing: &str) -> Result<Vec<EmbedTodo>> {
        let sql = format!(
            "SELECT c.id, f.path, f.mtime_ns FROM content c {NEWEST}
             WHERE c.state = 1 AND c.doc IS NOT NULL AND NOT EXISTS (SELECT 1 FROM {missing} x WHERE x.content_id = c.id) ORDER BY c.id"
        );
        let mut st = self.conn.prepare(&sql)?;
        let v = st.query_map([], |r| Ok(EmbedTodo { id: r.get(0)?, path: r.get(1)?, mtime_ns: r.get(2)? }))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// 카드를 만들 내용: 키워드에 반영됐고(Indexed) 본문이 있고 카드 행이 없다.
    pub fn card_todo(&self) -> Result<Vec<EmbedTodo>> {
        self.todo("card")
    }

    /// 청크 수를 세야 할 내용.
    pub fn body_todo(&self) -> Result<Vec<EmbedTodo>> {
        self.todo("body")
    }

    fn set_len(tx: &rusqlite::Transaction, key: &str, len: u64) -> rusqlite::Result<usize> {
        tx.execute("INSERT INTO kv(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, len.to_string()])
    }

    /// 카드 행(slot None = 빈 카드·실패)과 파일 길이를 한 트랜잭션에(DEC-048: 파일 fsync 뒤에 부른다).
    pub fn set_cards(&mut self, rows: &[(i64, Option<u64>)], cards_len: u64) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (id, slot) in rows {
            tx.execute("INSERT OR REPLACE INTO card(content_id, slot) VALUES(?1, ?2)", params![id, slot.map(|s| s as i64)])?;
        }
        Self::set_len(&tx, "cards_len", cards_len)?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_bodies(&mut self, rows: &[(i64, usize, bool)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (id, total, data) in rows {
            tx.execute("INSERT OR REPLACE INTO body(content_id, total, data) VALUES(?1, ?2, ?3)", params![id, *total as i64, *data as i64])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn bodies(&self) -> Result<Vec<BodyRow>> {
        let sql = format!("SELECT c.id, b.total, b.data, f.mtime_ns, f.path FROM body b JOIN content c ON c.id = b.content_id {NEWEST} ORDER BY c.id");
        let mut st = self.conn.prepare(&sql)?;
        let v = st
            .query_map([], |r| {
                Ok(BodyRow { id: r.get(0)?, total: r.get::<_, i64>(1)? as usize, data: r.get::<_, i64>(2)? != 0, mtime_ns: r.get(3)?, path: r.get(4)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn chunk_positions(&self, id: i64) -> Result<HashSet<usize>> {
        let mut st = self.conn.prepare_cached("SELECT pos FROM chunk WHERE content_id = ?1")?;
        let v = st.query_map([id], |r| r.get::<_, i64>(0))?.map(|x| x.map(|p| p as usize)).collect::<rusqlite::Result<HashSet<_>>>()?;
        Ok(v)
    }

    pub fn add_chunks(&mut self, rows: &[(i64, usize, Option<u64>)], chunks_len: u64) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (id, pos, slot) in rows {
            tx.execute("INSERT OR REPLACE INTO chunk(content_id, pos, slot) VALUES(?1, ?2, ?3)", params![id, *pos as i64, slot.map(|s| s as i64)])?;
        }
        Self::set_len(&tx, "chunks_len", chunks_len)?;
        tx.commit()?;
        Ok(())
    }

    fn live_in(conn: &Connection, kind: VecKind) -> rusqlite::Result<Vec<(u64, i64)>> {
        let sql = format!("SELECT slot, content_id FROM {} WHERE slot IS NOT NULL ORDER BY slot", kind.table());
        let mut st = conn.prepare(&sql)?;
        let v = st.query_map([], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)))?.collect();
        v
    }

    /// 살아 있는 슬롯(슬롯 → 내용 id).
    pub fn live_slots(&self, kind: VecKind) -> Result<Vec<(u64, i64)>> {
        Ok(Self::live_in(&self.conn, kind)?)
    }

    /// 검색용: 세대·길이·살아 있는 슬롯을 한 읽기 트랜잭션에서(색인 중 압축과 엇갈리지 않게).
    pub fn vector_snapshot(&self, kind: VecKind) -> Result<VectorSnapshot> {
        let tx = self.conn.unchecked_transaction()?;
        let get = |k: String| -> rusqlite::Result<u64> {
            Ok(tx.query_row("SELECT value FROM kv WHERE key = ?1", [k], |r| r.get::<_, String>(0)).optional()?.and_then(|s| s.parse().ok()).unwrap_or(0))
        };
        let gen = get(format!("{}_gen", kind.name()))?;
        let len = get(format!("{}_len", kind.name()))?;
        let live = Self::live_in(&tx, kind)?;
        tx.commit()?;
        Ok((gen, len, live))
    }

    pub fn chunked_contents(&self) -> Result<HashSet<i64>> {
        let mut st = self.conn.prepare("SELECT DISTINCT content_id FROM chunk WHERE slot IS NOT NULL")?;
        let v = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<HashSet<i64>>>()?;
        Ok(v)
    }

    /// 내용 → 가장 최근 경로의 수정 시각(문서 날짜, 스펙 §8.1 M2).
    pub fn newest_mtimes(&self) -> Result<HashMap<i64, i64>> {
        let mut st = self.conn.prepare("SELECT content_id, MAX(mtime_ns) FROM file WHERE content_id IS NOT NULL GROUP BY content_id")?;
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<HashMap<i64, i64>>>()?;
        Ok(v)
    }

    pub fn kv(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_kv(&mut self, key: &str, value: &str) -> Result<()> {
        self.conn.execute("INSERT INTO kv(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [key, value])?;
        Ok(())
    }

    /// 벡터 행을 모두 지운다(지문이 바뀌었거나 벡터 파일이 깨짐). 길이·세대는 0.
    pub fn clear_vectors(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch("DELETE FROM chunk; DELETE FROM body; DELETE FROM card;")?;
        for k in ["cards_len", "chunks_len", "cards_gen", "chunks_gen"] {
            Self::set_len(&tx, k, 0)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 압축 뒤 슬롯 번호를 바꾸고 새 세대·길이를 기록한다(옛 번호와 새 번호가 겹치지 않게 음수를 거친다).
    pub fn remap_slots(&mut self, kind: VecKind, map: &HashMap<u64, u64>, gen: u64, len: u64) -> Result<()> {
        let t = kind.table();
        let tx = self.conn.transaction()?;
        for (old, new) in map {
            tx.execute(&format!("UPDATE {t} SET slot = ?1 WHERE slot = ?2"), params![-1 - *new as i64, *old as i64])?;
        }
        tx.execute(&format!("UPDATE {t} SET slot = -1 - slot WHERE slot < 0"), [])?;
        Self::set_len(&tx, &format!("{}_gen", kind.name()), gen)?;
        Self::set_len(&tx, &format!("{}_len", kind.name()), len)?;
        tx.commit()?;
        Ok(())
    }

    /// 모든 쪽을 읽어 본다(DEC-056). 검색은 이제 표 전체(수정일·청크 여부)를 읽으므로, index 가 끝날 때 여기서
    /// 손상을 찾아 한 번 처음부터 다시 만든다 — 그러지 않으면 index 가 읽지 않는 쪽의 손상 때문에 검색만 계속 실패한다.
    pub fn check(&self) -> Result<()> {
        let first: String = self.conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if first == "ok" {
            Ok(())
        } else {
            Err(MetaError::WrongKeyOrCorrupt)
        }
    }

    pub fn embed_counts(&self, data_head: usize) -> Result<EmbedCounts> {
        let one = |sql: &str| -> Result<u64> { Ok(self.conn.query_row(sql, [data_head as i64], |r| r.get::<_, Option<i64>>(0))?.unwrap_or(0) as u64) };
        let has_path = "EXISTS (SELECT 1 FROM file f WHERE f.content_id = c.id)";
        Ok(EmbedCounts {
            cards_done: one(&format!("SELECT count(*) FROM card k JOIN content c ON c.id = k.content_id WHERE ?1 >= 0 AND {has_path}"))?,
            cards_total: one(&format!("SELECT count(*) FROM content c WHERE c.doc IS NOT NULL AND ?1 >= 0 AND {has_path}"))?,
            main_done: one("SELECT count(*) FROM chunk k JOIN body b ON b.content_id = k.content_id WHERE b.data = 0 OR k.pos < ?1")?,
            main_total: one("SELECT SUM(CASE WHEN data = 1 THEN MIN(total, ?1) ELSE total END) FROM body")?,
            tail_done: one("SELECT count(*) FROM chunk k JOIN body b ON b.content_id = k.content_id WHERE b.data = 1 AND k.pos >= ?1")?,
            tail_total: one("SELECT SUM(CASE WHEN data = 1 AND total > ?1 THEN total - ?1 ELSE 0 END) FROM body")?,
            unplanned: one(&format!("SELECT count(*) FROM content c WHERE c.doc IS NOT NULL AND ?1 >= 0 AND {has_path} AND NOT EXISTS (SELECT 1 FROM body b WHERE b.content_id = c.id)"))?,
        })
    }
```
6. 파일 위 `use std::collections::HashMap;` 을 `use std::collections::{HashMap, HashSet};` 로.

- [ ] **Step 4: 시험이 통과하는지 본다**

Run: `cargo test -p index --release meta && cargo test -p app --release`
Expected: PASS(앱 시험은 v2 로 새로 만든 DB 를 쓰므로 그대로 통과).

- [ ] **Step 5: 커밋**

```bash
git add crates/index
git commit -m "index: meta schema v2 with migration (DEC-054), card/body/chunk tables and embedding queries"
```

---
### Task 12: `app::vectors` — 임베딩 지문, 벡터 파일 열기·복구·압축, 검색용 읽기

**Files:**
- Create: `crates/app/src/vectors.rs`
- Modify: `crates/app/src/lib.rs`, `crates/app/src/store.rs`(`reset_store` 가 벡터 폴더도 지움), `crates/platform/src/paths.rs`(`vectors_dir`), `crates/app/Cargo.toml`

**Interfaces:**
- Consumes: Task 6 `VecFile`·`read_each`, Task 8 `VectorSet`, Task 9 `plan::tag`, Task 11 meta API.
- Produces (Task 13·14·17·18):
  - `app::vectors::{LAYOUT, file_path, fingerprint, model_hash, VecStore, open_for_write, compact_if_needed, Sets, SetsState, load_sets}`.
  - `fingerprint(model_hash: u128, profile: &ModelProfile) -> String`(32자리 16진수).
  - `model_hash(cached: Option<&str>, path: &Path) -> io::Result<(u128, String)>` — 둘째 값은 kv `model_hash` 에 둘 `"크기:수정시각:해시"`.
  - `VecStore { dir: PathBuf, cards: VecFile, chunks: VecFile }`, `VecStore::file(&mut self, VecKind) -> &mut VecFile`.
  - `open_for_write(&Home, &mut Meta, fp: &str) -> Result<VecStore, IndexError>`, `compact_if_needed(&mut Meta, &mut VecStore) -> Result<(), IndexError>`.
  - `Sets { cards: VectorSet, chunks: VectorSet, state: SetsState }`, `SetsState { Ready, Empty, ModelChanged, Unreadable }`, `load_sets(&Home, &Meta, fp_now: Option<&str>) -> Result<Sets, String>`.
  - `platform::paths::Home::vectors_dir()` = `data/vectors`.

- [ ] **Step 1: 의존성과 경로**

`crates/app/Cargo.toml` `[dependencies]` 에 더한다:
```toml
embed = { path = "../embed" }
query = { path = "../query" }
vector = { path = "../vector" }
doc = { path = "../doc" }
serde = { workspace = true }
serde_json.workspace = true
xxhash-rust = { version = "0.8", features = ["xxh3"] }
toml = "0.9"
```
(`serde_json` 이 `[dev-dependencies]` 에도 있으면 그 줄은 지운다.) `crates/platform/src/paths.rs` 의 `keyword_dir` 아래에:
```rust
    /// 의미 검색 벡터(임베딩 지문별 폴더, DEC-047)
    pub fn vectors_dir(&self) -> PathBuf {
        self.data().join("vectors")
    }
```
`layout_is_under_root` 시험에 `assert!(h.vectors_dir().starts_with(h.data()));` 를 더한다.

- [ ] **Step 2: 시험을 쓴다**

`crates/app/src/vectors.rs`(시험만 먼저):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use index::meta::{ContentData, Meta};
    use vector::record::Record;

    fn setup() -> (tempfile::TempDir, Home, Meta, i64, i64) {
        let d = tempfile::tempdir().unwrap();
        let home = Home::new(d.path());
        let mut m = Meta::open(&home.meta_db(), &[1u8; 32]).unwrap();
        let mut ids = Vec::new();
        for (h, p) in [(1u128, "/d/a.hwp"), (2, "/d/b.hwp")] {
            let doc = doc::Document { title: Some(p.into()), blocks: vec![] };
            let id = m.insert_content(h, &ContentData { fail: None, title: None, distribution: false, doc: Some(doc) }).unwrap();
            m.upsert_file(p, 1, 1, id).unwrap();
            m.finish_flush(&[id], &[], 1).unwrap();
            ids.push(id);
        }
        (d, home, m, ids[0], ids[1])
    }

    fn rec(x: f32) -> Record {
        let mut v = vec![0.01f32; 768];
        v[0] = x;
        Record::from_full(&v, LAYOUT).unwrap()
    }

    #[test]
    fn fingerprint_changes_with_model_or_profile() {
        let p = ModelProfile::embeddinggemma2();
        let a = fingerprint(1, &p);
        assert_eq!(a.len(), 32);
        assert_eq!(a, fingerprint(1, &p));
        assert_ne!(a, fingerprint(2, &p));
        let mut q = p.clone();
        q.query_template = "query: {text}".into();
        assert_ne!(a, fingerprint(1, &q));
    }

    #[test]
    fn model_hash_uses_cache_when_size_and_time_match() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("m.gguf");
        std::fs::write(&f, b"model bytes").unwrap();
        let (h, cache) = model_hash(None, &f).unwrap();
        let (h2, cache2) = model_hash(Some(&cache), &f).unwrap();
        assert_eq!((h, &cache), (h2, &cache2));
        let fake = format!("{}:{:032x}", cache.rsplit_once(':').unwrap().0, 7u128);
        assert_eq!(model_hash(Some(&fake), &f).unwrap().0, 7, "크기·시각이 같으면 파일을 다시 읽지 않는다");
    }

    #[test]
    fn open_new_then_reopen_and_fingerprint_change_clears() {
        let (_d, home, mut m, a, b) = setup();
        let mut vs = open_for_write(&home, &mut m, "fp1").unwrap();
        let first = vs.cards.append(&[rec(1.0), rec(2.0)]).unwrap();
        m.set_cards(&[(a, Some(first)), (b, Some(first + 1))], vs.cards.len()).unwrap();
        drop(vs);
        let vs = open_for_write(&home, &mut m, "fp1").unwrap();
        assert_eq!(vs.cards.len(), 2);
        drop(vs);
        let vs = open_for_write(&home, &mut m, "fp2").unwrap();
        assert_eq!(vs.cards.len(), 0);
        assert!(m.live_slots(VecKind::Cards).unwrap().is_empty());
        assert!(!home.vectors_dir().join("fp1").exists(), "옛 지문 폴더는 지운다");
    }

    #[test]
    fn damaged_file_rebuilds_vectors_only() {
        let (_d, home, mut m, a, _b) = setup();
        let mut vs = open_for_write(&home, &mut m, "fp").unwrap();
        vs.cards.append(&[rec(1.0)]).unwrap();
        m.set_cards(&[(a, Some(0))], 1).unwrap();
        drop(vs);
        std::fs::write(file_path(&home.vectors_dir().join("fp"), VecKind::Cards, 0), b"broken").unwrap();
        let vs = open_for_write(&home, &mut m, "fp").unwrap();
        assert_eq!(vs.cards.len(), 0);
        assert_eq!(m.card_todo().unwrap().len(), 2, "카드를 다시 만든다");
        assert_eq!(m.counts().unwrap().contents, 2, "meta 내용은 그대로");
    }

    /// Review Focus 5: index 가 파일에 덧붙였지만 meta 에 아직 기록하지 않은 꼬리는 검색이 읽지 않는다.
    #[test]
    fn reader_ignores_unrecorded_tail() {
        let (_d, home, mut m, a, b) = setup();
        let mut vs = open_for_write(&home, &mut m, "fp").unwrap();
        vs.cards.append(&[rec(1.0)]).unwrap();
        m.set_cards(&[(a, Some(0))], 1).unwrap();
        vs.cards.append(&[rec(2.0)]).unwrap(); // b 몫 — meta 기록 전
        let s = load_sets(&home, &m, Some("fp")).unwrap();
        assert_eq!(s.state, SetsState::Ready);
        assert_eq!(s.cards.owners, vec![a]);
        let _ = b;
    }

    #[test]
    fn model_change_and_missing_files_are_reported() {
        let (_d, home, mut m, a, _b) = setup();
        assert_eq!(load_sets(&home, &m, None).unwrap().state, SetsState::Empty);
        let mut vs = open_for_write(&home, &mut m, "fp").unwrap();
        vs.cards.append(&[rec(1.0)]).unwrap();
        m.set_cards(&[(a, Some(0))], 1).unwrap();
        drop(vs);
        assert_eq!(load_sets(&home, &m, Some("other")).unwrap().state, SetsState::ModelChanged);
        std::fs::remove_file(file_path(&home.vectors_dir().join("fp"), VecKind::Cards, 0)).unwrap();
        assert_eq!(load_sets(&home, &m, Some("fp")).unwrap().state, SetsState::Unreadable);
    }

    /// 다른 프로세스(검색)가 옛 세대 파일을 연 채로 압축해도 실패하지 않고, 옛 파일은 늦어도 다음 실행에서 지운다
    /// (Windows 는 열린 파일을 바꿔치기할 수 없어 세대 파일을 쓴다, DEC-048 — CI 의 windows-latest 에서도 돈다).
    #[test]
    fn compaction_while_old_generation_is_open() {
        let (_d, home, mut m, a, b) = setup();
        let mut vs = open_for_write(&home, &mut m, "fp").unwrap();
        vs.cards.append(&[rec(1.0), rec(2.0), rec(3.0), rec(4.0)]).unwrap();
        m.set_cards(&[(a, Some(1)), (b, Some(3))], 4).unwrap();
        let old = file_path(&home.vectors_dir().join("fp"), VecKind::Cards, 0);
        let reader = std::fs::File::open(&old).unwrap();
        compact_if_needed(&mut m, &mut vs).unwrap();
        assert_eq!(vs.cards.len(), 2);
        assert_eq!(load_sets(&home, &m, Some("fp")).unwrap().cards.owners, vec![a, b], "새 세대로 읽는다");
        drop(reader);
        drop(vs);
        let _vs = open_for_write(&home, &mut m, "fp").unwrap();
        assert!(!old.exists(), "옛 세대는 지운다");
    }

    #[test]
    fn compaction_keeps_live_records_and_owners() {
        let (_d, home, mut m, a, b) = setup();
        let mut vs = open_for_write(&home, &mut m, "fp").unwrap();
        vs.cards.append(&[rec(1.0), rec(2.0), rec(3.0), rec(4.0)]).unwrap();
        m.set_cards(&[(a, Some(1)), (b, Some(3))], 4).unwrap(); // 0·2 는 죽은 슬롯(50%)
        compact_if_needed(&mut m, &mut vs).unwrap();
        assert_eq!(vs.cards.len(), 2);
        let mut live = m.live_slots(VecKind::Cards).unwrap();
        live.sort();
        assert_eq!(live, vec![(0, a), (1, b)]);
        let s = load_sets(&home, &m, Some("fp")).unwrap();
        assert_eq!(s.cards.owners, vec![a, b]);
        assert_eq!(s.cards.index.search_exact(&{ let mut v = vec![0.01f32; 768]; v[0] = 4.0; v }, 1)[0].0, 1, "b 의 레코드(4.0)가 슬롯 1 로 옮겨짐");
        drop(vs);
        let vs = open_for_write(&home, &mut m, "fp").unwrap();
        assert_eq!(vs.cards.len(), 2, "새 세대로 다시 열린다");
        assert!(!file_path(&home.vectors_dir().join("fp"), VecKind::Cards, 0).exists(), "옛 세대는 지운다");
    }
}
```

- [ ] **Step 3: 시험이 실패하는지 본다**

`crates/app/src/lib.rs` 에 `pub mod vectors;` 를 더하고:
Run: `cargo test -p app --release --lib vectors`
Expected: FAIL — `cannot find function fingerprint` 등.

- [ ] **Step 4: 구현한다**

`crates/app/src/vectors.rs` 시험 위에:
```rust
//! 의미 검색 벡터 파일 관리(DEC-047·048): 임베딩 지문 폴더, 세대 파일, meta 기준 정합성, 압축, 검색용 읽기.
//! 쓰는 쪽(index)은 파일에 덧붙이고 fsync 한 뒤 meta 에 길이·슬롯을 기록한다. 읽는 쪽(search)은 meta 가 아는 만큼만 읽는다.

use crate::pipeline::IndexError;
use crate::store::user_error;
use embed::profile::ModelProfile;
use index::hybrid::VectorSet;
use index::meta::{Meta, VecKind};
use platform::paths::Home;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use vector::record::Layout;
use vector::store::{read_each, VecFile};

pub const LAYOUT: Layout = Layout::M0;
/// 죽은 슬롯이 이 비율을 넘으면 실행 끝에 압축한다.
const DEAD_RATIO: f64 = 0.3;
const KINDS: [VecKind; 2] = [VecKind::Cards, VecKind::Chunks];

pub fn file_path(dir: &Path, k: VecKind, gen: u64) -> PathBuf {
    dir.join(format!("{}.{gen}.vec", k.name()))
}

/// 임베딩 지문(DEC-047): 모델 파일 + 모델 프로필 + 카드·청크·순서 설정(`plan::tag`) + 레코드 형식.
pub fn fingerprint(model_hash: u128, profile: &ModelProfile) -> String {
    let s = format!(
        "model={model_hash:032x}\nprofile={}\nplan={}\nlayout={}x{}\nrecord=1",
        serde_json::to_string(profile).expect("프로필 직렬화"),
        embed::plan::tag(),
        LAYOUT.dim,
        LAYOUT.bit_dim
    );
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(s.as_bytes()))
}

/// 모델 파일 해시(xxh3-128). kv 에 둔 "크기:수정시각:해시"의 앞부분이 같으면 다시 읽지 않는다(모델 300MB).
pub fn model_hash(cached: Option<&str>, path: &Path) -> std::io::Result<(u128, String)> {
    let m = std::fs::metadata(path)?;
    let mtime = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    let key = format!("{}:{}", m.len(), mtime);
    if let Some((k, h)) = cached.and_then(|c| c.rsplit_once(':')) {
        if k == key {
            if let Ok(v) = u128::from_str_radix(h, 16) {
                return Ok((v, format!("{key}:{v:032x}")));
            }
        }
    }
    let h = crawler::hash_file(path)?;
    Ok((h, format!("{key}:{h:032x}")))
}

fn io_err(e: impl std::fmt::Display) -> IndexError {
    IndexError::Other(format!("의미 검색 색인 파일 오류: {e}"))
}

fn kv_u64(meta: &Meta, key: &str) -> Result<u64, IndexError> {
    Ok(meta.kv(key)?.and_then(|s| s.parse().ok()).unwrap_or(0))
}

pub struct VecStore {
    pub dir: PathBuf,
    pub cards: VecFile,
    pub chunks: VecFile,
}

impl VecStore {
    pub fn file(&mut self, k: VecKind) -> &mut VecFile {
        match k {
            VecKind::Cards => &mut self.cards,
            VecKind::Chunks => &mut self.chunks,
        }
    }
}

/// 다른 지문 폴더와, 지금 폴더의 옛 세대 파일을 지운다. 못 지우면(Windows 에서 검색 중인 다른 프로세스) 다음 실행에서.
fn remove_stale(root: &Path, dir: &Path, gens: &[(VecKind, u64)]) {
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            if e.path() != dir {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let keep = gens.iter().any(|(k, g)| e.path() == file_path(dir, *k, *g));
            if !keep {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// 색인용으로 연다. 지문이 다르거나 파일이 짧거나 형식이 다르면 벡터만 비우고 새로 만든다(키워드·meta 내용은 그대로).
pub fn open_for_write(home: &Home, meta: &mut Meta, fp: &str) -> Result<VecStore, IndexError> {
    let root = home.vectors_dir();
    let dir = root.join(fp);
    let prev = meta.kv("vec_fp")?;
    if prev.as_deref() == Some(fp) {
        let mut gens = Vec::new();
        let mut files = Vec::new();
        for k in KINDS {
            let (g, len) = (kv_u64(meta, &format!("{}_gen", k.name()))?, kv_u64(meta, &format!("{}_len", k.name()))?);
            gens.push((k, g));
            files.push(VecFile::open(&file_path(&dir, k, g), LAYOUT, len));
        }
        let chunks = files.pop().expect("2개");
        let cards = files.pop().expect("2개");
        match (cards, chunks) {
            (Ok(cards), Ok(chunks)) => {
                remove_stale(&root, &dir, &gens);
                return Ok(VecStore { dir, cards, chunks });
            }
            _ => eprintln!("의미 검색 색인이 손상되어 다시 만듭니다."),
        }
    } else if prev.is_some() {
        eprintln!("검색 모델이나 설정이 바뀌어 의미 검색 색인을 다시 만듭니다.");
    }
    meta.clear_vectors()?;
    std::fs::create_dir_all(&dir).map_err(io_err)?;
    let cards = VecFile::create(&file_path(&dir, VecKind::Cards, 0), LAYOUT).map_err(io_err)?;
    let chunks = VecFile::create(&file_path(&dir, VecKind::Chunks, 0), LAYOUT).map_err(io_err)?;
    meta.set_kv("vec_fp", fp)?;
    remove_stale(&root, &dir, &[(VecKind::Cards, 0), (VecKind::Chunks, 0)]);
    Ok(VecStore { dir, cards, chunks })
}

/// 죽은 슬롯이 많으면 살아 있는 레코드만 새 세대 파일로 옮긴다(슬롯 번호를 다시 매김).
pub fn compact_if_needed(meta: &mut Meta, store: &mut VecStore) -> Result<(), IndexError> {
    for k in KINDS {
        let live = meta.live_slots(k)?;
        let len = store.file(k).len();
        if len == 0 || (len as usize - live.len()) as f64 / len as f64 <= DEAD_RATIO {
            continue;
        }
        let gen = kv_u64(meta, &format!("{}_gen", k.name()))?;
        let (old, new) = (file_path(&store.dir, k, gen), file_path(&store.dir, k, gen + 1));
        let mut out = VecFile::create(&new, LAYOUT).map_err(io_err)?;
        let owners: HashMap<u64, i64> = live.into_iter().collect();
        let mut map: HashMap<u64, u64> = HashMap::new();
        let mut batch = Vec::new();
        read_each(&old, LAYOUT, len, |slot, rec| {
            if owners.contains_key(&slot) {
                let next = map.len() as u64;
                map.insert(slot, next);
                batch.push(rec);
                if batch.len() == 4096 {
                    out.append(&batch)?;
                    batch.clear();
                }
            }
            Ok(())
        })
        .map_err(io_err)?;
        out.append(&batch).map_err(io_err)?;
        meta.remap_slots(k, &map, gen + 1, out.len())?;
        *store.file(k) = out;
        let _ = std::fs::remove_file(&old);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetsState {
    Ready,
    /// 의미 검색 색인이 아직 없음
    Empty,
    /// 지금 모델·설정의 지문과 색인 지문이 다름 — index 를 다시 해야 함
    ModelChanged,
    /// 파일을 읽지 못함(유실·손상)
    Unreadable,
}

pub struct Sets {
    pub cards: VectorSet,
    pub chunks: VectorSet,
    pub state: SetsState,
}

/// 검색용 읽기. `fp_now` 가 있으면 색인 지문과 비교한다(다르면 섞지 않고 빈 집합 — Review Focus 3).
pub fn load_sets(home: &Home, meta: &Meta, fp_now: Option<&str>) -> Result<Sets, String> {
    let empty = |state| Sets { cards: VectorSet::new(LAYOUT), chunks: VectorSet::new(LAYOUT), state };
    let Some(fp) = meta.kv("vec_fp").map_err(user_error)? else { return Ok(empty(SetsState::Empty)) };
    if fp_now.is_some_and(|f| f != fp) {
        return Ok(empty(SetsState::ModelChanged));
    }
    let dir = home.vectors_dir().join(&fp);
    let mut sets = empty(SetsState::Ready);
    for k in KINDS {
        let (gen, len, live) = meta.vector_snapshot(k).map_err(user_error)?;
        let owners: HashMap<u64, i64> = live.into_iter().collect();
        let set = match k {
            VecKind::Cards => &mut sets.cards,
            VecKind::Chunks => &mut sets.chunks,
        };
        let r = read_each(&file_path(&dir, k, gen), LAYOUT, len, |slot, rec| {
            if let Some(o) = owners.get(&slot) {
                set.push(*o, &rec);
            }
            Ok(())
        });
        if r.is_err() {
            return Ok(empty(SetsState::Unreadable));
        }
    }
    if sets.cards.is_empty() && sets.chunks.is_empty() {
        sets.state = SetsState::Empty;
    }
    Ok(sets)
}
```
`store.rs` 의 `user_error` 를 `pub(crate)` 그대로 쓴다. `reset_store` 의 `remove_keyword_dir(home)?;` 다음 줄에:
```rust
    // 의미 검색 벡터도 지운다 — 남으면 새 meta 의 내용 id 와 엉킨다(DEC-048).
    match std::fs::remove_dir_all(home.vectors_dir()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("의미 검색 색인을 지우지 못함: {e}")),
    }
```

- [ ] **Step 5: 시험이 통과하는지 본다**

Run: `cargo test -p app --release --lib vectors && cargo test -p platform --release`
Expected: PASS.

- [ ] **Step 6: 커밋**

```bash
git add crates/app crates/platform Cargo.lock
git commit -m "app: vector store management — fingerprint dirs, generations, repair, compaction, snapshot reads"
```

---

### Task 13: `app::embedding` — 카드·본문 임베딩 단계와 `index` 명령

**Files:**
- Create: `crates/app/src/embedding.rs`, `crates/app/tests/embedding.rs`
- Modify: `crates/app/src/lib.rs`, `crates/app/src/main.rs`

**Interfaces:**
- Consumes: Task 9 `Embed`·`plan`·`build_card`·`build_chunks_marked`, Task 11 meta API, Task 12 `open_for_write`·`compact_if_needed`.
- Produces:
  - `app::embedding::{EmbedOptions { data_tail: bool, stop_after_append: Option<usize> }, EmbedSummary { cards, cards_empty, chunks, failed, secs_cards, secs_body, stopped }, run_embedding, embed_line, ext_of, title_of}`.
  - `run_embedding(store: &mut Store, home: &Home, e: &mut dyn Embed, fp: &str, profile: &ModelProfile, opt: &EmbedOptions, progress: &mut dyn FnMut(&EmbedCounts)) -> Result<EmbedSummary, IndexError>`.
  - `title_of(doc: &doc::Document, path: &str) -> String`(제목 없으면 파일 이름 — m0eval 과 같은 규칙).
  - CLI: 전역 `--model <파일>`(기본: 실행 파일 옆 `models/embeddinggemma-2-Q8_0.gguf`), 전역 `--threads <n>`, `index --data-tail on|off`(kv `data_tail` 에 저장, 기본 on). 단계 시간은 kv `stat_keyword_secs`·`stat_cards_secs`·`stat_body_secs`, 스레드 kv `stat_threads`.

- [ ] **Step 1: 통합 시험을 쓴다**

`crates/app/tests/embedding.rs`:
```rust
//! 의미 검색 색인 단계(가짜 임베더 — CI 에 모델이 없어도 파이프라인 전체를 돈다).

use app::embedding::{run_embedding, EmbedOptions};
use app::{open_store, reset_store, run_index};
use embed::fake::FakeEmbedder;
use embed::profile::ModelProfile;
use extract::worker::Worker;
use index::meta::VecKind;
use index::tokenize::Analyzers;
use platform::paths::Home;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../extract/tests/fixtures")
}

fn worker() -> Worker {
    Worker::new(|| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kra-search"));
        c.arg("worker");
        c
    })
}

struct T {
    _dirs: (tempfile::TempDir, tempfile::TempDir),
    docs: PathBuf,
    home: Home,
    a: Analyzers,
}

impl T {
    fn new() -> T {
        let (docs, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for (src, dst) in [("office/sample.docx", "운영계획.docx"), ("schedule.hwp", "추진일정.hwp"), ("office/cp949.csv", "예산.csv"), ("office/utf8.txt", "메모.txt")] {
            std::fs::copy(fixtures().join(src), docs.path().join(dst)).unwrap();
        }
        // 데이터 형식 뒷부분(③)이 생기도록 긴 CSV(청크 20개 넘게)
        let mut csv = String::from("번호,사업장,내용\n");
        for i in 0..600 {
            csv += &format!("{i},렛츠런파크 서울,관람대 좌석 {i}번 점검 결과 이상 없음 확인 완료\n");
        }
        std::fs::write(docs.path().join("점검기록.csv"), csv).unwrap();
        T { docs: docs.path().to_path_buf(), home: Home::new(home.path()), a: Analyzers::load().unwrap(), _dirs: (docs, home) }
    }

    fn index(&self) -> app::Store {
        let mut s = open_store(&self.home, &self.a).unwrap();
        run_index(&mut s, &mut worker(), std::slice::from_ref(&self.docs), Duration::from_secs(30), true).unwrap();
        s
    }

    fn embed(&self, s: &mut app::Store, e: &mut FakeEmbedder, fp: &str, opt: EmbedOptions) -> app::embedding::EmbedSummary {
        run_embedding(s, &self.home, e, fp, &ModelProfile::embeddinggemma2(), &opt, &mut |_| {}).unwrap()
    }
}

fn on() -> EmbedOptions {
    EmbedOptions { data_tail: true, stop_after_append: None }
}

#[test]
fn cards_then_body_documents_first() {
    let t = T::new();
    let mut s = t.index();
    let r = t.embed(&mut s, &mut FakeEmbedder::default(), "fp", on());
    assert!(r.cards == 5 && r.cards_empty == 0 && r.chunks > 20 && r.failed == 0, "표뿐인 CSV 도 카드가 있다(DEC-057): {r:?}");
    let c = s.meta.embed_counts(embed::plan::DATA_HEAD).unwrap();
    assert_eq!((c.cards_done, c.main_done, c.tail_done), (c.cards_total, c.main_total, c.tail_total), "{c:?}");
    assert!(c.tail_total > 0, "{c:?}");
    // 첫 청크 슬롯은 문서 형식 것이다(본문 ①이 먼저).
    let first = s.meta.live_slots(VecKind::Chunks).unwrap().into_iter().find(|x| x.0 == 0).unwrap().1;
    let path = &s.meta.files_of(first).unwrap()[0].path;
    assert!(path.ends_with(".docx") || path.ends_with(".hwp"), "{path}");
}

#[test]
fn rename_does_not_reembed() {
    let t = T::new();
    let mut s = t.index();
    t.embed(&mut s, &mut FakeEmbedder::default(), "fp", on());
    drop(s);
    std::fs::rename(t.docs.join("운영계획.docx"), t.docs.join("운영계획(최종).docx")).unwrap();
    let mut s = t.index();
    let mut e = FakeEmbedder::default();
    let r = t.embed(&mut s, &mut e, "fp", on());
    assert_eq!((e.calls, r.cards, r.chunks), (0, 0, 0), "{r:?}");
}

/// Review Focus 4: 파일에 쓴 뒤 meta 기록 전에 꺼져도 다음 실행이 이어서 하고 중복·엇갈림이 없다.
#[test]
fn crash_between_append_and_meta_resumes_cleanly() {
    let t = T::new();
    let mut s = t.index();
    let r = t.embed(&mut s, &mut FakeEmbedder::default(), "fp", EmbedOptions { data_tail: true, stop_after_append: Some(1) });
    assert!(r.stopped);
    drop(s);
    let mut s = open_store(&t.home, &t.a).unwrap();
    t.embed(&mut s, &mut FakeEmbedder::default(), "fp", on());
    for k in [VecKind::Cards, VecKind::Chunks] {
        let live = s.meta.live_slots(k).unwrap();
        let (_, len, _) = s.meta.vector_snapshot(k).unwrap();
        let mut slots: Vec<u64> = live.iter().map(|x| x.0).collect();
        slots.dedup();
        assert_eq!(slots.len(), live.len(), "슬롯 중복 없음");
        assert!(slots.iter().all(|&x| x < len), "모든 슬롯이 기록된 길이 안");
    }
    let c = s.meta.embed_counts(embed::plan::DATA_HEAD).unwrap();
    assert_eq!((c.cards_done, c.main_done, c.tail_done), (c.cards_total, c.main_total, c.tail_total), "{c:?}");
}

#[test]
fn fingerprint_change_rebuilds_and_data_tail_toggles() {
    let t = T::new();
    let mut s = t.index();
    let r = t.embed(&mut s, &mut FakeEmbedder::default(), "fp1", EmbedOptions { data_tail: false, stop_after_append: None });
    let c = s.meta.embed_counts(embed::plan::DATA_HEAD).unwrap();
    assert!(c.tail_total > 0 && c.tail_done == 0, "끄면 뒷부분은 하지 않는다: {c:?}");
    let r2 = t.embed(&mut s, &mut FakeEmbedder::default(), "fp1", on());
    assert_eq!(r2.cards, 0);
    assert_eq!(r2.chunks as u64, c.tail_total, "켜면 뒷부분만 더 한다");
    let r3 = t.embed(&mut s, &mut FakeEmbedder::default(), "fp2", on());
    assert_eq!(r3.cards, r.cards, "지문이 바뀌면 처음부터");
}

#[test]
fn failed_input_is_recorded_and_not_retried() {
    let t = T::new();
    let mut s = t.index();
    let mut e = FakeEmbedder { fail_on: Some("렛츠런파크".into()), ..Default::default() }; // 운영계획.docx·메모.txt 본문에 있다
    let r = t.embed(&mut s, &mut e, "fp", on());
    assert!(r.failed >= 1, "{r:?}");
    let mut e2 = FakeEmbedder::default();
    let r2 = t.embed(&mut s, &mut e2, "fp", on());
    assert_eq!((r2.cards, r2.chunks), (0, 0), "실패한 단위는 '없음'으로 기록하고 다시 하지 않는다");
}

#[test]
fn reset_store_removes_vectors() {
    let t = T::new();
    let mut s = t.index();
    t.embed(&mut s, &mut FakeEmbedder::default(), "fp", on());
    drop(s);
    assert!(t.home.vectors_dir().exists());
    reset_store(&t.home).unwrap();
    assert!(!t.home.vectors_dir().exists());
}
```

- [ ] **Step 2: 시험이 실패하는지 본다**

Run: `cargo test -p app --release --test embedding`
Expected: FAIL — `unresolved import app::embedding`.

- [ ] **Step 3: 임베딩 단계를 구현한다**

`crates/app/src/lib.rs` 에 `pub mod embedding;` 을 더하고 `pub use store::{…}` 줄 그대로 둔다.

`crates/app/src/embedding.rs`:
```rust
//! 의미 검색 색인(스펙 §6.2 의 2·3단계, DEC-046): 카드 → 본문 ①②③(embed::plan). 묶음마다 벡터 파일에 쓰고(fsync)
//! 그다음 meta 에 기록한다 — 사이에서 꺼져도 다음 실행이 꼬리를 자르고 이어서 한다(DEC-048).
//! 입력 하나가 실패하면 그 단위만 "없음"(슬롯 없음)으로 기록하고 계속한다(M2 설계 §3.5).

use crate::pipeline::IndexError;
use crate::store::Store;
use crate::vectors::{self, VecStore, LAYOUT};
use embed::api::Embed;
use embed::chunk::{build_card, build_chunks_marked};
use embed::plan::{self, BodyDoc, CardDoc, FormatClass};
use embed::profile::ModelProfile;
use index::meta::{EmbedCounts, Meta};
use platform::paths::Home;
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;
use vector::record::Record;

/// 한 번에 임베딩하는 입력 수
const BATCH: usize = 16;
/// 이만큼 모이면 파일 → meta 로 커밋
const COMMIT: usize = 64;

#[derive(Debug, Clone, Default)]
pub struct EmbedOptions {
    /// 본문 ③(데이터 형식 뒷부분)
    pub data_tail: bool,
    /// 시험용: n 번째 커밋에서 파일에 쓴 뒤 meta 기록 전에 멈춘다(강제 종료 흉내)
    pub stop_after_append: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct EmbedSummary {
    pub cards: usize,
    pub cards_empty: usize,
    pub chunks: usize,
    pub failed: usize,
    pub secs_cards: f64,
    pub secs_body: f64,
    pub stopped: bool,
}

pub fn ext_of(path: &str) -> String {
    Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// 카드·청크 머리의 제목: 문서 제목, 없으면 파일 이름(스펙 §6.6). m0eval 도 같은 규칙.
pub fn title_of(doc: &doc::Document, path: &str) -> String {
    doc.title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .unwrap_or_else(|| Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string())
}

/// 진행 한 줄(UI D7 용어).
pub fn embed_line(c: &EmbedCounts) -> String {
    let tail = if c.tail_total > 0 { format!(" · 데이터 파일 뒷부분 {}/{}", c.tail_done, c.tail_total) } else { String::new() };
    format!("빠른 의미 검색 {}/{} · 정밀 의미 검색 {}/{}{tail}", c.cards_done, c.cards_total, c.main_done, c.main_total)
}

/// 입력이 있는 것만 묶어 임베딩. 묶음이 실패하면 하나씩 다시 해 실패한 것만 None.
fn embed_each(e: &mut dyn Embed, inputs: &[Option<String>], failed: &mut usize) -> Vec<Option<Vec<f32>>> {
    let texts: Vec<String> = inputs.iter().flatten().cloned().collect();
    let got: Vec<Option<Vec<f32>>> = match e.embed(&texts) {
        Ok(v) if v.len() == texts.len() => v.into_iter().map(Some).collect(),
        _ => texts
            .iter()
            .map(|t| match e.embed(std::slice::from_ref(t)) {
                Ok(mut v) if v.len() == 1 => v.pop(),
                _ => {
                    *failed += 1;
                    None
                }
            })
            .collect(),
    };
    let mut it = got.into_iter();
    inputs.iter().map(|x| if x.is_some() { it.next().flatten() } else { None }).collect()
}

struct Committer<'a> {
    commits: usize,
    opt: &'a EmbedOptions,
}

impl Committer<'_> {
    /// 파일에 쓰고 meta 에 기록한다. 시험용 멈춤 지점이면 true(meta 기록 없이).
    fn cards(&mut self, meta: &mut Meta, vs: &mut VecStore, pending: &mut Vec<(i64, Option<Record>)>) -> Result<bool, IndexError> {
        if pending.is_empty() {
            return Ok(false);
        }
        let recs: Vec<Record> = pending.iter().filter_map(|x| x.1.clone()).collect();
        let mut slot = vs.cards.append(&recs).map_err(|e| IndexError::Other(format!("의미 검색 색인 파일 오류: {e}")))?;
        self.commits += 1;
        if self.opt.stop_after_append == Some(self.commits) {
            return Ok(true);
        }
        let rows: Vec<(i64, Option<u64>)> = pending
            .iter()
            .map(|(id, r)| {
                (*id, r.as_ref().map(|_| {
                    slot += 1;
                    slot - 1
                }))
            })
            .collect();
        meta.set_cards(&rows, vs.cards.len())?;
        pending.clear();
        Ok(false)
    }

    fn chunks(&mut self, meta: &mut Meta, vs: &mut VecStore, pending: &mut Vec<(i64, usize, Option<Record>)>) -> Result<bool, IndexError> {
        if pending.is_empty() {
            return Ok(false);
        }
        let recs: Vec<Record> = pending.iter().filter_map(|x| x.2.clone()).collect();
        let mut slot = vs.chunks.append(&recs).map_err(|e| IndexError::Other(format!("의미 검색 색인 파일 오류: {e}")))?;
        self.commits += 1;
        if self.opt.stop_after_append == Some(self.commits) {
            return Ok(true);
        }
        let rows: Vec<(i64, usize, Option<u64>)> = pending
            .iter()
            .map(|(id, pos, r)| {
                (*id, *pos, r.as_ref().map(|_| {
                    slot += 1;
                    slot - 1
                }))
            })
            .collect();
        meta.add_chunks(&rows, vs.chunks.len())?;
        pending.clear();
        Ok(false)
    }
}

pub fn run_embedding(
    store: &mut Store,
    home: &Home,
    e: &mut dyn Embed,
    fp: &str,
    profile: &ModelProfile,
    opt: &EmbedOptions,
    progress: &mut dyn FnMut(&EmbedCounts),
) -> Result<EmbedSummary, IndexError> {
    let mut vs = vectors::open_for_write(home, &mut store.meta, fp)?;
    let mut s = EmbedSummary::default();
    let mut c = Committer { commits: 0, opt };

    // ---- 카드: 문서 형식 먼저, 최근 수정 먼저(결정 #27)
    let t = Instant::now();
    let rows = store.meta.card_todo()?;
    let path_of: HashMap<i64, String> = rows.iter().map(|r| (r.id, r.path.clone())).collect();
    let mut order: Vec<CardDoc> = rows.iter().map(|r| CardDoc { id: r.id, class: plan::class_of_ext(&ext_of(&r.path)), mtime_ns: r.mtime_ns }).collect();
    plan::card_order(&mut order);
    let mut pending: Vec<(i64, Option<Record>)> = Vec::new();
    for group in order.chunks(BATCH) {
        let mut inputs = Vec::with_capacity(group.len());
        {
            let count = |x: &str| e.count_tokens(x);
            for d in group {
                let input = store.meta.doc(d.id)?.and_then(|doc| {
                    build_card(&doc, plan::CARD_TOKENS, &count).map(|card| profile.render_doc(Some(&title_of(&doc, &path_of[&d.id])), &card))
                });
                inputs.push(input);
            }
        }
        let vecs = embed_each(e, &inputs, &mut s.failed);
        for (d, v) in group.iter().zip(vecs) {
            let rec = v.and_then(|v| Record::from_full(&v, LAYOUT));
            if rec.is_some() {
                s.cards += 1;
            } else {
                s.cards_empty += 1;
            }
            pending.push((d.id, rec));
        }
        if pending.len() >= COMMIT {
            if c.cards(&mut store.meta, &mut vs, &mut pending)? {
                s.stopped = true;
                return Ok(s);
            }
            progress(&store.meta.embed_counts(plan::DATA_HEAD)?);
        }
    }
    if c.cards(&mut store.meta, &mut vs, &mut pending)? {
        s.stopped = true;
        return Ok(s);
    }
    s.secs_cards = t.elapsed().as_secs_f64();

    // ---- 본문: 청크 수부터 센다(진행률 분모, 토큰화만)
    let t = Instant::now();
    let todo = store.meta.body_todo()?;
    let mut plans: Vec<(i64, usize, bool)> = Vec::new();
    for r in &todo {
        let ext = ext_of(&r.path);
        let total = match store.meta.doc(r.id)? {
            Some(doc) => {
                let count = |x: &str| e.count_tokens(x);
                build_chunks_marked(&doc, &plan::chunk_params(&ext), &count).len()
            }
            None => 0,
        };
        plans.push((r.id, total, plan::class_of_ext(&ext) == FormatClass::Data));
        if plans.len() >= 200 {
            store.meta.set_bodies(&plans)?;
            plans.clear();
        }
    }
    store.meta.set_bodies(&plans)?;
    progress(&store.meta.embed_counts(plan::DATA_HEAD)?);

    // ---- 본문 ①②③
    let bodies = store.meta.bodies()?;
    let body_path: HashMap<i64, String> = bodies.iter().map(|b| (b.id, b.path.clone())).collect();
    let docs: Vec<BodyDoc> = bodies
        .iter()
        .map(|b| BodyDoc { id: b.id, class: if b.data { FormatClass::Data } else { FormatClass::Document }, mtime_ns: b.mtime_ns, total: b.total })
        .collect();
    let mut pending: Vec<(i64, usize, Option<Record>)> = Vec::new();
    for (id, range) in plan::schedule(&docs, opt.data_tail) {
        let done = store.meta.chunk_positions(id)?;
        let todo_pos: Vec<usize> = range.filter(|p| !done.contains(p)).collect();
        if todo_pos.is_empty() {
            continue;
        }
        let Some(doc) = store.meta.doc(id)? else { continue };
        let path = &body_path[&id];
        let chunks = {
            let count = |x: &str| e.count_tokens(x);
            build_chunks_marked(&doc, &plan::chunk_params(&ext_of(path)), &count)
        };
        let order = plan::priority_order(&chunks);
        let title = title_of(&doc, path);
        for group in todo_pos.chunks(BATCH) {
            let inputs: Vec<Option<String>> = group.iter().map(|&p| order.get(p).map(|&i| profile.render_doc(Some(&title), &chunks[i].text))).collect();
            let vecs = embed_each(e, &inputs, &mut s.failed);
            for (&p, v) in group.iter().zip(vecs) {
                let rec = v.and_then(|v| Record::from_full(&v, LAYOUT));
                if rec.is_some() {
                    s.chunks += 1;
                }
                pending.push((id, p, rec));
            }
            if pending.len() >= COMMIT {
                if c.chunks(&mut store.meta, &mut vs, &mut pending)? {
                    s.stopped = true;
                    return Ok(s);
                }
                progress(&store.meta.embed_counts(plan::DATA_HEAD)?);
            }
        }
    }
    if c.chunks(&mut store.meta, &mut vs, &mut pending)? {
        s.stopped = true;
        return Ok(s);
    }
    s.secs_body = t.elapsed().as_secs_f64();
    vectors::compact_if_needed(&mut store.meta, &mut vs)?;
    progress(&store.meta.embed_counts(plan::DATA_HEAD)?);
    Ok(s)
}
```

- [ ] **Step 4: 시험이 통과하는지 본다**

Run: `cargo test -p app --release --test embedding`
Expected: PASS(6개).

- [ ] **Step 5: `index`·`status` 명령에 붙인다**

`crates/app/src/main.rs`:
1. `use` 에 더한다: `use app::embedding::{embed_line, run_embedding, EmbedOptions};`, `use app::vectors;`, `use embed::engine::{Embedder, EngineOptions};`, `use embed::profile::ModelProfile;`, `use std::time::Instant;`.
2. `Cli` 에 전역 인자:
```rust
    /// 의미 검색 모델(GGUF). 기본: 실행 파일 옆 models\embeddinggemma-2-Q8_0.gguf
    #[arg(long, global = true)]
    model: Option<PathBuf>,
    /// 의미 검색 색인 스레드 수(기본: 성능 코어 수, 최대 4)
    #[arg(long, global = true)]
    threads: Option<usize>,
```
3. `Cmd::Index` 에 `#[arg(long, value_parser = ["on", "off"])] data_tail: Option<String>` 을 더한다(doc: `/// 텍스트·CSV·엑셀 파일 뒷부분까지 정밀 의미 검색(on/off, 저장됨)`).
4. 함수:
```rust
const MODEL_FILE: &str = "embeddinggemma-2-Q8_0.gguf";

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// 의미 검색 모델 경로(있을 때만). 상대 경로는 절대 경로로.
fn model_path(arg: &Option<PathBuf>) -> Option<PathBuf> {
    let p = match arg {
        Some(p) => absolute(p),
        None => exe_dir()?.join("models").join(MODEL_FILE),
    };
    p.is_file().then_some(p)
}

/// 키워드 색인 뒤 의미 검색 색인(DEC-046). 모델이 없거나 못 불러오면 키워드만으로 끝낸다(M2 설계 §3.5).
/// meta 손상은 `MetaCorrupt` 로 돌려준다 — 부르는 쪽이 키워드 단계와 같이 한 번 처음부터 다시 만든다.
fn index_semantic(cli: &Cli, home: &Home, store: &mut Store, data_tail: Option<&str>) -> Result<(), IndexError> {
    if let Some(v) = data_tail {
        store.meta.set_kv("data_tail", v)?;
    }
    let tail = store.meta.kv("data_tail")?.as_deref() != Some("off");
    let Some(path) = model_path(&cli.model) else {
        eprintln!("의미 검색 모델({MODEL_FILE})을 찾지 못해 키워드 검색만 준비했습니다.");
        return Ok(());
    };
    let threads = cli.threads.unwrap_or_else(platform::cpu::default_embed_threads);
    let (hash, cache) = vectors::model_hash(store.meta.kv("model_hash")?.as_deref(), &path).map_err(|x| IndexError::Other(format!("모델 파일을 읽지 못했습니다: {x}")))?;
    store.meta.set_kv("model_hash", &cache)?;
    let profile = ModelProfile::embeddinggemma2();
    let fp = vectors::fingerprint(hash, &profile);
    let mut m = match Embedder::load(&path, &EngineOptions { threads, max_tokens: 1024, backend_dir: exe_dir() }) {
        Ok(m) => m,
        Err(x) => {
            eprintln!("의미 검색 모델을 불러오지 못해 키워드 검색만 준비했습니다: {x}");
            return Ok(());
        }
    };
    let s = run_embedding(store, home, &mut m, &fp, &profile, &EmbedOptions { data_tail: tail, stop_after_append: None }, &mut |c| eprint!("\r{}", embed_line(c)))?;
    eprintln!();
    for (k, v) in [("stat_cards_secs", s.secs_cards), ("stat_body_secs", s.secs_body)] {
        store.meta.set_kv(k, &format!("{v:.1}"))?;
    }
    store.meta.set_kv("stat_threads", &threads.to_string())?;
    let mut line = format!("의미 검색: 빠른 {}개 · 정밀 {}개 새로 만듦 ({threads}스레드)", s.cards, s.chunks);
    if s.failed > 0 {
        line += &format!(" · 만들지 못함 {}개", s.failed);
    }
    println!("{line}");
    Ok(())
}
```
5. 요약 줄 만들기를 함수로 뺀다 — 기존 `Cmd::Index` 가지의 `let mut line = format!("파일 {}개 확인 …` 부터 `println!("{line}");` 직전까지를 그대로 옮긴다:
```rust
fn summary_line(s: &IndexSummary) -> String {
    let mut line = format!(
        "파일 {}개 확인 · 바뀐 파일 {}개 · 지운 파일 {}개 · 새로 읽음 {}개 · 같은 내용 재사용 {}개 · 다시 시도 {}개 · 열 수 없는 파일 {}개(다시 시도 포함) · 접근 못 함 {}개",
        s.scanned, s.changed, s.removed, s.extracted, s.reused, s.retried, s.failed, s.unreadable
    );
    if s.cloud_only > 0 {
        line += &format!(" · 클라우드 전용이라 건너뜀 {}개", s.cloud_only);
    }
    if s.pdf_skipped > 0 {
        line += &format!(" · PDF 건너뜀 {}개(다음에 다시)", s.pdf_skipped);
    }
    if s.changed_while_reading > 0 {
        line += &format!(" · 읽는 중 바뀌어 다음에 다시 {}개", s.changed_while_reading);
    }
    line
}
```
그리고 `Cmd::Index { root, data_tail }` 가지에서 `let s = match index_once(…) { … };` 부터 `println!("{line}");` 까지를 다음으로 바꾼다 — **meta 손상이 의미 검색 단계에서 드러나도** 키워드 단계와 같이 한 번 처음부터 다시 만든다(계획 검증 중 `any_corrupt_meta_page_is_rebuilt` 가 잡은 빈틈):
```rust
            let mut rebuilt = false;
            loop {
                let t = Instant::now();
                let r = index_once(&mut store, &mut worker, &mut roots, timeout, pdf_reader).and_then(|s| {
                    println!("{}", summary_line(&s));
                    store.meta.set_kv("stat_keyword_secs", &format!("{:.1}", t.elapsed().as_secs_f64()))?;
                    index_semantic(&cli, &home, &mut store, data_tail.as_deref())?;
                    // 검색이 읽는 쪽까지 모두 확인(DEC-056) — 손상이면 아래에서 한 번 다시 만든다.
                    Ok(store.meta.check()?)
                });
                match r {
                    // 열 때는 멀쩡했지만 색인 도중(키워드든 의미 검색이든) meta.db 손상이 드러남 — 한 번만 처음부터 다시 만든다.
                    Err(IndexError::MetaCorrupt) if !rebuilt => {
                        rebuilt = true;
                        eprintln!();
                        eprintln!("색인 데이터가 손상되어 처음부터 다시 만듭니다.");
                        drop(store);
                        reset_store(&home)?;
                        store = open_store(&home, &analyzers)?;
                    }
                    r => {
                        r.map_err(|e| e.to_string())?;
                        break;
                    }
                }
            }
```
(`match cli.cmd` 가 `cli.cmd` 를 옮기므로 `match cli.cmd` 를 `match &cli.cmd` 로 바꾸고 가지 안의 값은 참조로 쓴다 — `root.iter()`, `query.join(" ")`, `*limit`. `run(cli: Cli)` 의 앞부분 `if let Cmd::Worker = cli.cmd` 도 `if let Cmd::Worker = &cli.cmd`.)
6. `Cmd::Status` 의 `println!("저장 위치: …")` 앞에:
```rust
            let ec = store.meta.embed_counts(embed::plan::DATA_HEAD).map_err(|e| IndexError::from(e).to_string())?;
            let tail = if store.meta.kv("data_tail").map_err(|e| IndexError::from(e).to_string())?.as_deref() == Some("off") { "끔" } else { "켬" };
            println!("{} (텍스트·CSV·엑셀 파일 뒷부분: {tail})", embed_line(&ec));
```

- [ ] **Step 6: CLI 시험이 모델 유무와 상관없이 같게 돈다**

개발 PC 에는 M0 진단 개발 때 만든 `target/release/models/` 가 있을 수 있어, 기본 경로(실행 파일 옆 `models\`)로 실제 모델을 찾으면 CLI 시험이 PC 마다 다르게 돈다(계획 검증 중 확인). `crates/app/tests/cli.rs` 의 `exec_with_pdfium` 에서 `.arg("--pdfium").arg(pdfium)` 다음에 `.arg("--model").arg(self.home.path().join("모델없음.gguf"))` 를 더해 항상 키워드만 쓰게 한다(실모델은 `tests/semantic.rs` 가 따로 본다).

Run: `cargo test -p app --release`
Expected: PASS — 기존 `cli.rs` 시험은 "키워드 검색만 준비" 안내가 한 줄 더 나올 뿐 결과는 같다.

- [ ] **Step 7: 실제 모델로 손으로 한 번 돌려 본다**

Run:
```bash
bash tools/fetch-models.sh
cargo run -p app --release -- --home /tmp/kra-m2 --model models/embeddinggemma-2-Q8_0.gguf index --root crates/extract/tests/fixtures
cargo run -p app --release -- --home /tmp/kra-m2 status
```
Expected: 진행 줄 `빠른 의미 검색 n/N · 정밀 의미 검색 n/N` 이 늘고, 마지막에 `의미 검색: 빠른 …개 · 정밀 …개 새로 만듦 (4스레드)`(macOS 는 논리 코어 기준 최대 4). llama.cpp 로그가 화면에 나오지 않는다(KI-003). `status` 에 `정밀 의미 검색 N/N`.

- [ ] **Step 8: 커밋**

```bash
git add crates/app
git commit -m "app: semantic indexing stage in index (cards → body tiers), --model/--threads/--data-tail, status counts"
```

---

### Task 14: 하이브리드 검색 — `Searcher` 와 `search` 명령

**Files:**
- Modify: `crates/app/src/search.rs`, `crates/app/src/lib.rs`, `crates/app/src/main.rs`
- Create: `crates/app/tests/search.rs`, `crates/app/tests/semantic.rs`(실모델, `#[ignore]`)

**Interfaces:**
- Consumes: Task 5 `query::parse`, Task 8 `index::hybrid::{search, query_texts, HybridParams, MapMeta, Hit}`, Task 12 `load_sets`·`fingerprint`·`model_hash`.
- Produces (Task 17·18):
  - `app::search::Searcher { params: HybridParams, notice: Option<String>, .. }`.
  - `Searcher::open(store: &Store, home: &Home, model: Option<&Path>, threads: usize) -> Result<Searcher, String>`.
  - `Searcher::with_embedder(store: &Store, home: &Home, embedder: Option<Box<dyn Embed>>, fp_now: Option<&str>) -> Result<Searcher, String>`(시험·평가용).
  - `Searcher::search(&mut self, store: &Store, q: &str, today: Day, limit: usize) -> Result<(ParsedQuery, Vec<HybridHit>), String>`.
  - `HybridHit { hit: Hit /* 기존: content_id·score·title·paths·fail */, why: index::hybrid::Hit }`, `why_line(&index::hybrid::Hit) -> String`.
  - CLI: 숨김 전역 `--now YYYY-MM-DD`(기준 시점).

- [ ] **Step 1: 시험을 쓴다**

`crates/app/tests/search.rs`:
```rust
//! 하이브리드 검색(가짜 임베더). 실제 모델 품질은 m0eval·eval 이 잰다.

use app::embedding::{run_embedding, EmbedOptions};
use app::search::Searcher;
use app::{open_store, run_index};
use embed::fake::FakeEmbedder;
use embed::profile::ModelProfile;
use extract::worker::Worker;
use index::tokenize::Analyzers;
use platform::paths::Home;
use query::calendar::{noon_kst_unix_secs, parse_ymd};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../extract/tests/fixtures")
}

fn set_date(p: &Path, ymd: &str) {
    let secs = noon_kst_unix_secs(parse_ymd(ymd).unwrap());
    let f = std::fs::File::options().write(true).open(p).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::from_secs(secs as u64)).unwrap();
}

fn setup(names_dates: &[(&str, &str, &str)]) -> (tempfile::TempDir, tempfile::TempDir, Home, app::Store) {
    let (docs, home_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    for (src, dst, date) in names_dates {
        let p = docs.path().join(dst);
        std::fs::copy(fixtures().join(src), &p).unwrap();
        set_date(&p, date);
    }
    let home = Home::new(home_dir.path());
    let a = Analyzers::load().unwrap();
    let mut store = open_store(&home, &a).unwrap();
    let mut w = Worker::new(|| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kra-search"));
        c.arg("worker");
        c
    });
    run_index(&mut store, &mut w, &[docs.path().to_path_buf()], Duration::from_secs(30), true).unwrap();
    run_embedding(&mut store, &home, &mut FakeEmbedder::default(), "fp", &ModelProfile::embeddinggemma2(), &EmbedOptions { data_tail: true, stop_after_append: None }, &mut |_| {}).unwrap();
    (docs, home_dir, home, store)
}

fn today() -> query::Day {
    parse_ymd("2026-10-15").unwrap()
}

#[test]
fn semantic_paths_take_part_when_vectors_match() {
    let (_d, _h, home, store) = setup(&[("office/sample.docx", "운영계획.docx", "2026-09-01"), ("schedule.hwp", "추진일정.hwp", "2026-09-02")]);
    let mut s = Searcher::with_embedder(&store, &home, Some(Box::new(FakeEmbedder::default())), Some("fp")).unwrap();
    assert!(s.notice.is_none(), "{:?}", s.notice);
    let (_, hits) = s.search(&store, "렛츠런파크 운영", today(), 10).unwrap();
    assert!(!hits.is_empty());
    assert!(hits.iter().any(|h| h.why.card.is_some() || h.why.chunk.is_some()), "의미 경로가 결합에 들어간다");
}

/// Review Focus 3: 색인 뒤 모델이 바뀌면 의미 검색을 쓰지 않고 알린다(키워드 결과는 그대로).
#[test]
fn model_change_disables_semantic_until_reindex() {
    let (_d, _h, home, store) = setup(&[("office/sample.docx", "운영계획.docx", "2026-09-01")]);
    let mut s = Searcher::with_embedder(&store, &home, Some(Box::new(FakeEmbedder::default())), Some("다른모델")).unwrap();
    assert!(s.notice.as_deref().unwrap().contains("index"), "{:?}", s.notice);
    let (_, hits) = s.search(&store, "렛츠런파크", today(), 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].why.card.is_none() && hits[0].why.chunk.is_none());
}

#[test]
fn date_chip_moves_last_years_copy_up() {
    let (_d, _h, home, store) = setup(&[("office/sample.docx", "운영계획 올해.docx", "2026-09-01"), ("office/utf8.txt", "운영계획 작년.txt", "2025-05-01")]);
    let mut s = Searcher::with_embedder(&store, &home, None, None).unwrap();
    s.params.date_exact = 1.0; // 기본값(Task 16 이 정함)에 기대지 않는다
    let (pq, hits) = s.search(&store, "작년 렛츠런파크", today(), 10).unwrap();
    assert_eq!(pq.chips().count(), 1);
    assert!(hits[0].hit.paths[0].contains("작년"), "{:?}", hits.iter().map(|h| &h.hit.paths).collect::<Vec<_>>());
    assert!(hits[0].why.date_boost > 0.0);
}
```
`fixtures/office/utf8.txt` 의 본문("한국마사회 경마 시행 계획 / 렛츠런파크 서울 운영 방안을 보고합니다.")에도 "렛츠런파크"가 있어 두 파일 모두 키워드로 걸리고 날짜가 순위를 가른다.

`crates/app/tests/semantic.rs`(실모델 — CI 모델 단계에서만):
```rust
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
```

- [ ] **Step 2: 시험이 실패하는지 본다**

Run: `cargo test -p app --release --test search`
Expected: FAIL — `unresolved import app::search::Searcher`.

- [ ] **Step 3: 구현한다**

`crates/app/src/lib.rs` 의 `mod search;` 를 `pub mod search;` 로(기존 `pub use search::{fail_label, search, Hit};` 는 그대로).

`crates/app/src/search.rs` 끝(시험 모듈 위)에 더한다:
```rust
use crate::vectors::{self, Sets, SetsState};
use embed::api::Embed;
use embed::engine::{Embedder, EngineOptions};
use embed::profile::ModelProfile;
use index::hybrid::{self, HybridParams, MapMeta};
use platform::paths::Home;
use query::{Day, DateTable, ParsedQuery};

/// 하이브리드 검색 결과 한 줄: 화면용 정보 + 경로별 기여(UI D3 "왜 이 결과?"의 재료).
#[derive(Debug)]
pub struct HybridHit {
    pub hit: Hit,
    pub why: hybrid::Hit,
}

pub struct Searcher {
    sets: Sets,
    docs: MapMeta,
    embedder: Option<Box<dyn Embed>>,
    profile: ModelProfile,
    table: DateTable,
    pub params: HybridParams,
    /// 사용자에게 알릴 한 줄(의미 검색을 못 쓰는 이유)
    pub notice: Option<String>,
}

impl Searcher {
    /// 제품 경로: 모델이 있으면 지문을 계산해 색인과 맞을 때만 의미 검색을 쓴다.
    pub fn open(store: &Store, home: &Home, model: Option<&Path>, threads: usize) -> Result<Searcher, String> {
        let Some(path) = model else {
            let mut s = Self::with_embedder(store, home, None, None)?;
            if s.sets.state == SetsState::Ready {
                s.notice = Some("의미 검색 모델을 찾지 못해 키워드 검색만 합니다.".into());
            }
            return Ok(s);
        };
        let cached = store.meta.kv("model_hash").map_err(user_error)?;
        let (hash, _) = vectors::model_hash(cached.as_deref(), path).map_err(|e| format!("모델 파일을 읽지 못했습니다: {e}"))?;
        let fp = vectors::fingerprint(hash, &ModelProfile::embeddinggemma2());
        // 벡터가 있을 때만 모델을 불러온다(검색어 임베딩 — 1초 안팎).
        let probe = vectors::load_sets(home, &store.meta, Some(&fp))?;
        let embedder: Option<Box<dyn Embed>> = if probe.state == SetsState::Ready {
            let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
            match Embedder::load(path, &EngineOptions { threads, max_tokens: 1024, backend_dir: exe_dir }) {
                Ok(m) => Some(Box::new(m)),
                Err(e) => {
                    let mut s = Self::from_sets(store, probe, None)?;
                    s.notice = Some(format!("의미 검색 모델을 불러오지 못해 키워드 검색만 합니다: {e}"));
                    return Ok(s);
                }
            }
        } else {
            None
        };
        Self::from_sets(store, probe, embedder)
    }

    /// 시험·평가: 임베더와 "지금 지문"을 직접 준다.
    pub fn with_embedder(store: &Store, home: &Home, embedder: Option<Box<dyn Embed>>, fp_now: Option<&str>) -> Result<Searcher, String> {
        let sets = vectors::load_sets(home, &store.meta, fp_now)?;
        Self::from_sets(store, sets, embedder)
    }

    fn from_sets(store: &Store, sets: Sets, embedder: Option<Box<dyn Embed>>) -> Result<Searcher, String> {
        let dates = store.meta.newest_mtimes().map_err(user_error)?.into_iter().map(|(id, ns)| (id, query::calendar::day_of_unix_ns(ns))).collect();
        let chunked = store.meta.chunked_contents().map_err(user_error)?;
        let notice = match sets.state {
            SetsState::ModelChanged => Some("검색 모델이나 설정이 바뀌어 의미 검색을 쓰지 않습니다. `kra-search index` 를 실행하면 다시 만듭니다.".to_string()),
            SetsState::Unreadable => Some("의미 검색 색인을 읽지 못해 키워드 검색만 합니다. `kra-search index` 를 실행하세요.".to_string()),
            SetsState::Ready | SetsState::Empty => None,
        };
        let embedder = if sets.state == SetsState::Ready { embedder } else { None };
        Ok(Searcher {
            sets,
            docs: MapMeta { dates, chunked },
            embedder,
            profile: ModelProfile::embeddinggemma2(),
            table: DateTable::builtin(),
            params: HybridParams::default(),
            notice,
        })
    }

    pub fn search(&mut self, store: &Store, q: &str, today: Day, limit: usize) -> Result<(ParsedQuery, Vec<HybridHit>), String> {
        let pq = query::parse(q, today, &self.table);
        let texts = hybrid::query_texts(&pq, &self.params);
        let qv = match self.embedder.as_mut() {
            Some(e) => match e.embed(&[self.profile.render_query(&texts.semantic)]) {
                Ok(mut v) if v.len() == 1 => v.pop(),
                Ok(_) => None,
                Err(err) => {
                    self.notice = Some(format!("검색어의 뜻을 계산하지 못해 키워드 검색만 했습니다: {err}"));
                    None
                }
            },
            None => None,
        };
        // 경로 없는 내용(반영 안 된 삭제)을 건너뛰므로 넉넉히 받는다.
        let raw = hybrid::search(&store.keyword, &self.sets.cards, &self.sets.chunks, &pq, qv.as_deref(), &self.params, today, &self.docs, limit + 20)
            .map_err(|e| format!("검색 색인 오류: {e}"))?;
        let mut out = Vec::new();
        for h in raw {
            let Some(c) = store.meta.content(h.id).map_err(user_error)? else { continue };
            let paths: Vec<String> = store.meta.files_of(h.id).map_err(user_error)?.into_iter().map(|f| f.path).collect();
            if paths.is_empty() {
                continue;
            }
            let title = c.title.clone().unwrap_or_else(|| Path::new(&paths[0]).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string());
            out.push(HybridHit { hit: Hit { content_id: h.id, score: h.score as f32, title, paths, fail: c.fail }, why: h });
            if out.len() == limit {
                break;
            }
        }
        Ok((pq, out))
    }
}

/// "왜 이 결과?" 한 줄(UI D3·D7 용어).
pub fn why_line(h: &hybrid::Hit) -> String {
    let mut v = Vec::new();
    if let Some(r) = h.keyword_rank {
        v.push(format!("키워드 일치 {r}위"));
    }
    match (h.chunk, h.card) {
        (Some((r, _)), _) => v.push(format!("뜻이 비슷함 {r}위")),
        (None, Some((r, _))) => v.push(format!("뜻이 비슷함(빠른 의미 검색) {r}위")),
        _ => {}
    }
    if h.date_boost > 0.0 {
        v.push("기간 안".into());
    }
    v.join(" · ")
}
```

`crates/app/src/main.rs`:
1. `Cli` 에 숨김 전역 인자:
```rust
    /// (평가·시험용) 기준 시점 YYYY-MM-DD — 날짜 표현("작년")을 이 날 기준으로 읽는다
    #[arg(long, global = true, hide = true)]
    now: Option<String>,
```
2. 함수:
```rust
fn today(now: &Option<String>) -> Result<query::Day, String> {
    match now {
        Some(s) => query::calendar::parse_ymd(s).ok_or_else(|| format!("--now 는 YYYY-MM-DD 형식입니다: {s}")),
        None => Ok(query::calendar::today()),
    }
}
```
3. `Cmd::Search` 가지를 바꾼다:
```rust
        Cmd::Search { limit, query } => {
            let store = open_store_readonly(&home, &analyzers)?;
            let threads = cli.threads.unwrap_or_else(platform::cpu::default_embed_threads);
            let mut s = app::search::Searcher::open(&store, &home, model_path(&cli.model).as_deref(), threads)?;
            let (pq, hits) = s.search(&store, &query.join(" "), today(&cli.now)?, *limit)?;
            if let Some(n) = &s.notice {
                eprintln!("{n}");
            }
            for c in pq.chips() {
                let r = c.range.expect("칩");
                println!("📅 {} → {} ~ {} (이 기간 문서를 앞으로)", &pq.original[c.start..c.end], query::calendar::format_ymd(r.start), query::calendar::format_ymd(r.end));
            }
            if hits.is_empty() {
                println!("결과 없음");
            }
            for (i, h) in hits.iter().enumerate() {
                println!("{:>3}. {}", i + 1, h.hit.title);
                println!("     {}", h.hit.paths[0]);
                if h.hit.paths.len() > 1 {
                    println!("     {}", duplicate_line(&h.hit.paths[1..]));
                }
                if let Some(f) = &h.hit.fail {
                    println!("     ※ {}", fail_label(f));
                }
                let w = app::search::why_line(&h.why);
                if !w.is_empty() {
                    println!("     {w}");
                }
            }
        }
```
(점수 `[0.02]` 는 RRF 값이라 사용자에게 뜻이 없어 보이지 않는다. 기존 `cli.rs` 시험은 점수를 보지 않는다.)
4. 맨 위 `use app::{…}` 에서 `search` 를 뺀다(이제 `Searcher` 를 쓴다 — 남기면 사용하지 않는 import 경고).

- [ ] **Step 4: 시험이 통과하는지 본다**

Run: `cargo test -p app --release`
Expected: PASS(`search.rs` 3개 포함, `semantic.rs` 는 ignored).

Run: `bash tools/fetch-models.sh && cargo test -p app --release --test semantic -- --include-ignored --test-threads 1`
Expected: PASS. 실패하면 검색어를 고치지 말고 결과 출력을 보고 의미 경로가 실제로 붙는지 확인한다(`뜻이 비슷함` 이 한 줄도 없으면 벡터 읽기·지문 문제).

- [ ] **Step 5: CLI 날짜 칩 시험을 더한다**

`crates/app/tests/cli.rs` 에:
```rust
#[test]
fn date_chip_is_shown_and_now_is_validated() {
    let e = Env::new();
    e.index();
    let r = e.run(&["search", "--now", "2026-10-15", "작년", "렛츠런파크"]);
    assert!(r.contains("📅 작년 → 2025-01-01 ~ 2025-12-31"), "{r}");
    let r = e.run_fail(&["search", "--now", "2026-13-01", "렛츠런파크"]);
    assert!(r.contains("YYYY-MM-DD"), "{r}");
}
```
Run: `cargo test -p app --release --test cli`
Expected: PASS.

- [ ] **Step 6: 커밋**

```bash
git add crates/app
git commit -m "app: hybrid search (Searcher) with date chips, why-line, model-change guard, --now"
```

---
### Task 15: m0eval — 하이브리드 실험(벡터 캐시 + 제품 검색 코어)과 결정 규칙

**Files:**
- Create: `crates/app/src/evalset.rs`(평가 공용: 파일 이름·분할·지표), `crates/m0eval/src/hybrid.rs`
- Modify: `crates/app/src/lib.rs`, `crates/m0eval/Cargo.toml`, `crates/m0eval/src/corpus.rs`, `crates/m0eval/src/main.rs`, `crates/m0eval/src/metrics.rs`

**Interfaces:**
- Consumes: Task 8 `hybrid::search`·`query_texts`·`HybridParams`·`Combine`·`VectorSet`·`MapMeta`, Task 9 `plan`·`build_chunks_marked`, Task 13 `app::embedding::title_of`, Task 5 `query::parse`.
- Produces:
  - `app::evalset::{file_name(id, title, ext) -> String, id_of(&str) -> Option<String>, tune_split(items) -> HashSet<String>, ndcg_at_k, recall_at_k, first_relevant_rank}` — 평가 문서 파일 이름은 `"{제목} ({id}).{확장자}"`(금지 문자는 공백, 제목 80자까지). Task 17 이 같은 함수로 복사본을 만든다.
  - m0eval 플래그: `--hybrid`(보고서 `docs/m2/eval-hybrid.md`, 고른 값 `docs/m2/params.json`), `--model <gguf>`(기본 `models/embeddinggemma-2-Q8_0.gguf`), `--now YYYY-MM-DD`(기본 2026-10-15), `--params <json>`(고르지 않고 이 값으로), `--dump <json>`(질의별 순위), `--parity <앱 덤프 json>`(일치 검사 → `docs/m2/parity.md`, 어긋나면 종료 코드 1).
  - 순위 덤프 형식: `{"qid": ["문서 id", …]}`(상위 100, JSON 객체).

- [ ] **Step 1: 평가 공용 모듈 시험을 쓴다**

`crates/app/src/evalset.rs`(시험만 먼저):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_safe_and_carry_the_id() {
        assert_eq!(file_name("P0001", "근로복지공단, 장애학생 방학 중  교육·돌봄", "hwpx"), "근로복지공단, 장애학생 방학 중 교육·돌봄 (P0001).hwpx");
        assert_eq!(file_name("K0001", "「규정」 a/b:c*?\"<>|", "hwp"), "「규정」 a b c (K0001).hwp");
        assert_eq!(id_of("근로복지공단 (P0001).hwpx").as_deref(), Some("P0001"));
        assert_eq!(id_of("/x/y/2025년 계획(안) (K0011).hwp").as_deref(), Some("K0011"));
        assert_eq!(id_of("그냥 파일.hwp"), None);
        let long = "가".repeat(200);
        assert_eq!(file_name("P1", &long, "hwp").chars().count(), 80 + " (P1).hwp".chars().count());
    }

    #[test]
    fn tune_split_is_stable_and_stratified() {
        let items: Vec<(String, String)> = (0..10).map(|i| (format!("Q{i:03}"), "a".to_string())).chain((0..5).map(|i| (format!("R{i:03}"), "b".to_string()))).collect();
        let s1 = tune_split(items.iter().map(|(q, k)| (q.as_str(), k.as_str())));
        let s2 = tune_split(items.iter().rev().map(|(q, k)| (q.as_str(), k.as_str())));
        assert_eq!(s1, s2, "입력 순서와 무관");
        assert_eq!(s1.iter().filter(|q| q.starts_with('Q')).count(), 6);
        assert_eq!(s1.iter().filter(|q| q.starts_with('R')).count(), 3, "5 × 0.6 = 3");
    }

    #[test]
    fn metrics() {
        let r: Vec<String> = ["a", "b", "c"].iter().map(|x| x.to_string()).collect();
        let rel: HashSet<String> = ["b".to_string()].into_iter().collect();
        assert!((ndcg_at_k(&r, &rel, 10) - 1.0 / 3f64.log2()).abs() < 1e-9);
        assert_eq!(first_relevant_rank(&r, &rel), Some(2));
        assert_eq!(recall_at_k(&r, &rel, 1), 0.0);
    }
}
```

- [ ] **Step 2: 시험이 실패하는지 본다**

`crates/app/src/lib.rs` 에 `pub mod evalset;` 를 더하고:
Run: `cargo test -p app --release --lib evalset`
Expected: FAIL — `cannot find function file_name`.

- [ ] **Step 3: 평가 공용 모듈을 구현한다**

`crates/app/src/evalset.rs` 시험 위에:
```rust
//! 평가 세트 공용(DEC-051·052): 평가 문서 복사본의 파일 이름, 조정용/검증용 분할, 지표.
//! m0eval(실험)과 `kra-search eval`(제품)이 같은 함수를 쓴다 — 파일 이름이 다르면 제목 없는 문서의 카드 머리와
//! 키워드 파일명 필드가 달라져 일치 검사가 깨진다.

use std::collections::{HashMap, HashSet};
use std::path::Path;

/// `"{제목} ({id}).{확장자}"` — Windows 금지 문자·제어 문자는 공백, 공백은 하나로, 제목은 80자까지.
pub fn file_name(id: &str, title: &str, ext: &str) -> String {
    let t: String = title.chars().map(|c| if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { ' ' } else { c }).collect();
    let t: String = t.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect();
    format!("{} ({id}).{ext}", t.trim_end_matches(['.', ' ']))
}

/// 복사본 파일 이름(또는 경로)에서 평가 문서 id 를 꺼낸다.
pub fn id_of(file_name: &str) -> Option<String> {
    let stem = Path::new(file_name).file_stem()?.to_str()?;
    let open = stem.rfind(" (")?;
    let id = stem[open + 2..].strip_suffix(')')?;
    (!id.is_empty()).then(|| id.to_string())
}

/// FNV-1a 64 — 러스트 버전이 바뀌어도 같은 값.
fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// 조정용 qid 집합: 유형마다 qid 해시 순서로 앞 60%(올림). 나머지는 검증용(DEC-051).
pub fn tune_split<'a>(items: impl IntoIterator<Item = (&'a str, &'a str)>) -> HashSet<String> {
    let mut by: HashMap<&str, Vec<&str>> = HashMap::new();
    for (q, k) in items {
        by.entry(k).or_default().push(q);
    }
    let mut out = HashSet::new();
    for (_, mut v) in by {
        v.sort_by_key(|q| (fnv(q), *q));
        let n = (v.len() * 6).div_ceil(10);
        out.extend(v.into_iter().take(n).map(String::from));
    }
    out
}

pub fn ndcg_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    let dcg: f64 = ranked.iter().take(k).enumerate().filter(|(_, d)| relevant.contains(*d)).map(|(i, _)| 1.0 / ((i + 2) as f64).log2()).sum();
    let idcg: f64 = (0..relevant.len().min(k)).map(|i| 1.0 / ((i + 2) as f64).log2()).sum();
    if idcg == 0.0 {
        0.0
    } else {
        dcg / idcg
    }
}

pub fn recall_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    if relevant.is_empty() {
        return 0.0;
    }
    ranked.iter().take(k).filter(|d| relevant.contains(*d)).count() as f64 / relevant.len() as f64
}

/// 첫 정답 문서의 순위(1부터).
pub fn first_relevant_rank(ranked: &[String], relevant: &HashSet<String>) -> Option<usize> {
    ranked.iter().position(|d| relevant.contains(d)).map(|i| i + 1)
}
```

`crates/m0eval/src/metrics.rs` 의 `ndcg_at_k`·`recall_at_k`·`first_relevant_rank` 본문을 지우고 파일 위에 `pub use app::evalset::{first_relevant_rank, ndcg_at_k, recall_at_k};` 를 둔다(`section_rank` 와 시험은 그대로 — 지표 구현은 한 곳).

- [ ] **Step 4: 시험이 통과하는지 본다**

`crates/m0eval/Cargo.toml` `[dependencies]` 에 `app = { path = "../app" }`, `query = { path = "../query" }` 를 더하고:
Run: `cargo test -p app --release --lib evalset && cargo test -p m0eval --release`
Expected: PASS.

- [ ] **Step 5: 말뭉치에 날짜·복사본 이름을 싣는다**

`crates/m0eval/src/corpus.rs`:
```rust
pub struct EvalDoc {
    pub id: String,
    /// 매니페스트 제목 + 확장자 — M0·M1 보고서의 파일명 필드(그대로 둔다 — 그 결과를 다시 만들 수 있게)
    pub name: String,
    /// M2: 평가 복사본 파일 이름(app::evalset::file_name) — 제품 `eval` 과 같게
    pub file: String,
    pub ext: String,
    /// 매니페스트 date(한국 날짜)
    pub date: Option<query::Day>,
    pub doc: Document,
    /// 추출 제목이 없어 매니페스트 제목을 넣었는가 — M2 는 이때 제목을 비워 제품처럼 파일 이름을 쓴다
    pub title_filled: bool,
}

/// 추출에 실패한 문서 — 제품처럼 파일 이름으로만 찾히게 M2 실험에도 넣는다.
pub struct FailedDoc {
    pub id: String,
    pub why: String,
    pub file: String,
    pub ext: String,
    pub date: Option<query::Day>,
}
```
`Entry` 에 `date: Option<String>` 를 더하고 `load_docs` 를 `-> (Vec<EvalDoc>, Vec<FailedDoc>)` 로:
```rust
pub fn load_docs(eval_dir: &Path) -> (Vec<EvalDoc>, Vec<FailedDoc>) {
    let m: Manifest = toml::from_str(&std::fs::read_to_string(eval_dir.join("manifest.toml")).expect("manifest.toml")).expect("manifest");
    let (mut ok, mut failed) = (Vec::new(), Vec::new());
    for e in m.doc {
        let path = eval_dir.join("docs").join(format!("{}.{}", e.id, e.ext));
        let name = format!("{}.{}", e.title, e.ext);
        let file = app::evalset::file_name(&e.id, &e.title, &e.ext);
        let date = e.date.as_deref().and_then(query::calendar::parse_ymd);
        match extract::cli::extract_one(&path) {
            extract::runner::Outcome::Ok { mut doc, .. } => {
                let title_filled = doc.title.as_deref().is_none_or(|t| t.trim().is_empty());
                if title_filled {
                    doc.title = Some(e.title.clone());
                }
                ok.push(EvalDoc { id: e.id, name, file, ext: e.ext, date, doc, title_filled });
            }
            other => failed.push(FailedDoc { id: e.id, why: format!("{other:?}"), file, ext: e.ext, date }),
        }
    }
    (ok, failed)
}
```
`main.rs` 에서 실패 목록을 쓰는 두 곳(`eprintln!("추출 실패: {id} {why}")` 과 보고서 첫 줄의 `format!("{} {}", f.0, f.1)`)을 `f.id`·`f.why` 로 바꾼다.

- [ ] **Step 6: 하이브리드 실험을 구현한다**

`crates/m0eval/src/hybrid.rs`:
```rust
//! M2 하이브리드 평가(M2 설계 §6, DEC-051·052). 벡터는 항목별 캐시(DEC-032), 검색은 제품 코어 `index::hybrid::search` 그대로.
//! 벡터를 넣는 순서도 제품의 슬롯 순서(카드 순서·본문 3단 일정)와 같게 한다 — 이진 후보에서 해밍 거리가 같으면 슬롯 번호로 가르기 때문.
//! 조정용 질의로 매개변수를 고르고(좌표 하강, 0.005 이내면 바꾸지 않음), 검증용으로 판정하고, 결정 규칙을 숫자로 적용한다.

use crate::cache::Store as Cache;
use crate::corpus::{EvalDoc, FailedDoc, Query};
use app::embedding::title_of;
use app::evalset::{first_relevant_rank, ndcg_at_k, recall_at_k, tune_split};
use doc::Document;
use embed::chunk::{build_card, build_chunks_marked};
use embed::engine::{Embedder, EngineOptions};
use embed::plan::{self, BodyDoc, CardDoc};
use embed::profile::ModelProfile;
use index::hybrid::{self, Combine, HybridParams, MapMeta, VectorSet};
use index::keyword::{KeywordDoc, KeywordIndex};
use index::tokenize::Analyzers;
use query::calendar::{format_ymd, noon_kst_unix_secs};
use query::{DateTable, Day, QueryKind};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use vector::record::{Layout, Record};

const LAYOUT: Layout = Layout::M0;
const CAPS: [usize; 2] = [256, 128];
/// 결정 규칙의 "차이 없음" 폭(DEC-051)
const EPS: f64 = 0.005;

pub struct HArgs {
    pub model: PathBuf,
    pub threads: usize,
    pub now: Day,
}

struct HDoc {
    id: String,
    file: String,
    ext: String,
    date: Option<Day>,
    doc: Option<Document>,
}

pub struct World {
    docs: Vec<HDoc>,
    kw: KeywordIndex,
    cards: HashMap<usize, VectorSet>,
    chunks_head: VectorSet,
    chunks_all: VectorSet,
    meta_head: MapMeta,
    meta_all: MapMeta,
    qvec: HashMap<String, Vec<f32>>,
    /// 문서별 자르기 전 카드 토큰 수(결정 #27 판정)
    card_tokens: Vec<usize>,
    now: Day,
    table: DateTable,
}

#[derive(Clone)]
pub struct Cfg {
    pub params: HybridParams,
    pub cap: usize,
    pub tail: bool,
}

fn embed_all(m: &mut Embedder, items: Vec<(String, String)>) -> Vec<(String, Vec<f32>)> {
    let mut out = Vec::with_capacity(items.len());
    for b in items.chunks(8) {
        let t: Vec<String> = b.iter().map(|x| x.1.clone()).collect();
        let es = m.embed(&t).expect("임베딩");
        out.extend(b.iter().map(|x| x.0.clone()).zip(es.into_iter().map(|e| e.vector)));
    }
    out
}

fn mtime_of(d: &HDoc) -> i64 {
    d.date.map(|x| noon_kst_unix_secs(x) * 1_000_000_000).unwrap_or(0)
}

pub fn build(docs: &[EvalDoc], failed: &[FailedDoc], queries: &[Query], a: &HArgs) -> World {
    let mut hd: Vec<HDoc> = docs
        .iter()
        .map(|d| {
            let mut doc = d.doc.clone();
            if d.title_filled {
                doc.title = None; // 제품처럼: 제목이 없으면 파일 이름(title_of)
            }
            HDoc { id: d.id.clone(), file: d.file.clone(), ext: d.ext.clone(), date: d.date, doc: Some(doc) }
        })
        .collect();
    hd.extend(failed.iter().map(|f| HDoc { id: f.id.clone(), file: f.file.clone(), ext: f.ext.clone(), date: f.date, doc: None }));

    let an = Analyzers::load().expect("형태소 사전");
    let mut kw = KeywordIndex::in_ram(&an).expect("색인");
    for (i, d) in hd.iter().enumerate() {
        kw.upsert(&KeywordDoc::new(i as u64, d.doc.as_ref(), std::slice::from_ref(&d.file))).expect("색인 쓰기");
    }
    kw.commit().expect("커밋");

    let name = a.model.file_name().expect("모델 파일").to_string_lossy().to_string();
    let mut m = Embedder::load(&a.model, &EngineOptions { threads: a.threads, max_tokens: 1024, backend_dir: None }).expect("모델 불러오기");
    let mut cache = Cache::open(&Path::new("target/m0cache").join(&name));
    let p = ModelProfile::embeddinggemma2();
    let table = DateTable::builtin();

    // 카드: 제품의 카드 순서(문서 형식·최근 수정 먼저)대로 넣는다.
    let mut order: Vec<CardDoc> = hd
        .iter()
        .enumerate()
        .filter(|(_, d)| d.doc.is_some())
        .map(|(i, d)| CardDoc { id: i as i64, class: plan::class_of_ext(&d.ext), mtime_ns: mtime_of(d) })
        .collect();
    plan::card_order(&mut order);
    let mut cards = HashMap::new();
    let mut card_tokens = vec![0usize; hd.len()];
    for cap in CAPS {
        let mut inputs = Vec::new();
        {
            let count = |s: &str| m.token_count(s).unwrap_or(s.chars().count());
            for o in &order {
                let d = &hd[o.id as usize];
                let doc = d.doc.as_ref().expect("본문");
                if cap == CAPS[0] {
                    card_tokens[o.id as usize] = build_card(doc, 100_000, &count).map(|c| count(&c)).unwrap_or(0);
                }
                if let Some(c) = build_card(doc, cap, &count) {
                    inputs.push((o.id.to_string(), p.render_doc(Some(&title_of(doc, &d.file)), &c)));
                }
            }
        }
        let vs = cache.get_or_compute(&format!("card{cap}"), inputs, |x| embed_all(&mut m, x));
        let mut set = VectorSet::new(LAYOUT);
        for (id, v) in vs {
            if let Some(r) = Record::from_full(&v, LAYOUT) {
                set.push(id.parse().expect("id"), &r);
            }
        }
        cards.insert(cap, set);
    }

    // 본문: 청크를 만들고 제품의 3단 일정 순서로 넣는다(③ 끔 = 일정에서 ③을 뺀 것).
    let mut texts: HashMap<(i64, usize), String> = HashMap::new();
    let mut bodies = Vec::new();
    {
        let count = |s: &str| m.token_count(s).unwrap_or(s.chars().count());
        for (i, d) in hd.iter().enumerate() {
            let Some(doc) = &d.doc else { continue };
            let chunks = build_chunks_marked(doc, &plan::chunk_params(&d.ext), &count);
            let order = plan::priority_order(&chunks);
            let title = title_of(doc, &d.file);
            for (pos, &ci) in order.iter().enumerate() {
                texts.insert((i as i64, pos), p.render_doc(Some(&title), &chunks[ci].text));
            }
            bodies.push(BodyDoc { id: i as i64, class: plan::class_of_ext(&d.ext), mtime_ns: mtime_of(d), total: chunks.len() });
        }
    }
    let sched_all: Vec<(i64, usize)> = plan::schedule(&bodies, true).into_iter().flat_map(|(id, r)| r.map(move |p| (id, p))).collect();
    let inputs: Vec<(String, String)> = sched_all.iter().map(|(id, p)| (format!("{id}:{p}"), texts[&(*id, *p)].clone())).collect();
    let vs: HashMap<String, Vec<f32>> = cache.get_or_compute("chunk", inputs, |x| embed_all(&mut m, x)).into_iter().collect();
    let fill = |tail: bool| {
        let mut set = VectorSet::new(LAYOUT);
        let mut chunked = HashSet::new();
        for (id, r) in plan::schedule(&bodies, tail) {
            for pos in r {
                if let Some(rec) = Record::from_full(&vs[&format!("{id}:{pos}")], LAYOUT) {
                    set.push(id, &rec);
                    chunked.insert(id);
                }
            }
        }
        (set, chunked)
    };
    let (chunks_head, chunked_head) = fill(false);
    let (chunks_all, chunked_all) = fill(true);
    let dates: HashMap<i64, Day> = hd.iter().enumerate().filter_map(|(i, d)| d.date.map(|x| (i as i64, x))).collect();

    // 검색어: 날짜 빼기 4가지 조합의 글을 모두 임베딩해 둔다.
    let mut qtexts: Vec<(String, String)> = Vec::new();
    for q in queries {
        let pq = query::parse(&q.text, a.now, &table);
        for (sr, sa) in [(true, true), (true, false), (false, true), (false, false)] {
            let t = hybrid::query_texts(&pq, &HybridParams { strip_relative: sr, strip_absolute: sa, ..Default::default() }).semantic;
            qtexts.push((t.clone(), p.render_query(&t)));
        }
    }
    qtexts.sort();
    qtexts.dedup();
    let qvec: HashMap<String, Vec<f32>> = cache.get_or_compute("query", qtexts, |x| embed_all(&mut m, x)).into_iter().collect();

    World {
        docs: hd,
        kw,
        cards,
        chunks_head,
        chunks_all,
        meta_head: MapMeta { dates: dates.clone(), chunked: chunked_head },
        meta_all: MapMeta { dates, chunked: chunked_all },
        qvec,
        card_tokens,
        now: a.now,
        table,
    }
}

impl World {
    /// 질의 하나의 문서 id 순위(상위 100) — 제품 검색 코어 그대로.
    pub fn rank(&self, cfg: &Cfg, q: &Query) -> Vec<String> {
        let pq = query::parse(&q.text, self.now, &self.table);
        let t = hybrid::query_texts(&pq, &cfg.params);
        let qv = self.qvec.get(&t.semantic).map(|v| v.as_slice());
        let (chunks, meta) = if cfg.tail { (&self.chunks_all, &self.meta_all) } else { (&self.chunks_head, &self.meta_head) };
        hybrid::search(&self.kw, &self.cards[&cfg.cap], chunks, &pq, qv, &cfg.params, self.now, meta, 100)
            .expect("검색")
            .iter()
            .map(|h| self.docs[h.id as usize].id.clone())
            .collect()
    }
}

/// 질의별 (nDCG@10, Recall@20, 첫 정답 순위)
type Per = HashMap<String, (f64, f64, Option<usize>)>;

fn evaluate(w: &World, cfg: &Cfg, qs: &[&Query]) -> Per {
    qs.iter()
        .map(|q| {
            let r = w.rank(cfg, q);
            (q.qid.clone(), (ndcg_at_k(&r, &q.relevant, 10), recall_at_k(&r, &q.relevant, 20), first_relevant_rank(&r, &q.relevant)))
        })
        .collect()
}

fn mean(v: impl Iterator<Item = f64>) -> f64 {
    let (s, n) = v.fold((0.0, 0usize), |(s, n), x| (s + x, n + 1));
    if n == 0 {
        0.0
    } else {
        s / n as f64
    }
}

/// 유형별 평균 nDCG 의 평균(매크로, DEC-051)
fn macro_ndcg(per: &Per, qs: &[&Query]) -> f64 {
    let mut by: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for q in qs {
        by.entry(q.kind.as_str()).or_default().push(per[&q.qid].0);
    }
    mean(by.values().map(|v| mean(v.iter().copied())))
}

fn kind_mean(per: &Per, qs: &[&Query], kind: &str) -> Option<f64> {
    let v: Vec<f64> = qs.iter().filter(|q| q.kind == kind).map(|q| per[&q.qid].0).collect();
    (!v.is_empty()).then(|| mean(v.into_iter()))
}

fn all_mean(per: &Per, qs: &[&Query]) -> f64 {
    mean(qs.iter().map(|q| per[&q.qid].0))
}

type Setter = Box<dyn Fn(&mut HybridParams)>;

/// 좌표 하강 축: (이름, [(값 표시, 바꾸기)])
fn axes() -> Vec<(&'static str, Vec<(String, Setter)>)> {
    fn ax<T: std::fmt::Debug + Copy + 'static>(name: &'static str, vals: &[T], set: fn(&mut HybridParams, T)) -> (&'static str, Vec<(String, Setter)>) {
        (name, vals.iter().map(|&x| (format!("{x:?}"), Box::new(move |p: &mut HybridParams| set(p, x)) as Setter)).collect())
    }
    vec![
        ax("rrf_k", &[10.0, 20.0, 30.0, 45.0, 60.0], |p, x| p.rrf_k = x),
        ax("w_semantic[키워드형]", &[0.25, 0.5, 1.0, 1.5, 2.0], |p, x| p.w_semantic[0] = x),
        ax("w_semantic[의미형]", &[0.5, 1.0, 1.5, 2.0, 3.0], |p, x| p.w_semantic[1] = x),
        ax("combine", &[Combine::ChunkElseCard, Combine::Max, Combine::Separate, Combine::CardOnly], |p, x| p.combine = x),
        ax("floor", &[0.0f32, 0.2, 0.25, 0.3, 0.35, 0.4], |p, x| p.floor = x),
        ax("keyword.title", &[2.0f32, 3.0, 4.0], |p, x| p.keyword.title = x),
        ax("keyword.filename", &[1.5f32, 2.5, 3.5], |p, x| p.keyword.filename = x),
        ax("keyword.heading", &[1.0f32, 2.0, 3.0], |p, x| p.keyword.heading = x),
        ax("keyword.bigram", &[0.25f32, 0.5, 0.75, 1.0], |p, x| p.keyword.bigram = x),
        ax("date_exact", &[0.0, 0.5, 1.0, 2.0, 4.0], |p, x| p.date_exact = x),
        ax("date_vague", &[0.0, 0.1, 0.25, 0.5, 1.0], |p, x| p.date_vague = x),
        ax("recency", &[0.0, 0.05, 0.1, 0.2], |p, x| p.recency = x),
        ax("phrase", &[0.0, 0.25, 0.5, 1.0], |p, x| p.phrase = x),
    ]
}

/// 조정용 매크로 nDCG 를 올리는 쪽으로 한 축씩. 지금 값보다 EPS 넘게 좋아질 때만 바꾼다(차이가 작으면 단순한 기본값 유지).
fn descend(w: &World, base: &Cfg, tune: &[&Query], trace: &mut Vec<String>) -> HybridParams {
    let mut best = base.params.clone();
    let mut best_s = macro_ndcg(&evaluate(w, base, tune), tune);
    trace.push(format!("시작(기본값) {best_s:.4}"));
    for pass in 1..=3 {
        let mut changed = false;
        for (name, vals) in axes() {
            let mut cand: Option<(f64, String, HybridParams)> = None;
            for (label, set) in &vals {
                let mut p = best.clone();
                set(&mut p);
                if p == best {
                    continue;
                }
                let s = macro_ndcg(&evaluate(w, &Cfg { params: p.clone(), ..base.clone() }, tune), tune);
                if s > best_s + EPS && cand.as_ref().is_none_or(|c| s > c.0) {
                    cand = Some((s, label.clone(), p));
                }
            }
            if let Some((s, label, p)) = cand {
                trace.push(format!("{pass}회차: {name} = {label} → {s:.4}"));
                best = p;
                best_s = s;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    best
}

/// 승·무·패(질의별 nDCG@10, 차이 1e-9 이하는 무)
fn wtl(a: &Per, b: &Per, qs: &[&Query]) -> (usize, usize, usize) {
    qs.iter().fold((0, 0, 0), |(w, t, l), q| {
        let d = a[&q.qid].0 - b[&q.qid].0;
        if d > 1e-9 {
            (w + 1, t, l)
        } else if d < -1e-9 {
            (w, t, l + 1)
        } else {
            (w, t + 1, l)
        }
    })
}

/// 검색어 유형 분류의 정답(질의 유형 열): keyword → 키워드형, 문장형 유형 → 의미형, abbrev·kra 는 섞여 있어 뺀다.
fn kind_truth(kind: &str) -> Option<QueryKind> {
    match kind {
        "keyword" => Some(QueryKind::Keyword),
        "abbrev" | "kra" => None,
        _ => Some(QueryKind::Semantic),
    }
}

pub struct Outcome {
    pub report: String,
    pub params: HybridParams,
    pub cap: usize,
    pub ranks: BTreeMap<String, Vec<String>>,
}

pub fn run(w: &World, queries: &[Query], fixed: Option<HybridParams>) -> Outcome {
    let tune_ids = tune_split(queries.iter().map(|q| (q.qid.as_str(), q.kind.as_str())));
    let all: Vec<&Query> = queries.iter().collect();
    let tune: Vec<&Query> = queries.iter().filter(|q| tune_ids.contains(&q.qid)).collect();
    let hold: Vec<&Query> = queries.iter().filter(|q| !tune_ids.contains(&q.qid)).collect();
    let base = Cfg { params: HybridParams::default(), cap: plan::CARD_TOKENS, tail: true };
    let mut trace = Vec::new();
    let mut params = match fixed {
        Some(p) => {
            trace.push("고정 매개변수(--params) — 고르지 않음".into());
            p
        }
        None => descend(w, &base, &tune, &mut trace),
    };
    let mut s = String::new();

    // 날짜 빼기 규칙(DEC-051): 그 종류를 빼면 temporal 조정용 nDCG 가 떨어지면 빼지 않는다.
    let temporal: Vec<&Query> = tune.iter().copied().filter(|q| q.kind == "temporal").collect();
    let mut strip_notes = Vec::new();
    for (label, rel) in [("상대 표현", true), ("숫자 해", false)] {
        let with = |on: bool| {
            let mut p = params.clone();
            if rel {
                p.strip_relative = on;
            } else {
                p.strip_absolute = on;
            }
            mean(evaluate(w, &Cfg { params: p, ..base.clone() }, &temporal).values().map(|x| x.0))
        };
        let (on, off) = (with(true), with(false));
        let keep = on + 1e-9 >= off;
        if rel {
            params.strip_relative = keep;
        } else {
            params.strip_absolute = keep;
        }
        strip_notes.push(format!("| {label} | {on:.4} | {off:.4} | {} |", if keep { "뺀다" } else { "빼지 않는다(회귀)" }));
    }
    let chosen = Cfg { params: params.clone(), cap: plan::CARD_TOKENS, tail: true };

    // 기준선과 판정
    let keyword_only = Cfg {
        params: HybridParams { w_semantic: [0.0, 0.0], date_exact: 0.0, date_vague: 0.0, recency: 0.0, phrase: 0.0, strip_relative: false, strip_absolute: false, ..HybridParams::default() },
        ..chosen.clone()
    };
    let semantic_only = |c: Combine| Cfg { params: HybridParams { w_keyword: [0.0, 0.0], combine: c, date_exact: 0.0, date_vague: 0.0, recency: 0.0, phrase: 0.0, ..params.clone() }, ..chosen.clone() };
    let no_date = Cfg { params: HybridParams { date_exact: 0.0, date_vague: 0.0, ..params.clone() }, ..chosen.clone() };
    let runs: Vec<(&str, Cfg)> = vec![
        ("키워드 단독(M1)", keyword_only.clone()),
        ("카드 단독", semantic_only(Combine::CardOnly)),
        ("청크 단독(없으면 카드)", semantic_only(Combine::ChunkElseCard)),
        ("하이브리드(고른 값)", chosen.clone()),
        ("하이브리드 − 날짜 가산", no_date.clone()),
        ("하이브리드 − 하한", Cfg { params: HybridParams { floor: 0.0, ..params.clone() }, ..chosen.clone() }),
        ("하이브리드 − 데이터 뒷부분", Cfg { tail: false, ..chosen.clone() }),
        ("하이브리드 − 날짜 빼기", Cfg { params: HybridParams { strip_relative: false, strip_absolute: false, ..params.clone() }, ..chosen.clone() }),
    ];
    let per: Vec<Per> = runs.iter().map(|(_, c)| evaluate(w, c, &all)).collect();
    let kinds: Vec<String> = {
        let mut k: Vec<String> = queries.iter().map(|q| q.kind.clone()).collect();
        k.sort();
        k.dedup();
        k
    };

    writeln!(s, "# M2 하이브리드 평가 (자동 생성: `cargo run -p m0eval --release -- --hybrid --eval eval/public --eval eval/kra --eval eval/data`)\n").unwrap();
    writeln!(s, "- 문서 {}건 · 질의 {}개(조정용 {} · 검증용 {}) · 기준 시점 {} · 카드 상한 {} · 실험과 제품이 같은 검색 코어(index::hybrid)\n", w.docs.len(), queries.len(), tune.len(), hold.len(), format_ymd(w.now), plan::CARD_TOKENS).unwrap();

    writeln!(s, "## 1. 합격 판정\n").unwrap();
    let (k_h, h_h) = (macro_ndcg(&per[0], &hold), macro_ndcg(&per[3], &hold));
    let (wn, tn, ln) = wtl(&per[3], &per[0], &hold);
    writeln!(s, "| 기준 | 값 | 판정 |\n|---|---|---|").unwrap();
    writeln!(s, "| 하이브리드 > 키워드 단독(검증용 매크로 nDCG@10) | {h_h:.4} vs {k_h:.4} · 승 {wn} 무 {tn} 패 {ln} | {} |", if h_h > k_h { "통과" } else { "**실패**" }).unwrap();
    let dates: Vec<&Query> = all.iter().copied().filter(|q| q.kind == "date").collect();
    let dates_h: Vec<&Query> = hold.iter().copied().filter(|q| q.kind == "date").collect();
    let (d_on, d_off) = (mean(dates.iter().map(|q| per[3][&q.qid].0)), mean(dates.iter().map(|q| per[4][&q.qid].0)));
    let (dh_on, dh_off) = (mean(dates_h.iter().map(|q| per[3][&q.qid].0)), mean(dates_h.iter().map(|q| per[4][&q.qid].0)));
    let (dw, dt, dl) = wtl(&per[3], &per[4], &dates);
    writeln!(s, "| 날짜 가산 켬 > 끔(date 전체 {} · 검증용 {}) | 전체 {d_on:.4} vs {d_off:.4} · 검증용 {dh_on:.4} vs {dh_off:.4} · 승 {dw} 무 {dt} 패 {dl} | {} |", dates.len(), dates_h.len(), if d_on > d_off && dh_on >= dh_off { "통과" } else { "**실패**" }).unwrap();
    let non_date: Vec<&Query> = all.iter().copied().filter(|q| q.kind != "date").collect();
    let (nd_on, nd_off) = (macro_ndcg(&per[3], &non_date), macro_ndcg(&per[4], &non_date));
    writeln!(s, "| 날짜 없는 질의가 떨어지지 않음(매크로) | {nd_on:.4} vs {nd_off:.4} | {} |", if nd_on + 0.003 >= nd_off { "통과" } else { "**실패**" }).unwrap();
    writeln!(s).unwrap();

    writeln!(s, "## 2. 유형별 nDCG@10 (전체 질의)\n").unwrap();
    writeln!(s, "| 설정 | 매크로(조정) | 매크로(검증) | {} |", kinds.join(" | ")).unwrap();
    writeln!(s, "|---|---|---|{}", "---|".repeat(kinds.len())).unwrap();
    for (i, (name, _)) in runs.iter().enumerate() {
        let cells: Vec<String> = kinds.iter().map(|k| kind_mean(&per[i], &all, k).map_or("-".into(), |x| format!("{x:.4}"))).collect();
        writeln!(s, "| {name} | {:.4} | {:.4} | {} |", macro_ndcg(&per[i], &tune), macro_ndcg(&per[i], &hold), cells.join(" | ")).unwrap();
    }
    writeln!(s).unwrap();

    writeln!(s, "## 3. 고른 매개변수\n\n```json\n{}\n```\n\n좌표 하강 기록:\n", serde_json::to_string_pretty(&params).expect("json")).unwrap();
    for t in &trace {
        writeln!(s, "- {t}").unwrap();
    }
    writeln!(s, "\n날짜 표현 빼기(temporal 조정용 nDCG, DEC-051):\n\n| 종류 | 뺌 | 안 뺌 | 결정 |\n|---|---|---|---|").unwrap();
    for n in &strip_notes {
        writeln!(s, "{n}").unwrap();
    }

    // 결정 #27: 카드 상한 128
    let with_cap = |cap: usize| evaluate(w, &Cfg { cap, ..chosen.clone() }, &all);
    let (c256, c128) = (with_cap(256), with_cap(128));
    let cards_n = w.card_tokens.iter().filter(|&&t| t > 0).count();
    let cut = w.card_tokens.iter().filter(|&&t| t > 128).count();
    let share = cut as f64 / cards_n.max(1) as f64;
    let overall = all_mean(&c128, &all) - all_mean(&c256, &all);
    let kra_drop: Vec<(String, f64)> = ["kra", "kra-deep", "kra-plain"]
        .iter()
        .filter_map(|k| Some((k.to_string(), kind_mean(&c128, &all, k)? - kind_mean(&c256, &all, k)?)))
        .collect();
    let long_q: Vec<&Query> = all.iter().copied().filter(|q| q.relevant.iter().any(|id| w.docs.iter().position(|d| &d.id == id).is_some_and(|i| w.card_tokens[i] > 128))).collect();
    let (cap, why) = if share < 0.25 {
        (256, format!("판단 불가 — 128에서 잘리는 카드 {:.1}% < 25%", share * 100.0))
    } else if overall <= -0.003 || kra_drop.iter().any(|(_, d)| *d <= -0.01) {
        (256, "하락 — 256 유지".to_string())
    } else {
        (128, "하락 없음 — 128 로 바꾼다".to_string())
    };
    writeln!(s, "\n## 4. 카드 상한 128 판정(결정 #27)\n").unwrap();
    writeln!(s, "- 잘리는 카드 {cut}/{cards_n} ({:.1}%) · 전체 nDCG 차이(128−256) {overall:+.4} · 마사회 유형 {}", share * 100.0, kra_drop.iter().map(|(k, d)| format!("{k} {d:+.4}")).collect::<Vec<_>>().join(", ")).unwrap();
    writeln!(s, "- 정답 문서 카드가 128토큰을 넘는 질의 {}개: 256 {:.4} · 128 {:.4}", long_q.len(), mean(long_q.iter().map(|q| c256[&q.qid].0)), mean(long_q.iter().map(|q| c128[&q.qid].0))).unwrap();
    writeln!(s, "- **결정: {cap}** ({why})").unwrap();

    // 데이터 형식 뒷부분(DEC-051)
    let non_data: Vec<&Query> = all.iter().copied().filter(|q| !q.kind.starts_with("data-")).collect();
    let nd_tail = macro_ndcg(&per[3], &non_data) - macro_ndcg(&per[6], &non_data);
    writeln!(s, "\n## 5. 데이터 형식 뒷부분(본문 ③)\n").unwrap();
    writeln!(s, "| 유형 | 켬 | 끔 |\n|---|---|---|").unwrap();
    for k in ["data-deep", "data-head"] {
        writeln!(s, "| {k} | {} | {} |", kind_mean(&per[3], &all, k).map_or("-".into(), |x| format!("{x:.4}")), kind_mean(&per[6], &all, k).map_or("-".into(), |x| format!("{x:.4}"))).unwrap();
    }
    writeln!(s, "\n- 데이터 아닌 질의 매크로 차이(켬−끔) {nd_tail:+.4} → {}", if nd_tail <= -0.003 { "**−0.3%p 이상 떨어짐 — 기본 끔 안을 사용자에게 올린다**" } else { "기본 켬 유지" }).unwrap();

    writeln!(s, "\n## 6. 날짜 질의\n\n| 질의 | 검색어 | 해석 | 켬 | 끔 |\n|---|---|---|---|---|").unwrap();
    for q in &dates {
        let pq = query::parse(&q.text, w.now, &w.table);
        let chips: Vec<String> = pq.chips().map(|c| { let r = c.range.expect("칩"); format!("{}~{}", format_ymd(r.start), format_ymd(r.end)) }).collect();
        let rk = |p: &Per| p[&q.qid].2.map_or("-".into(), |r| if r > 10 { format!("**{r}**") } else { r.to_string() });
        writeln!(s, "| {} | {} | {} | {} | {} |", q.qid, q.text, if chips.is_empty() { "(미래·없음)".into() } else { chips.join(", ") }, rk(&per[3]), rk(&per[4])).unwrap();
    }

    let (mut ok, mut n) = (0, 0);
    for q in &all {
        if let Some(t) = kind_truth(&q.kind) {
            n += 1;
            ok += (query::parse(&q.text, w.now, &w.table).kind == t) as usize;
        }
    }
    writeln!(s, "\n## 7. 검색어 유형 분류 정확도\n\n- {ok}/{n} ({:.1}%) — 정답: keyword 유형 = 키워드형, 문장형 유형 = 의미형(abbrev·kra 제외)", ok as f64 * 100.0 / n.max(1) as f64).unwrap();

    writeln!(s, "\n## 8. 키워드 단독보다 나빠진 질의(검증용)\n\n| 질의 | 유형 | 키워드 순위 | 하이브리드 순위 |\n|---|---|---|---|").unwrap();
    for q in &hold {
        if per[3][&q.qid].0 + 1e-9 < per[0][&q.qid].0 {
            let r = |p: &Per| p[&q.qid].2.map_or("-".into(), |x| x.to_string());
            writeln!(s, "| {} | {} | {} | {} |", q.qid, q.kind, r(&per[0]), r(&per[3])).unwrap();
        }
    }

    let final_cfg = Cfg { cap, ..chosen };
    let ranks: BTreeMap<String, Vec<String>> = all.iter().map(|q| (q.qid.clone(), w.rank(&final_cfg, q))).collect();
    Outcome { report: s, params, cap, ranks }
}

/// 일치 검사(DEC-052): 같은 매개변수(제품 기본값)로 m0eval 과 앱 `eval --set` 의 순위를 비교한다.
pub fn parity(w: &World, queries: &[Query], app_dump: &Path) -> (String, bool) {
    let app: BTreeMap<String, Vec<String>> = serde_json::from_str(&std::fs::read_to_string(app_dump).expect("앱 덤프")).expect("덤프 JSON");
    let cfg = Cfg { params: HybridParams::default(), cap: plan::CARD_TOKENS, tail: true };
    let (mut same, mut n, mut da, mut dm) = (0usize, 0usize, 0.0, 0.0);
    let mut diff_q = Vec::new();
    for q in queries {
        let Some(a) = app.get(&q.qid) else { continue };
        let m = w.rank(&cfg, q);
        n += 1;
        let top = |v: &[String]| v.iter().take(10).cloned().collect::<Vec<_>>();
        if top(a) == top(&m) {
            same += 1;
        } else {
            diff_q.push(q.qid.clone());
        }
        da += ndcg_at_k(a, &q.relevant, 10);
        dm += ndcg_at_k(&m, &q.relevant, 10);
    }
    let (na, nm) = (da / n.max(1) as f64, dm / n.max(1) as f64);
    let frac = same as f64 / n.max(1) as f64;
    let pass = (na - nm).abs() <= 0.002 && frac >= 0.98 && n == queries.len();
    let mut s = String::new();
    writeln!(s, "# M2 일치 검사 (실험 = 제품, DEC-052)\n").unwrap();
    writeln!(s, "| 항목 | 값 | 기준 |\n|---|---|---|").unwrap();
    writeln!(s, "| 비교한 질의 | {n}/{} | 전부 |", queries.len()).unwrap();
    writeln!(s, "| nDCG@10 앱 · m0eval | {na:.4} · {nm:.4} (차이 {:+.4}) | ≤ 0.002 |", na - nm).unwrap();
    writeln!(s, "| 상위 10개가 순서까지 같은 질의 | {same}/{n} ({:.1}%) | ≥ 98% |", frac * 100.0).unwrap();
    writeln!(s, "| 판정 | **{}** | |", if pass { "통과" } else { "실패" }).unwrap();
    if !diff_q.is_empty() {
        writeln!(s, "\n상위 10개가 다른 질의: {}", diff_q.join(", ")).unwrap();
    }
    (s, pass)
}
```

- [ ] **Step 7: 실행 진입점을 붙인다**

`crates/m0eval/src/main.rs`:
1. `mod hybrid;` 를 더한다.
2. `Args` 에:
```rust
    /// M2 하이브리드 평가(docs/m2/eval-hybrid.md · docs/m2/params.json)
    #[arg(long)]
    hybrid: bool,
    /// M2 실험 모델
    #[arg(long, default_value = "models/embeddinggemma-2-Q8_0.gguf")]
    model: PathBuf,
    /// 기준 시점(날짜 표현 해석)
    #[arg(long, default_value = "2026-10-15")]
    now: String,
    /// 매개변수를 고르지 않고 이 JSON 을 쓴다
    #[arg(long)]
    params: Option<PathBuf>,
    /// 질의별 순위 JSON 저장
    #[arg(long)]
    dump: Option<PathBuf>,
    /// 앱 `kra-search eval --dump` 결과와 비교(일치 검사)
    #[arg(long)]
    parity: Option<PathBuf>,
```
3. `if a.keyword { … return; }` 다음에:
```rust
    if a.hybrid || a.parity.is_some() {
        let now = query::calendar::parse_ymd(&a.now).expect("--now YYYY-MM-DD");
        let w = hybrid::build(&docs, &failed, &queries, &hybrid::HArgs { model: a.model.clone(), threads: a.threads, now });
        if let Some(app) = &a.parity {
            let (report, pass) = hybrid::parity(&w, &queries, app);
            std::fs::write("docs/m2/parity.md", &report).unwrap();
            eprintln!("{report}");
            std::process::exit(if pass { 0 } else { 1 });
        }
        let fixed = a.params.as_ref().map(|p| serde_json::from_str(&std::fs::read_to_string(p).unwrap()).expect("params JSON"));
        let o = hybrid::run(&w, &queries, fixed);
        let out = a.out.clone().unwrap_or_else(|| PathBuf::from("docs/m2/eval-hybrid.md"));
        std::fs::create_dir_all("docs/m2").unwrap();
        std::fs::write(&out, &o.report).unwrap();
        std::fs::write("docs/m2/params.json", serde_json::to_string_pretty(&o.params).unwrap() + "\n").unwrap();
        if let Some(d) = &a.dump {
            std::fs::write(d, serde_json::to_string(&o.ranks).unwrap()).unwrap();
        }
        eprintln!("저장: {} · 카드 상한 결정 {}", out.display(), o.cap);
        return;
    }
```

- [ ] **Step 8: 작은 세트로 끝까지 도는지 본다(평가 세트 전체는 Task 16)**

Run:
```bash
mkdir -p /tmp/kra-mini/docs && cp eval/kra/docs/K000{1,2,3,4}.hwp /tmp/kra-mini/docs/
python3 - <<'PY'
import tomllib, re
src = open('eval/kra/manifest.toml', encoding='utf-8').read()
blocks = src.split('\n[[doc]]\n')[1:5]
open('/tmp/kra-mini/manifest.toml', 'w', encoding='utf-8').write('\n'.join('[[doc]]\n' + b for b in blocks))
q = open('eval/kra/queries.tsv', encoding='utf-8').read().splitlines()
keep = [l for l in q[1:] if all(x.strip() in {'K0001', 'K0002', 'K0003', 'K0004'} for x in l.split('\t')[3].split(','))][:12]
open('/tmp/kra-mini/queries.tsv', 'w', encoding='utf-8').write('\n'.join([q[0]] + keep) + '\n')
PY
cargo run -p m0eval --release -- --hybrid --eval /tmp/kra-mini --out /tmp/kra-mini/report.md
head -30 /tmp/kra-mini/report.md
```
Expected: 보고서가 만들어지고 `## 1. 합격 판정` 표가 있다(질의가 적어 판정 값은 뜻이 없다 — 코드가 끝까지 도는지만 본다). `docs/m2/params.json` 이 생긴다 — 이 실행의 것이므로 `git checkout -- docs/m2/params.json` 또는 지운다(Task 16 이 다시 만든다).

- [ ] **Step 9: 커밋**

```bash
git add crates/app crates/m0eval Cargo.lock
git commit -m "m0eval: hybrid experiments on the product search core (cached vectors, product slot order), decision rules, parity check"
```

---
### Task 16: 실험 실행 — 매개변수·카드 상한·데이터 뒷부분 결정

이 작업은 코드보다 **실행과 판정**이다. 결정은 Task 15 보고서의 숫자 규칙으로만 한다(DEC-051). 규칙으로 정할 수 없는 일이 생기면 멈추고 사용자에게 묻는다.

**Files:**
- Create: `docs/m2/eval-hybrid.md`, `docs/m2/params.json`(m0eval 이 씀)
- Modify: `crates/index/src/hybrid.rs`(`HybridParams::default` 를 고른 값으로), 필요하면 `crates/embed/src/plan.rs`(`CARD_TOKENS`), `docs/engineering/decisions.md`(DEC-058)

- [ ] **Step 1: 평가 세트가 다 있는지 확인한다**

Run:
```bash
python3 tools/eval/fetch_all.py && python3 tools/eval/import_local.py eval/kra && python3 tools/eval/make_data_set.py --check
python3 tools/eval/check_sets.py eval/public eval/kra eval/data
cargo test -p query --release --test eval_dates
```
Expected: 세 개 `ok`, `통과: 문서 1059 · 질의 251`, 시험 통과. Task 3 의 사용자 검토로 정답이 바뀌었으면 이미 반영돼 있어야 한다.

- [ ] **Step 2: 실험을 돌린다**

Run: `cargo run -p m0eval --release -- --hybrid --eval eval/public --eval eval/kra --eval eval/data --threads 8`
Expected: `저장: docs/m2/eval-hybrid.md · 카드 상한 결정 N`. 처음에는 새 카드(128)·청크(제목 없는 문서의 머리 변경분, 데이터 세트 전부)·검색어만 계산한다(캐시 `target/m0cache/embeddinggemma-2-Q8_0.gguf/`). 개발 PC 에서 수십 분.

- [ ] **Step 3: 판정을 읽고 따른다**

`docs/m2/eval-hybrid.md` §1 을 본다.
- **하이브리드 > 키워드 단독이 "실패"** 면 여기서 멈추고 사용자에게 보고한다(질의별 표 §8 과 함께). 매개변수를 손으로 맞추지 않는다 — 조정용으로 고르고 검증용으로 판정하는 원칙(DEC-051)을 깨기 때문.
- **날짜 가산이 "실패"** 면 §6 질의별 표에서 원인(해석이 틀림 / 가산이 약함 / 정답이 기간 밖)을 적어 사용자에게 보고한다. 해석이 틀린 경우만 고친다(`dates.tsv`·시험 추가) — 그 뒤 Step 2 부터 다시.
- §5 가 "기본 끔 안을 사용자에게 올린다" 면 수치와 함께 묻는다. 답을 받기 전에는 기본 켬.

- [ ] **Step 4: 고른 값을 제품 기본값으로 옮긴다**

`docs/m2/params.json` 의 값을 `crates/index/src/hybrid.rs` 의 `impl Default for HybridParams` 에 그대로 옮긴다(주석: `// docs/m2/params.json(Task 16, 2026-10-xx) — 조정용 질의로 고름(DEC-051·058)`). §4 결정이 128 이면 `crates/embed/src/plan.rs` 의 `CARD_TOKENS` 를 128 로 바꾸고 주석에 판정 근거(잘리는 비율·차이)를 적는다 — `plan::tag()` 가 바뀌므로 이미 만든 벡터는 다음 `index` 에서 다시 만들어진다(DEC-047).

hybrid 시험 중 기본값에 기대는 것(`rrf_scores_follow_formula` 는 `p.w_keyword[1]` 을 읽으므로 그대로 통과)이 있는지 `cargo test -p index --release hybrid` 로 확인한다.

- [ ] **Step 5: 기본값으로 다시 돌려 같은 결과가 나오는지 본다**

Run: `cargo run -p m0eval --release -- --hybrid --eval eval/public --eval eval/kra --eval eval/data --params docs/m2/params.json --dump target/m0eval-ranks.json`
Expected: §1·§2 의 숫자가 Step 2 와 같다(고르지 않고 같은 값을 썼으므로). `target/m0eval-ranks.json` 이 생긴다.

- [ ] **Step 6: 기록하고 커밋**

`docs/engineering/decisions.md` "M2 의미 검색·하이브리드" 표에 더한다:
```markdown
| DEC-058 | 하이브리드 기본 매개변수 = `docs/m2/params.json`(Task 16 실험, 조정용 질의 좌표 하강). 카드 상한 {256|128}(결정 #27 판정: 잘리는 카드 N% · 전체 차이 ±x · 마사회 유형 ±y). 데이터 뒷부분 기본 {켬|끔}(비데이터 매크로 차이 ±z) | DEC-051 의 숫자 규칙 | 실제 업무 문서에서 다를 수 있음 — 폐쇄망 확인(Task 20)으로 다시 판단(KI-045) | 유효 |
```
(중괄호 안은 보고서 값으로 채운다.) 그리고:
```bash
git add docs/m2/eval-hybrid.md docs/m2/params.json crates/index/src/hybrid.rs crates/embed/src/plan.rs docs/engineering/decisions.md
git commit -m "m2: hybrid parameters chosen on tune split, card cap and data-tail decisions (DEC-058)"
```

---

### Task 17: `kra-search eval --set` — 평가 세트를 제품 경로 그대로, 그리고 일치 검사

**Files:**
- Create: `crates/app/src/eval.rs`
- Modify: `crates/app/src/evalset.rs`(매니페스트·복사·질의 읽기), `crates/app/src/lib.rs`, `crates/app/src/main.rs`
- Test: `crates/app/tests/eval_set.rs`

**Interfaces:**
- Consumes: Task 15 `evalset::{file_name, id_of, tune_split, ndcg_at_k, recall_at_k}`, Task 14 `Searcher`, Task 13 `index` 흐름.
- Produces:
  - `app::evalset::{EvalQuery { qid, text, kind, relevant: HashSet<String> }, materialize(sets: &[PathBuf], out: &Path) -> Result<usize, String>, queries(set: &Path) -> Result<Vec<EvalQuery>, String>}`.
  - `app::eval::{rank_set(&Store, &mut Searcher, &[EvalQuery], Day) -> Result<BTreeMap<String, Vec<String>>, String>, set_report(&[EvalQuery], &BTreeMap<String, Vec<String>>) -> String}`.
  - CLI: `kra-search --home <폴더> eval --set <세트>... [--now] [--out <md>] [--dump <json>]` — `--home` 를 반드시 따로 준다(내 색인과 섞이지 않게).

- [ ] **Step 1: 시험을 쓴다**

`crates/app/tests/eval_set.rs`:
```rust
//! 평가 세트 → 실제 경로: 복사본 이름·수정일, 그리고 eval --set 이 순위 덤프를 남기는지(모델 없이 — 키워드만).

use app::evalset::{id_of, materialize, queries};
use query::calendar::{day_of_unix_ns, parse_ymd};
use std::path::Path;
use std::time::UNIX_EPOCH;

fn set_dir(root: &Path) -> std::path::PathBuf {
    let set = root.join("mini");
    std::fs::create_dir_all(set.join("docs")).unwrap();
    let fx = Path::new(env!("CARGO_MANIFEST_DIR")).join("../extract/tests/fixtures/office");
    std::fs::copy(fx.join("sample.docx"), set.join("docs/A0001.docx")).unwrap();
    std::fs::copy(fx.join("utf8.txt"), set.join("docs/A0002.txt")).unwrap();
    std::fs::write(
        set.join("manifest.toml"),
        "[[doc]]\nid = \"A0001\"\ntitle = \"운영 계획: 서울/부산\"\next = \"docx\"\ndate = \"2026-09-01\"\n\n[[doc]]\nid = \"A0002\"\ntitle = \"메모\"\next = \"txt\"\ndate = \"2025-05-01\"\n",
    )
    .unwrap();
    std::fs::write(set.join("queries.tsv"), "qid\tquery\ttype\trelevant\nZ001\t렛츠런파크\tkeyword\tA0001,A0002\nZ002\t작년 렛츠런파크\tdate\tA0002\t2025-01-01..2025-12-31\n").unwrap();
    set
}

#[test]
fn materialize_names_and_dates_files() {
    let d = tempfile::tempdir().unwrap();
    let set = set_dir(d.path());
    let out = d.path().join("out");
    assert_eq!(materialize(std::slice::from_ref(&set), &out).unwrap(), 2);
    let f = out.join("mini").join("운영 계획 서울 부산 (A0001).docx");
    assert!(f.exists());
    assert_eq!(id_of(f.to_str().unwrap()).as_deref(), Some("A0001"));
    let ns = std::fs::metadata(&f).unwrap().modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_nanos() as i64;
    assert_eq!(day_of_unix_ns(ns), parse_ymd("2026-09-01").unwrap());
    std::fs::write(out.join("mini").join("남은 파일.txt"), b"x").unwrap();
    materialize(std::slice::from_ref(&set), &out).unwrap();
    assert!(!out.join("mini").join("남은 파일.txt").exists(), "매니페스트에 없는 파일은 지운다");
    let qs = queries(&set).unwrap();
    assert_eq!(qs.len(), 2);
    assert_eq!(qs[1].relevant.iter().collect::<Vec<_>>(), vec!["A0002"]);
}

#[test]
fn eval_set_cli_writes_report_and_dump() {
    let d = tempfile::tempdir().unwrap();
    let set = set_dir(d.path());
    let home = d.path().join("home");
    let dump = d.path().join("ranks.json");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_kra-search"))
        .arg("--home").arg(&home)
        .arg("--model").arg(d.path().join("모델없음.gguf"))
        .args(["eval", "--now", "2026-10-15", "--keyword-only", "--set"])
        .arg(&set)
        .arg("--dump").arg(&dump)
        .output()
        .unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("nDCG@10"), "{text}");
    let ranks: std::collections::BTreeMap<String, Vec<String>> = serde_json::from_str(&std::fs::read_to_string(&dump).unwrap()).unwrap();
    let mut z1 = ranks["Z001"].clone();
    z1.sort();
    assert_eq!(z1, vec!["A0001", "A0002"], "복사본 파일 이름의 (id) 로 문서를 알아본다: {ranks:?}");
    assert!(ranks["Z002"].contains(&"A0002".to_string()), "{ranks:?}");
}
```
(`--keyword-only` 는 모델 없이 평가 경로를 시험하려는 숨김 플래그 — 실제 평가에서는 쓰지 않는다. 모델이 없는데 이 플래그가 없으면 `eval --set` 은 오류로 끝난다.)

- [ ] **Step 2: 시험이 실패하는지 본다**

Run: `cargo test -p app --release --test eval_set`
Expected: FAIL — `cannot find function materialize`.

- [ ] **Step 3: 복사와 질의 읽기를 구현한다**

`crates/app/src/evalset.rs` 의 `use` 를 다음으로 바꾸고:
```rust
use query::calendar::{noon_kst_unix_secs, parse_ymd};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
```
`first_relevant_rank` 아래에 더한다:
```rust
#[derive(serde::Deserialize)]
struct Manifest {
    doc: Vec<Entry>,
}

#[derive(serde::Deserialize)]
struct Entry {
    id: String,
    title: String,
    ext: String,
    date: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EvalQuery {
    pub qid: String,
    pub text: String,
    pub kind: String,
    pub relevant: HashSet<String>,
}

/// 평가 세트 문서를 `{out}/{세트 폴더 이름}/{file_name}` 으로 복사하고 수정일을 매니페스트 날짜 정오(한국 시각)로 둔다(DEC-052).
/// 같은 크기로 이미 있으면 복사하지 않고 날짜만 맞춘다. 매니페스트에 없는 파일은 지운다. date 가 없으면 오류.
pub fn materialize(sets: &[PathBuf], out: &Path) -> Result<usize, String> {
    let mut n = 0;
    for set in sets {
        let name = set.file_name().and_then(|x| x.to_str()).ok_or_else(|| format!("세트 폴더 이름을 읽지 못함: {}", set.display()))?;
        let dir = out.join(name);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let text = std::fs::read_to_string(set.join("manifest.toml")).map_err(|e| format!("{}: {e}", set.display()))?;
        let m: Manifest = toml::from_str(&text).map_err(|e| format!("{}/manifest.toml: {e}", set.display()))?;
        let mut keep = HashSet::new();
        for e in m.doc {
            let date = e.date.as_deref().and_then(parse_ymd).ok_or_else(|| format!("{} 에 date 가 없습니다(python3 tools/eval/check_sets.py 로 확인)", e.id))?;
            let src = set.join("docs").join(format!("{}.{}", e.id, e.ext));
            let len = std::fs::metadata(&src)
                .map_err(|_| format!("평가 문서가 없습니다: {} (tools/eval/fetch_all.py · import_local.py · make_data_set.py 먼저)", src.display()))?
                .len();
            let fname = file_name(&e.id, &e.title, &e.ext);
            let dst = dir.join(&fname);
            if std::fs::metadata(&dst).map(|m| m.len()).ok() != Some(len) {
                std::fs::copy(&src, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
            }
            let t = UNIX_EPOCH + Duration::from_secs(noon_kst_unix_secs(date) as u64);
            std::fs::File::options().write(true).open(&dst).and_then(|f| f.set_modified(t)).map_err(|e| format!("{}: {e}", dst.display()))?;
            keep.insert(fname);
            n += 1;
        }
        for e in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
            if !e.file_name().to_str().is_some_and(|f| keep.contains(f)) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(n)
}

/// queries.tsv: qid · 검색어 · 유형 · 정답(쉼표) · (date 질의만) 기간.
pub fn queries(set: &Path) -> Result<Vec<EvalQuery>, String> {
    let text = std::fs::read_to_string(set.join("queries.tsv")).map_err(|e| format!("{}/queries.tsv: {e}", set.display()))?;
    Ok(text
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            EvalQuery { qid: c[0].into(), text: c[1].into(), kind: c[2].into(), relevant: c[3].split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect() }
        })
        .collect())
}
```

- [ ] **Step 4: 평가 실행과 보고서를 구현한다**

`crates/app/src/lib.rs` 에 `pub mod eval;`.

`crates/app/src/eval.rs`:
```rust
//! `kra-search eval`: 평가 세트를 제품 경로 그대로(--set, DEC-052) 또는 사용자 질의로(--queries, 폐쇄망 확인 — Task 18).

use crate::evalset::{id_of, ndcg_at_k, recall_at_k, tune_split, EvalQuery};
use crate::search::Searcher;
use crate::store::Store;
use query::Day;
use std::collections::BTreeMap;
use std::fmt::Write;

/// 질의별 평가 문서 id 순위(상위 100). 경로 파일 이름의 "(id)" 로 문서를 알아본다.
pub fn rank_set(store: &Store, s: &mut Searcher, qs: &[EvalQuery], today: Day) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut out = BTreeMap::new();
    for q in qs {
        let (_, hits) = s.search(store, &q.text, today, 100)?;
        let mut ids: Vec<String> = Vec::new();
        for h in hits {
            for p in &h.hit.paths {
                if let Some(id) = id_of(p) {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
        }
        out.insert(q.qid.clone(), ids);
    }
    Ok(out)
}

pub fn set_report(qs: &[EvalQuery], ranks: &BTreeMap<String, Vec<String>>) -> String {
    let tune = tune_split(qs.iter().map(|q| (q.qid.as_str(), q.kind.as_str())));
    let mut by: BTreeMap<&str, Vec<(bool, f64, f64)>> = BTreeMap::new();
    for q in qs {
        let r = &ranks[&q.qid];
        by.entry(q.kind.as_str()).or_default().push((tune.contains(&q.qid), ndcg_at_k(r, &q.relevant, 10), recall_at_k(r, &q.relevant, 20)));
    }
    let mean = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
    let mut s = String::new();
    writeln!(s, "# kra-search eval --set (제품 경로)\n\n| 유형 | 질의 수 | nDCG@10 | Recall@20 |\n|---|---|---|---|").unwrap();
    let (mut mt, mut mh) = (Vec::new(), Vec::new());
    for (k, v) in &by {
        let n: Vec<f64> = v.iter().map(|x| x.1).collect();
        let r: Vec<f64> = v.iter().map(|x| x.2).collect();
        writeln!(s, "| {k} | {} | {:.4} | {:.4} |", v.len(), mean(&n), mean(&r)).unwrap();
        mt.push(mean(&v.iter().filter(|x| x.0).map(|x| x.1).collect::<Vec<_>>()));
        mh.push(mean(&v.iter().filter(|x| !x.0).map(|x| x.1).collect::<Vec<_>>()));
    }
    writeln!(s, "\n- 매크로 nDCG@10: 조정용 {:.4} · 검증용 {:.4}", mean(&mt), mean(&mh)).unwrap();
    s
}
```

- [ ] **Step 5: `index` 흐름을 함수로 빼고 `eval` 명령을 붙인다**

`crates/app/src/main.rs`:
1. `Cmd::Index` 가지의 본문(잠금부터 재시도 루프 끝까지)을 함수로 옮긴다 — `eval --set` 도 같은 길로 색인한다:
```rust
/// `index` 와 `eval --set` 이 같이 쓰는 색인 한 번(키워드 → 의미 검색 → meta 검사, 손상이면 한 번 다시).
fn index_command(cli: &Cli, home: &Home, analyzers: &Analyzers, pdfium: Option<PathBuf>, mut roots: Vec<PathBuf>, data_tail: Option<&str>) -> Result<(), String> {
    let _lock = lock_index(home)?;
    let mut store = open_store(home, analyzers)?;
    let pdf_reader = pdfium.as_deref().is_some_and(|p| extract::pdf::init(p).is_ok());
    if !pdf_reader {
        eprintln!("PDF 읽기 도구({})를 찾지 못해 PDF 는 이번에 건너뜁니다.", extract::pdf::LIB_NAME);
    }
    let mut worker = Worker::new(worker_cmd(pdfium));
    let timeout = Duration::from_secs(cli.timeout_secs);
    let mut rebuilt = false;
    loop {
        let t = Instant::now();
        let r = index_once(&mut store, &mut worker, &mut roots, timeout, pdf_reader).and_then(|s| {
            println!("{}", summary_line(&s));
            store.meta.set_kv("stat_keyword_secs", &format!("{:.1}", t.elapsed().as_secs_f64()))?;
            index_semantic(cli, home, &mut store, data_tail)?;
            // 검색이 읽는 쪽까지 모두 확인(DEC-056) — 손상이면 아래에서 한 번 다시 만든다.
            Ok(store.meta.check()?)
        });
        match r {
            // 열 때는 멀쩡했지만 색인 도중(키워드든 의미 검색이든) meta.db 손상이 드러남 — 한 번만 처음부터 다시 만든다.
            Err(IndexError::MetaCorrupt) if !rebuilt => {
                rebuilt = true;
                eprintln!();
                eprintln!("색인 데이터가 손상되어 처음부터 다시 만듭니다.");
                drop(store);
                reset_store(home)?;
                store = open_store(home, analyzers)?;
            }
            r => return r.map_err(|e| e.to_string()),
        }
    }
}
```
(PDF·지정 루트 주석 두 줄은 함수 안 해당 줄 위로 함께 옮긴다.) `Cmd::Index { root, data_tail }` 가지는:
```rust
        Cmd::Index { root, data_tail } => {
            // 지정한 루트(없으면 index_once 가 저장된 루트를 읽어 채운다). 다시 만들 때 이 값을 다시 저장한다.
            let roots: Vec<PathBuf> = root.iter().map(|r| absolute(r)).collect();
            index_command(&cli, &home, &analyzers, pdfium, roots, data_tail.as_deref())?;
        }
```
2. `Cmd` 에:
```rust
    /// 검색 품질 평가: --set 평가 세트(개발 PC), --queries 내 질의 파일(폐쇄망 확인)
    Eval {
        /// 평가 세트 폴더(manifest.toml·queries.tsv·docs/). 여러 번 지정 가능. --home 을 따로 지정해야 한다.
        #[arg(long)]
        set: Vec<PathBuf>,
        /// 내 질의 파일: 한 줄에 `질의<탭>정답 파일 이름[,정답 파일 이름…]`(UTF-8)
        #[arg(long)]
        queries: Option<PathBuf>,
        /// 보고서 파일(기본: --set 은 화면, --queries 는 kra-search-eval-report.txt)
        #[arg(long)]
        out: Option<PathBuf>,
        /// 질의별 순위 JSON(일치 검사용)
        #[arg(long, hide = true)]
        dump: Option<PathBuf>,
        /// 모델 없이 평가 경로만 시험(시험용)
        #[arg(long, hide = true)]
        keyword_only: bool,
    },
```
3. 가지:
```rust
        Cmd::Eval { set, queries, out, dump, keyword_only } => {
            if !set.is_empty() {
                if cli.home.is_none() {
                    return Err("평가 세트는 --home 을 따로 지정하세요(내 색인과 섞이지 않게).".into());
                }
                let docs_dir = absolute(&home.root().join("eval-docs"));
                eprintln!("평가 문서 {}개 준비", app::evalset::materialize(set, &docs_dir)?);
                index_command(&cli, &home, &analyzers, pdfium, vec![docs_dir], None)?;
                let store = open_store_readonly(&home, &analyzers)?;
                let threads = cli.threads.unwrap_or_else(platform::cpu::default_embed_threads);
                let mut searcher = app::search::Searcher::open(&store, &home, model_path(&cli.model).as_deref(), threads)?;
                if let Some(n) = &searcher.notice {
                    return Err(format!("평가에는 의미 검색이 있어야 합니다: {n}"));
                }
                if model_path(&cli.model).is_none() && !keyword_only {
                    return Err(format!("평가에는 의미 검색 모델이 있어야 합니다({MODEL_FILE}, --model)."));
                }
                let mut qs = Vec::new();
                for d in set {
                    qs.extend(app::evalset::queries(d)?);
                }
                let ranks = app::eval::rank_set(&store, &mut searcher, &qs, today(&cli.now)?)?;
                let report = app::eval::set_report(&qs, &ranks);
                match out {
                    Some(p) => std::fs::write(p, &report).map_err(|e| format!("{}: {e}", p.display()))?,
                    None => println!("{report}"),
                }
                if let Some(d) = dump {
                    std::fs::write(d, serde_json::to_string(&ranks).expect("json")).map_err(|e| format!("{}: {e}", d.display()))?;
                }
            } else if let Some(_tsv) = queries {
                return Err("--queries 는 Task 18 에서".into());
            } else {
                return Err("--set 또는 --queries 를 주세요.".into());
            }
        }
```

- [ ] **Step 6: 시험이 통과하는지 본다**

Run: `cargo test -p app --release`
Expected: PASS.

- [ ] **Step 7: 제품 경로로 평가 세트를 돌려 일치 검사를 한다**

Run:
```bash
cargo run -p app --release -- --home target/eval-home --model models/embeddinggemma-2-Q8_0.gguf --threads 8 \
  eval --now 2026-10-15 --set eval/public --set eval/kra --set eval/data --out target/app-eval.md --dump target/app-ranks.json
cargo run -p m0eval --release -- --eval eval/public --eval eval/kra --eval eval/data --parity target/app-ranks.json
```
Expected: 첫 명령은 색인(처음 한 번 수십 분 — 평가 색인 폴더 `target/eval-home` 는 다음에 재사용, DEC-052)과 보고서. 둘째 명령은 `docs/m2/parity.md` 를 쓰고 `판정 **통과**`(종료 코드 0). 실패하면 "상위 10개가 다른 질의" 목록의 첫 질의를 양쪽에서 비교한다: 문서 날짜(수정일), 카드·청크 수(`status`), 검색어 텍스트(날짜 빼기) 순으로 — 원인과 수정은 INC 로 기록한다.

- [ ] **Step 8: 커밋**

```bash
git add crates/app docs/m2/parity.md Cargo.lock
git commit -m "app: eval --set runs eval sets through the product path (copies with manifest dates); parity with m0eval passes"
```

---

### Task 18: `kra-search eval --queries` — 폐쇄망 확인용 품질 보고서

**Files:**
- Modify: `crates/app/src/eval.rs`, `crates/app/src/main.rs`, `crates/index/src/meta.rs`(`all_files`), `CLAUDE.md`(보고서 원칙 한 줄)
- Test: `crates/app/tests/eval_queries.rs`

**Interfaces:**
- Produces:
  - `index::meta::Meta::all_files(&self) -> Result<Vec<FileRow>>`.
  - `app::eval::{UserQuery { line: usize, text: String, answers: Vec<String> }, read_user_queries(&str) -> Vec<UserQuery>, UserResult { line, has_date, found, on: (f64, f64, Option<usize>), off: (f64, f64, Option<usize>), ms }, run_user_queries(&Store, &mut Searcher, &[UserQuery], Day) -> Result<Vec<UserResult>, String>, PcInfo { perf: Option<usize>, eff: Option<usize>, logical: usize, threads: usize }, IndexInfo { files, contents, failed: u64, embed: EmbedCounts, stats: Vec<(String, String)> }, user_report(&[UserResult], &IndexInfo, &PcInfo, model_ms: f64) -> String}`.
  - 보고서에는 **질의 원문·정답 파일 이름·경로·문서 내용이 없다** — `user_report` 는 그것들을 받지도 않는다(구조로 보장) + 종단 시험.

- [ ] **Step 1: 시험을 쓴다**

`crates/app/tests/eval_queries.rs`:
```rust
//! 폐쇄망 확인 보고서: 지표는 나오고, 질의·파일 이름·경로는 나오지 않는다(INC-010 원칙).

use std::path::Path;
use std::process::Command;

#[test]
fn user_query_report_has_metrics_but_no_text_names_or_paths() {
    let d = tempfile::tempdir().unwrap();
    let docs = d.path().join("업무문서");
    std::fs::create_dir_all(&docs).unwrap();
    let fx = Path::new(env!("CARGO_MANIFEST_DIR")).join("../extract/tests/fixtures/office");
    std::fs::copy(fx.join("sample.docx"), docs.join("홍길동_운영계획.docx")).unwrap();
    std::fs::copy(fx.join("utf8.txt"), docs.join("비밀메모.txt")).unwrap();
    let home = d.path().join("home");
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_kra-search"))
            .arg("--home").arg(&home)
            .arg("--model").arg(d.path().join("모델없음.gguf"))
            .args(args)
            .current_dir(d.path())
            .output()
            .unwrap();
        let t = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success(), "{args:?}\n{t}");
        t
    };
    run(&["index", "--root", docs.to_str().unwrap()]);
    let tsv = d.path().join("내질의.tsv");
    std::fs::write(&tsv, "질의\t정답\n홍길동 렛츠런파크 운영\t홍길동_운영계획.docx\n작년 비밀 메모\t비밀메모.txt\n없는 문서 찾기\t없는파일.hwp\n").unwrap();
    let out = run(&["eval", "--now", "2026-10-15", "--queries", tsv.to_str().unwrap()]);
    let report = std::fs::read_to_string(d.path().join("kra-search-eval-report.txt")).unwrap();
    assert!(report.contains("nDCG@10") && report.contains("p95"), "{report}");
    assert!(report.contains("정답 파일을 색인에서 찾지 못한 질의 1개"), "{report}");
    for secret in ["홍길동", "운영계획", "비밀", "렛츠런파크", "없는파일", "업무문서", d.path().to_str().unwrap()] {
        assert!(!report.contains(secret), "보고서에 '{secret}' 가 있으면 안 된다:\n{report}");
        assert!(!out.contains(secret) || secret == d.path().to_str().unwrap(), "화면 출력에도 질의·파일 이름은 없다: {secret}\n{out}");
    }
}

#[test]
fn reads_tsv_with_header_bom_and_multiple_answers() {
    let qs = app::eval::read_user_queries("\u{feff}질의\t정답\n가 나 다\ta.hwp, b.pdf\n\n라마\tc.txt\n");
    assert_eq!(qs.len(), 2);
    assert_eq!((qs[0].line, qs[0].answers.clone()), (2, vec!["a.hwp".to_string(), "b.pdf".to_string()]));
    assert_eq!(qs[1].line, 4);
}
```

- [ ] **Step 2: 시험이 실패하는지 본다**

Run: `cargo test -p app --release --test eval_queries`
Expected: FAIL — `cannot find function read_user_queries`.

- [ ] **Step 3: meta 에 전체 파일 목록을 더한다**

`crates/index/src/meta.rs` 의 `files_of` 아래에:
```rust
    /// 모든 경로(평가: 정답 파일 이름으로 내용을 찾는다).
    pub fn all_files(&self) -> Result<Vec<FileRow>> {
        let mut st = self.conn.prepare("SELECT path, size, mtime_ns, content_id FROM file ORDER BY path")?;
        let v = st
            .query_map([], |r| Ok(FileRow { path: r.get(0)?, size: r.get::<_, i64>(1)? as u64, mtime_ns: r.get(2)?, content_id: r.get(3)? }))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }
```

- [ ] **Step 4: 사용자 질의 평가를 구현한다**

`crates/app/src/eval.rs` 에 더한다(파일 위 `use` 에 `use crate::evalset::first_relevant_rank;`, `use index::meta::EmbedCounts;`, `use std::collections::{HashMap, HashSet};`, `use std::path::Path;`, `use std::time::Instant;` 를 합친다):
```rust
/// 사용자 질의 한 줄(줄 번호는 보고서에서 질의를 가리키는 유일한 단서 — 질의 글은 남기지 않는다).
#[derive(Debug, Clone, PartialEq)]
pub struct UserQuery {
    pub line: usize,
    pub text: String,
    pub answers: Vec<String>,
}

/// `질의<탭>정답 파일 이름[,정답…]`. BOM·빈 줄·첫 줄 머리("질의"/"query")는 건너뛴다.
pub fn read_user_queries(text: &str) -> Vec<UserQuery> {
    let text = text.trim_start_matches('\u{feff}');
    let mut out = Vec::new();
    for (i, l) in text.lines().enumerate() {
        let Some((q, a)) = l.split_once('\t') else { continue };
        let q = q.trim();
        if q.is_empty() || (i == 0 && (q == "질의" || q.eq_ignore_ascii_case("query"))) {
            continue;
        }
        let answers: Vec<String> = a.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
        if !answers.is_empty() {
            out.push(UserQuery { line: i + 1, text: q.to_string(), answers });
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct UserResult {
    pub line: usize,
    /// 날짜 표현이 있는 질의(칩이 생김)
    pub has_date: bool,
    /// 정답 파일 이름을 색인에서 찾았는가
    pub found: bool,
    /// 날짜 가산 켬: (nDCG@10, Recall@20, 첫 정답 순위)
    pub on: (f64, f64, Option<usize>),
    /// 날짜 가산 끔
    pub off: (f64, f64, Option<usize>),
    /// 검색 한 번(해석 + 뜻 계산 + 검색) 밀리초
    pub ms: f64,
}

fn file_key(name: &str) -> String {
    name.trim().to_lowercase()
}

pub fn run_user_queries(store: &Store, s: &mut Searcher, qs: &[UserQuery], today: Day) -> Result<Vec<UserResult>, String> {
    // 파일 이름(소문자) → 내용 id. 확장자 없이 쓴 정답은 이름 앞부분으로도 찾는다.
    let mut by_name: HashMap<String, HashSet<i64>> = HashMap::new();
    for f in store.meta.all_files().map_err(crate::store::user_error)? {
        let Some(id) = f.content_id else { continue };
        let name = Path::new(&f.path).file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let stem = Path::new(&name).file_stem().and_then(|n| n.to_str()).unwrap_or("").to_string();
        by_name.entry(file_key(&name)).or_default().insert(id);
        by_name.entry(file_key(&stem)).or_default().insert(id);
    }
    let base = s.params.clone();
    let mut out = Vec::new();
    for q in qs {
        let relevant: HashSet<String> = q.answers.iter().filter_map(|a| by_name.get(&file_key(a))).flatten().map(|id| id.to_string()).collect();
        let t = Instant::now();
        let (pq, hits) = s.search(store, &q.text, today, 100)?;
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let ids = |hits: &[crate::search::HybridHit]| hits.iter().map(|h| h.hit.content_id.to_string()).collect::<Vec<_>>();
        let on_r = ids(&hits);
        s.params = index::hybrid::HybridParams { date_exact: 0.0, date_vague: 0.0, ..base.clone() };
        let (_, hits_off) = s.search(store, &q.text, today, 100)?;
        s.params = base.clone();
        let off_r = ids(&hits_off);
        let m = |r: &[String]| (ndcg_at_k(r, &relevant, 10), recall_at_k(r, &relevant, 20), first_relevant_rank(r, &relevant));
        out.push(UserResult { line: q.line, has_date: pq.chips().count() > 0, found: !relevant.is_empty(), on: m(&on_r), off: m(&off_r), ms });
    }
    Ok(out)
}

pub struct PcInfo {
    pub perf: Option<usize>,
    pub eff: Option<usize>,
    pub logical: usize,
    pub threads: usize,
}

pub struct IndexInfo {
    pub files: u64,
    pub contents: u64,
    pub failed: u64,
    pub embed: EmbedCounts,
    /// kv stat_* (마지막 index 기준)
    pub stats: Vec<(String, String)>,
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(((v.len().max(1) - 1) as f64 * p).round() as usize).copied().unwrap_or(0.0)
}

/// 반출용 보고서. 질의 글·파일 이름·경로는 받지 않으므로 들어갈 수 없다(M2 설계 §7, INC-010 원칙).
pub fn user_report(rs: &[UserResult], ix: &IndexInfo, pc: &PcInfo, model_ms: f64) -> String {
    let mean = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
    let found: Vec<&UserResult> = rs.iter().filter(|r| r.found).collect();
    let missing: Vec<String> = rs.iter().filter(|r| !r.found).map(|r| r.line.to_string()).collect();
    let nd: Vec<f64> = found.iter().map(|r| r.on.0).collect();
    let rc: Vec<f64> = found.iter().map(|r| r.on.1).collect();
    let top1 = found.iter().filter(|r| r.on.2 == Some(1)).count();
    let top10 = found.iter().filter(|r| r.on.2.is_some_and(|x| x <= 10)).count();
    let fails: Vec<String> = found.iter().filter(|r| !r.on.2.is_some_and(|x| x <= 10)).map(|r| r.line.to_string()).collect();
    let (dated, plain): (Vec<&&UserResult>, Vec<&&UserResult>) = found.iter().partition(|r| r.has_date);
    let (mut w, mut t, mut l) = (0, 0, 0);
    for r in &dated {
        let d = r.on.0 - r.off.0;
        if d > 1e-9 {
            w += 1
        } else if d < -1e-9 {
            l += 1
        } else {
            t += 1
        }
    }
    let mut ms: Vec<f64> = rs.iter().map(|r| r.ms).collect();
    let mut s = String::new();
    let none = |v: &[String]| if v.is_empty() { "없음".to_string() } else { v.join(", ") };
    writeln!(s, "KRA 문서검색 — 폐쇄망 품질 확인 보고서 (M2)").unwrap();
    writeln!(s, "이 파일에는 검색어·파일 이름·경로·문서 내용이 들어 있지 않습니다. 줄 번호는 질의 파일의 줄입니다.\n").unwrap();
    writeln!(s, "[요약]").unwrap();
    writeln!(s, "질의 {}개 · 정답 파일을 색인에서 찾지 못한 질의 {}개(줄: {})", rs.len(), missing.len(), none(&missing)).unwrap();
    writeln!(s, "nDCG@10 {:.4} · Recall@20 {:.4} · 1위 정답 {top1}개 · 10위 안 {top10}개 (정답을 찾은 질의 {}개 기준)", mean(&nd), mean(&rc), found.len()).unwrap();
    writeln!(
        s,
        "날짜 표현 있는 질의 {}개: nDCG@10 가산 켬 {:.4} · 끔 {:.4} (승 {w} 무 {t} 패 {l})",
        dated.len(),
        mean(&dated.iter().map(|r| r.on.0).collect::<Vec<_>>()),
        mean(&dated.iter().map(|r| r.off.0).collect::<Vec<_>>())
    )
    .unwrap();
    writeln!(s, "날짜 표현 없는 질의 {}개: nDCG@10 {:.4}", plain.len(), mean(&plain.iter().map(|r| r.on.0).collect::<Vec<_>>())).unwrap();
    writeln!(s, "10위 밖 질의(줄): {}\n", none(&fails)).unwrap();
    writeln!(s, "[색인]").unwrap();
    writeln!(s, "파일 {} · 내용 {} · 내용을 못 읽은 것 {}", ix.files, ix.contents, ix.failed).unwrap();
    let e = &ix.embed;
    writeln!(s, "빠른 의미 검색 {}/{} · 정밀 의미 검색 {}/{} · 데이터 파일 뒷부분 {}/{}", e.cards_done, e.cards_total, e.main_done, e.main_total, e.tail_done, e.tail_total).unwrap();
    for (k, v) in &ix.stats {
        writeln!(s, "{k}: {v}").unwrap();
    }
    writeln!(s, "\n[속도]").unwrap();
    writeln!(s, "검색 p50 {:.0}ms · p95 {:.0}ms (검색어 해석 + 뜻 계산 + 검색, 모델을 불러온 뒤) · 모델 불러오기 {model_ms:.0}ms", pct(&mut ms, 0.5), pct(&mut ms, 0.95)).unwrap();
    writeln!(s, "\n[PC]").unwrap();
    let opt = |x: Option<usize>| x.map_or("알 수 없음".to_string(), |v| v.to_string());
    writeln!(s, "성능 코어 {} · 효율 코어 {} · 논리 코어 {} · 의미 검색 스레드 {}", opt(pc.perf), opt(pc.eff), pc.logical, pc.threads).unwrap();
    writeln!(s, "\n[질의별] 줄 · 날짜 표현 · 정답 순위(가산 켬) · 정답 순위(가산 끔)").unwrap();
    for r in rs {
        let rank = |x: Option<usize>| x.map_or("-".to_string(), |v| v.to_string());
        let shown = if r.found { format!("{} · {}", rank(r.on.2), rank(r.off.2)) } else { "정답 파일 없음".into() };
        writeln!(s, "{} · {} · {shown}", r.line, if r.has_date { "있음" } else { "없음" }).unwrap();
    }
    s
}
```

- [ ] **Step 5: 명령에 붙인다**

`crates/app/src/main.rs` 의 `Cmd::Eval` 가지에서 `--queries` 자리를 바꾼다:
```rust
            } else if let Some(tsv) = queries {
                let store = open_store_readonly(&home, &analyzers)?;
                let bytes = std::fs::read(tsv).map_err(|e| format!("질의 파일을 읽지 못했습니다: {e}"))?;
                let text = String::from_utf8(bytes).map_err(|_| "질의 파일을 UTF-8 로 저장해 주세요(메모장: 다른 이름으로 저장 → 인코딩 UTF-8).".to_string())?;
                let qs = app::eval::read_user_queries(&text);
                if qs.is_empty() {
                    return Err("질의가 없습니다 — 한 줄에 `질의<탭>정답 파일 이름`".into());
                }
                let threads = cli.threads.unwrap_or_else(platform::cpu::default_embed_threads);
                let t = Instant::now();
                let mut s = app::search::Searcher::open(&store, &home, model_path(&cli.model).as_deref(), threads)?;
                let model_ms = t.elapsed().as_secs_f64() * 1000.0;
                if let Some(n) = &s.notice {
                    eprintln!("{n}");
                }
                let rs = app::eval::run_user_queries(&store, &mut s, &qs, today(&cli.now)?)?;
                let me = |e: index::meta::MetaError| IndexError::from(e).to_string();
                let c = store.meta.counts().map_err(me)?;
                let mut stats = Vec::new();
                for (k, label) in [("stat_keyword_secs", "키워드 검색 색인(초)"), ("stat_cards_secs", "빠른 의미 검색 색인(초)"), ("stat_body_secs", "정밀 의미 검색 색인(초)"), ("stat_threads", "색인 스레드")] {
                    if let Some(v) = store.meta.kv(k).map_err(me)? {
                        stats.push((label.to_string(), v));
                    }
                }
                let ix = app::eval::IndexInfo { files: c.files, contents: c.contents, failed: c.failed, embed: store.meta.embed_counts(embed::plan::DATA_HEAD).map_err(me)?, stats };
                let split = platform::cpu::core_split();
                let pc = app::eval::PcInfo {
                    perf: split.map(|x| x.0),
                    eff: split.map(|x| x.1),
                    logical: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
                    threads,
                };
                let report = app::eval::user_report(&rs, &ix, &pc, model_ms);
                let path = out.clone().unwrap_or_else(|| PathBuf::from("kra-search-eval-report.txt"));
                std::fs::write(&path, &report).map_err(|e| format!("보고서를 쓰지 못했습니다: {e}"))?;
                println!("{report}");
                eprintln!("보고서: {}", path.display());
            } else {
```
(`eprintln!("보고서: …")` 의 경로는 사용자 PC 화면에만 나온다 — 보고서 파일에는 없다.)

`CLAUDE.md` "반드시 지킬 것" 의 진단 보고서 줄 뒤에 한 줄:
```markdown
- **`kra-search eval --queries` 보고서도 같다**: 질의 원문·정답 파일 이름·경로·문서 내용을 남기지 않는다(줄 번호와 지표만, `crates/app/tests/eval_queries.rs`).
```

- [ ] **Step 6: 시험이 통과하는지 본다**

Run: `cargo test -p app --release && cargo test -p index --release meta`
Expected: PASS. 시험의 화면 출력 검사에서 임시 폴더 경로는 예외다(`보고서: …` 줄).

- [ ] **Step 7: 커밋**

```bash
git add crates/app crates/index CLAUDE.md
git commit -m "app: eval --queries for the air-gapped check — metrics by line number, no query text, names or paths"
```

---

### Task 19: 검색 지연 벤치 — 본문 청크 50만

**Files:**
- Create: `crates/index/tests/bench_hybrid.rs`

**Interfaces:**
- Consumes: Task 8 `VectorSet`·`semantic`·`fuse`.

- [ ] **Step 1: 벤치를 쓴다**

`crates/index/tests/bench_hybrid.rs`:
```rust
//! 스펙 §13 "의미 포함 최종 결과 p95 ≤ 500ms(본문 청크 50만)" 중 검색 코어 몫(벡터 스캔 + 결합).
//! 검색어 임베딩(폐쇄망 Q8 p95 62ms, results.md §5.2)은 여기서 재지 않는다 — 코어는 400ms 안이어야 한다.
//! 실행: cargo test -p index --release --test bench_hybrid -- --include-ignored --nocapture

use index::hybrid::{fuse, semantic, HybridParams, MapMeta, VectorSet};
use query::calendar::parse_ymd;
use query::{parse, DateTable};
use std::collections::HashSet;
use std::time::Instant;
use vector::record::{Layout, Record};

fn rand_vecs(n: usize, seed: u64) -> impl Iterator<Item = Vec<f32>> {
    let mut s = seed;
    (0..n).map(move |_| {
        (0..768)
            .map(|_| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((s >> 33) as f32 / (1u64 << 31) as f32) - 0.5
            })
            .collect()
    })
}

#[test]
#[ignore = "벤치(약 1분, 메모리 약 400MB)"]
fn semantic_and_fuse_p95_under_400ms_at_500k_chunks() {
    let (mut cards, mut chunks) = (VectorSet::new(Layout::M0), VectorSet::new(Layout::M0));
    for (i, v) in rand_vecs(10_000, 1).enumerate() {
        cards.push(i as i64, &Record::from_full(&v, Layout::M0).unwrap());
    }
    for (i, v) in rand_vecs(500_000, 2).enumerate() {
        chunks.push((i / 50) as i64, &Record::from_full(&v, Layout::M0).unwrap());
    }
    let today = parse_ymd("2026-10-15").unwrap();
    let pq = parse("작년 내부통제 규정 개정 이유", today, &DateTable::builtin());
    let p = HybridParams::default();
    let meta = MapMeta { dates: (0..10_000).map(|i| (i, today - (i as i32 % 2000))).collect(), chunked: (0..10_000).collect() };
    let keyword: Vec<(i64, f32)> = (0..200).map(|i| (i * 37 % 10_000, 10.0 - i as f32 * 0.01)).collect();
    let mut ms = Vec::new();
    let mut checksum = 0usize;
    for q in rand_vecs(40, 3) {
        let t = Instant::now();
        let sem = semantic(&cards, &chunks, &q, &p);
        let hits = fuse(&keyword, &sem, &pq, &p, today, &meta, &HashSet::new());
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
        checksum += hits.len();
    }
    ms.sort_by(f64::total_cmp);
    let p95 = ms[(ms.len() - 1) * 95 / 100];
    println!("청크 50만 · 카드 1만: p50 {:.1}ms · p95 {:.1}ms (결과 합 {checksum})", ms[ms.len() / 2], p95);
    assert!(checksum > 0);
    assert!(p95 < 400.0, "p95 {p95:.1}ms");
}
```

- [ ] **Step 2: 실행한다**

Run: `cargo test -p index --release --test bench_hybrid -- --include-ignored --nocapture`
Expected: PASS, `p95` 한 줄(개발 PC 수십 ms 예상). 이 값을 Task 21 의 결과 문서에 적는다.

- [ ] **Step 3: 커밋**

```bash
git add crates/index/tests/bench_hybrid.rs
git commit -m "index: 500k-chunk search-core latency bench (spec §13)"
```

---

### Task 20: CI 실모델 종단 시험과 폐쇄망 확인 릴리즈

**Files:**
- Modify: `.github/workflows/ci.yml`, `.claude/rules/windows-release.md`
- Create: `.github/workflows/release-check.yml`, `docs/check/README-check.txt`, `docs/check/질의-예시.tsv`, `docs/engineering/runbooks/release-check.md`

**Interfaces:**
- Produces: 태그 `m2-check-N` → Windows 에서 빌드·스모크·프리릴리즈 `kra-search-m2-check-N-win64.zip`(+ `.sha256`).

- [ ] **Step 1: CI 에 실모델 종단 시험과 벤치를 더한다**

`.github/workflows/ci.yml` 의 `model tests` 단계 `run:` 끝에 두 줄:
```yaml
          # M2: 실제 모델로 색인 → 의미 검색(명령줄 그대로), 청크 50만 검색 코어 지연
          cargo test -p app --release --test semantic -- --include-ignored --test-threads 1
          cargo test -p index --release --test bench_hybrid -- --include-ignored --nocapture
```
`.claude/rules/windows-release.md` 의 실모델 단계 줄에 "와 `app --test semantic`·`index --test bench_hybrid`" 를 덧붙인다.

- [ ] **Step 2: 사용자 안내문을 쓴다**

`docs/check/README-check.txt`(zip 에 넣는다, 사용자가 읽는 글):
```text
KRA 문서검색 — 폐쇄망 품질 확인 (M2)

이 프로그램은 PC 안에서만 동작하고 아무것도 밖으로 보내지 않습니다.
색인은 %LOCALAPPDATA%\KraSearch 에 만들어집니다(확인이 끝나면 그 폴더를 지워도 됩니다).

1. 압축을 풉니다(예: D:\kra-check). 폴더 안에서 주소창에 cmd 를 쳐 명령 프롬프트를 엽니다.
2. 색인: kra-search.exe index --root "D:\업무문서"
   - 화면에 "빠른 의미 검색 n/N · 정밀 의미 검색 n/N" 이 늘어납니다. 끝까지 기다립니다(문서가 많으면 몇 시간).
   - 중간에 꺼도 다시 같은 명령을 치면 이어서 합니다.
3. 질의 파일 만들기: 메모장에서 한 줄에 하나씩
       찾고 싶은 말<Tab>정답 파일 이름
   예) 작년 적극행정 추진계획<Tab>2025년 적극행정 추진계획(안).hwp
   - 20~30줄. "작년·지난달·올해 상반기"처럼 날짜 표현을 넣은 줄도 섞어 주세요.
   - 정답 파일이 여러 개면 쉼표로: 파일1.hwp, 파일2.pdf
   - 다른 이름으로 저장 → 인코딩 UTF-8 → 내질의.tsv
   - 질의-예시.tsv 를 고쳐 써도 됩니다.
4. 평가: kra-search.exe eval --queries 내질의.tsv
   - 같은 폴더에 kra-search-eval-report.txt 가 생깁니다. 이 파일만 반출합니다.
   - 보고서에는 검색어·파일 이름·경로·문서 내용이 없습니다(줄 번호와 숫자만). 반출 전에 열어 확인해 주세요.
5. (선택) 검색해 보기: kra-search.exe search 찾고 싶은 말
```

`docs/check/질의-예시.tsv`(탭 구분, UTF-8):
```text
질의	정답
작년 적극행정 추진계획	2025년 적극행정 추진계획(안).hwp
일하다 실수했을 때 징계 안 받는 경우	내부통제규정.hwp
```

- [ ] **Step 3: 릴리즈 워크플로를 쓴다**

`.github/workflows/release-check.yml`:
```yaml
name: release-check
on:
  push:
    tags: ["m2-check-*"]
permissions: { contents: write }
jobs:
  build:
    runs-on: windows-latest
    steps:
      - run: git config --global core.autocrlf false
      - uses: actions/checkout@v4
        with: { submodules: recursive }
      - uses: dtolnay/rust-toolchain@1.97.0
      - name: fetch rhwp sources
        shell: bash
        run: bash tools/fetch-rhwp.sh
      - name: fetch pdfium
        shell: bash
        run: bash tools/fetch-pdfium.sh win-x64
      - name: fetch ko-dic
        shell: bash
        run: bash tools/fetch-ko-dic.sh
      - run: echo "LIBCLANG_PATH=C:\Program Files\LLVM\bin" >> $env:GITHUB_ENV
      - run: echo "OPENSSL_SRC_PERL=C:\Strawberry\perl\bin\perl.exe" >> $env:GITHUB_ENV
      - run: cargo build --release -p app
      - name: package
        shell: bash
        run: |
          set -euo pipefail
          OUT=kra-search-${GITHUB_REF_NAME}-win64
          mkdir -p $OUT/models
          cp target/release/kra-search.exe $OUT/
          find target/release/build -path '*llama-sys*' -name '*.dll' -exec cp {} $OUT/ \;
          for f in vcruntime140.dll vcruntime140_1.dll msvcp140.dll; do cp "/c/Windows/System32/$f" $OUT/; done
          bash tools/fetch-models.sh
          cp models/embeddinggemma-2-Q8_0.gguf $OUT/models/
          cp third_party/pdfium/win-x64/bin/pdfium.dll $OUT/
          mkdir -p $OUT/licenses/pdfium && cp -r third_party/pdfium/win-x64/LICENSE third_party/pdfium/win-x64/licenses/* $OUT/licenses/pdfium/
          cp docs/check/README-check.txt docs/check/질의-예시.tsv $OUT/
          ls -la $OUT $OUT/models
          7z a $OUT.zip $OUT > /dev/null
          sha256sum $OUT.zip > $OUT.zip.sha256
          cat $OUT.zip.sha256
          echo "OUT=$OUT" >> $GITHUB_ENV
      - name: smoke test (packaged build, default model and pdfium next to exe)
        shell: bash
        run: |
          set -euo pipefail
          cd "$OUT"
          ./kra-search.exe --home "$RUNNER_TEMP/kh" index --root "$GITHUB_WORKSPACE/crates/extract/tests/fixtures" | tee "$RUNNER_TEMP/index.txt"
          grep -q "의미 검색: 빠른" "$RUNNER_TEMP/index.txt"
          ./kra-search.exe --home "$RUNNER_TEMP/kh" search 렛츠런파크 | tee "$RUNNER_TEMP/search.txt"
          grep -q "korean.pdf" "$RUNNER_TEMP/search.txt"
          grep -q "뜻이 비슷함" "$RUNNER_TEMP/search.txt"
          printf '질의\t정답\n렛츠런파크 운영 방안\tsample.docx\n' > "$RUNNER_TEMP/q.tsv"
          ./kra-search.exe --home "$RUNNER_TEMP/kh" eval --queries "$RUNNER_TEMP/q.tsv" --out "$RUNNER_TEMP/r.txt"
          grep -q "nDCG@10" "$RUNNER_TEMP/r.txt"
          # set -e 는 `! grep` 실패로 멈추지 않는다 — 직접 끝낸다.
          for secret in 렛츠런파크 sample.docx fixtures; do
            if grep -q "$secret" "$RUNNER_TEMP/r.txt"; then echo "보고서에 '$secret' 가 있다"; exit 1; fi
          done
      - name: release
        env: { GH_TOKEN: "${{ github.token }}" }
        shell: bash
        run: gh release create "$GITHUB_REF_NAME" "$OUT.zip" "$OUT.zip.sha256" --prerelease --title "폐쇄망 품질 확인 $GITHUB_REF_NAME" --notes-file docs/check/README-check.txt
```

- [ ] **Step 4: 안내서(runbook)를 쓴다**

`docs/engineering/runbooks/release-check.md`:
```markdown
# 폐쇄망 품질 확인 릴리즈(kra-search 명령줄)

M2 완료 조건의 폐쇄망 확인(M2 설계 §7). 사용자가 반입해 업무 폴더를 색인하고 자기 질의로 `eval --queries` 를 돌려 보고서 하나를 반출한다. **잘못된 릴리즈는 사용자의 하루를 버리게 한다**(INC-006·009 와 같은 원칙).

## 태그 전 점검(INC-020)
이 결과로 정할 것과 그 숫자가 보고서에 있는지 확인한다(로컬에서 `kra-search eval --queries docs/check/질의-예시.tsv` 를 돌려 보고서를 직접 본다):

| 정할 것 | 보고서의 숫자 |
|---|---|
| 개발 PC 매개변수를 실제 문서에서 다시 정할지 | nDCG@10·Recall@20·1위/10위 안 개수, 10위 밖 줄 번호 |
| 날짜 가산이 실제 수정일에서도 이득인지 | 날짜 표현 있는 질의의 가산 켬/끔과 승·무·패 |
| 실제 색인 시간(카드 30분 기준 KI-041, 본문) | [색인] 단계별 초, 청크 수 |
| 검색 지연 §13 | [속도] p50·p95, 모델 불러오기 |
| 스레드 정책(DEC-050) | [PC] 성능·효율 코어, 스레드 |

## 절차
1. 브랜치 CI(macOS·Windows·deny)가 통과했는지 확인.
2. `git tag m2-check-N && git push origin m2-check-N` → `.github/workflows/release-check.yml` 이 Windows 에서 빌드·패키지·스모크·프리릴리즈.
   - zip: `kra-search.exe`, llama/ggml DLL 전부, VC++ 런타임 3개(INC-005), `models/embeddinggemma-2-Q8_0.gguf`, `pdfium.dll`, `licenses/pdfium/`, `README-check.txt`, `질의-예시.tsv`.
3. 스모크 확인: 워크플로 로그에서 `의미 검색: 빠른 …개`(모델·DLL 이 실행 파일 옆에서 불려 왔다), `뜻이 비슷함`(의미 경로), 보고서에 검색어·파일 이름이 없음.
4. 사용자에게 전달: 릴리즈 링크, zip SHA-256, `README-check.txt` 요약.
5. 결과는 `docs/m2/results.md` "폐쇄망 확인" 절에 기록하고(보고서 원본은 `eval/downloads/m2-check-N/`, 커밋하지 않음), 개발 PC 결과와 크게 다르면 매개변수를 다시 정할지 사용자와 정한다.
6. 불량 릴리즈는 지우지 말고 릴리즈 노트 맨 위에 "사용 금지 — 이유, 대체 릴리즈"(DEC-019).

## 릴리즈 이력
| 태그 | 상태 | 비고 |
|---|---|---|
```

- [ ] **Step 5: 확인하고 커밋한다**

Run: `bash tools/check-no-net.sh && cargo test --workspace --release`
Expected: 통과(새 의존성 `toml`·`xxhash-rust`·`serde` 는 네트워크 없음).

```bash
git add .github .claude/rules/windows-release.md docs/check docs/engineering/runbooks/release-check.md
git commit -m "ci: real-model end-to-end and 500k bench; release-check workflow and runbook for the air-gapped quality check"
```
태그는 Task 21 기록을 마친 뒤 사용자 확인을 받고 붙인다.

---

### Task 21: 기록 정리와 M2 결과 문서

**Files:**
- Create: `docs/m2/results.md`
- Modify: `docs/engineering/decisions.md`, `docs/engineering/known-issues.md`, `docs/superpowers/specs/2026-10-12-kra-search-design.md`(§13 품질·지연 줄), `eval/data/README.md`, 메모리 `m2-status`

- [ ] **Step 1: 결정·문제 기록**

DEC-056·057 과 DEC-055 의 데이터 방해 문서 변경은 계획 단계에서 이미 기록했다(2026-10-15) — 구현 중 내용이 바뀌었으면 그 줄을 고친다.

`docs/engineering/known-issues.md`:
- KI-001 상태 → `해결: cut_to 창 탐색(M2 Task 9, 시험 huge_paragraph_is_cut_inside_a_window)`.
- KI-003 상태 → `해결: llama_log_set 무음, KRA_LLAMA_LOG=1 이면 보임(M2 Task 9)`.
- KI-017 상태 끝에 `· 구현: platform::cpu::default_embed_threads(M2 Task 10)`.
- KI-041 상태 → Task 16 판정 결과(`128 채택` 또는 `256 유지(이유)`)와 개발 PC 카드 시간, 실제 PC 시간은 폐쇄망 확인 대기.
- 새 줄:
```markdown
| KI-047 | 제목 없는 문서의 카드·청크 머리는 가장 최근 경로의 파일 이름 — 이름만 바꾸면(내용 같음) 다시 임베딩하지 않으므로 머리의 옛 이름이 남는다 | `app::embedding` | 제목 없는 문서를 이름 바꾼 뒤 의미 검색이 옛 이름 쪽으로 약간 기욺 | 이름이 바뀐 내용 중 제목 없는 것만 카드 다시 | 실사용 피드백 | 미해결 |
| KI-048 | 명령줄 `search` 는 실행마다 모델을 불러오고(약 1초) meta 의 수정일·청크 여부를 전부 읽는다 | `app::search` | 명령줄 검색이 느림(검색 자체는 기준 안) | 상주 앱에서 한 번만 불러오고 캐시 | M3 검색창 | 미해결 |
```

- [ ] **Step 2: 결과 문서**

`docs/m2/results.md` — 다음 절로 쓴다(숫자는 보고서에서 옮긴다):
```markdown
# M2 결과 — 의미 검색 + 하이브리드 검색

## 1. 개발 PC 평가 (docs/m2/eval-hybrid.md)
- 합격 판정 표(하이브리드 vs 키워드, 날짜 가산 켬/끔, 날짜 없는 질의) — §1 그대로
- 고른 매개변수와 이유(DEC-058), 카드 상한 판정(#27), 데이터 뒷부분 판정
- 검색어 유형 분류 정확도

## 2. 일치 검사 (docs/m2/parity.md)
- nDCG 차이, 상위 10 일치 비율, 판정

## 3. 성능
- 청크 50만 검색 코어 p50·p95(Task 19), 개발 PC 색인 시간(평가 세트)

## 4. 폐쇄망 확인 (m2-check-N, 대기)
- 반입 일자·릴리즈·SHA, 보고서 요약(질의 수·nDCG·날짜 질의·색인 시간·지연·코어) — 도착하면 채운다

## 5. M3 에 넘길 것
- KI-042(벡터 메모리), KI-048(검색마다 모델·meta 읽기), 폐쇄망 결과에 따른 매개변수 재조정 여부
```
스펙 §13 표의 `품질` 줄 끝에 `M2 개발 PC: 검증용 매크로 nDCG@10 하이브리드 x vs 키워드 y(docs/m2/results.md)`, `의미 포함 최종 결과` 줄 끝에 `M2 검색 코어 청크 50만 p95 z ms(개발 PC)` 를 덧붙인다.

- [ ] **Step 3: 메모리와 마무리**

메모리 `m2-status` 를 갱신한다(진행: 구현 완료, 폐쇄망 확인 대기, PR 상태). 전체 시험:

Run: `cargo test --workspace --release && bash tools/check-no-net.sh && python3 tools/eval/check_sets.py eval/public eval/kra eval/data`
Expected: 모두 통과.

```bash
git add docs
git commit -m "docs(m2): results, decisions DEC-056/057, known issues KI-047/048"
```
그 다음 사용자에게: (1) 폐쇄망 확인 릴리즈 태그를 붙여도 되는지, (2) PR 을 열지 — 순서로 묻는다. M2 완료(스펙 §14)는 폐쇄망 보고서 반출까지다.
