#!/usr/bin/env bash
# 개발·CI 전용: EmbeddingGemma 2 텍스트 GGUF(Q8_0, Q4)를 models/ 에 받는다. 제품 실행 파일에는 포함되지 않는다.
set -euo pipefail
mkdir -p models
REPO="unsloth/embeddinggemma-2-GGUF"
LIST=$(curl -sfL "https://huggingface.co/api/models/$REPO")
pick() {
  python3 -c 'import json,sys; m=sys.argv[1]; print([s["rfilename"] for s in json.load(sys.stdin)["siblings"] if s["rfilename"].endswith(".gguf") and m in s["rfilename"] and "mmproj" not in s["rfilename"]][0])' "$1"
}
Q8=$(echo "$LIST" | pick Q8_0)
Q4=$(echo "$LIST" | pick Q4_K_XL)
echo "Q8=$Q8 Q4=$Q4"
[ -f models/embeddinggemma-2-Q8_0.gguf ] || curl -sfL -o models/embeddinggemma-2-Q8_0.gguf "https://huggingface.co/$REPO/resolve/main/$Q8"
[ -f models/embeddinggemma-2-Q4.gguf ] || curl -sfL -o models/embeddinggemma-2-Q4.gguf "https://huggingface.co/$REPO/resolve/main/$Q4"
(sha256sum models/*.gguf 2>/dev/null || shasum -a 256 models/*.gguf)
