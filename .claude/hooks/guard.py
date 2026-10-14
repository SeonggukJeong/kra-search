"""PreToolUse 훅: 위험 작업 직전에 관련 절차 문서를 모델 문맥에 넣는다(막지는 않는다).

경로 지정 규칙(.claude/rules)은 파일을 읽거나 고칠 때만 불린다. 셸 명령으로 하는 작업
(서브모듈 버전 변경, 릴리즈 태그, 커밋)은 여기서 잡는다. 규칙 추가: RULES 에 (정규식, 안내) 한 줄.
"""
import json
import re
import sys

RULES = [
    (
        r"llama\.cpp.*\b(checkout|fetch|pull|switch|reset)\b|\b(checkout|switch)\b.*llama\.cpp|submodule\s+update\s+.*--remote",
        "llama.cpp 버전을 바꾸는 명령이다. docs/engineering/runbooks/upgrade-llama-cpp.md 를 따른다 — "
        "로컬 패치(patches/0001 logits 버퍼, INC-004) 재적용, build.rs 옵션·Windows CRT/최적화(INC-005·006), "
        "API(n_embd_out·encode) 확인, patches·model·reference·memory 시험, 문서의 '현재 버전' 갱신.",
    ),
    (
        r"fetch-rhwp\.sh.*\bREV\b|\bREV\b.*fetch-rhwp\.sh|rm\s+-rf\s+\S*third_party/rhwp",
        "rhwp 버전을 바꾸는 명령이다. docs/engineering/runbooks/upgrade-rhwp.md — "
        "rag::build_chunks 로 되돌리지 말 것(INC-011), sparse 목록을 바꾸면 SPEC_VERSION 올리기, 커버리지 기준값 비교.",
    ),
    (
        r"git\s+(tag|push)\b.*m0-diag|gh\s+release",
        "진단 릴리즈 작업이다. docs/engineering/runbooks/release-diag.md — 스모크 수치(Q4 초당 토큰)를 이전 값과 비교하기 전에는 "
        "사용자에게 넘기지 않는다(INC-006), 릴리즈 이력 표 갱신.",
    ),
    (
        r"git\s+commit\b",
        "커밋 전 확인: 이번 변경이 문제를 고쳤으면 docs/engineering/incidents.md(INC + 재발 방지 장치), "
        "스펙에 없는 것을 정했으면 decisions.md(DEC), 미뤘으면 known-issues.md(KI)에 적었는가?",
    ),
]


def main() -> None:
    try:
        data = json.load(sys.stdin)
    except Exception:
        return
    command = (data.get("tool_input") or {}).get("command") or ""
    notes = [msg for pattern, msg in RULES if re.search(pattern, command)]
    if notes:
        print(json.dumps({"hookSpecificOutput": {"hookEventName": "PreToolUse", "additionalContext": "\n".join(notes)}}, ensure_ascii=False))


if __name__ == "__main__":
    main()
