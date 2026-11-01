"""데이터 형식(CSV·XLSX·TXT) 방해 문서와 그 질의를 만든다 — 고정 시드, 원본은 저장소에 넣지 않는다.
설계: docs/superpowers/specs/2026-10-15-m2-hybrid-search-design.md §5, DEC-055.

- 48개(CSV 16 · XLSX 16 · TXT 16), 각 600행. 행 하나 ≈ 50~70자 → 본문 청크 하나(300토큰 ≈ 480자)에 약 8행.
- 정답이 앞 20청크(약 160행) 뒤 — 420~560행 — 에 있는 질의 8개(data-deep), 앞 30행 안에 있는 질의 4개(data-head).
- 다시 돌려도 같은 바이트가 나온다(--check 가 매니페스트 SHA 와 비교). 다르면 생성기가 바뀐 것이다 — 매니페스트·질의를 새로 쓰고 이유를 README 에 적는다.

사용: python3 tools/eval/make_data_set.py          # 문서·매니페스트·질의 생성
      python3 tools/eval/make_data_set.py --check  # 문서만 다시 만들어 매니페스트 SHA 와 비교
"""
import datetime as dt
import hashlib
import io
import random
import sys
import zipfile
from pathlib import Path
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parents[2] / "eval" / "data"
SEED = 20261015
ROWS = 600
SITES = ["렛츠런파크 서울", "렛츠런파크 부산경남", "렛츠런파크 제주", "렛츠런CCC 강남", "장외발매소 분당",
         "장외발매소 일산", "장외발매소 대전", "장외발매소 광주", "장외발매소 수원", "장외발매소 인천"]
FACILITIES = ["관람대", "주차장", "마사", "발매창구", "화장실", "엘리베이터", "에스컬레이터", "식당", "전광판", "경주로"]
PROBLEMS = ["청소 상태", "조명 밝기", "안내 표지", "냉난방", "소음", "대기 시간", "직원 응대", "좌석 파손", "출입 통제", "물품 분실"]
STATUS = ["접수", "처리중", "완료", "이관", "보류"]
CHECKS = ["소화기 압력", "비상구 표시등", "누전 차단기", "배수 상태", "난간 고정", "CCTV 작동", "방송 설비", "급수 설비"]
RESULTS = ["양호", "양호", "양호", "주의", "불량"]
LOGS = ["발매기 #{n} 정상 동작 확인", "발매 서버 {n}번 응답 시간 {ms}ms", "현장 단말 {n} 재부팅", "회선 점검 완료 구간 {n}",
        "출력 용지 교체 단말 {n}", "정산 배치 {n}건 처리", "백업 작업 {n}번 정상 종료", "단말 {n} 접속 지연 경보 해제"]

# (형식, 같은 형식 안의 파일 순번, 행 위치 범위, 심는 행 문구, 질의, 유형)
PLANTED = [
    ("csv", 0, (420, 560), "렛츠런파크 부산경남 주차장 야간 조명이 꺼져 있다는 민원", "부경 주차장 밤에 불 꺼진 민원", "data-deep"),
    ("csv", 5, (420, 560), "장외발매소 분당 엘리베이터 문에 손님 손이 끼인 사고 신고", "분당 지점 승강기 문에 끼인 사고", "data-deep"),
    ("csv", 11, (420, 560), "은퇴한 경주마를 개인이 입양하는 절차 문의", "경주 끝난 말 데려가는 방법 문의", "data-deep"),
    ("xlsx", 2, (420, 560), "제주 마사 지붕 누수로 방수 공사 실시", "제주 마구간 비 새서 공사한 기록", "data-deep"),
    ("xlsx", 9, (420, 560), "서울 관람대 3층 소화기 압력 미달 12개 교체", "서울 관람석 소화기 바꾼 점검", "data-deep"),
    ("xlsx", 14, (420, 560), "부산경남 마필 수송차량 냉방장치 고장 수리", "말 실어 나르는 트럭 에어컨 고장", "data-deep"),
    ("txt", 1, (420, 560), "발매기 #4471 지폐 인식 모듈 고장으로 현장 교체 완료", "지폐 투입 안 되는 발매기 수리", "data-deep"),
    ("txt", 12, (420, 560), "제주 장외발매 네트워크 회선 이중화 장비 전원 장애 복구", "제주 발매 통신 끊긴 장애 처리", "data-deep"),
    ("csv", 3, (5, 25), "렛츠런CCC 강남 대형 전광판 글자 깨짐 민원", "강남 지점 큰 화면 글씨 깨짐", "data-head"),
    ("xlsx", 6, (5, 25), "광주 장외발매소 급수 설비 녹물 발생으로 배관 교체", "광주 지점 수돗물 녹물 배관", "data-head"),
    ("txt", 7, (5, 25), "일산 장외발매소 출력 용지 걸림 반복으로 프린터 교체", "일산 프린터 종이 걸려서 바꿈", "data-head"),
    ("csv", 14, (5, 25), "대전 장외발매소 식당 위생 불량 신고", "대전 지점 식당 더럽다는 신고", "data-head"),
]
KINDS = {"csv": ("민원 접수 현황", ["접수번호", "접수일", "사업장", "유형", "처리상태", "요지"]),
         "xlsx": ("시설 점검 기록", ["점검일", "사업장", "시설", "점검 항목", "결과", "조치 내용"]),
         "txt": ("발매 시스템 운영 일지", None)}
XML = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
ZIP_TIME = (2026, 1, 1, 0, 0, 0)


def rows_for(kind: str, rng: random.Random, start: dt.date) -> list[list[str] | str]:
    out: list[list[str] | str] = []
    for r in range(ROWS):
        day = start + dt.timedelta(days=r // 3)
        site, fac = rng.choice(SITES), rng.choice(FACILITIES)
        if kind == "csv":
            out.append([f"{day.year}-{r + 1:05d}", day.isoformat(), site, rng.choice(["불편", "건의", "칭찬", "문의"]),
                        rng.choice(STATUS), f"{fac} {rng.choice(PROBLEMS)} 관련 {rng.choice(['문의', '불편 신고', '개선 요청'])}"])
        elif kind == "xlsx":
            res = rng.choice(RESULTS)
            out.append([day.isoformat(), site, fac, rng.choice(CHECKS), res,
                        "이상 없음" if res == "양호" else f"{fac} {rng.choice(['보수 예정', '부품 교체', '재점검 요청'])}"])
        else:
            t = f"{rng.randrange(8, 20):02d}:{rng.randrange(60):02d}:{rng.randrange(60):02d}"
            msg = rng.choice(LOGS).format(n=rng.randrange(1000, 9999), ms=rng.randrange(20, 900))
            out.append(f"{day.isoformat()} {t} [{rng.choice(['정보', '정보', '경고'])}] {site} {msg}")
    return out


def plant(kind: str, rows: list, text: str, at: int) -> None:
    if kind == "csv":
        rows[at][5] = text
    elif kind == "xlsx":
        rows[at][5] = text
    else:
        head = rows[at].split("] ", 1)[0]
        rows[at] = f"{head}] {text}"


def csv_bytes(header: list[str], rows: list[list[str]], cp949: bool) -> bytes:
    def q(v: str) -> str:
        return f'"{v}"' if ("," in v or '"' in v) else v
    text = "\n".join(",".join(q(c) for c in line) for line in [header, *rows]) + "\n"
    return text.encode("cp949" if cp949 else "utf-8")


def col(i: int) -> str:
    s = ""
    i += 1
    while i:
        i, r = divmod(i - 1, 26)
        s = chr(65 + r) + s
    return s


def xlsx_bytes(title: str, header: list[str], rows: list[list[str]]) -> bytes:
    def row_xml(n: int, cells: list[str]) -> str:
        return f'<row r="{n}">' + "".join(
            f'<c r="{col(i)}{n}" t="inlineStr"><is><t>{escape(v)}</t></is></c>' for i, v in enumerate(cells)) + "</row>"
    sheet = (XML + '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>'
             + "".join(row_xml(n + 1, r) for n, r in enumerate([header, *rows])) + "</sheetData></worksheet>")
    parts = [
        ("[Content_Types].xml", XML + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
         '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
         '<Default Extension="xml" ContentType="application/xml"/>'
         '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
         '<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
         '<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/></Types>'),
        ("_rels/.rels", XML + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
         '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>'),
        ("xl/workbook.xml", XML + '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
         'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>'
         '<sheet name="기록" sheetId="1" r:id="rId1"/></sheets></workbook>'),
        ("xl/_rels/workbook.xml.rels", XML + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
         '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>'),
        ("xl/worksheets/sheet1.xml", sheet),
        ("docProps/core.xml", XML + '<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" '
         f'xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{escape(title)}</dc:title></cp:coreProperties>'),
    ]
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        for name, data in parts:
            info = zipfile.ZipInfo(name, date_time=ZIP_TIME)
            info.compress_type = zipfile.ZIP_STORED
            info.external_attr = 0o644 << 16
            z.writestr(info, data.encode("utf-8"))
    return buf.getvalue()


def build() -> tuple[list[dict], list[tuple[str, str, str, str]]]:
    rng = random.Random(SEED)
    docs: list[dict] = []
    queries: list[tuple[str, str, str, str]] = []
    n = 0
    for kind in ["csv", "xlsx", "txt"]:
        title_base, header = KINDS[kind]
        for k in range(16):
            n += 1
            did = f"D{n:04d}"
            start = dt.date(2024, 1, 1) + dt.timedelta(days=rng.randrange(0, 820))
            rows = rows_for(kind, rng, start)
            for (pk, pi, (lo, hi), text, query, qtype) in PLANTED:
                if pk == kind and pi == k:
                    plant(kind, rows, text, rng.randrange(lo, hi))
                    queries.append((f"X{len(queries) + 1:03d}", query, qtype, did))
            last = start + dt.timedelta(days=(ROWS - 1) // 3)
            title = f"{start.year}년 {title_base} {k + 1:02d}"
            if kind == "csv":
                data = csv_bytes(header, rows, cp949=(k % 2 == 0))
            elif kind == "xlsx":
                data = xlsx_bytes(title, header, rows)
            else:
                data = ("\n".join(rows) + "\n").encode("utf-8")
            docs.append({"id": did, "title": title, "ext": kind, "date": last.isoformat(), "data": data})
    return docs, queries


def manifest(docs: list[dict]) -> str:
    out = ["# 데이터 형식 방해 문서(합성). 원본은 저장소에 넣지 않는다 — `python3 tools/eval/make_data_set.py` 로 다시 만든다.\n"]
    for d in docs:
        out.append(f'[[doc]]\nid = "{d["id"]}"\ntitle = "{d["title"]}"\nsource = "합성(tools/eval/make_data_set.py, seed {SEED})"\n'
                   f'sha256 = "{hashlib.sha256(d["data"]).hexdigest()}"\next = "{d["ext"]}"\ndate = "{d["date"]}"\n')
    return "\n".join(out)


def main() -> None:
    docs, queries = build()
    out = ROOT / "docs"
    out.mkdir(parents=True, exist_ok=True)
    for d in docs:
        (out / f'{d["id"]}.{d["ext"]}').write_bytes(d["data"])
    if "--check" in sys.argv:
        want = (ROOT / "manifest.toml").read_text("utf-8")
        if want != manifest(docs):
            sys.exit("생성 결과가 매니페스트와 다름 — 생성기가 바뀌었다")
        print(f"ok: {len(docs)} documents match")
        return
    (ROOT / "manifest.toml").write_text(manifest(docs), "utf-8")
    lines = ["qid\tquery\ttype\trelevant"] + ["\t".join(q) for q in queries]
    (ROOT / "queries.tsv").write_text("\n".join(lines) + "\n", "utf-8")
    print(f"문서 {len(docs)} · 질의 {len(queries)}")


if __name__ == "__main__":
    main()
