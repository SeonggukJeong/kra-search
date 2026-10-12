#!/usr/bin/env bash
# rhwp(MIT)의 라이브러리 소스만 고정 리비전으로 third_party/rhwp 에 받는다(개발·CI 전용).
# rhwp 저장소는 샘플·테스트 때문에 수 GB라서 cargo git 의존 대신 부분 복제(blobless + sparse)를 쓴다.
set -euo pipefail
# Windows Git Bash가 "/src/" 같은 인자를 Windows 경로로 바꾸지 않게 한다.
export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL="*"
REV="1a76570e833917d15817415a53c09ad61ab3203f"
SPEC_VERSION=3  # sparse 목록이 바뀌면 올린다
DIR="$(cd "$(dirname "$0")/.." && pwd)/third_party/rhwp"

if [ -f "$DIR/.rev" ] && [ "$(cat "$DIR/.rev")" = "$REV-$SPEC_VERSION" ]; then
  echo "rhwp $REV already present"; exit 0
fi
rm -rf "$DIR"
git clone --quiet --filter=blob:none --no-checkout https://github.com/edwardkim/rhwp "$DIR"
# 라이브러리가 include_bytes!/include_str! 로 포함하는 src 밖 파일들(폰트·빈 문서 템플릿·문서 등)
EXTRA=('/assets/logo/logo-32.png' '/llms.txt' '/mydocs/manual/agent_knowledge_map.md' '/mydocs/manual/agent_troubleshooting_guide.md' '/mydocs/manual/gym_optional_tool.md' '/mydocs/manual/recipes/01_fill_form_and_submit.md' '/mydocs/manual/recipes/02_table_csv_roundtrip.md' '/mydocs/manual/recipes/03_redact_before_sharing.md' '/mydocs/manual/recipes/04_safety_check_untrusted_doc.md' '/mydocs/manual/recipes/05_mail_merge_batch_fill.md' '/mydocs/manual/recipes/06_visual_regression_before_after.md' '/mydocs/tech/agent_roadmap/atlas_r1_r200.md' '/samples/3-09월_교육_통합_2022.hwp' '/samples/hml/formatting_table.hml' '/samples/hwp3-sample16-hwp5.hwp' '/samples/hwpx/aift.hwpx' '/samples/hwpx/ref/ref_empty.hwpx' '/samples/render-p35-font-native-bitmap.hwpx' '/saved/blank2010.hwp' '/tests/fixtures/fonts/RHWPBitmapSvgGlyphSmoke.ttf' '/tests/fixtures/fonts/RHWPExactFaceSmoke.ttc' '/tests/fixtures/fonts/RHWPHostFixture-Regular.ttf' '/tests/fixtures/fonts/RHWPHostFixture.ttc' '/ttfs/opensource/NotoSansKR-Regular.ttf')
git -C "$DIR" sparse-checkout set --no-cone '/src/' '/crates/' '/Cargo.toml' '/LICENSE*' '/build.rs' "${EXTRA[@]}"
git -C "$DIR" checkout --quiet "$REV"
# 부분 복제의 .git 이 남아 있으면 cargo 가 패키지 파일 목록을 git 으로 만들다 실패한다 → 제거(버전은 .rev 로 추적).
rm -rf "$DIR/.git"

# 우리 워크스페이스의 경로 의존으로 쓸 수 있게 Cargo.toml 을 다듬는다:
# 자체 [workspace]·테스트·바이너리·예제·벤치 대상 제거(그 파일들은 받지 않았다).
python3 - "$DIR/Cargo.toml" <<'PY'
import re, sys
p = sys.argv[1]
s = open(p, encoding="utf-8").read()
out, skip = [], False
for line in s.splitlines():
    if re.match(r"^\[(\[)?[^\]]+\]", line):
        skip = line.strip() in ("[workspace]", "[[test]]", "[[bin]]", "[[example]]", "[[bench]]", "[dev-dependencies]") \
            or line.startswith("[workspace.")
    if not skip:
        out.append(line)
s = "\n".join(out) + "\n"
s = s.replace("[package]\n", "[package]\nautobins = false\nautotests = false\nautobenches = false\nautoexamples = false\n", 1)
s = re.sub(r"(?m)^auto(tests|examples) = false\n(?=(.|\n)*^auto\1 = false)", "", s)
open(p, "w", encoding="utf-8").write(s)
PY
echo "$REV-$SPEC_VERSION" > "$DIR/.rev"
echo "rhwp $REV ready at $DIR"
