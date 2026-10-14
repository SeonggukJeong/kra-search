"""매니페스트의 모든 평가 문서를 (없으면) 내려받고 SHA-256을 검증한다. 개발 PC 전용."""
import hashlib
import pathlib
import sys
import time
import tomllib
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2] / "eval" / "public"


def main() -> None:
    docs = tomllib.loads((ROOT / "manifest.toml").read_text("utf-8"))["doc"]
    bad = []
    for d in docs:
        p = ROOT / "docs" / f'{d["id"]}.{d["ext"]}'
        if not p.exists():
            p.parent.mkdir(parents=True, exist_ok=True)
            req = urllib.request.Request(d["url"], headers={"User-Agent": "Mozilla/5.0"})
            p.write_bytes(urllib.request.urlopen(req, timeout=60).read())
            time.sleep(1)
        if hashlib.sha256(p.read_bytes()).hexdigest() != d["sha256"]:
            bad.append(d["id"])
    if bad:
        sys.exit(f"SHA-256 mismatch: {', '.join(bad)}")
    print(f"ok: {len(docs)} documents verified")


if __name__ == "__main__":
    main()
