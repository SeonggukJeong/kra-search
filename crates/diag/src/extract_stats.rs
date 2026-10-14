//! 실제 업무 문서(HWP/HWPX)의 추출 통계. 경로·파일명·본문은 결과에 남기지 않는다.

use crate::report::ExtractInfo;
use embed::chunk::{build_chunks, ChunkParams};
use embed::engine::Tokenizer;
use extract::hwp::FailKind;
use extract::runner::{run_command, Outcome};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const SKIP_DIRS: &[&str] = &["$recycle.bin", "system volume information", "appdata", "windows", "program files", "program files (x86)", ".git", "node_modules"];

pub fn default_roots() -> Vec<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    let mut v: Vec<PathBuf> = home
        .map(|h| ["Documents", "Desktop", "Downloads"].iter().map(|d| h.join(d)).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    if cfg!(windows) && Path::new("D:\\").is_dir() {
        v.push(PathBuf::from("D:\\"));
    }
    v
}

fn walk(root: &Path, out: &mut Vec<PathBuf>) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(md) = std::fs::symlink_metadata(e.path()) else { continue };
            if md.file_type().is_symlink() {
                continue;
            }
            let p = e.path();
            if md.is_dir() {
                let name = e.file_name().to_string_lossy().to_lowercase();
                if !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(p);
                }
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("hwp") || x.eq_ignore_ascii_case("hwpx")) {
                out.push(p);
            }
        }
    }
}

fn percentile(v: &mut [f64], p: f64) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[((v.len() - 1) as f64 * p).round() as usize])
}

struct Acc {
    x: ExtractInfo,
    magics: HashMap<String, usize>,
    ratios: Vec<f64>,
    chunk_counts: Vec<f64>,
    coverages: Vec<f64>,
    start: Instant,
}

/// 커버리지는 조판이 필요해 느리므로 최대 이만큼만 고르게 표본으로 잰다.
const COVERAGE_SAMPLES: usize = 500;
/// 이보다 큰 파일은 커버리지를 재지 않는다(조판 시간이 30초 제한에 걸리지 않도록).
const COVERAGE_MAX_BYTES: u64 = 5 * 1024 * 1024;

impl Acc {
    /// 누적값으로 보고서용 통계를 만든다(중간 저장과 최종 결과에 공통).
    fn snapshot(&self) -> ExtractInfo {
        let mut x = ExtractInfo { not_container_magic_top: vec![], ..clone_counts(&self.x) };
        // 알 수 없는 이진 헤더는 5개 이상 같은 경우(형식 시그니처일 가능성)만 남기고 나머지는 합친다.
        let mut merged: HashMap<String, usize> = HashMap::new();
        for (label, n) in &self.magics {
            let key = if label.starts_with("기타:") && *n < 5 { "기타(5건 미만)".to_string() } else { label.clone() };
            *merged.entry(key).or_default() += n;
        }
        let mut top: Vec<(String, usize)> = merged.into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        top.truncate(10);
        x.not_container_magic_top = top;
        x.tokens_per_1k_chars_median = percentile(&mut self.ratios.clone(), 0.5);
        x.chunks_per_doc_p50 = percentile(&mut self.chunk_counts.clone(), 0.5);
        x.chunks_per_doc_p90 = percentile(&mut self.chunk_counts.clone(), 0.9);
        x.coverage_samples = self.coverages.len();
        x.coverage_median = percentile(&mut self.coverages.clone(), 0.5);
        x.coverage_p10 = percentile(&mut self.coverages.clone(), 0.1);
        x.coverage_below_90 = self.coverages.iter().filter(|c| **c < 0.9).count();
        x.elapsed_sec = self.start.elapsed().as_secs_f64();
        x
    }
}

fn clone_counts(x: &ExtractInfo) -> ExtractInfo {
    ExtractInfo {
        total: x.total,
        processed: x.processed,
        ok: x.ok,
        distribution: x.distribution,
        encrypted: x.encrypted,
        drm: x.drm,
        corrupt: x.corrupt,
        unsupported: x.unsupported,
        not_container: x.not_container,
        timeout: x.timeout,
        crashed: x.crashed,
        ..Default::default()
    }
}

/// 실제 실행: 파일마다 `kra-diag extract-one` 자식 프로세스(30초 제한). 200개마다 중간 결과를 알린다.
pub fn run(roots: &[PathBuf], tokenizer: Option<&Tokenizer>, on_progress: &mut dyn FnMut(&ExtractInfo)) -> ExtractInfo {
    let exe = std::env::current_exe().expect("current exe");
    let extract = |f: &Path, coverage: bool| {
        let mut cmd = Command::new(&exe);
        cmd.arg("extract-one").arg(f);
        if coverage {
            cmd.arg("--coverage");
        }
        run_command(cmd, Duration::from_secs(30))
    };
    run_with(roots, tokenizer, &extract, 200, on_progress)
}

pub fn run_with(
    roots: &[PathBuf],
    tokenizer: Option<&Tokenizer>,
    extract: &dyn Fn(&Path, bool) -> Outcome,
    every: usize,
    on_progress: &mut dyn FnMut(&ExtractInfo),
) -> ExtractInfo {
    let mut files = Vec::new();
    for r in roots {
        walk(r, &mut files);
    }
    let mut a = Acc {
        x: ExtractInfo { total: files.len(), ..Default::default() },
        magics: HashMap::new(),
        ratios: Vec::new(),
        chunk_counts: Vec::new(),
        coverages: Vec::new(),
        start: Instant::now(),
    };
    let stride = files.len().div_ceil(COVERAGE_SAMPLES).max(1);
    for (i, f) in files.iter().enumerate() {
        eprint!("\r문서 확인 중 {}/{}", i + 1, files.len());
        let measure = i % stride == 0 && std::fs::metadata(f).is_ok_and(|m| m.len() <= COVERAGE_MAX_BYTES);
        match extract(f, measure) {
            Outcome::Ok { doc, distribution, coverage } => {
                a.x.ok += 1;
                if let Some(c) = coverage {
                    a.coverages.push(c as f64);
                }
                a.x.distribution += distribution as usize;
                if let Some(tk) = tokenizer {
                    let text: String = doc.blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
                    let chars = text.chars().count();
                    if chars >= 200 {
                        if let Ok(t) = tk.count(&text) {
                            a.ratios.push(t as f64 * 1000.0 / chars as f64);
                        }
                    }
                    let count = |s: &str| tk.count(s).unwrap_or_else(|_| s.chars().count());
                    let n = build_chunks(&doc, &ChunkParams { target_tokens: 400, max_tokens: 500, max_chunks: 2000 }, &count).len();
                    a.chunk_counts.push(n as f64);
                }
            }
            Outcome::Failed { kind, magic } => match kind {
                FailKind::Encrypted => a.x.encrypted += 1,
                FailKind::Drm => a.x.drm += 1,
                FailKind::Corrupt => a.x.corrupt += 1,
                FailKind::Unsupported => a.x.unsupported += 1,
                FailKind::NotContainer => {
                    a.x.not_container += 1;
                    *a.magics.entry(extract::magic::report_label(&magic)).or_default() += 1;
                }
            },
            Outcome::Timeout => a.x.timeout += 1,
            Outcome::Crashed { .. } => a.x.crashed += 1,
        }
        a.x.processed = i + 1;
        if every > 0 && (i + 1) % every == 0 {
            on_progress(&a.snapshot());
        }
    }
    eprintln!();
    a.snapshot()
}

#[cfg(test)]
mod tests {
    #[test]
    fn progress_callback_reports_partial_results() {
        let dir = std::env::temp_dir().join("kra-diag-progress");
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..3 {
            std::fs::write(dir.join(format!("{i}.hwp")), b"NOT-A-HWP").unwrap();
        }
        let mut seen = Vec::new();
        let fake = |_: &std::path::Path, _: bool| extract::runner::Outcome::Failed { kind: extract::hwp::FailKind::NotContainer, magic: "4e4f54".into() };
        let x = super::run_with(&[dir], None, &fake, 1, &mut |p| seen.push(p.processed));
        assert_eq!(x.processed, 3);
        assert_eq!(seen, vec![1, 2, 3]);
    }

    #[test]
    fn coverage_is_requested_and_summarized() {
        let dir = std::env::temp_dir().join("kra-diag-coverage");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (i, c) in ["1.0", "0.99", "0.5", "0.95"].iter().enumerate() {
            std::fs::write(dir.join(format!("{i}.hwp")), c).unwrap();
        }
        let requested = std::cell::Cell::new(0);
        let fake = |p: &std::path::Path, measure: bool| {
            requested.set(requested.get() + measure as usize);
            let c: f32 = std::fs::read_to_string(p).unwrap().parse().unwrap();
            extract::runner::Outcome::Ok { doc: doc::Document { title: None, blocks: vec![] }, distribution: false, coverage: measure.then_some(c) }
        };
        let x = super::run_with(&[dir], None, &fake, 0, &mut |_| {});
        assert_eq!(requested.get(), 4, "적은 문서는 모두 측정한다");
        assert_eq!(x.coverage_samples, 4);
        assert_eq!(x.coverage_below_90, 1);
        assert!((x.coverage_median.unwrap() - 0.99).abs() < 1e-6, "{:?}", x.coverage_median);
        assert!((x.coverage_p10.unwrap() - 0.5).abs() < 1e-6, "{:?}", x.coverage_p10);
    }
}
