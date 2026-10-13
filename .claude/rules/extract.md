---
paths:
  - "crates/extract/**"
  - "tools/fetch-rhwp.sh"
---
# 문서 추출(rhwp) 영역 규칙
- 본문은 `walk.rs` 가 rhwp 문서 모델을 직접 순회해 만든다. rhwp `rag::build_chunks` 로 되돌리지 않는다 — 본문 없는 조문 줄·글상자·각주·중첩 표를 버린다(INC-011, DEC-027).
- 추출을 바꾸면 `cargo test -p extract`(커버리지 시험 포함)와 공개 평가 커버리지(`cargo run --release -p extract --example coverage -- eval/public/docs/*`)를 기준값과 비교한다.
- rhwp 버전을 바꾸면 `docs/engineering/runbooks/upgrade-rhwp.md`. sparse 목록을 바꾸면 `SPEC_VERSION` 을 올린다.
- 실패 분류(`FailKind`)·DRM 시그니처는 진단 통계와 제품 표시에 쓰인다(DEC-009, KI-010).
- 미뤄 둔 추출 문제: `docs/engineering/known-issues.md` KI-011~016.
