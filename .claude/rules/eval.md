---
paths:
  - "crates/m0eval/**"
  - "eval/**"
  - "tools/eval/**"
---
# 평가 규칙
- 평가 문서 원본은 커밋하지 않는다(`eval/public/docs/`, `eval/kra/docs/`, `eval/downloads/`). 매니페스트(SHA-256)와 질의만 커밋.
- 질의를 만들 때 문서 문장을 베끼지 않는다(유형별 규칙: `eval/public/README.md`, `eval/kra/README.md`). 정답은 해당 의도를 직접 충족하는 문서 전부.
- 평가 세트를 만들면 먼저 설정 간 차이가 보이는지 확인한다(천장 효과, INC-008).
- 캐시는 입력 해시로 자동 무효화된다(INC-014) — 캐시를 수동으로 믿지 말 것.
