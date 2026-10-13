//! 추출 누락 측정: rhwp 조판 결과(페이지 텍스트)의 줄이 우리 블록 텍스트에 들어 있는 비율.
//!
//! 글자·숫자만 비교한다 — 조판이 그리는 글머리표(-, ·, ▸)·장식선·자동 번호의 기호는 본문에 없다.
//! 자동 번호의 숫자("1.", "①")도 문단 텍스트에 없으므로 줄 앞 숫자는 떼고 다시 비교한다.
//! 조판이 필요해 느리므로(문서당 수십 ms) 진단·평가에서만 쓴다.

use doc::Block;

/// 5자 미만 줄(쪽 번호·머리표 조각)은 비교하지 않는다.
const MIN_LINE_CHARS: usize = 5;

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric() && !matches!(*c as u32, 0xE000..=0xF8FF | 0xF0000..=0x10FFFD)).collect()
}

/// 0.0~1.0(글자 수 가중). 페이지 텍스트를 얻지 못하거나 비교할 글자가 없으면 None.
pub fn measure(bytes: &[u8], blocks: &[Block]) -> Option<f32> {
    let page_text = std::panic::catch_unwind(|| {
        let d = rhwp::wasm_api::HwpDocument::from_bytes(bytes).ok()?;
        let pages: Vec<String> = (0..d.page_count()).map(|p| d.extract_page_text_native(p).unwrap_or_default()).collect();
        Some(pages.join("\n"))
    })
    .ok()??;
    let ours: String = blocks.iter().map(|b| norm(&b.text)).collect();
    let (mut hit, mut total) = (0usize, 0usize);
    for line in page_text.lines() {
        let l = norm(line);
        let n = l.chars().count();
        if n < MIN_LINE_CHARS {
            continue;
        }
        total += n;
        // 자동 번호("1.", "①", "10.")는 조판만 그린다 — 앞쪽 숫자를 떼고 한 번 더 본다.
        if ours.contains(&l) || ours.contains(l.trim_start_matches(|c: char| c.is_numeric())) {
            hit += n;
        }
    }
    (total > 0).then(|| hit as f32 / total as f32)
}
