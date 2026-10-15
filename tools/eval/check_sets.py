"""평가 세트 검사. 0 으로 끝나면 통과.

- 모든 [[doc]] 에 date(YYYY-MM-DD)가 있다.
- 질의 qid 는 모든 세트를 통틀어 겹치지 않는다. 유형은 KINDS 안이다.
- 정답 id 는 함께 검사하는 세트들의 문서 안에 있다(공개·마사회·데이터 세트를 합쳐 평가하므로).
- date 유형 질의는 5번째 열(range)이 있다: `YYYY-MM-DD..YYYY-MM-DD` 면 정답 문서마다 날짜(`date`)가 그 안이거나
  제목에 쓴 연도가 그 기간과 겹쳐야 하고, `future` 면 미래 표현 질의다.
  제목 연도가 파일 날짜보다 우선한다(사용자 요구 2026-10-15: 2026년에 "작년 경마시행계획" = 2025년 경마시행계획).
  제목 연도는 절대 연도만 읽는다: `2025년`, `25년`, `'25` → 2025.

사용: python3 tools/eval/check_sets.py eval/public eval/kra eval/data
"""
import datetime as dt
import re
import sys
import tomllib
from pathlib import Path

KINDS = {"semantic", "keyword", "abbrev", "temporal", "deep", "kra", "kra-deep", "kra-plain", "date", "data-deep", "data-head"}


TITLE_YEAR = [
    (re.compile(r"(?<!\d)(20\d{2})년"), 0),
    (re.compile(r"(?<![\d'])(\d{2})년"), 2000),
    (re.compile(r"'(\d{2})(?!\d)"), 2000),
]


def day(s: str) -> dt.date:
    return dt.date.fromisoformat(s)


def title_years(title: str) -> list[int]:
    """제목에 쓴 절대 연도(문서가 다루는 해)."""
    ys = {int(m.group(1)) + base for rx, base in TITLE_YEAR for m in rx.finditer(title)}
    return sorted(ys)


def main() -> None:
    dirs = [Path(a) for a in sys.argv[1:]]
    if not dirs:
        sys.exit("사용: check_sets.py <세트 폴더>...")
    errors: list[str] = []
    dates: dict[str, dt.date] = {}
    years: dict[str, list[int]] = {}
    for d in dirs:
        for doc in tomllib.loads((d / "manifest.toml").read_text("utf-8"))["doc"]:
            if "date" not in doc:
                errors.append(f"{d}: {doc['id']} 에 date 없음")
                continue
            try:
                dates[doc["id"]] = day(doc["date"])
                years[doc["id"]] = title_years(doc.get("title", ""))
            except ValueError:
                errors.append(f"{d}: {doc['id']} date 형식 오류 {doc['date']!r}")
    seen: set[str] = set()
    for d in dirs:
        q = d / "queries.tsv"
        if not q.exists():
            continue
        for n, line in enumerate(q.read_text("utf-8").splitlines()[1:], start=2):
            if not line.strip():
                continue
            c = line.split("\t")
            if len(c) < 4:
                errors.append(f"{q}:{n} 칸이 4개 미만")
                continue
            qid, _text, kind, rel = c[0], c[1], c[2], c[3]
            if qid in seen:
                errors.append(f"{q}:{n} qid 중복 {qid}")
            seen.add(qid)
            if kind not in KINDS:
                errors.append(f"{q}:{n} 알 수 없는 유형 {kind}")
            ids = [x.strip() for x in rel.split(",") if x.strip()]
            for i in ids:
                if i not in dates:
                    errors.append(f"{q}:{n} 정답 id 없음 {i}")
            if kind == "date":
                rng = c[4].strip() if len(c) > 4 else ""
                if rng == "future":
                    continue
                if ".." not in rng:
                    errors.append(f"{q}:{n} date 질의에 기간 열(5번째) 없음")
                    continue
                a, b = (day(x) for x in rng.split(".."))
                for i in ids:
                    if i not in dates:
                        continue
                    by_date = a <= dates[i] <= b
                    by_title = any(dt.date(y, 1, 1) <= b and a <= dt.date(y, 12, 31) for y in years[i])
                    if not (by_date or by_title):
                        ys = ",".join(map(str, years[i])) or "없음"
                        errors.append(f"{q}:{n} {i} 날짜 {dates[i]} · 제목 연도 {ys} 모두 기간 {rng} 밖")
    for e in errors:
        print(e)
    print(f"{'실패' if errors else '통과'}: 문서 {len(dates)} · 질의 {len(seen)}")
    sys.exit(1 if errors else 0)


if __name__ == "__main__":
    main()
