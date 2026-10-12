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

pub fn run(roots: &[PathBuf], tokenizer: Option<&Tokenizer>) -> ExtractInfo {
    let start = Instant::now();
    let mut files = Vec::new();
    for r in roots {
        walk(r, &mut files);
    }
    let exe = std::env::current_exe().expect("current exe");
    let mut x = ExtractInfo { total: files.len(), ..Default::default() };
    let mut magics: HashMap<String, usize> = HashMap::new();
    let (mut ratios, mut chunk_counts) = (Vec::new(), Vec::new());
    for (i, f) in files.iter().enumerate() {
        eprint!("\r문서 확인 중 {}/{}", i + 1, files.len());
        let mut cmd = Command::new(&exe);
        cmd.arg("extract-one").arg(f);
        match run_command(cmd, Duration::from_secs(30)) {
            Outcome::Ok { doc, distribution } => {
                x.ok += 1;
                x.distribution += distribution as usize;
                if let Some(tk) = tokenizer {
                    let text: String = doc.blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
                    let chars = text.chars().count();
                    if chars >= 200 {
                        if let Ok(t) = tk.count(&text) {
                            ratios.push(t as f64 * 1000.0 / chars as f64);
                        }
                    }
                    let count = |s: &str| tk.count(s).unwrap_or_else(|_| s.chars().count());
                    let n = build_chunks(&doc, &ChunkParams { target_tokens: 400, max_tokens: 500, max_chunks: 2000 }, &count).len();
                    chunk_counts.push(n as f64);
                }
            }
            Outcome::Failed { kind, magic } => match kind {
                FailKind::Encrypted => x.encrypted += 1,
                FailKind::Drm => x.drm += 1,
                FailKind::Corrupt => x.corrupt += 1,
                FailKind::Unsupported => x.unsupported += 1,
                FailKind::NotContainer => {
                    x.not_container += 1;
                    *magics.entry(extract::magic::report_label(&magic)).or_default() += 1;
                }
            },
            Outcome::Timeout => x.timeout += 1,
            Outcome::Crashed { .. } => x.crashed += 1,
        }
    }
    eprintln!();
    // 알 수 없는 이진 헤더는 5개 이상 같은 경우(형식 시그니처일 가능성)만 남기고 나머지는 합친다.
    let mut merged: HashMap<String, usize> = HashMap::new();
    for (label, n) in magics {
        let key = if label.starts_with("기타:") && n < 5 { "기타(5건 미만)".to_string() } else { label };
        *merged.entry(key).or_default() += n;
    }
    let mut top: Vec<(String, usize)> = merged.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    top.truncate(10);
    x.not_container_magic_top = top;
    x.tokens_per_1k_chars_median = percentile(&mut ratios, 0.5);
    x.chunks_per_doc_p50 = percentile(&mut chunk_counts, 0.5);
    x.chunks_per_doc_p90 = percentile(&mut chunk_counts, 0.9);
    x.elapsed_sec = start.elapsed().as_secs_f64();
    x
}
