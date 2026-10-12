//! Windows 전용 환경 점검: EFS 사용 가능 여부, WebView2 버전, 전역 단축키 등록, 백신 목록.
//! 실패해도 패닉하지 않고 문자열로 기록한다.

use crate::report::{EnvInfo, HotkeyCheck};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Storage::FileSystem::{DecryptFileW, EncryptFileW, FileEncryptionStatusW, FILE_IS_ENCRYPTED};
use windows::Win32::System::Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, VK_SPACE,
};

pub fn fill(e: &mut EnvInfo) {
    e.efs = efs_probe();
    e.webview2 = webview2_version();
    e.hotkeys = vec![
        hotkey("Alt+Space", MOD_ALT | MOD_NOREPEAT),
        hotkey("Ctrl+Alt+Space", MOD_CONTROL | MOD_ALT | MOD_NOREPEAT),
    ];
    e.antivirus = antivirus();
}

fn efs_probe() -> String {
    let Some(base) = std::env::var_os("LOCALAPPDATA") else { return "error(LOCALAPPDATA 없음)".into() };
    let dir = std::path::Path::new(&base).join("KraSearchDiag").join("efs-probe");
    if std::fs::create_dir_all(&dir).is_err() {
        return "error(폴더 생성 실패)".into();
    }
    let h = HSTRING::from(dir.as_os_str());
    let result = unsafe {
        match EncryptFileW(PCWSTR(h.as_ptr())) {
            Err(err) => format!("unsupported({:#x})", err.code().0),
            Ok(()) => {
                let mut status = 0u32;
                let r = match FileEncryptionStatusW(PCWSTR(h.as_ptr()), &mut status) {
                    Ok(()) if status == FILE_IS_ENCRYPTED => "supported".to_string(),
                    Ok(()) => format!("unsupported(status {status})"),
                    Err(err) => format!("error({:#x})", err.code().0),
                };
                let _ = DecryptFileW(PCWSTR(h.as_ptr()), None);
                r
            }
        }
    };
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    result
}

fn reg_sz(root: HKEY, key: &str, value: &str) -> Option<String> {
    let (k, v) = (HSTRING::from(key), HSTRING::from(value));
    let mut buf = vec![0u16; 256];
    let mut size = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(root, PCWSTR(k.as_ptr()), PCWSTR(v.as_ptr()), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut size))
    };
    if rc.is_err() {
        return None;
    }
    let n = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..n.min(buf.len())]).trim_end_matches('\0').to_string())
}

fn webview2_version() -> Option<String> {
    const GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    reg_sz(HKEY_LOCAL_MACHINE, &format!(r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{GUID}"), "pv")
        .or_else(|| reg_sz(HKEY_CURRENT_USER, &format!(r"Software\Microsoft\EdgeUpdate\Clients\{GUID}"), "pv"))
        .filter(|v| !v.is_empty() && v != "0.0.0.0")
}

fn hotkey(name: &str, mods: HOT_KEY_MODIFIERS) -> HotkeyCheck {
    let ok = unsafe { RegisterHotKey(None, 0x4B52, mods, VK_SPACE.0 as u32).is_ok() };
    if ok {
        unsafe {
            let _ = UnregisterHotKey(None, 0x4B52);
        }
    }
    HotkeyCheck { combo: name.into(), registered: ok }
}

#[derive(serde::Deserialize)]
#[serde(rename = "AntiVirusProduct")]
#[serde(rename_all = "camelCase")]
struct AntiVirusProduct {
    display_name: String,
}

fn antivirus() -> Vec<String> {
    let Ok(con) = wmi::WMIConnection::with_namespace_path("root\\SecurityCenter2") else { return vec![] };
    con.query::<AntiVirusProduct>().map(|v| v.into_iter().map(|p| p.display_name).collect()).unwrap_or_default()
}
