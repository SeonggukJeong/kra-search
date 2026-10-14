//! 파일 앞부분으로 컨테이너 형식을 판별한다.

pub enum Container {
    Ole,
    Zip,
    Other([u8; 8]),
}

pub fn sniff(b: &[u8]) -> Container {
    if b.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return Container::Ole;
    }
    if b.starts_with(b"PK\x03\x04") {
        return Container::Zip;
    }
    let mut m = [0u8; 8];
    for (i, x) in b.iter().take(8).enumerate() {
        m[i] = *x;
    }
    Container::Other(m)
}

/// 알려진 문서보안(DRM) 헤더인지.
pub fn is_known_drm(b: &[u8]) -> bool {
    b.starts_with(b"\x9b DRMONE") || b.starts_with(b"SCDSA")
}

/// 앞 8바이트 16진수(진단 통계용 — 내용이 아닌 형식 식별자).
pub fn magic_hex(b: &[u8]) -> String {
    b.iter().take(8).map(|x| format!("{x:02x}")).collect()
}

/// 진단 보고서용 헤더 표시. 알려진 형식은 이름으로, 텍스트로 보이면 내용 없이 "텍스트"로,
/// 그 밖의 이진 형식만 앞 4바이트(16진수)로 돌려준다. 원본 바이트를 그대로 내보내지 않기 위함.
pub fn report_label(hex8: &str) -> String {
    let bytes: Vec<u8> = (0..hex8.len() / 2).filter_map(|i| u8::from_str_radix(&hex8[i * 2..i * 2 + 2], 16).ok()).collect();
    const KNOWN: &[(&[u8], &str)] = &[
        (b"%PDF", "PDF"),
        (b"{\\rtf", "RTF"),
        (b"<?xml", "XML(HWPML 등)"),
        (b"<HWPML", "HWPML"),
        (b"HWP Docu", "HWP 3.x"),
        (b"\x9b DRMONE", "Fasoo DRM"),
        (b"SCDSA", "SoftCamp DRM"),
        (b"\xEF\xBB\xBF", "텍스트(UTF-8)"),
        (b"\xFF\xFE", "텍스트(UTF-16)"),
    ];
    if let Some((_, name)) = KNOWN.iter().find(|(sig, _)| bytes.starts_with(sig)) {
        return (*name).to_string();
    }
    if bytes.is_empty() {
        return "빈 파일".into();
    }
    let textual = bytes.iter().all(|&b| b == b'\t' || b == b'\n' || b == b'\r' || (0x20..0x7F).contains(&b) || b >= 0x80);
    if textual {
        return "텍스트(내용 비표시)".into();
    }
    format!("기타:{}", &hex8[..hex8.len().min(8)])
}

#[cfg(test)]
mod tests {
    use super::report_label;
    #[test]
    fn labels_hide_text_bytes() {
        assert_eq!(report_label("c8abb1e6b5bf2032"), "텍스트(내용 비표시)");
        assert_eq!(report_label("255044462d312e37"), "PDF");
        assert_eq!(report_label("9b2044524d4f4e45"), "Fasoo DRM");
        assert_eq!(report_label("0001020304050607"), "기타:00010203");
    }
}
