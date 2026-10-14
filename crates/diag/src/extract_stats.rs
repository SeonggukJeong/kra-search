//! 실제 업무 문서(HWP·PDF·Office·ODF·텍스트)의 형식별 추출 통계. 경로·파일명·본문은 결과에 남기지 않는다.

use crate::report::ExtractInfo;
use embed::chunk::{build_chunks, ChunkParams};
use embed::engine::Tokenizer;
use extract::hwp::FailKind;
use extract::runner::{run_command, Outcome};
use extract::format::Format;
use std::collections::{BTreeMap, HashMap};
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

/// 형식마다 이만큼만 고르게 뽑아 추출한다 — D:\ 의 로그·데이터 .txt/.csv 수만 개 때문에 진단이 끝나지 않는 것을 막는다.
/// 형식별 전체 개수는 따로 센다(FormatCount.found).
pub const PER_FORMAT_CAP: usize = 3_000;

fn is_target(p: &Path) -> bool {
    Format::from_path(p).is_some() && !p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("~$"))
}

/// 클라우드 자리표시자(OneDrive "온라인 전용" 등)인가. 이런 파일을 열면 내려받기가 일어나 네트워크를 쓰고
/// PC 상태가 바뀐다(DEC-022) — 열지 않고 개수만 센다. 속성 읽기(symlink_metadata)는 내려받기를 일으키지 않는다.
#[cfg(windows)]
fn is_cloud_placeholder(md: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    // FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
    md.file_attributes() & (0x1000 | 0x4_0000 | 0x40_0000) != 0
}

#[cfg(not(windows))]
fn is_cloud_placeholder(_: &std::fs::Metadata) -> bool {
    false
}

/// 대상 파일을 모은다. 돌려주는 값 = 건너뛴 클라우드 자리표시자 수.
fn walk(root: &Path, out: &mut Vec<PathBuf>) -> usize {
    let mut cloud = 0;
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
            } else if is_target(&p) {
                if is_cloud_placeholder(&md) {
                    cloud += 1;
                } else {
                    out.push(p);
                }
            }
        }
    }
    cloud
}

/// 형식마다 최대 `cap` 개를 고르게 뽑는다(경로 순 정렬 후 i*n/cap 번째). 돌려주는 값 = (뽑은 파일, 형식별 전체 개수).
fn sample_per_format(files: Vec<PathBuf>, cap: usize) -> (Vec<PathBuf>, BTreeMap<&'static str, usize>) {
    let mut by: BTreeMap<&'static str, Vec<PathBuf>> = BTreeMap::new();
    for f in files {
        if let Some(fmt) = Format::from_path(&f) {
            by.entry(fmt.ext()).or_default().push(f);
        }
    }
    let mut found = BTreeMap::new();
    let mut out = Vec::new();
    for (ext, mut v) in by {
        found.insert(ext, v.len());
        v.sort();
        let n = v.len();
        if n <= cap {
            out.extend(v);
        } else {
            out.extend((0..cap).map(|i| v[i * n / cap].clone()));
        }
    }
    (out, found)
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
    by_format: BTreeMap<String, crate::report::FormatCount>,
    card_tokens: Vec<f64>,
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
        x.by_format = self.by_format.values().cloned().collect();
        if !self.card_tokens.is_empty() {
            x.card_tokens_mean = Some(self.card_tokens.iter().sum::<f64>() / self.card_tokens.len() as f64);
        }
        x.card_tokens_p50 = percentile(&mut self.card_tokens.clone(), 0.5);
        x.card_tokens_p90 = percentile(&mut self.card_tokens.clone(), 0.9);
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
        too_large: x.too_large,
        no_text: x.no_text,
        unreadable: x.unreadable,
        found: x.found,
        cloud_only: x.cloud_only,
        pdf_pages: x.pdf_pages,
        pdf_bad_pages: x.pdf_bad_pages,
        article_numbered_docs: x.article_numbered_docs,
        article_numbered_paras: x.article_numbered_paras,
        ..Default::default()
    }
}

/// 실제 실행: 파일마다 `kra-diag extract-one` 자식 프로세스(30초 제한). 200개마다 중간 결과를 알린다.
pub fn run(roots: &[PathBuf], tokenizer: Option<&Tokenizer>, on_progress: &mut dyn FnMut(&ExtractInfo)) -> ExtractInfo {
    let exe = std::env::current_exe().expect("current exe");
    let pdfium = exe.parent().map(|d| d.join(extract::pdf::LIB_NAME)).filter(|p| p.exists());
    let extract = |f: &Path, coverage: bool| {
        let mut cmd = Command::new(&exe);
        cmd.arg("extract-one").arg(f);
        if coverage {
            cmd.arg("--coverage");
        }
        if let Some(p) = &pdfium {
            cmd.arg("--pdfium").arg(p);
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
    let mut all = Vec::new();
    let mut cloud_only = 0;
    for r in roots {
        cloud_only += walk(r, &mut all);
    }
    let found_total = all.len();
    let (files, found) = sample_per_format(all, PER_FORMAT_CAP);
    let by_format = found
        .iter()
        .map(|(ext, n)| (ext.to_string(), crate::report::FormatCount { ext: ext.to_string(), found: *n, ..Default::default() }))
        .collect();
    let mut a = Acc {
        x: ExtractInfo { total: files.len(), found: found_total, cloud_only, ..Default::default() },
        magics: HashMap::new(),
        ratios: Vec::new(),
        chunk_counts: Vec::new(),
        coverages: Vec::new(),
        by_format,
        card_tokens: Vec::new(),
        start: Instant::now(),
    };
    let is_hwp = |f: &Path| matches!(Format::from_path(f), Some(Format::Hwp | Format::Hwpx));
    let stride = files.iter().filter(|f| is_hwp(f)).count().div_ceil(COVERAGE_SAMPLES).max(1);
    let mut hwp_i = 0;
    for (i, f) in files.iter().enumerate() {
        eprint!("\r문서 확인 중 {}/{}", i + 1, files.len());
        let mut measure = false;
        if is_hwp(f) {
            measure = hwp_i % stride == 0 && std::fs::metadata(f).is_ok_and(|m| m.len() <= COVERAGE_MAX_BYTES);
            hwp_i += 1;
        }
        let ext = Format::from_path(f).map(|x| x.ext()).unwrap_or("?").to_string();
        let fc = a.by_format.entry(ext.clone()).or_insert_with(|| crate::report::FormatCount { ext, ..Default::default() });
        fc.total += 1;
        match extract(f, measure) {
            Outcome::Ok { doc, distribution, coverage, stats } => {
                fc.ok += 1;
                a.x.pdf_pages += stats.pdf_pages as u64;
                a.x.pdf_bad_pages += stats.pdf_bad_pages as u64;
                if stats.article_numbered_paras > 0 {
                    a.x.article_numbered_docs += 1;
                    a.x.article_numbered_paras += stats.article_numbered_paras as u64;
                }
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
                    if let Some(card) = embed::chunk::build_card(&doc, 256, &count) {
                        let input = embed::profile::ModelProfile::embeddinggemma2().render_doc(doc.title.as_deref(), &card);
                        a.card_tokens.push(count(&input) as f64);
                    }
                }
            }
            Outcome::Failed { kind, magic } => {
                match &kind {
                    FailKind::Drm => fc.drm += 1,
                    FailKind::Encrypted => fc.encrypted += 1,
                    FailKind::NoText => fc.no_text += 1,
                    _ => fc.other_fail += 1,
                }
                match kind {
                FailKind::Encrypted => a.x.encrypted += 1,
                FailKind::Drm => a.x.drm += 1,
                FailKind::Corrupt => a.x.corrupt += 1,
                FailKind::Unsupported => a.x.unsupported += 1,
                FailKind::TooLarge => a.x.too_large += 1,
                FailKind::NoText => a.x.no_text += 1,
                FailKind::Unreadable => a.x.unreadable += 1,
                FailKind::NotContainer => {
                    a.x.not_container += 1;
                    *a.magics.entry(extract::magic::report_label(&magic)).or_default() += 1;
                }
                }
            }
            Outcome::Timeout => {
                fc.timeout += 1;
                a.x.timeout += 1;
            }
            Outcome::Crashed { .. } => {
                fc.timeout += 1;
                a.x.crashed += 1;
            }
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
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
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
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        for (i, c) in ["1.0", "0.99", "0.5", "0.95"].iter().enumerate() {
            std::fs::write(dir.join(format!("{i}.hwp")), c).unwrap();
        }
        let requested = std::cell::Cell::new(0);
        let fake = |p: &std::path::Path, measure: bool| {
            requested.set(requested.get() + measure as usize);
            let c: f32 = std::fs::read_to_string(p).unwrap().parse().unwrap();
            extract::runner::Outcome::Ok { doc: doc::Document { title: None, blocks: vec![] }, distribution: false, coverage: measure.then_some(c), stats: Default::default() }
        };
        let x = super::run_with(&[dir], None, &fake, 0, &mut |_| {});
        assert_eq!(requested.get(), 4, "적은 문서는 모두 측정한다");
        assert_eq!(x.coverage_samples, 4);
        assert_eq!(x.coverage_below_90, 1);
        assert!((x.coverage_median.unwrap() - 0.99).abs() < 1e-6, "{:?}", x.coverage_median);
        assert!((x.coverage_p10.unwrap() - 0.5).abs() < 1e-6, "{:?}", x.coverage_p10);
    }

    #[test]
    fn sampling_caps_each_format_and_keeps_totals() {
        use std::path::PathBuf;
        let files: Vec<PathBuf> = (0..10).map(|i| PathBuf::from(format!("{i:02}.txt"))).chain((0..3).map(|i| PathBuf::from(format!("{i}.hwp")))).collect();
        let (s, found) = super::sample_per_format(files, 4);
        assert_eq!((found["txt"], found["hwp"]), (10, 3));
        let txt: Vec<&str> = s.iter().filter_map(|p| p.to_str()).filter(|p| p.ends_with(".txt")).collect();
        assert_eq!(txt, ["00.txt", "02.txt", "05.txt", "07.txt"], "고르게(i*n/cap) 뽑는다");
        assert_eq!(s.iter().filter(|p| p.extension().is_some_and(|e| e == "hwp")).count(), 3, "상한 이하 형식은 전부");
    }

    #[test]
    fn coverage_stride_counts_only_hwp() {
        // HWP 2개 + TXT 600개: 간격을 전체 수(602)로 계산하면 HWP 하나만 재게 된다.
        let dir = tempfile::tempdir().unwrap();
        for i in 0..2 {
            std::fs::write(dir.path().join(format!("{i}.hwp")), "1.0").unwrap();
        }
        for i in 0..600 {
            std::fs::write(dir.path().join(format!("{i}.txt")), "x").unwrap();
        }
        let requested = std::cell::Cell::new(0);
        let fake = |_: &std::path::Path, measure: bool| {
            requested.set(requested.get() + measure as usize);
            extract::runner::Outcome::Ok { doc: doc::Document { title: None, blocks: vec![] }, distribution: false, coverage: measure.then_some(1.0), stats: Default::default() }
        };
        super::run_with(&[dir.path().to_path_buf()], None, &fake, 0, &mut |_| {});
        assert_eq!(requested.get(), 2, "HWP 는 모두 잰다");
    }
}
