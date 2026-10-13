# 진단 도구(kra-diag) 릴리즈

폐쇄망 PC 에서 돌릴 `kra-diag` zip 을 만드는 절차. 사용자는 이것을 반입해 실행하고 결과 파일 두 개를 반출한다 — **잘못된 릴리즈는 사용자의 하루를 버리게 한다**(INC-006, INC-009).

## 절차
1. `main`(또는 작업 브랜치) CI 가 macOS·Windows·deny 모두 통과했는지 확인.
2. 태그: `git tag m0-diag-N && git push origin m0-diag-N` → `.github/workflows/release-m0.yml` 이 Windows 에서 빌드·패키지·스모크 시험·프리릴리즈 생성.
   - zip 구성: `kra-diag.exe`, llama/ggml DLL(CPU 변형 전부), VC++ 런타임 3개(INC-005), `models/*.gguf`(Q4·Q8), `README-diag.txt`, `manual-checks.txt`.
3. **스모크 수치 확인**(사용자에게 넘기기 전 필수): 워크플로 로그의 `bench` 요약에서 Q4 초당 토큰이 2코어 러너 기준 약 380~580 범위인지. 수십 토큰이면 최적화 빠짐(INC-006) — 넘기지 말 것.
4. **개인정보 확인**: 보고서에 문서 내용·경로·파일명·사용자명·컴퓨터명이 없어야 한다 — `cargo test -p diag --release`(report 시험)가 지킨다. 보고서 항목을 추가했다면 그 시험에 경우를 추가한다.
5. 사용자에게 전달: 릴리즈 링크, zip 의 SHA-256, 실행 방법(zip 풀고 `kra-diag.exe` 더블클릭 → `kra-diag-summary.txt`, `kra-diag-report.json` 반출), 수동 점검표(zip 안 `manual-checks.txt`).
6. 불량 릴리즈는 지우지 말고 릴리즈 노트 맨 위에 "사용 금지 — 이유, 대체 릴리즈"를 적는다(DEC-019).

## 릴리즈 이력
| 태그 | 상태 | 비고 |
|---|---|---|
| m0-diag-1 | 빌드 실패(게시 안 됨) | |
| m0-diag-2 | **사용 금지** | llama.cpp 최적화 없음(INC-006) |
| m0-diag-3 | **사용 금지** | 긴 표 머리글 청크 무한 반복(INC-009) |
| m0-diag-4 | 대체됨 | 정상이지만 HWP 본문 누락 어댑터(INC-011) — 추출 통계·토큰 비율이 낮게 나옴 |
| m0-diag-5 | 현재 | 본문 순회 어댑터 + 추출 커버리지 보고 |
