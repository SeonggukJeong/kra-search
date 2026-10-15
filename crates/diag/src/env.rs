//! PC 환경 수집(사용자명·컴퓨터 이름은 수집하지 않는다).

use crate::report::EnvInfo;
use sysinfo::System;

fn cpu_features() -> Vec<String> {
    #[cfg(target_arch = "x86_64")]
    {
        let mut f = Vec::new();
        macro_rules! chk {
            ($($name:tt),*) => { $( if std::is_x86_feature_detected!($name) { f.push($name.to_string()); } )* };
        }
        chk!("sse4.2", "popcnt", "avx", "avx2", "fma", "avx512f");
        f
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        vec![std::env::consts::ARCH.to_string()]
    }
}

pub fn collect() -> EnvInfo {
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut e = EnvInfo {
        os: System::name().unwrap_or_default(),
        os_version: System::long_os_version().unwrap_or_default(),
        cpu_brand: sys.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default(),
        physical_cores: System::physical_core_count().unwrap_or(0),
        logical_cores: sys.cpus().len(),
        ram_total_mb: sys.total_memory() / 1024 / 1024,
        cpu_features: cpu_features(),
        llama_system_info: String::new(),
        efs: "n/a".into(),
        ..Default::default()
    };
    if let Some((perf, eff)) = core_split() {
        e.perf_cores = Some(perf);
        e.efficiency_cores = Some(eff);
    }
    #[cfg(windows)]
    crate::env_windows::fill(&mut e);
    e
}

/// (성능 코어 수, 효율 코어 수). 하이브리드 CPU 가 아니면 효율 코어 0. Windows 에서만 잰다.
#[cfg(windows)]
pub fn core_split() -> Option<(usize, usize)> {
    crate::env_windows::core_split()
}

#[cfg(not(windows))]
pub fn core_split() -> Option<(usize, usize)> {
    None
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn core_split_counts_physical_cores() {
        let (perf, eff) = super::core_split().expect("GetLogicalProcessorInformationEx");
        assert!(perf >= 1, "{perf}");
        assert_eq!(perf + eff, sysinfo::System::physical_core_count().unwrap_or(perf + eff));
    }
}
