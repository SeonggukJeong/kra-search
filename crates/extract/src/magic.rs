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
