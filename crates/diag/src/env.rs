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
    #[cfg(windows)]
    crate::env_windows::fill(&mut e);
    e
}
