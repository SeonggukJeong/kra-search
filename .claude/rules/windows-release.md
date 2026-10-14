---
paths:
  - ".github/workflows/**"
  - ".cargo/config.toml"
  - "docs/diag/README-diag.txt"
---
# CI·Windows 빌드·릴리즈 규칙
- Windows 패키지에는 llama/ggml DLL(CPU 변형 전부)과 VC++ 런타임 3개(vcruntime140, vcruntime140_1, msvcp140)를 exe 옆에 둔다(INC-005).
- CI 실모델 단계는 `embed`(model·reference)와 `diag --test memory` 를 macOS·Windows 모두에서 돌린다(INC-015). 지우지 않는다.
- 진단 릴리즈는 `docs/engineering/runbooks/release-diag.md` — 스모크 수치를 이전 값과 비교하기 전에는 사용자에게 넘기지 않는다(INC-006). 릴리즈 이력 표를 갱신한다.
