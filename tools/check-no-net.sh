#!/usr/bin/env bash
# 실행 의존(normal) 그래프에 HTTP 크레이트가 없는지 확인한다 — 실행 중 네트워크 0(CLAUDE.md).
# cargo-deny 는 빌드 의존까지 함께 보므로 lindera 사전 빌드 스크립트의 ureq 를 wrappers 로 허용했다. 이 검사가 그 예외를 메운다.
# 출력을 먼저 변수로 받은 뒤 검사한다: `cargo tree | grep -q` 는 grep 이 일찍 끝나면 cargo 가 EPIPE 로 실패해
# pipefail 아래에서 "있음"이 "없음"으로 뒤집히는 경쟁이 있고, cargo tree 자체 실패(매니페스트 오류)도 조용히 통과시킨다.
set -euo pipefail
bad=0
for c in reqwest hyper ureq ureq-proto isahc curl attohttpc; do
  if out=$(cargo tree --workspace -e normal --target all -i "$c" 2>&1); then
    # 성공: Cargo.lock 에 있음. 일반 그래프에 없으면 "nothing to print" 만 나온다(빌드 전용 ureq 가 이 경우).
    if grep -q "^$c v" <<<"$out"; then
      echo "실행 의존 그래프에 $c 가 있음:" >&2
      echo "$out" >&2
      bad=1
    fi
  else
    case "$out" in
      *"did not match any packages"*) ;; # Cargo.lock 에 아예 없음 — 정상
      *)
        echo "cargo tree 실패($c) — 검사할 수 없음:" >&2
        echo "$out" >&2
        exit 2
        ;;
    esac
  fi
done
[ $bad -eq 0 ] && echo "실행 의존에 HTTP 크레이트 없음"
exit $bad
