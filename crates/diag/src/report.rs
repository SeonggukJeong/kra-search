//! 진단 보고서. 경로·파일명·사용자명·컴퓨터 이름 필드는 의도적으로 존재하지 않는다.

use serde::Serialize;
use std::path::Path;

#[derive(Serialize, Default)]
pub struct Report {
    pub schema: u32,
    pub tool_version: String,
    pub generated_utc: String,
    pub env: Option<EnvInfo>,
    pub bench: Option<BenchInfo>,
    pub extract: Option<ExtractInfo>,
    pub summary: Vec<String>,
}

#[derive(Serialize, Default)]
pub struct EnvInfo {
    pub os: String,
    pub os_version: String,
    pub cpu_brand: String,
    pub physical_cores: usize,
    pub logical_cores: usize,
    pub ram_total_mb: u64,
    pub cpu_features: Vec<String>,
    pub llama_system_info: String,
    pub efs: String,
    pub webview2: Option<String>,
    pub hotkeys: Vec<HotkeyCheck>,
    pub antivirus: Vec<String>,
}

#[derive(Serialize)]
pub struct HotkeyCheck {
    pub combo: String,
    pub registered: bool,
}

#[derive(Serialize, Default)]
pub struct BenchInfo {
    pub models: Vec<ModelBench>,
    pub flat: FlatBench,
}

#[derive(Serialize, serde::Deserialize, Default)]
pub struct ModelBench {
    pub model_file: String,
    pub error: Option<String>,
    pub load_ms: u64,
    pub bench_tokens: usize,
    pub tokens_per_sec: Vec<(usize, f64)>,
    pub card_minutes_per_10k: f64,
    pub query_ms_p50: f64,
    pub query_ms_p95: f64,
    pub first_query_ms: f64,
    pub rss_mb_after_load: u64,
    pub rss_mb_peak: u64,
}

#[derive(Serialize, Default)]
pub struct FlatBench {
    pub n: usize,
    pub exact_ms: f64,
    pub hamming256_ms: f64,
    pub hamming768_ms: f64,
    pub asym256_ms: f64,
    /// 모든 측정에서 나온 1위 결과 id 의 합(검색이 실제로 수행됐다는 증거)
    pub result_checksum: u64,
}

#[derive(Serialize, Default)]
pub struct ExtractInfo {
    pub total: usize,
    pub ok: usize,
    pub distribution: usize,
    pub encrypted: usize,
    pub drm: usize,
    pub corrupt: usize,
    pub unsupported: usize,
    pub not_container: usize,
    pub timeout: usize,
    pub crashed: usize,
    pub not_container_magic_top: Vec<(String, usize)>,
    pub tokens_per_1k_chars_median: Option<f64>,
    pub chunks_per_doc_p50: Option<f64>,
    pub chunks_per_doc_p90: Option<f64>,
    pub elapsed_sec: f64,
}

impl Report {
    pub fn new() -> Self {
        let now = time::OffsetDateTime::now_utc();
        Self {
            schema: 1,
            tool_version: env!("CARGO_PKG_VERSION").into(),
            generated_utc: now.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
            ..Default::default()
        }
    }
}

fn pct(n: usize, d: usize) -> String {
    if d == 0 { "-".into() } else { format!("{:.1}%", n as f64 * 100.0 / d as f64) }
}

fn thousands(n: f64) -> String {
    let s = format!("{:.0}", n);
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

/// 사람이 읽을 요약 줄(한국어).
pub fn summarize(r: &Report) -> Vec<String> {
    let mut s = vec![format!("[kra-diag {}] {}", r.tool_version, r.generated_utc)];
    if let Some(e) = &r.env {
        s.push(format!(
            "CPU: {} · {}코어/{}스레드 · {} · RAM {}MB · {} {}",
            e.cpu_brand,
            e.physical_cores,
            e.logical_cores,
            e.cpu_features.join(","),
            thousands(e.ram_total_mb as f64),
            e.os,
            e.os_version
        ));
        let hk: Vec<String> = e
            .hotkeys
            .iter()
            .map(|h| format!("{} {}", h.combo, if h.registered { "등록 가능" } else { "등록 실패" }))
            .collect();
        s.push(format!(
            "암호화(EFS): {} · WebView2 {} · 단축키 {} · 백신: {}",
            e.efs,
            e.webview2.as_deref().unwrap_or("없음"),
            if hk.is_empty() { "-".into() } else { hk.join(" · ") },
            if e.antivirus.is_empty() { "-".into() } else { e.antivirus.join(", ") }
        ));
    }
    if let Some(b) = &r.bench {
        for m in &b.models {
            if let Some(err) = &m.error {
                s.push(format!("모델 {}: 오류 — {}", m.model_file, err));
                continue;
            }
            let (th, best) = m.tokens_per_sec.iter().cloned().fold((0, 0.0), |a, x| if x.1 > a.1 { x } else { a });
            s.push(format!(
                "모델 {}: 초당 {}토큰({}스레드) → 빠른 의미 검색 문서 1만 개 약 {:.0}분 · 검색어 p95 {:.0}ms · 첫 검색 {:.0}ms · 로드 {}ms · 메모리 로드 후 {}MB / 최대 {}MB",
                m.model_file, thousands(best), th, m.card_minutes_per_10k, m.query_ms_p95, m.first_query_ms, m.load_ms, m.rss_mb_after_load, m.rss_mb_peak
            ));
        }
        if b.flat.n > 0 {
            s.push(format!(
                "벡터 {}개 검색: 정밀 {:.0}ms · 해밍256 {:.0}ms · 해밍768 {:.0}ms · 비대칭256 {:.0}ms",
                thousands(b.flat.n as f64), b.flat.exact_ms, b.flat.hamming256_ms, b.flat.hamming768_ms, b.flat.asym256_ms
            ));
        }
    }
    if let Some(x) = &r.extract {
        let top: Vec<String> = x.not_container_magic_top.iter().take(3).map(|(m, c)| format!("{m}×{c}")).collect();
        s.push(format!(
            "HWP {}개: 성공 {} · 배포용 {} · 암호 {} · 보안문서(DRM) {} · 손상 {} · 미지원 {} · HWP아님 {}(상위 헤더: {}) · 시간초과 {} · 비정상종료 {} · 1,000자당 토큰 {} · 문서당 청크 p50 {} / p90 {} · {:.0}초",
            thousands(x.total as f64), pct(x.ok, x.total), x.distribution, x.encrypted, x.drm, x.corrupt, x.unsupported,
            x.not_container, if top.is_empty() { "-".into() } else { top.join(" ") }, x.timeout, x.crashed,
            x.tokens_per_1k_chars_median.map(|v| format!("{v:.0}")).unwrap_or("-".into()),
            x.chunks_per_doc_p50.map(|v| format!("{v:.0}")).unwrap_or("-".into()),
            x.chunks_per_doc_p90.map(|v| format!("{v:.0}")).unwrap_or("-".into()),
            x.elapsed_sec
        ));
    }
    s
}

/// `dir`에 kra-diag-report.json, kra-diag-summary.txt 를 쓴다.
pub fn write(r: &mut Report, dir: &Path) -> std::io::Result<()> {
    r.summary = summarize(r);
    std::fs::write(dir.join("kra-diag-report.json"), serde_json::to_string_pretty(r)?)?;
    std::fs::write(dir.join("kra-diag-summary.txt"), r.summary.join("\n") + "\n")?;
    for l in &r.summary {
        println!("{l}");
    }
    Ok(())
}
