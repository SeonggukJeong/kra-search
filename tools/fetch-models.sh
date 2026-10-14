#!/usr/bin/env bash
# 개발·CI 전용: EmbeddingGemma 2 텍스트 GGUF(Q8_0, Q4)를 models/ 에 받는다. 제품 실행 파일에는 포함되지 않는다.
# 제품 모델은 Q8_0(M0 결정, 스펙 §6.7). Q4 는 평가·비교용.
# 리비전과 SHA-256 을 고정한다(KI-005) — 모델을 바꾸려면 docs/engineering/embedding-model.md 를 먼저 본다.
set -euo pipefail
mkdir -p models
REPO="unsloth/embeddinggemma-2-GGUF"
REV="031f0d4b35536f69ab3509d4893c923264fcf253" # 2026-10-07
fetch() { # <저장 이름> <저장소 파일 이름> <SHA-256>
  local out="models/$1"
  if [ ! -f "$out" ]; then
    curl -sfL -o "$out.part" "https://huggingface.co/$REPO/resolve/$REV/$2"
    mv "$out.part" "$out"
  fi
  local got
  got=$( (sha256sum "$out" 2>/dev/null || shasum -a 256 "$out") | cut -d' ' -f1)
  if [ "$got" != "$3" ]; then
    echo "SHA-256 불일치: $out ($got, 기대 $3)" >&2
    exit 1
  fi
  echo "$3  $out"
}
fetch embeddinggemma-2-Q8_0.gguf embeddinggemma-2-Q8_0.gguf 6f1bd4ac6c5df7444f9cca7ca36cafe6cfa34cd6f49fefb1e0b4be8143aed8bc
fetch embeddinggemma-2-Q4.gguf embeddinggemma-2-UD-Q4_K_XL.gguf ea905fd08e8061db77a0031cbb5096d459d7cfd2bdd0ad2ba66e483f16719493
