//! 메타 DB 키: 32바이트 OS 난수를 Windows DPAPI(CurrentUser)로 보호해 파일에 둔다(스펙 §4·§7.1).
//! 다른 사용자·다른 PC 에서는 풀 수 없다. 관리자가 로컬 계정 비밀번호를 재설정하면 풀 수 없게 되므로
//! 그때는 Unreadable 을 돌려주고, 앱(`index`)이 meta.db·키워드 색인·키 파일을 지우고 다시 색인한다(스펙 §5.3).
//! 단, 프로필이 아직 안 올라온 경우처럼 일시적일 수 있는 실패는 Io 로 돌려준다 — 색인을 지우지 않고 오류로 끝낸다.
//! 어떤 DPAPI 오류가 "영구 손상"인지는 아래 목록으로 판단하며, 실제 PC 에서의 분류는 KI-030 으로 지켜본다.

use std::io;
use std::path::Path;

pub fn new_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    getrandom::fill(&mut k).expect("OS 난수");
    k
}

#[derive(Debug)]
pub enum KeyError {
    Io(io::Error),
    Unreadable,
}

/// 키 파일이 있으면 풀어 돌려주고, 없으면 새로 만들어 보호해 저장한다.
/// `Unreadable` 은 키를 다시는 풀 수 없다고 판단될 때만(→ 앱이 data 를 지우고 다시 색인). 그 밖의 실패는 `Io`(지우지 않고 오류로 끝냄).
pub fn load_or_create(path: &Path) -> Result<[u8; 32], KeyError> {
    match std::fs::read(path) {
        Ok(blob) => {
            let raw = unprotect(&blob).map_err(|e| if e.kind() == io::ErrorKind::InvalidData { KeyError::Unreadable } else { KeyError::Io(e) })?;
            raw.try_into().map_err(|_| KeyError::Unreadable)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let key = new_key();
            let blob = protect(&key).map_err(KeyError::Io)?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(KeyError::Io)?;
            }
            let tmp = path.with_extension("tmp");
            {
                use std::io::Write;
                let mut f = std::fs::File::create(&tmp).map_err(KeyError::Io)?;
                f.write_all(&blob).map_err(KeyError::Io)?;
                // 바꿔치기 전에 디스크에 내린다 — 전원이 꺼져도 빈 키 파일이 남지 않게.
                f.sync_all().map_err(KeyError::Io)?;
            }
            std::fs::rename(&tmp, path).map_err(KeyError::Io)?;
            Ok(key)
        }
        Err(e) => Err(KeyError::Io(e)),
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use windows::Win32::Foundation::{LocalFree, ERROR_INVALID_DATA, ERROR_INVALID_PARAMETER, HLOCAL, NTE_BAD_DATA, NTE_BAD_KEY_STATE};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};

    /// blob 이 손상됐거나 다른 사용자·PC·비밀번호 재설정 전의 것 → 다시는 못 푼다(InvalidData).
    /// 그 밖(프로필 미적재·메모리 부족 등)은 일시적일 수 있어 Other.
    fn classify(e: windows::core::Error) -> io::Error {
        let c = e.code();
        let permanent = [ERROR_INVALID_DATA.to_hresult(), ERROR_INVALID_PARAMETER.to_hresult(), NTE_BAD_DATA, NTE_BAD_KEY_STATE].contains(&c);
        let kind = if permanent { io::ErrorKind::InvalidData } else { io::ErrorKind::Other };
        io::Error::new(kind, e.to_string())
    }

    /// DPAPI 가 LocalAlloc 으로 준 출력 버퍼를 Vec 으로 복사하고 LocalFree 한다. `wipe` 면 해제 전에 0 으로 지운다.
    unsafe fn take_blob(out: CRYPT_INTEGER_BLOB, wipe: bool) -> Vec<u8> {
        if out.pbData.is_null() {
            return Vec::new();
        }
        let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        if wipe {
            unsafe { std::ptr::write_bytes(out.pbData, 0, out.cbData as usize) };
        }
        unsafe { LocalFree(Some(HLOCAL(out.pbData.cast()))) };
        v
    }

    pub fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB::default();
        unsafe {
            // 설명 문자열은 blob 에 평문으로 들어간다 — 일반 명칭만.
            CryptProtectData(&input, windows::core::w!("kra-search key"), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
                .map_err(|e| io::Error::other(e.to_string()))?;
            Ok(take_blob(out, false))
        }
    }

    pub fn unprotect(blob: &[u8]) -> io::Result<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB { cbData: blob.len() as u32, pbData: blob.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB::default();
        unsafe {
            CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).map_err(classify)?;
            Ok(take_blob(out, true))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    //! 개발용(macOS): 보호하지 않는다. 제품은 Windows 전용이다.
    use std::io;
    const DEV: &[u8] = b"KRA-DEV-UNPROTECTED:";

    pub fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
        Ok([DEV, data].concat())
    }

    pub fn unprotect(blob: &[u8]) -> io::Result<Vec<u8>> {
        blob.strip_prefix(DEV).map(<[u8]>::to_vec).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "개발용 키 형식이 아님"))
    }
}

/// Windows: DPAPI(CurrentUser). 그 밖: 개발용 표지만 붙임(보호 없음).
/// `unprotect`: blob 이 손상됐거나 이 사용자 것이 아니면 `ErrorKind::InvalidData`, 그 밖의 실패(일시적일 수 있음)는 다른 종류.
pub use imp::{protect, unprotect};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protect_roundtrip_and_tamper_fails() {
        let key = new_key();
        let blob = protect(&key).unwrap();
        assert_eq!(unprotect(&blob).unwrap(), key);
        let mut bad = blob.clone();
        bad[0] ^= 0xFF;
        let e = unprotect(&bad).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData, "손상된 blob 은 '다시는 못 푼다'로 분류: {e}");
    }

    #[test]
    fn load_or_create_is_stable_and_detects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("keys/meta.key");
        let a = load_or_create(&p).unwrap();
        let b = load_or_create(&p).unwrap();
        assert_eq!(a, b);
        assert!(!p.with_extension("tmp").exists(), "임시 파일이 남지 않음");
        std::fs::write(&p, b"garbage").unwrap();
        assert!(matches!(load_or_create(&p), Err(KeyError::Unreadable)));
    }

    #[test]
    fn keys_are_random() {
        assert_ne!(new_key(), new_key());
    }
}
