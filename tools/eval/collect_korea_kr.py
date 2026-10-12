"""정책브리핑(korea.kr) 보도자료의 HWP/HWPX 첨부를 평가용으로 수집한다. 개발 PC 전용.

- 공공누리 제1유형(출처표시) 표시가 있는 보도자료만 받는다.
- 보도자료 하나당 첨부 하나(HWPX 우선)만 받는다.
- 요청 사이에 1초 쉰다(robots.txt: 전체 허용).

사용: python3 tools/eval/collect_korea_kr.py --target 200 [--start-page 1]
"""
import argparse
import html
import re
import subprocess
import sys
import time
import tomllib
import urllib.request
from pathlib import Path

BASE = "https://www.korea.kr"
UA = {"User-Agent": "Mozilla/5.0 (kra-search eval collector)"}
ROOT = Path(__file__).resolve().parents[2] / "eval" / "public"
ATTACH = re.compile(r'<a href="/common/download\.do\?fileId=(\d+)&amp;tblKey=(\w+)">\s*<img[^>]*>\s*([^<]+?)\s*</a>', re.S)


def get(url: str) -> str:
    with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=30) as r:
        return r.read().decode("utf-8", "replace")


def existing_urls() -> set[str]:
    man = ROOT / "manifest.toml"
    if not man.exists():
        return set()
    return {d["url"] for d in tomllib.loads(man.read_text("utf-8")).get("doc", [])}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--target", type=int, default=200)
    ap.add_argument("--start-page", type=int, default=1)
    a = ap.parse_args()
    have = existing_urls()
    added = 0
    page = a.start_page
    while added < a.target:
        ids = sorted(set(re.findall(r"pressReleaseView\.do\?newsId=(\d+)", get(f"{BASE}/briefing/pressReleaseList.do?pageIndex={page}"))))
        if not ids:
            break
        for nid in ids:
            if added >= a.target:
                break
            time.sleep(1)
            try:
                page_html = get(f"{BASE}/briefing/pressReleaseView.do?newsId={nid}")
            except Exception as e:  # noqa: BLE001 — 개별 실패는 건너뛴다
                print(f"skip {nid}: {e}", file=sys.stderr)
                continue
            if "공공누리" not in page_html or "1유형" not in page_html:
                continue
            title = html.unescape(re.search(r"<title>([^<]*)", page_html).group(1).split(" - ")[0]).strip()
            files = [(fid, key, html.unescape(name)) for fid, key, name in ATTACH.findall(page_html)]
            files = [f for f in files if f[2].lower().endswith((".hwpx", ".hwp"))]
            if not files:
                continue
            files.sort(key=lambda f: 0 if f[2].lower().endswith(".hwpx") else 1)
            fid, key, name = files[0]
            url = f"{BASE}/common/download.do?fileId={fid}&tblKey={key}"
            if url in have:
                continue
            ext = "hwpx" if name.lower().endswith(".hwpx") else "hwp"
            r = subprocess.run(
                [sys.executable, str(Path(__file__).with_name("add_doc.py")), "--url", url, "--title", title,
                 "--source", f"korea.kr newsId={nid}", "--license", "공공누리 제1유형", "--ext", ext],
                capture_output=True, text=True,
            )
            if r.returncode == 0:
                added += 1
                have.add(url)
                print(f"[{added}/{a.target}] {r.stdout.strip()} {title[:40]}")
            else:
                print(f"fail {nid}: {r.stderr.strip()[:120]}", file=sys.stderr)
        page += 1
    print(f"added {added}, last page {page}")


if __name__ == "__main__":
    main()
