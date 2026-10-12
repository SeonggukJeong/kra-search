"""공개 문서 1건을 내려받아 eval/public/manifest.toml 에 추가한다. 개발 PC 전용."""
import argparse
import hashlib
import pathlib
import sys
import tomllib
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2] / "eval" / "public"


def esc(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"')


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--title", required=True)
    ap.add_argument("--source", required=True)
    ap.add_argument("--license", required=True)
    ap.add_argument("--ext", required=True, choices=["hwp", "hwpx"])
    a = ap.parse_args()
    man = ROOT / "manifest.toml"
    existing = tomllib.loads(man.read_text("utf-8")).get("doc", []) if man.exists() else []
    if any(d["url"] == a.url for d in existing):
        sys.exit("already added")
    new_id = f"P{len(existing) + 1:04d}"
    req = urllib.request.Request(a.url, headers={"User-Agent": "Mozilla/5.0"})
    data = urllib.request.urlopen(req, timeout=60).read()
    if len(data) < 512:
        sys.exit("too small — probably not a document")
    sha = hashlib.sha256(data).hexdigest()
    if any(d["sha256"] == sha for d in existing):
        sys.exit("duplicate content")
    (ROOT / "docs").mkdir(parents=True, exist_ok=True)
    (ROOT / "docs" / f"{new_id}.{a.ext}").write_bytes(data)
    with man.open("a", encoding="utf-8") as f:
        f.write(
            f'\n[[doc]]\nid = "{new_id}"\nurl = "{esc(a.url)}"\ntitle = "{esc(a.title)}"\nsource = "{esc(a.source)}"\n'
            f'license = "{esc(a.license)}"\nsha256 = "{sha}"\next = "{a.ext}"\n'
        )
    print(new_id, sha[:12])


if __name__ == "__main__":
    main()
