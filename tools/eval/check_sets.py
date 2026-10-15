"""평가 세트 검사. 0 으로 끝나면 통과.

- 모든 [[doc]] 에 date(YYYY-MM-DD)가 있다.
- 질의 qid 는 모든 세트를 통틀어 겹치지 않는다. 유형은 KINDS 안이다.
- 정답 id 는 함께 검사하는 세트들의 문서 안에 있다(공개·마사회·데이터 세트를 합쳐 평가하므로).
- date 유형 질의는 5번째 열(range)이 있다: `YYYY-MM-DD..YYYY-MM-DD` 면 정답 문서 날짜가 모두 그 안이어야 하고, `future` 면 미래 표현 질의다.

사용: python3 tools/eval/check_sets.py eval/public eval/kra eval/data
"""
import datetime as dt
import sys
import tomllib
from pathlib import Path

KINDS = {"semantic", "keyword", "abbrev", "temporal", "deep", "kra", "kra-deep", "kra-plain", "date", "data-deep", "data-head"}


def day(s: str) -> dt.date:
    return dt.date.fromisoformat(s)


def main() -> None:
    dirs = [Path(a) for a in sys.argv[1:]]
    if not dirs:
        sys.exit("사용: check_sets.py <세트 폴더>...")
    errors: list[str] = []
    dates: dict[str, dt.date] = {}
    for d in dirs:
        for doc in tomllib.loads((d / "manifest.toml").read_text("utf-8"))["doc"]:
            if "date" not in doc:
                errors.append(f"{d}: {doc['id']} 에 date 없음")
                continue
            try:
                dates[doc["id"]] = day(doc["date"])
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
                    if i in dates and not (a <= dates[i] <= b):
                        errors.append(f"{q}:{n} {i} 날짜 {dates[i]} 가 기간 {rng} 밖")
    for e in errors:
        print(e)
    print(f"{'실패' if errors else '통과'}: 문서 {len(dates)} · 질의 {len(seen)}")
    sys.exit(1 if errors else 0)


if __name__ == "__main__":
    main()
