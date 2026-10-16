"""공개 평가 세트(eval/public) 매니페스트에 문서 날짜(date = korea.kr 게시일)를 채운다. 개발 PC 전용.

- date 가 없는 [[doc]] 만 보도자료 페이지를 받아 게시일을 읽는다(요청 사이 1초, robots.txt 전체 허용).
- 매니페스트 서식을 그대로 두려고 TOML 을 다시 쓰지 않고 `ext = ...` 줄 바로 뒤에 `date = "YYYY-MM-DD"` 줄을 끼운다.
- 50건마다 저장한다 — 중간에 끊겨도 다시 돌리면 남은 것만 받는다.

사용: python3 tools/eval/fetch_dates.py
"""
import re
import sys
import time
import tomllib
import urllib.request
from pathlib import Path

MAN = Path(__file__).resolve().parents[2] / "eval" / "public" / "manifest.toml"
UA = {"User-Agent": "Mozilla/5.0 (kra-search eval collector)"}
# 보도자료 본문 위 "info" 칸의 첫 날짜가 게시일이다(2026-10-15 확인: <div class="info"> <span>2026.10.12</span>).
DATE = re.compile(r'<div class="info">\s*<span>(\d{4})\.(\d{2})\.(\d{2})</span>')
SEP = "\n[[doc]]\n"


def posted(news_id: str) -> str:
    url = f"https://www.korea.kr/briefing/pressReleaseView.do?newsId={news_id}"
    with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=30) as r:
        html = r.read().decode("utf-8", "replace")
    m = DATE.search(html)
    if not m:
        raise ValueError(f"게시일을 찾지 못함: newsId={news_id}")
    return f"{m.group(1)}-{m.group(2)}-{m.group(3)}"


def save(blocks: list[str]) -> None:
    MAN.write_text(SEP.join(blocks), "utf-8")


def main() -> None:
    blocks = MAN.read_text("utf-8").split(SEP)
    done = failed = 0
    for i, b in enumerate(blocks):
        if re.search(r"^date = ", b, re.M):
            continue
        idm = re.search(r'^id = "(P\d{4})"', b, re.M)
        src = re.search(r'^source = "korea\.kr newsId=(\d+)"', b, re.M)
        if not (idm and src):
            continue
        time.sleep(1)
        try:
            d = posted(src.group(1))
        except Exception as e:  # noqa: BLE001 — 하나가 실패해도 나머지는 받는다
            print(f"실패 {idm.group(1)}: {e}", file=sys.stderr)
            failed += 1
            continue
        blocks[i] = re.sub(r'^(ext = "[^"]+")$', lambda x: f'{x.group(1)}\ndate = "{d}"', b, count=1, flags=re.M)
        done += 1
        if done % 50 == 0:
            save(blocks)
            print(f"{done}건 저장")
    save(blocks)
    # 다시 읽어 TOML 이 깨지지 않았는지 확인
    tomllib.loads(MAN.read_text("utf-8"))
    print(f"완료 {done}건, 실패 {failed}건")
    if failed:
        sys.exit(1)


if __name__ == "__main__":
    main()
