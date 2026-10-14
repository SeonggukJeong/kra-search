//! TXT / CSV: 인코딩 감지(BOM → BOM 없는 UTF-16 → UTF-8 → 레거시 추정) + csv 파싱.

use crate::norm::push;
use crate::table::push_table;
use crate::FailKind;
use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use doc::{BlockKind, Document};
use encoding_rs::Encoding;

/// 바이트 → 문자열. 실패해도 대체 문자(U+FFFD)로 채워 돌려준다.
pub(crate) fn decode(bytes: &[u8]) -> String {
    // 1) BOM(UTF-8 / UTF-16LE / UTF-16BE)
    if let Some((enc, bom_len)) = Encoding::for_bom(bytes) {
        return enc.decode_without_bom_handling(&bytes[bom_len..]).0.into_owned();
    }
    // 2) BOM 없는 UTF-16: ASCII(공백·숫자·줄바꿈)의 상위 바이트가 0 이라 **한쪽 자리(짝/홀)에만** NUL 이 몰린다.
    //    그 비율로 판정한다 — 한쪽 ≥ 10%, 다른 쪽 ≤ 2%. NUL 하나 섞인 UTF-8 은 비율이 낮아 여기서 걸리지 않는다
    //    (UTF-8 한글을 UTF-16 으로 풀면 오류 없이 깨진 한자가 나오므로 had_errors 로는 못 거른다 — 리뷰에서 재현).
    //    ASCII 만 든 UTF-16 은 올바른 UTF-8 이기도 해서 UTF-8 검사보다 먼저 한다.
    //    공백·숫자 없이 한글만 있는 UTF-16 은 NUL 이 거의 없어 못 잡는다 — 메모장은 항상 BOM 을 쓴다.
    let units = bytes.len() / 2;
    let zeros_even = bytes.iter().step_by(2).filter(|&&b| b == 0).count();
    let zeros_odd = bytes.iter().skip(1).step_by(2).filter(|&&b| b == 0).count();
    let (major, minor) = (zeros_even.max(zeros_odd), zeros_even.min(zeros_odd));
    if bytes.len().is_multiple_of(2) && units > 0 && major * 10 >= units && minor * 50 <= units {
        let enc = if zeros_odd >= zeros_even { encoding_rs::UTF_16LE } else { encoding_rs::UTF_16BE };
        let (s, had_errors) = enc.decode_without_bom_handling(bytes);
        if !had_errors {
            return s.into_owned();
        }
    }
    // 3) 올바른 UTF-8 이면 그대로(ASCII 포함)
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_owned();
    }
    // 4) 레거시 인코딩 추정. tld "kr" 은 EUC-KR(=CP949) 쪽으로 기운다. tld 는 반드시 소문자 ASCII 상수(대문자면 chardetng 패닉).
    let mut det = EncodingDetector::new(Iso2022JpDetection::Deny);
    det.feed(bytes, true);
    let enc = det.guess(Some(b"kr"), Utf8Detection::Allow);
    enc.decode_without_bom_handling(bytes).0.into_owned()
}

/// 빈 줄로 문단을 나눈다.
pub(crate) fn txt(bytes: &[u8]) -> Document {
    let s = decode(bytes).replace("\r\n", "\n");
    let mut out = Vec::new();
    let mut para = String::new();
    for line in s.split('\n') {
        if line.trim().is_empty() {
            push(&mut out, BlockKind::Paragraph, &para);
            para.clear();
        } else {
            para.push_str(line);
            para.push('\n');
        }
    }
    push(&mut out, BlockKind::Paragraph, &para);
    Document { title: None, blocks: out }
}

pub(crate) fn csv(bytes: &[u8]) -> Result<Document, FailKind> {
    let s = decode(bytes);
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false) // 머리글 판단은 push_table 이 한다
        .flexible(true) // 행마다 칸 수가 달라도 허용
        .from_reader(s.as_bytes());
    let mut rows = Vec::new();
    for rec in rdr.records().take(crate::sheet::MAX_ROWS_PER_SHEET) {
        let rec = rec.map_err(|_| FailKind::Corrupt)?;
        rows.push(rec.iter().map(str::to_owned).collect::<Vec<_>>());
    }
    let mut out = Vec::new();
    push_table(&mut out, &rows);
    Ok(Document { title: None, blocks: out })
}
