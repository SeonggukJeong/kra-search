#!/usr/bin/env bash
# 개발·CI·릴리즈 전용: bblanchon/pdfium-binaries 의 고정 태그를 third_party/pdfium/<플랫폼>/ 에 받는다.
# 실행 시(제품)에는 네트워크를 쓰지 않는다 — 이 스크립트가 받은 라이브러리를 실행 파일 옆에 동봉한다.
# 사용: bash tools/fetch-pdfium.sh [플랫폼...]   (생략 시 현재 호스트 플랫폼; all = 전부)
# 태그를 바꾸면 SHA-256 4개를 같이 바꾸고 pdfium-render 의 pdfium_XXXX 기능도 맞춘다(runbooks/upgrade-lindera-pdfium.md).
set -euo pipefail
export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL="*"
TAG="chromium/7881"   # pdfium-render 0.9.4 의 pdfium_7881 과 맞춤
ROOT="${PDFIUM_DIR:-$(cd "$(dirname "$0")/.." && pwd)/third_party/pdfium}"

sha_of() { (sha256sum "$1" 2>/dev/null || shasum -a 256 "$1") | cut -d' ' -f1; }

asset_sha() {
  case "$1" in
    mac-arm64) echo 52e94ca5aa8847934330daf3f8150c190682c5ca93831468794f8b90d4392e40 ;;
    mac-x64)   echo 6dedf83990e0e3d6b7c93c9e7589c5a126b0ae14b7464d76120cff7a26afb18b ;;
    win-x64)   echo 73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08 ;;
    linux-x64) echo 1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d ;;
    *) echo "알 수 없는 플랫폼: $1" >&2; exit 1 ;;
  esac
}

lib_path() {
  case "$1" in
    mac-*)   echo lib/libpdfium.dylib ;;
    win-*)   echo bin/pdfium.dll ;;
    linux-*) echo lib/libpdfium.so ;;
  esac
}

host_platform() {
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) echo mac-arm64 ;;
    Darwin-x86_64) echo mac-x64 ;;
    MINGW*-x86_64|MSYS*-x86_64|CYGWIN*-x86_64) echo win-x64 ;;
    Linux-x86_64) echo linux-x64 ;;
    *) echo "지원하지 않는 호스트: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
  esac
}

fetch() {
  local p="$1" want dir tgz got
  want="$(asset_sha "$p")"
  dir="$ROOT/$p"
  if [ -f "$dir/.tag" ] && [ "$(cat "$dir/.tag")" = "$TAG $want" ] && [ -f "$dir/$(lib_path "$p")" ]; then
    echo "pdfium $TAG $p already present"; return
  fi
  rm -rf "$dir"; mkdir -p "$dir"
  tgz="$dir/pdfium-$p.tgz"
  curl -sSfL -o "$tgz.part" "https://github.com/bblanchon/pdfium-binaries/releases/download/$TAG/pdfium-$p.tgz"
  mv "$tgz.part" "$tgz"
  got="$(sha_of "$tgz")"
  if [ "$got" != "$want" ]; then
    echo "SHA-256 불일치: pdfium-$p.tgz ($got, 기대 $want)" >&2
    rm -f "$tgz"; exit 1
  fi
  # 라이브러리·라이선스만 푼다(헤더·cmake 설정은 쓰지 않음).
  tar -xzf "$tgz" -C "$dir" "$(lib_path "$p")" LICENSE VERSION licenses
  rm -f "$tgz"
  echo "$TAG $want" > "$dir/.tag"
  echo "$want  pdfium-$p.tgz → $dir/$(lib_path "$p")"
}

PLATFORMS=("$@")
[ ${#PLATFORMS[@]} -eq 0 ] && PLATFORMS=("$(host_platform)")
[ "${PLATFORMS[0]}" = all ] && PLATFORMS=(mac-arm64 mac-x64 win-x64 linux-x64)
for p in "${PLATFORMS[@]}"; do fetch "$p"; done
