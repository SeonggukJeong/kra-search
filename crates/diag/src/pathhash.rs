//! 진단 보고서용 경로 익명화: 경로·파일명 대신 해시 일부 + 확장자만 남긴다.

use sha2::{Digest, Sha256};
use std::path::Path;

// 파일별 항목을 보고서에 넣어야 할 때를 위한 유일한 경로 표시 방법. 현재 보고서는 집계만 담는다.
#[allow(dead_code)]
pub fn anon(p: &Path) -> String {
    let h = Sha256::digest(p.to_string_lossy().as_bytes());
    let hex: String = h.iter().take(6).map(|b| format!("{b:02x}")).collect();
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    format!("{hex}.{ext}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn anon_is_stable_and_hides_name() {
        let p = std::path::Path::new(r"C:\Users\홍길동\문서\응시원서.HWP");
        let a = super::anon(p);
        assert_eq!(a, super::anon(p));
        assert!(a.ends_with(".hwp") && !a.contains("홍길동") && a.len() == 16);
    }
}
