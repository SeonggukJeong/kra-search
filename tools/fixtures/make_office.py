# 시험용 DOCX/PPTX/ODT/ODP/XLSX/ODS/TXT/CSV 생성기 — 표준 라이브러리(zipfile)만 사용. 내용은 모두 합성.
# 사용: python3 -I tools/fixtures/make_office.py crates/extract/tests/fixtures/office
import os, sys, zipfile

OUT = sys.argv[1]
os.makedirs(OUT, exist_ok=True)
XML = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
FIXED_TIME = (2026, 1, 1, 0, 0, 0)  # 다시 만들어도 같은 바이트가 나오게 고정


def write_zip(name, parts, mimetype=None):
    """parts: [(경로, 문자열 또는 bytes)]. ODF 는 mimetype 을 무압축 첫 항목으로 넣는다."""
    with zipfile.ZipFile(os.path.join(OUT, name), "w", zipfile.ZIP_DEFLATED) as z:
        if mimetype:
            z.writestr(zipfile.ZipInfo("mimetype", FIXED_TIME), mimetype, compress_type=zipfile.ZIP_STORED)
        for path, data in parts:
            raw = data if isinstance(data, bytes) else data.encode("utf-8")
            z.writestr(zipfile.ZipInfo(path, FIXED_TIME), raw, compress_type=zipfile.ZIP_DEFLATED)


def core_xml(title):
    return XML + (
        '<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" '
        'xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" '
        'xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">'
        f'<dc:title>{title}</dc:title><dc:creator>작성자</dc:creator></cp:coreProperties>')


# ---------------- DOCX ----------------
W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" ' \
    'xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" ' \
    'xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" ' \
    'xmlns:v="urn:schemas-microsoft-com:vml"'

def p(text, style=None, ppr_extra=""):
    style_xml = f'<w:pStyle w:val="{style}"/>' if style else ""
    ppr = f'<w:pPr>{style_xml}{ppr_extra}</w:pPr>' if (style or ppr_extra) else ""
    return f'<w:p>{ppr}<w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p>'

def tc(*paras):
    return "<w:tc><w:tcPr/>" + "".join(p(x) for x in paras) + "</w:tc>"

docx_body = "".join([
    p("사업 개요", style="1"),                      # 한국어 Word: 제목 1 의 styleId 가 "1"
    p("본 문서는 렛츠런파크 운영 계획이다."),
    # 여러 run + 탭 + 줄바꿈 + 엔티티 + 문자참조(&#xAC00; = 가)
    '<w:p><w:r><w:t>항목</w:t></w:r><w:r><w:tab/><w:t>A&amp;B &lt;중요&gt; &#xAC00;나다</w:t></w:r>'
    '<w:r><w:br/><w:t>둘째 줄</w:t></w:r></w:p>',
    p("세부 추진 계획", style="Heading2"),          # 영문 Word styleId
    p("개요 수준만 지정된 제목", ppr_extra='<w:outlineLvl w:val="2"/>'),
    # 변경 추적 삭제 글·옮기기 전 자리(moveFrom, 글은 w:t 로 들어 있음)·필드 코드는 빠지고, 옮긴 자리(moveTo)는 남아야 한다
    '<w:p><w:r><w:t>남은 글</w:t></w:r><w:del w:id="1" w:author="x"><w:r><w:delText>지운 글</w:delText></w:r></w:del>'
    '<w:moveFrom w:id="2" w:author="x"><w:r><w:t>옮기기 전 글</w:t></w:r></w:moveFrom>'
    '<w:moveTo w:id="3" w:author="x"><w:r><w:t xml:space="preserve"> 옮긴 글</w:t></w:r></w:moveTo>'
    '<w:r><w:instrText> PAGE </w:instrText></w:r></w:p>',
    # 표: 첫 행 = 머리글, 한 칸에 문단 두 개
    '<w:tbl><w:tblPr/><w:tblGrid/>'
    '<w:tr>' + tc("구분") + tc("예산(백만원)") + tc("비고") + '</w:tr>'
    '<w:tr>' + tc("경마 시행") + tc("1,200") + tc("전년 대비", "5% 증가") + '</w:tr>'
    '<w:tr>' + tc("시설 개선") + tc("300") + tc("") + '</w:tr>'
    '</w:tbl>',
    # 글상자: mc:Choice 와 mc:Fallback 에 같은 글이 두 번 들어 있다 → 한 번만 나와야 함
    '<w:p><w:r><w:t>글상자 앞</w:t></w:r><w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing><wps:txbx><w:txbxContent>'
    + p("글상자 안 문장") + '</w:txbxContent></wps:txbx></w:drawing></mc:Choice><mc:Fallback><w:pict><v:textbox><w:txbxContent>'
    + p("글상자 안 문장") + '</w:txbxContent></v:textbox></w:pict></mc:Fallback></mc:AlternateContent></w:r></w:p>',
    p("맺음말"),
])
docx_document = XML + f'<w:document {W}><w:body>{docx_body}<w:sectPr/></w:body></w:document>'
docx_styles = XML + (
    '<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
    '<w:style w:type="paragraph" w:default="1" w:styleId="a"><w:name w:val="Normal"/></w:style>'
    '<w:style w:type="paragraph" w:styleId="1"><w:name w:val="heading 1"/><w:basedOn w:val="a"/>'
    '<w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style>'
    '<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/></w:style>'
    '</w:styles>')
def write_docx(name, document, core_title):
    """core_title 이 None 이면 docProps/core.xml 없이(제목 메타데이터 없는 문서) 만든다."""
    core_override = ('<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>'
                     if core_title is not None else '')
    core_rel = ('<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>'
                if core_title is not None else '')
    parts = [
        ("[Content_Types].xml", XML + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
         '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
         '<Default Extension="xml" ContentType="application/xml"/>'
         '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
         '<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>'
         + core_override + '</Types>'),
        ("_rels/.rels", XML + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
         '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>'
         + core_rel + '</Relationships>'),
        ("word/_rels/document.xml.rels", XML + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
         '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>'),
        ("word/document.xml", document),
        ("word/styles.xml", docx_styles),
    ]
    if core_title is not None:
        parts.append(("docProps/core.xml", core_xml(core_title)))
    write_zip(name, parts)

write_docx("sample.docx", docx_document, "2026 운영 계획 &amp; 예산")
# 제목 메타데이터가 없는 문서 → 첫 제목 문단이 문서 제목이 된다
write_docx("no_core.docx", XML + f'<w:document {W}><w:body>' + p("보고서 본문 앞 문단") + p("인사 규정 개정 검토", style="1")
           + p("검토 결과를 보고한다.") + '<w:sectPr/></w:body></w:document>', None)

# ---------------- PPTX ----------------
PNS = 'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" ' \
      'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" ' \
      'xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"'

def sp(ph_type, paras):
    ph = f'<p:ph type="{ph_type}"/>' if ph_type else '<p:ph idx="1"/>'
    body = "".join(f'<a:p>{x}</a:p>' for x in paras)
    return (f'<p:sp><p:nvSpPr><p:cNvPr id="2" name="s"/><p:cNvSpPr/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:spPr/>'
            f'<p:txBody><a:bodyPr/><a:lstStyle/>{body}</p:txBody></p:sp>')

def run(t):
    return f'<a:r><a:rPr lang="ko-KR"/><a:t>{t}</a:t></a:r>'

def slide(*shapes):
    return XML + f'<p:sld {PNS}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{"".join(shapes)}</p:spTree></p:cSld></p:sld>'

pptx_table = ('<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="t"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm/>'
              '<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblGrid/>'
              '<a:tr h="0"><a:tc><a:txBody><a:p>' + run("지역") + '</a:p></a:txBody></a:tc><a:tc><a:txBody><a:p>' + run("입장객") + '</a:p></a:txBody></a:tc></a:tr>'
              '<a:tr h="0"><a:tc><a:txBody><a:p>' + run("서울") + '</a:p></a:txBody></a:tc><a:tc><a:txBody><a:p>' + run("12,000") + '</a:p></a:txBody></a:tc></a:tr>'
              '</a:tbl></a:graphicData></a:graphic></p:graphicFrame>')
slides = {
    "slide1.xml": slide(sp("ctrTitle", [run("더비 운영 계획")]), sp("subTitle", [run("2026년 3월")])),
    "slide2.xml": slide(sp("title", [run("마지막 슬라이드")]), sp(None, [run("감사합니다")])),
    # PowerPoint 는 줄바꿈을 빈 요소가 아니라 <a:br><a:rPr …/></a:br> 로 쓴다 — 이 형태로 넣어야 실제 파일을 시험한다
    "slide10.xml": slide(sp("title", [run("추진 일정")]), sp(None, [run("1단계: 준비") + '<a:br><a:rPr lang="ko-KR"/></a:br>' + run("2단계: 시행"), run("세부 항목 &amp; 비고")]), pptx_table),
}
# 발표 순서: slide1 → slide10 → slide2 (파일 이름 숫자 순서와 다름)
order = [("rId2", "slide1.xml"), ("rId3", "slide10.xml"), ("rId4", "slide2.xml")]

def write_pptx(name, slides, order, core_title):
    pres = XML + f'<p:presentation {PNS}><p:sldIdLst>' + "".join(
        f'<p:sldId id="{256 + i}" r:id="{rid}"/>' for i, (rid, _) in enumerate(order)) + '</p:sldIdLst></p:presentation>'
    pres_rels = XML + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">' + "".join(
        f'<Relationship Id="{rid}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/{f}"/>'
        for rid, f in order) + '</Relationships>'
    write_zip(name, [
        ("[Content_Types].xml", XML + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
         '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
         '<Default Extension="xml" ContentType="application/xml"/>'
         '<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>'
         + "".join(f'<Override PartName="/ppt/slides/{f}" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>' for f in slides)
         + '</Types>'),
        ("_rels/.rels", XML + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
         '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>'),
        ("ppt/presentation.xml", pres),
        ("ppt/_rels/presentation.xml.rels", pres_rels),
        *[(f"ppt/slides/{f}", s) for f, s in slides.items()],
        ("docProps/core.xml", core_xml(core_title)),
    ])

write_pptx("sample.pptx", slides, order, "더비 발표")
# 한국어 PowerPoint 가 새 파일에 넣는 기본 제목 → 버리고 첫 슬라이드 제목을 쓴다
write_pptx("placeholder_title.pptx",
           {"slide1.xml": slide(sp("ctrTitle", [run("2026 고객 만족 개선 방안")]), sp(None, [run("고객지원팀")]))},
           [("rId2", "slide1.xml")], "PowerPoint 프레젠테이션")

# ---------------- ODT / ODP ----------------
ONS = ('xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" '
       'xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" '
       'xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" '
       'xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" '
       'xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" '
       'xmlns:dc="http://purl.org/dc/elements/1.1/" '
       'xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0"')

def odf_parts(mime, content_body, title):
    manifest = XML + ('<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3">'
                      f'<manifest:file-entry manifest:full-path="/" manifest:media-type="{mime}"/>'
                      '<manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>'
                      '<manifest:file-entry manifest:full-path="meta.xml" manifest:media-type="text/xml"/></manifest:manifest>')
    content = XML + f'<office:document-content {ONS} office:version="1.3"><office:body>{content_body}</office:body></office:document-content>'
    meta = XML + f'<office:document-meta {ONS} office:version="1.3"><office:meta><dc:title>{title}</dc:title></office:meta></office:document-meta>'
    return [("META-INF/manifest.xml", manifest), ("content.xml", content), ("meta.xml", meta)]

def odf_encrypted_parts(mime):
    """암호를 건 ODF 의 모양: 여전히 ZIP 이고, manifest 에 encryption-data 가 있으며 content.xml 은 암호문(UTF-8 아님)."""
    manifest = XML + ('<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3">'
                      f'<manifest:file-entry manifest:full-path="/" manifest:media-type="{mime}"/>'
                      '<manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml" manifest:size="512">'
                      '<manifest:encryption-data manifest:checksum-type="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#sha256-1k" manifest:checksum="AAAA">'
                      '<manifest:algorithm manifest:algorithm-name="http://www.w3.org/2001/04/xmlenc#aes256-cbc" manifest:initialisation-vector="AAAA"/>'
                      '<manifest:start-key-generation manifest:start-key-generation-name="http://www.w3.org/2000/09/xmldsig#sha256" manifest:key-size="32"/>'
                      '<manifest:key-derivation manifest:key-derivation-name="PBKDF2" manifest:key-size="32" manifest:iteration-count="100000" manifest:salt="AAAA"/>'
                      '</manifest:encryption-data></manifest:file-entry></manifest:manifest>')
    cipher = bytes((i * 151 + 7) % 256 for i in range(512))  # 고정된 가짜 암호문(0x80 이상 바이트 포함 → UTF-8 아님)
    return [("META-INF/manifest.xml", manifest), ("content.xml", cipher)]

def cell(t):
    return f'<table:table-cell office:value-type="string"><text:p>{t}</text:p></table:table-cell>'

odt_body = ('<office:text>'
            '<text:h text:outline-level="1">제1장 <text:span>총칙</text:span></text:h>'
            '<text:p>목적<text:s/>및<text:s text:c="3"/>범위<text:tab/>탭 뒤<text:line-break/>다음 줄</text:p>'
            '<text:p>각주가 있는 문장<text:note text:note-class="footnote"><text:note-citation>1</text:note-citation>'
            '<text:note-body><text:p>각주 본문</text:p></text:note-body></text:note> 이어지는 글</text:p>'
            '<text:list><text:list-item><text:p>목록 항목 &amp; 하나</text:p></text:list-item></text:list>'
            '<table:table table:name="표1"><table:table-column table:number-columns-repeated="3"/>'
            '<table:table-header-rows><table:table-row>' + cell("구분") + cell("수량") + cell("비고") + '</table:table-row></table:table-header-rows>'
            '<table:table-row>' + cell("말") + cell("12") + '<table:table-cell/></table:table-row>'
            '<table:table-row><table:table-cell table:number-columns-spanned="2" office:value-type="string"><text:p>합쳐진 칸</text:p></table:table-cell>'
            '<table:covered-table-cell/>' + cell("끝") + '</table:table-row>'
            '</table:table>'
            '<text:p>맺음</text:p>'
            '</office:text>')
write_zip("sample.odt", odf_parts("application/vnd.oasis.opendocument.text", odt_body, "규정 개정안"),
          mimetype="application/vnd.oasis.opendocument.text")
# 제목 메타데이터가 비어 있음 → 첫 제목 문단("제1장 총칙")이 문서 제목
write_zip("untitled.odt", odf_parts("application/vnd.oasis.opendocument.text", odt_body, ""),
          mimetype="application/vnd.oasis.opendocument.text")
write_zip("encrypted.odt", odf_encrypted_parts("application/vnd.oasis.opendocument.text"),
          mimetype="application/vnd.oasis.opendocument.text")

def frame(cls, inner):
    c = f' presentation:class="{cls}"' if cls else ""
    return f'<draw:frame{c}><draw:text-box>{inner}</draw:text-box></draw:frame>'

odp_body = ('<office:presentation>'
            '<draw:page draw:name="page1">' + frame("title", "<text:p>발표 제목</text:p>") + frame("subtitle", "<text:p>부제목</text:p>") + '</draw:page>'
            '<draw:page draw:name="page2">' + frame("title", "<text:p>둘째 장</text:p>")
            + frame("outline", "<text:list><text:list-item><text:p>요점 하나</text:p></text:list-item><text:list-item><text:p>요점 둘</text:p></text:list-item></text:list>")
            + '<presentation:notes><draw:frame presentation:class="notes"><draw:text-box><text:p>발표자 메모</text:p></draw:text-box></draw:frame></presentation:notes>'
            + '</draw:page>'
            '</office:presentation>')
write_zip("sample.odp", odf_parts("application/vnd.oasis.opendocument.presentation", odp_body, "발표 자료"),
          mimetype="application/vnd.oasis.opendocument.presentation")

# ---------------- ODS ----------------
ods_body = ('<office:spreadsheet>'
            '<table:table table:name="예산">'
            '<table:table-row><table:table-cell office:value-type="string"><text:p>구분</text:p></table:table-cell>'
            '<table:table-cell office:value-type="string"><text:p>금액</text:p></table:table-cell>'
            '<table:table-cell office:value-type="string"><text:p>일자</text:p></table:table-cell></table:table-row>'
            '<table:table-row><table:table-cell office:value-type="string"><text:p>경마</text:p></table:table-cell>'
            '<table:table-cell office:value-type="float" office:value="1200"><text:p>1,200</text:p></table:table-cell>'
            '<table:table-cell office:value-type="date" office:date-value="2026-03-01"><text:p>2026-03-01</text:p></table:table-cell></table:table-row>'
            '<table:table-row><table:table-cell office:value-type="string"><text:p>시설</text:p></table:table-cell>'
            '<table:table-cell office:value-type="float" office:value="0.5"><text:p>0.5</text:p></table:table-cell>'
            '<table:table-cell/></table:table-row>'
            # 실제 LibreOffice 파일처럼 끝에 거대한 반복 빈 행
            '<table:table-row table:number-rows-repeated="1048573"><table:table-cell table:number-columns-repeated="1024"/></table:table-row>'
            '</table:table>'
            '<table:table table:name="빈 시트"><table:table-row><table:table-cell/></table:table-row></table:table>'
            '</office:spreadsheet>')
write_zip("sample.ods", odf_parts("application/vnd.oasis.opendocument.spreadsheet", ods_body, "예산표"),
          mimetype="application/vnd.oasis.opendocument.spreadsheet")

# ---------------- XLSX (최소 구성) ----------------
def c_str(ref, idx):
    return f'<c r="{ref}" t="s"><v>{idx}</v></c>'
shared = ["구분", "금액", "일자", "경마 시행", "시설 개선", "비고"]
sheet1 = XML + ('<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>'
                # 1행은 비어 있음(행 자체 없음) → 2행이 첫 비어있지 않은 행
                '<row r="2">' + c_str("A2", 0) + c_str("B2", 1) + c_str("C2", 2) + c_str("D2", 5) + '</row>'
                '<row r="3">' + c_str("A3", 3) + '<c r="B3"><v>1200</v></c><c r="C3" s="1"><v>46082</v></c>'
                '<c r="D3" t="inlineStr"><is><t>인라인 문자열</t></is></c></row>'
                '<row r="4">' + c_str("A4", 4) + '<c r="B4"><v>300.25</v></c><c r="D4" t="b"><v>1</v></c></row>'
                '<row r="5"><c r="B5"><f>SUM(B3:B4)</f><v>1500.25</v></c></row>'
                '</sheetData></worksheet>')
sheet2 = XML + '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/></worksheet>'
sst = XML + f'<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{len(shared)}" uniqueCount="{len(shared)}">' + \
    "".join(f"<si><t>{s}</t></si>" for s in shared) + '</sst>'
styles = XML + ('<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
                '<cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="14" applyNumberFormat="1"/></cellXfs></styleSheet>')
workbook = XML + ('<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
                  'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>'
                  '<sheet name="예산" sheetId="1" r:id="rId1"/><sheet name="빈 시트" sheetId="2" r:id="rId2"/></sheets></workbook>')
wb_rels = XML + ('<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
                 '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>'
                 '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>'
                 '<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>'
                 '<Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>')
write_zip("sample.xlsx", [
    ("[Content_Types].xml", XML + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
     '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
     '<Default Extension="xml" ContentType="application/xml"/>'
     '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
     '<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
     '<Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
     '<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>'
     '<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>'),
    ("_rels/.rels", XML + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
     '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>'),
    ("xl/workbook.xml", workbook), ("xl/_rels/workbook.xml.rels", wb_rels),
    ("xl/worksheets/sheet1.xml", sheet1), ("xl/worksheets/sheet2.xml", sheet2),
    ("xl/sharedStrings.xml", sst), ("xl/styles.xml", styles),
    ("docProps/core.xml", core_xml("예산 집계")),
])

# ---------------- TXT / CSV ----------------
KO = "한국마사회 경마 시행 계획\n렛츠런파크 서울 운영 방안을 보고합니다.\n"
def wb(name, data):
    with open(os.path.join(OUT, name), "wb") as f:
        f.write(data)
wb("utf8.txt", KO.encode("utf-8"))
wb("utf8_bom.txt", b"\xef\xbb\xbf" + KO.encode("utf-8"))
wb("cp949.txt", KO.encode("cp949"))
wb("cp949_short.txt", "회의록".encode("cp949"))            # 6바이트 — 감지 어려운 짧은 입력
wb("cp949_ext.txt", "똠방각하 펲뷁 샾".encode("cp949"))     # 완성형 밖(확장 영역) 글자
wb("utf16le_bom.txt", "﻿".encode("utf-16-le") + KO.encode("utf-16-le"))
wb("utf16be_bom.txt", "﻿".encode("utf-16-be") + KO.encode("utf-16-be"))
wb("utf16le_nobom.txt", KO.encode("utf-16-le"))
wb("ascii.txt", b"plain ascii only\n")
wb("cp949.csv", ('구분,금액,비고\n경마 시행,"1,200","따옴표 ""안"" 글"\n시설 개선,300,\n'
                 '"여러 줄\n칸",5,끝\n').encode("cp949"))
print("ok")
