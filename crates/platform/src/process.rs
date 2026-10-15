//! 프로세스 강화(스펙 §7.5·§7.6). 시작 직후 한 번 호출한다.
//! 레지스트리를 바꾸는 WerAddExcludedApplication 은 쓰지 않는다(PC 에 흔적 — 설치 프로그램이 결정할 일).

/// Windows: DLL 검색 경로를 System32·실행 파일 폴더로 제한하고, 비정상 종료 시 오류 창을 띄우지 않으며
/// (오류 모드는 자식 프로세스 = 워커에 상속), 오류 보고 덤프에 힙(문서 텍스트)을 넣지 않는다. 그 밖: 아무것도 안 함.
#[cfg(windows)]
pub fn harden() {
    use windows::Win32::System::Diagnostics::Debug::{SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX};
    use windows::Win32::System::ErrorReporting::{WerSetFlags, WER_FAULT_REPORTING, WER_FAULT_REPORTING_FLAG_NOHEAP, WER_FAULT_REPORTING_NO_UI};
    use windows::Win32::System::LibraryLoader::{SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32};
    unsafe {
        let _ = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_APPLICATION_DIR);
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
        let _ = WerSetFlags(WER_FAULT_REPORTING(WER_FAULT_REPORTING_NO_UI | WER_FAULT_REPORTING_FLAG_NOHEAP.0));
    }
}

#[cfg(not(windows))]
pub fn harden() {}

#[cfg(test)]
mod tests {
    #[test]
    fn harden_is_idempotent() {
        // 두 번 불러도 안전해야 한다(시험 하네스·워커 하위 명령에서 다시 불릴 수 있음).
        super::harden();
        super::harden();
    }
}
