# 시험용 PDF 생성기 — 내용은 모두 합성. 글꼴을 임베드하지 않는다(라이선스 문제 없음).
# 사용: uv run --with reportlab==5.0.1 --with pillow python -I tools/fixtures/make_pdf.py crates/extract/tests/fixtures/pdf
import os, sys
from reportlab.pdfgen import canvas
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.cidfonts import UnicodeCIDFont
from reportlab.lib.utils import ImageReader
from PIL import Image

out = sys.argv[1]
os.makedirs(out, exist_ok=True)
pdfmetrics.registerFont(UnicodeCIDFont("HYGothic-Medium"))  # 비임베드 CID 글꼴(Adobe-Korea1) — pdfium 이 유니코드로 풀어 준다

KOREAN = [
    ["한국마사회 경마 시행 계획", "2026년 렛츠런파크 서울 운영 방안을 보고합니다.", "Mixed English 텍스트 123"],
    ["제2장 예산", "총 사업비는 1,234백만원이며 세부 내역은 다음과 같다."],
]
# ToUnicode 없는 한글 글꼴을 pdfium 이 읽으면 이런 Latin-1 글자가 나온다(조사 2026-10-14) — 같은 모양을 Helvetica 로 흉내.
GARBLED = ["ÄÅÇÉ ÑÖÜá àâäãåç éèêëíìîï", "ñóòôöõ úùûü ÀÁÂÃ ÈÉÊË ÌÍÎÏ"]

def page(c, font, lines):
    y = 780
    for ln in lines:
        c.setFont(font, 14); c.drawString(72, y, ln); y -= 24
    c.showPage()

def new(name, title):
    c = canvas.Canvas(os.path.join(out, name), invariant=1)  # invariant: 시각·ID 고정 → 같은 바이트
    if title:
        c.setTitle(title)
    return c

c = new("korean.pdf", "마사회 검색 시험")
for lines in KOREAN:
    page(c, "HYGothic-Medium", lines)
c.save()

c = new("mixed.pdf", "섞인 문서")
page(c, "HYGothic-Medium", KOREAN[0])
page(c, "Helvetica", GARBLED)
c.save()

c = new("garbled.pdf", "깨진 문서")
page(c, "Helvetica", GARBLED)
c.save()

img = Image.new("RGB", (200, 100), (255, 255, 255))
for x in range(200):
    img.putpixel((x, 50), (0, 0, 0))
c = new("image_only.pdf", "스캔 문서")
c.drawImage(ImageReader(img), 72, 600, 400, 200)
c.showPage(); c.save()

c = new("no_title.pdf", None)  # reportlab 은 제목 자리에 "untitled" 를 쓴다
page(c, "HYGothic-Medium", ["제목 없는 문서 본문"])
c.save()

# 열 때 암호가 필요한 PDF(사용자 암호 "pw", reportlab 기본 암호화) → pdfium PasswordError → Encrypted
c = canvas.Canvas(os.path.join(out, "encrypted.pdf"), invariant=1, encrypt="pw")
page(c, "Helvetica", ["secret body"])
c.save()
print("ok")
