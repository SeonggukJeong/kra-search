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

/// 형식(확장자) 하나의 집계. 확장자는 `Format::ext()` 고정 라벨이다(파일명 아님 — INC-010).
#[derive(Serialize, serde::Deserialize, Default, Clone)]
pub struct FormatCount {
    pub ext: String,
    /// 표본을 뽑기 전 전체 개수
    pub found: usize,
    /// 실제로 추출한(표본) 개수
    pub total: usize,
    pub ok: usize,
    pub no_text: usize,
    pub drm: usize,
    pub encrypted: usize,
    pub other_fail: usize,
    pub timeout: usize,
}

#[derive(Serialize, serde::Deserialize, Default)]
pub struct ExtractInfo {
    /// 추출한(표본) 파일 수
    pub total: usize,
    /// 지금까지 확인한 파일 수(중간 저장이면 total 보다 작다)
    pub processed: usize,
    pub ok: usize,
    pub distribution: usize,
    pub encrypted: usize,
    pub drm: usize,
    pub corrupt: usize,
    pub unsupported: usize,
    pub not_container: usize,
    pub timeout: usize,
    pub crashed: usize,
    #[serde(default)]
    pub too_large: usize,
    #[serde(default)]
    pub no_text: usize,
    #[serde(default)]
    pub unreadable: usize,
    pub not_container_magic_top: Vec<(String, usize)>,
    pub tokens_per_1k_chars_median: Option<f64>,
    pub chunks_per_doc_p50: Option<f64>,
    pub chunks_per_doc_p90: Option<f64>,
    /// 추출 커버리지(조판 텍스트 대비 글자 비율)를 잰 문서 수 — 최대 약 500개 표본
    pub coverage_samples: usize,
    pub coverage_median: Option<f64>,
    pub coverage_p10: Option<f64>,
    /// 커버리지 0.9 미만(본문이 눈에 띄게 빠진) 문서 수
    pub coverage_below_90: usize,
    pub elapsed_sec: f64,
    /// 표본을 뽑기 전 전체 대상 수(클라우드 전용 제외)
    #[serde(default)]
    pub found: usize,
    /// 열지 않고 건너뛴 클라우드 자리표시자 수(OneDrive 온라인 전용 등)
    #[serde(default)]
    pub cloud_only: usize,
    #[serde(default)]
    pub by_format: Vec<FormatCount>,
    #[serde(default)]
    pub pdf_pages: u64,
    #[serde(default)]
    pub pdf_bad_pages: u64,
    #[serde(default)]
    pub article_numbered_docs: usize,
    #[serde(default)]
    pub article_numbered_paras: u64,
    #[serde(default)]
    pub card_tokens_mean: Option<f64>,
    #[serde(default)]
    pub card_tokens_p50: Option<f64>,
    #[serde(default)]
    pub card_tokens_p90: Option<f64>,
}

impl Report {
    pub fn new() -> Self {
        let now = time::OffsetDateTime::now_utc();
        Self {
            schema: 2,
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
            let speed = if m.tokens_per_sec.is_empty() {
                "초당 토큰 -(측정 실패)".to_string()
            } else {
                let (th, best) = m.tokens_per_sec.iter().cloned().fold((0, 0.0), |a, x| if x.1 > a.1 { x } else { a });
                format!("초당 {}토큰({}스레드)", thousands(best), th)
            };
            let card = if m.card_minutes_per_10k > 0.0 { format!("약 {:.0}분", m.card_minutes_per_10k) } else { "-".into() };
            let per_thread: Vec<String> = m.tokens_per_sec.iter().map(|(t, v)| format!("{t}→{v:.0}")).collect();
            s.push(format!(
                "모델 {}: {} → 빠른 의미 검색 문서 1만 개 {} · 검색어 p95 {:.0}ms · 첫 검색 {:.0}ms · 로드 {}ms · 메모리 로드 후 {}MB / 최대 {}MB · 스레드별 {}",
                m.model_file, speed, card, m.query_ms_p95, m.first_query_ms, m.load_ms, m.rss_mb_after_load, m.rss_mb_peak,
                if per_thread.is_empty() { "-".into() } else { per_thread.join(" · ") }
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
            "문서 {}개{}{}: 성공 {} · 배포용 {} · 암호 {} · 보안문서(DRM) {} · 손상 {} · 미지원 {} · 형식 아님 {}(상위 헤더: {}) · 시간초과 {} · 비정상종료 {} · 1,000자당 토큰 {} · 문서당 청크 p50 {} / p90 {} · 추출 커버리지 {} · {:.0}초",
            thousands(x.total as f64),
            if x.found > x.total { format!("(전체 {}개 중 형식마다 최대 3,000개)", thousands(x.found as f64)) } else { String::new() },
            if x.processed < x.total { format!("(중간 저장 — {}개 확인)", thousands(x.processed as f64)) } else { String::new() }, pct(x.ok, x.processed.max(1)), x.distribution, x.encrypted, x.drm, x.corrupt, x.unsupported,
            x.not_container, if top.is_empty() { "-".into() } else { top.join(" ") }, x.timeout, x.crashed,
            x.tokens_per_1k_chars_median.map(|v| format!("{v:.0}")).unwrap_or("-".into()),
            x.chunks_per_doc_p50.map(|v| format!("{v:.0}")).unwrap_or("-".into()),
            x.chunks_per_doc_p90.map(|v| format!("{v:.0}")).unwrap_or("-".into()),
            match (x.coverage_median, x.coverage_p10) {
                (Some(m), Some(p)) => format!("중앙값 {:.1}% · 하위10% {:.1}% · 90% 미만 {}개/{}개", m * 100.0, p * 100.0, x.coverage_below_90, x.coverage_samples),
                _ => "-".into(),
            },
            x.elapsed_sec
        ));
        s.push(format!("읽기 오류 {} · 100MB 초과 {} · 클라우드 전용(열지 않음) {}", x.unreadable, x.too_large, x.cloud_only));
        if !x.by_format.is_empty() {
            let parts: Vec<String> = x
                .by_format
                .iter()
                .map(|f| {
                    let mut p = format!("{} {}/{}", f.ext, thousands(f.ok as f64), thousands(f.total as f64));
                    if f.no_text > 0 {
                        p.push_str(&format!("(글자 없음 {})", f.no_text));
                    }
                    if f.found > f.total {
                        p.push_str(&format!("(전체 {}개 중 표본)", thousands(f.found as f64)));
                    }
                    p
                })
                .collect();
            s.push(format!("형식별 읽기 성공: {}", parts.join(" · ")));
        }
        if x.pdf_pages > 0 {
            s.push(format!("PDF 쪽: 전체 {} · 비었거나 깨져 버린 쪽 {}", thousands(x.pdf_pages as f64), thousands(x.pdf_bad_pages as f64)));
        }
        s.push(format!("\"제N조\" 자동 번호: 문서 {}개 · 문단 {}개", x.article_numbered_docs, x.article_numbered_paras));
        if let (Some(m), Some(p50), Some(p90)) = (x.card_tokens_mean, x.card_tokens_p50, x.card_tokens_p90) {
            s.push(format!("문서 카드 길이(토큰): 평균 {m:.0} · 중앙값 {p50:.0} · 90% {p90:.0}"));
        }
    }
    s
}

/// 보고서를 쓸 폴더: 실행 폴더에 쓸 수 있으면 거기, 아니면 exe 폴더, 그래도 안 되면 임시 폴더.
pub fn output_dir(cwd: &Path, exe_dir: &Path) -> std::path::PathBuf {
    for d in [cwd, exe_dir, &std::env::temp_dir()] {
        let probe = d.join(".kra-diag-write-test");
        if std::fs::write(&probe, b"").is_ok() {
            let _ = std::fs::remove_file(probe);
            return d.to_path_buf();
        }
    }
    std::env::temp_dir()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// `dir`에 kra-diag-report.json, kra-diag-summary.txt 를 쓴다(임시 파일 후 교체 — 중간에 멈춰도 직전 결과가 남는다).
/// 요약 파일은 오래된 메모장에서도 한글이 보이도록 UTF-8 BOM을 붙인다. `print` 이면 요약을 화면에도 출력한다.
pub fn write(r: &mut Report, dir: &Path, print: bool) -> std::io::Result<()> {
    r.summary = summarize(r);
    write_atomic(&dir.join("kra-diag-report.json"), serde_json::to_string_pretty(r)?.as_bytes())?;
    let mut txt = vec![0xEF, 0xBB, 0xBF];
    txt.extend_from_slice((r.summary.join("\r\n") + "\r\n").as_bytes());
    write_atomic(&dir.join("kra-diag-summary.txt"), &txt)?;
    if print {
        for l in &r.summary {
            println!("{l}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_speed_shows_dash_not_zero() {
        let r = Report {
            bench: Some(BenchInfo { models: vec![ModelBench { model_file: "m.gguf".into(), ..Default::default() }], ..Default::default() }),
            ..Default::default()
        };
        let s = summarize(&r).join("\n");
        assert!(s.contains("초당 토큰 -(측정 실패)") && !s.contains("0스레드") && !s.contains("약 0분"), "{s}");
    }

    #[test]
    fn summary_shows_unreadable_too_large_and_cloud() {
        let x = ExtractInfo { total: 5, processed: 5, found: 5, unreadable: 1, too_large: 2, cloud_only: 3, ..Default::default() };
        let r = Report { extract: Some(x), ..Default::default() };
        let s = summarize(&r).join("\n");
        assert!(s.contains("읽기 오류 1 · 100MB 초과 2 · 클라우드 전용(열지 않음) 3"), "{s}");
    }
}
