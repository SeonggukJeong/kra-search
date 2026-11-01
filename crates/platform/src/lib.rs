//! Windows 전용 기능(DPAPI·데이터 위치·프로세스 강화). macOS 는 개발용 스텁(스펙 §5.2 platform).
pub mod cpu;
pub mod keys;
pub mod paths;
pub mod process;
