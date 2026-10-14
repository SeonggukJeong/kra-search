# 엔지니어링 지식 베이스

사람과 AI 에이전트가 **같은 실수를 되풀이하지 않도록** 남기는 기록. 제품 "무엇을"은 스펙과 UI 결정에, "어떻게·왜 이렇게 됐는지"는 여기에 있다.

| 문서 | 내용 | 언제 본다 |
|---|---|---|
| [incidents.md](incidents.md) | 발견해서 고친 문제(INC-xxx): 증상·원인·수정·재발 방지 장치 | 비슷한 증상을 볼 때, 해당 영역을 고칠 때 |
| [decisions.md](decisions.md) | 구현 중 내린 판단(DEC-xxx): 이유·틀렸을 때 비용·상태 | 이상해 보이는 코드를 "고치기" 전에 |
| [known-issues.md](known-issues.md) | 미뤄 둔 문제(KI-xxx)와 고칠 계기 | 새 마일스톤·영역 작업을 시작할 때 |
| [embedding-model.md](embedding-model.md) | 임베딩 모델 사실(차원·프롬프트·토큰) | `crates/embed`, 모델 교체 |
| [runbooks/upgrade-llama-cpp.md](runbooks/upgrade-llama-cpp.md) | llama.cpp 올리기 | 서브모듈 버전 변경 |
| [runbooks/upgrade-rhwp.md](runbooks/upgrade-rhwp.md) | rhwp 올리기 | `tools/fetch-rhwp.sh` REV 변경 |
| [runbooks/release-diag.md](runbooks/release-diag.md) | 진단 도구 릴리즈 | `m0-diag-*` 태그 |

관련: [스펙](../superpowers/specs/2026-10-12-kra-search-design.md) · [UI 결정(D1~D8)](../design/ui-decisions.md) · [M0 결과](../m0/results.md)

## 기록 규칙
- **문제를 고치면** incidents.md 에 INC 를 추가한다. "재발 방지 장치"(시험·빌드 검사·CI 단계)가 없으면 만든다 — 문서만으로는 장치가 아니다.
- **스펙·계획에 없는 것을 정하면** decisions.md 에 DEC 를 추가한다. 뒤집을 때는 상태를 "대체됨(→ 새 ID)"으로 바꾼다.
- **고치지 않고 미루면** known-issues.md 에 KI 와 "계기"를 적는다. 고치면 상태만 바꾼다.
- 번호는 재사용·변경하지 않는다(코드 주석·시험이 번호로 가리킨다).
- 코드의 위험 지점에는 `INC-xxx`/`DEC-xxx` 와 이 문서 경로를 주석으로 남긴다.
- 위험 영역 파일을 열면 Claude Code 가 `.claude/rules/` 의 해당 규칙을 자동으로 불러온다(경로 지정 규칙). 새 위험 영역이 생기면 규칙 파일도 추가한다.
