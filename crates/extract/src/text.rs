//! TXT / CSV: 인코딩 감지(BOM → BOM 없는 UTF-16 → UTF-8 → 레거시 추정) + csv 파싱.

use crate::norm::push;
use crate::table::push_table;
use crate::FailKind;
use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use doc::{BlockKind, Document};
use encoding_rs::Encoding;

/// BOM 없는 UTF-16 판정: NUL 이 몰린 쪽이 전체 코드 단위의 이 비율(%) 이상
const UTF16_MAJOR_MIN_PERCENT: usize = 10;
/// 반대쪽 NUL 수 × 이 값 ≤ 몰린 쪽 NUL 수
const UTF16_MINOR_MAX_RATIO_INV: usize = 4;

/// TXT·CSV 를 디코딩하기 전에 남길 바이트 수. 글자 하나는 어떤 인코딩이든 4바이트 이하라
/// 이만큼이면 `MAX_DOC_CHARS` 글자를 채울 수 있다(공백이 아주 많은 파일은 덜 채우고 잘림으로 표시된다).
const MAX_INPUT_BYTES: usize = crate::MAX_DOC_CHARS * 4;

/// 큰 입력의 앞부분만 남긴다. 돌려주는 값 = (남긴 바이트, 잘랐는가).
/// 4의 배수에서 자르고(UTF-16 짝 유지), 앞부분이 올바른 UTF-8 인데 마지막 글자만 걸쳤으면 그 글자 앞에서 자른다
/// — 걸친 글자 하나 때문에 UTF-8 판정이 실패해 레거시 인코딩으로 잘못 읽히지 않게.
pub(crate) fn cap_input(bytes: &[u8]) -> (&[u8], bool) {
    cap_input_at(bytes, MAX_INPUT_BYTES)
}

fn cap_input_at(bytes: &[u8], max: usize) -> (&[u8], bool) {
    if bytes.len() <= max {
        return (bytes, false);
    }
    let mut cut = max & !3;
    if let Err(e) = std::str::from_utf8(&bytes[..cut]) {
        if e.error_len().is_none() {
            cut = e.valid_up_to();
        }
    }
    (&bytes[..cut], true)
}

/// 바이트 → 문자열. 실패해도 대체 문자(U+FFFD)로 채워 돌려준다.
pub(crate) fn decode(bytes: &[u8]) -> String {
    // 1) BOM(UTF-8 / UTF-16LE / UTF-16BE)
    if let Some((enc, bom_len)) = Encoding::for_bom(bytes) {
        return enc.decode_without_bom_handling(&bytes[bom_len..]).0.into_owned();
    }
    // 2) BOM 없는 UTF-16: ASCII(공백·숫자·줄바꿈)의 상위 바이트가 0 이라 **한쪽 자리(짝/홀)에 NUL 이 몰린다**(major).
    //    반대쪽(minor)에도 NUL 이 조금 있다 — 가(AC00)·글(AE00)·대(B300) 같은 U+xx00 음절은 낮은 바이트가 0 이다
    //    (실제 한국어 문장에서 minor 2~3%, major 30% 대). 그래서 minor 는 절대 비율이 아니라 major 대비로 본다:
    //    major ≥ 10%(UTF16_MAJOR_MIN_PERCENT) 이고 minor × UTF16_MINOR_MAX_RATIO_INV(4) ≤ major.
    //    NUL 하나 섞인 UTF-8·CP949 는 양쪽 다 0% 근처라 major 문턱에서 걸러진다
    //    (UTF-8 한글을 UTF-16 으로 풀면 오류 없이 깨진 한자가 나오므로 had_errors 로는 못 거른다 — 리뷰에서 재현).
    //    ASCII 만 든 UTF-16 은 올바른 UTF-8 이기도 해서 UTF-8 검사보다 먼저 한다.
    //    공백·숫자 없이 한글만 있는 UTF-16 은 NUL 이 거의 없어 못 잡는다 — 메모장은 항상 BOM 을 쓴다.
    //    빅엔디언은 짝/홀이 뒤바뀐 대칭이다.
    let units = bytes.len() / 2;
    let zeros_even = bytes.iter().step_by(2).filter(|&&b| b == 0).count();
    let zeros_odd = bytes.iter().skip(1).step_by(2).filter(|&&b| b == 0).count();
    let (major, minor) = (zeros_even.max(zeros_odd), zeros_even.min(zeros_odd));
    if bytes.len().is_multiple_of(2)
        && units > 0
        && major * 100 >= units * UTF16_MAJOR_MIN_PERCENT
        && minor * UTF16_MINOR_MAX_RATIO_INV <= major
    {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_input_keeps_utf8_and_utf16_decodable() {
        // UTF-8 한글(3바이트)을 글자 중간에서 자르지 않는다 → 레거시 인코딩으로 오판하지 않음
        let s = "가나다라마바사".as_bytes();
        let (head, cut) = cap_input_at(s, 8);
        assert!(cut);
        assert_eq!(decode(head), "가나");
        // UTF-16LE(BOM 없음): 4의 배수에서 잘라 짝이 맞는다
        let u: Vec<u8> = "한 1 국 2 마 3 사 4".encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        let (head, cut) = cap_input_at(&u, 10);
        assert!(cut);
        assert_eq!(decode(head), "한 1 ");
        // 작으면 그대로
        assert_eq!(cap_input_at(s, 100), (s, false));
    }

    #[test]
    fn csv_cut_inside_quoted_cell_still_parses() {
        // 바이트 상한이 따옴표 칸 가운데를 자를 수 있다 — 손상으로 보지 않는다
        assert!(csv("구분,\"여러 줄\n칸".as_bytes()).is_ok());
    }
}
