#!/usr/bin/env bash
# 개발·CI·릴리즈 전용: lindera 한국어 사전(mecab-ko-dic, Apache-2.0) 원본을 lindera 빌드 캐시 위치에 받아 둔다.
# lindera-ko-dic 빌드 스크립트는 캐시에 이 파일이 있으면(MD5 일치) 내려받지 않는다 → 빌드가 네트워크 없이 재현된다.
# 캐시 위치는 .cargo/config.toml 의 LINDERA_BUILD_DICTIONARY_CACHE_DIR(= third_party/ko-dic).
# lindera 를 올리면 LINDERA_VER(폴더 이름)를 바꾼다 — runbooks/upgrade-lindera-pdfium.md.
set -euo pipefail
LINDERA_VER="6.2.0"
FMT="2"   # lindera-dictionary 의 DICTIONARY_FORMAT_VERSION(캐시 폴더 이름의 일부)
FILE="mecab-ko-dic-2.1.1-20180720.tar.gz"
URL="https://Lindera.dev/$FILE"
SHA="702ced21c6167e9d9aebc674ab5ee54af58d4443975f2940d37d0567c020591a"
DIR="$(cd "$(dirname "$0")/.." && pwd)/third_party/ko-dic/$LINDERA_VER-fmt$FMT"

sha_of() { (sha256sum "$1" 2>/dev/null || shasum -a 256 "$1") | cut -d' ' -f1; }

mkdir -p "$DIR"
if [ -f "$DIR/$FILE" ] && [ "$(sha_of "$DIR/$FILE")" = "$SHA" ]; then
  echo "ko-dic $FILE already present"; exit 0
fi
curl -sSfL -o "$DIR/$FILE.part" "$URL"
got="$(sha_of "$DIR/$FILE.part")"
if [ "$got" != "$SHA" ]; then
  echo "SHA-256 불일치: $FILE ($got, 기대 $SHA)" >&2
  rm -f "$DIR/$FILE.part"; exit 1
fi
mv "$DIR/$FILE.part" "$DIR/$FILE"
echo "$SHA  $FILE → $DIR"
