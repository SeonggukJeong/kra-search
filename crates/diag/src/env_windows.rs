//! Windows 전용 환경 점검: EFS 사용 가능 여부, WebView2 버전, 전역 단축키 등록, 백신 목록.
//! 실패해도 패닉하지 않고 문자열로 기록한다.

use crate::report::{EnvInfo, HotkeyCheck};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Storage::FileSystem::GetVolumeInformationW;

/// GetVolumeInformationW 파일 시스템 플래그: 볼륨이 EFS 를 지원함(winnt.h FILE_SUPPORTS_ENCRYPTION).
const FILE_SUPPORTS_ENCRYPTION: u32 = 0x0002_0000;
use windows::Win32::System::Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};
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

/// EFS 사용 가능 여부를 **아무것도 바꾸지 않고** 확인한다(파일 암호화·인증서 생성 없음).
/// Windows 에디션(Home은 EFS 없음) · 사용자 데이터 볼륨의 암호화 지원 · EFS 사용 안 함 정책을 읽는다.
fn efs_probe() -> String {
    let edition = reg_sz(HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "EditionID").unwrap_or_default();
    let policy_off = reg_dword(HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\EFS", "EfsConfiguration") == Some(1);
    let volume = std::env::var("LOCALAPPDATA").ok().and_then(|p| p.get(..3).map(str::to_string)).unwrap_or_else(|| "C:\\".into());
    let mut flags = 0u32;
    let root = HSTRING::from(volume.as_str());
    let vol_ok = unsafe { GetVolumeInformationW(PCWSTR(root.as_ptr()), None, None, None, Some(&mut flags), None) }.is_ok()
        && flags & FILE_SUPPORTS_ENCRYPTION != 0;
    let home = edition.starts_with("Core");
    let ok = vol_ok && !policy_off && !home;
    format!(
        "{}(에디션 {}, 볼륨 {}, 정책 {})",
        if ok { "supported" } else { "unsupported" },
        if edition.is_empty() { "?" } else { edition.as_str() },
        if vol_ok { "지원" } else { "미지원" },
        if policy_off { "사용 안 함" } else { "허용" }
    )
}

fn reg_dword(root: HKEY, key: &str, value: &str) -> Option<u32> {
    let (k, v) = (HSTRING::from(key), HSTRING::from(value));
    let mut data = 0u32;
    let mut size = 4u32;
    let rc = unsafe {
        RegGetValueW(root, PCWSTR(k.as_ptr()), PCWSTR(v.as_ptr()), RRF_RT_REG_DWORD, None, Some((&mut data as *mut u32).cast()), Some(&mut size))
    };
    rc.is_ok().then_some(data)
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
