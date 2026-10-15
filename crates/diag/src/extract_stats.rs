//! 실제 업무 문서(HWP·PDF·Office·ODF·텍스트)의 형식별 추출 통계. 경로·파일명·본문은 결과에 남기지 않는다.

use crate::report::ExtractInfo;
use embed::chunk::{build_chunks, BODY_CHUNK};
use embed::engine::Tokenizer;
use extract::hwp::FailKind;
use extract::runner::{run_command, Outcome};
use extract::format::Format;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

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

/// 토큰 비율·청크·카드 통계는 문서 본문이 이 글자 수 이하일 때만 부모 프로세스에서 계산한다.
/// 줄바꿈 없는 거대한 로그 .txt 는 한 문단이 되어 `cut_to` 가 O(n²) 로 느려진다(KI-001) — 부모에는 시간 제한이 없다.
pub const STATS_MAX_CHARS: usize = 200_000;

fn within_stats_cap(doc: &doc::Document) -> bool {
    let mut n = 0usize;
    for b in &doc.blocks {
        n += b.text.chars().count() + 1;
        if n > STATS_MAX_CHARS {
            return false;
        }
    }
    true
}

/// 부모가 실행 파일 옆 pdfium 을 불러올 수 있었는지(보고서 pdfium_loaded).
pub static PDFIUM_LOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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

/// 대상 파일을 모은다(수정 시각도 함께). 돌려주는 값 = 건너뛴 클라우드 자리표시자 수.
fn walk(root: &Path, out: &mut Vec<PathBuf>, mtimes: &mut Vec<SystemTime>) -> usize {
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
                // 일부만 내려받은 클라우드 폴더는 들어가지 않는다(열거도 내려받기를 일으킬 수 있다 — DEC-022).
                if is_cloud_placeholder(&md) {
                    continue;
                }
                let name = e.file_name().to_string_lossy().to_lowercase();
                if !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(p);
                }
            } else if is_target(&p) {
                if is_cloud_placeholder(&md) {
                    cloud += 1;
                } else {
                    if let Ok(m) = md.modified() {
                        mtimes.push(m);
                    }
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
    // 중간에 멈춰도 모든 형식이 조금씩은 들어 있도록 형식별 진행률(j/m) 순으로 섞는다.
    let mut keyed: Vec<(u64, &'static str, PathBuf)> = Vec::new();
    for (ext, mut v) in by {
        found.insert(ext, v.len());
        v.sort();
        let n = v.len();
        let picked: Vec<PathBuf> = if n <= cap { v } else { (0..cap).map(|i| v[i * n / cap].clone()).collect() };
        let m = picked.len() as u64;
        for (j, p) in picked.into_iter().enumerate() {
            keyed.push((j as u64 * 1_000_000 / m, ext, p));
        }
    }
    keyed.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)).then_with(|| a.2.cmp(&b.2)));
    let out: Vec<PathBuf> = keyed.into_iter().map(|k| k.2).collect();
    (out, found)
}

fn percentile(v: &mut [f64], p: f64) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[((v.len() - 1) as f64 * p).round() as usize])
}

/// 수정일 경과 기간 구간(이 일수 이하). 마지막 구간은 그보다 오래된 것.
const AGE_BUCKETS: &[(u64, &str)] = &[(30, "30일 이내"), (365, "1년 이내"), (730, "1~2년"), (1095, "2~3년"), (1826, "3~5년")];

/// 수정일 분포: (구간별 개수, 가장 많이 몰린 하루의 비율). 하루 앞(시계 차이)보다 미래면 "미래"로 센다.
/// 날짜 값 자체는 돌려주지 않는다 — 비율만 남긴다.
fn mtime_stats(mtimes: &[SystemTime], now: SystemTime) -> (Vec<(String, usize)>, Option<f64>) {
    if mtimes.is_empty() {
        return (vec![], None);
    }
    let mut counts = vec![0usize; AGE_BUCKETS.len() + 2];
    let mut days: HashMap<u64, usize> = HashMap::new();
    for m in mtimes {
        if let Ok(d) = m.duration_since(SystemTime::UNIX_EPOCH) {
            *days.entry(d.as_secs() / 86_400).or_default() += 1;
        }
        let i = match now.duration_since(*m) {
            Ok(age) => {
                let d = age.as_secs() / 86_400;
                AGE_BUCKETS.iter().position(|(lim, _)| d <= *lim).unwrap_or(AGE_BUCKETS.len())
            }
            Err(e) if e.duration().as_secs() <= 86_400 => 0,
            Err(_) => AGE_BUCKETS.len() + 1,
        };
        counts[i] += 1;
    }
    let labels = AGE_BUCKETS.iter().map(|(_, l)| *l).chain(["5년 넘음", "미래"]);
    let buckets = labels.zip(counts).map(|(l, n)| (l.to_string(), n)).collect();
    let top = days.values().copied().max().unwrap_or(0);
    (buckets, Some(top as f64 / mtimes.len() as f64))
}

/// 찾은 전체 파일로 늘려 잡은 (청크 수, 청크 입력 토큰). 형식마다 추출한(표본) 문서의 합 × 찾은 수 / 추출한 수.
/// 실패한 문서는 청크 0 으로 표본에 들어 있으므로 실패 비율도 그대로 반영된다. 통계를 낸 문서가 없으면 None.
fn estimate_all(by_format: &[crate::report::FormatCount]) -> Option<(f64, f64)> {
    let mut any = false;
    let (mut chunks, mut tokens) = (0.0, 0.0);
    for f in by_format.iter().filter(|f| f.total > 0) {
        any |= f.stats_docs > 0;
        let scale = f.found.max(f.total) as f64 / f.total as f64;
        chunks += f.chunks as f64 * scale;
        tokens += f.chunk_tokens as f64 * scale;
    }
    any.then_some((chunks, tokens))
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
        if !self.chunk_counts.is_empty() {
            x.chunks_per_doc_mean = Some(self.chunk_counts.iter().sum::<f64>() / self.chunk_counts.len() as f64);
        }
        x.coverage_samples = self.coverages.len();
        x.coverage_median = percentile(&mut self.coverages.clone(), 0.5);
        x.coverage_p10 = percentile(&mut self.coverages.clone(), 0.1);
        x.coverage_below_90 = self.coverages.iter().filter(|c| **c < 0.9).count();
        x.pdfium_loaded = PDFIUM_LOADED.load(std::sync::atomic::Ordering::Relaxed);
        x.by_format = self.by_format.values().cloned().collect();
        if let Some((c, tk)) = estimate_all(&x.by_format) {
            x.est_chunks_all = Some(c);
            x.est_chunk_tokens_all = Some(tk);
        }
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
        too_complex: x.too_complex,
        truncated: x.truncated,
        no_text: x.no_text,
        unreadable: x.unreadable,
        found: x.found,
        cloud_only: x.cloud_only,
        stats_skipped_huge: x.stats_skipped_huge,
        pdf_pages: x.pdf_pages,
        pdf_bad_pages: x.pdf_bad_pages,
        article_numbered_docs: x.article_numbered_docs,
        article_numbered_paras: x.article_numbered_paras,
        mtime_age: x.mtime_age.clone(),
        mtime_top_day_share: x.mtime_top_day_share,
        ..Default::default()
    }
}

/// 실제 실행: 파일마다 `kra-diag extract-one` 자식 프로세스(30초 제한). 200개마다 중간 결과를 알린다.
pub fn run(roots: &[PathBuf], tokenizer: Option<&Tokenizer>, on_progress: &mut dyn FnMut(&ExtractInfo)) -> ExtractInfo {
    let exe = std::env::current_exe().expect("current exe");
    let pdfium = exe.parent().map(|d| d.join(extract::pdf::LIB_NAME)).filter(|p| p.exists());
    // 부모에서도 한 번 불러 보아 실제로 불러와지는지 기록한다(없음·손상과 진짜 PDF 실패를 구별).
    PDFIUM_LOADED.store(pdfium.as_ref().is_some_and(|p| extract::pdf::init(p).is_ok()), std::sync::atomic::Ordering::Relaxed);
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
    let count = tokenizer.map(|tk| move |s: &str| tk.count(s).ok());
    run_with(roots, count.as_ref().map(|c| c as Counter), &extract, 200, on_progress)
}

/// 토큰 수 세기(실제: 모델 토크나이저, 시험: 글자 수).
pub type Counter<'a> = &'a dyn Fn(&str) -> Option<usize>;

/// `count` 가 없으면 토큰·청크·카드 통계를 내지 않는다.
pub fn run_with(
    roots: &[PathBuf],
    count: Option<Counter>,
    extract: &dyn Fn(&Path, bool) -> Outcome,
    every: usize,
    on_progress: &mut dyn FnMut(&ExtractInfo),
) -> ExtractInfo {
    let mut all = Vec::new();
    let mut mtimes = Vec::new();
    let mut cloud_only = 0;
    for r in roots {
        cloud_only += walk(r, &mut all, &mut mtimes);
    }
    let found_total = all.len();
    let (mtime_age, mtime_top_day_share) = mtime_stats(&mtimes, SystemTime::now());
    let (files, found) = sample_per_format(all, PER_FORMAT_CAP);
    let by_format = found
        .iter()
        .map(|(ext, n)| (ext.to_string(), crate::report::FormatCount { ext: ext.to_string(), found: *n, ..Default::default() }))
        .collect();
    let mut a = Acc {
        x: ExtractInfo { total: files.len(), found: found_total, cloud_only, mtime_age, mtime_top_day_share, ..Default::default() },
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
                a.x.truncated += stats.truncated as usize;
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
                if count.is_some() && !within_stats_cap(&doc) {
                    a.x.stats_skipped_huge += 1;
                    fc.stats_skipped_huge += 1;
                } else if let Some(tk) = count {
                    let text: String = doc.blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
                    let chars = text.chars().count();
                    if chars >= 200 {
                        if let Some(t) = tk(&text) {
                            a.ratios.push(t as f64 * 1000.0 / chars as f64);
                        }
                    }
                    let count = |s: &str| tk(s).unwrap_or_else(|| s.chars().count());
                    let profile = embed::profile::ModelProfile::embeddinggemma2();
                    let chunks = build_chunks(&doc, &BODY_CHUNK, &count);
                    a.chunk_counts.push(chunks.len() as f64);
                    fc.stats_docs += 1;
                    fc.chunks += chunks.len() as u64;
                    // 실제 임베딩 입력(제목 접두어 포함)의 토큰 — 벤치의 초당 토큰과 같은 기준
                    fc.chunk_tokens += chunks.iter().map(|c| count(&profile.render_doc(doc.title.as_deref(), c)) as u64).sum::<u64>();
                    if let Some(card) = embed::chunk::build_card(&doc, 256, &count) {
                        let input = profile.render_doc(doc.title.as_deref(), &card);
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
                FailKind::TooComplex => a.x.too_complex += 1,
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
                fc.crashed += 1;
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

    #[test]
    fn processing_order_covers_every_format_early() {
        use std::path::PathBuf;
        let files: Vec<PathBuf> = (0..50)
            .map(|i| PathBuf::from(format!("{i:02}.txt")))
            .chain((0..4).map(|i| PathBuf::from(format!("{i}.hwp"))))
            .chain((0..4).map(|i| PathBuf::from(format!("{i}.pdf"))))
            .collect();
        let (s, _) = super::sample_per_format(files, 3_000);
        let head: Vec<&str> = s.iter().take(6).filter_map(|p| p.extension()?.to_str()).collect();
        for e in ["txt", "hwp", "pdf"] {
            assert!(head.contains(&e), "앞 6개에 {e} 없음: {head:?}");
        }
    }

    /// 비정상 종료는 형식별 crashed, 시간 초과는 timeout, 내부 상한은 too_complex, 잘린 문서는 truncated 로 따로 센다.
    #[test]
    fn crash_timeout_too_complex_and_truncated_are_counted_apart() {
        use extract::runner::Outcome;
        use extract::FailKind;
        let dir = tempfile::tempdir().unwrap();
        for n in ["crash", "hang", "complex", "big", "large"] {
            std::fs::write(dir.path().join(format!("{n}.ods")), n).unwrap();
        }
        let fake = |p: &std::path::Path, _: bool| match std::fs::read_to_string(p).unwrap().as_str() {
            "crash" => Outcome::Crashed { code: Some(3) },
            "hang" => Outcome::Timeout,
            "complex" => Outcome::Failed { kind: FailKind::TooComplex, magic: String::new() },
            "large" => Outcome::Failed { kind: FailKind::TooLarge, magic: String::new() },
            _ => Outcome::Ok {
                doc: doc::Document::default(),
                distribution: false,
                coverage: None,
                stats: extract::Stats { truncated: true, ..Default::default() },
            },
        };
        let x = super::run_with(&[dir.path().to_path_buf()], None, &fake, 0, &mut |_| {});
        let f = &x.by_format[0];
        assert_eq!((f.ext.as_str(), f.timeout, f.crashed, f.other_fail, f.ok), ("ods", 1, 1, 2, 1));
        assert_eq!((x.timeout, x.crashed, x.too_complex, x.too_large, x.truncated), (1, 1, 1, 1, 1));
    }

    #[test]
    fn mtime_buckets_and_top_day_share() {
        use std::time::{Duration, SystemTime};
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(20_000 * 86_400);
        let day = |d: u64| now - Duration::from_secs(d * 86_400);
        // 같은 날 3개(복사로 덮인 날짜처럼), 200일·800일·4000일 전 각 1개, 시계 차이(1시간 미래)·먼 미래 각 1개
        let m = vec![day(10), day(10), day(10), day(200), day(800), day(4000), now + Duration::from_secs(3600), now + Duration::from_secs(90 * 86_400)];
        let (b, top) = super::mtime_stats(&m, now);
        let got: Vec<(&str, usize)> = b.iter().map(|(l, n)| (l.as_str(), *n)).collect();
        assert_eq!(got, [("30일 이내", 4), ("1년 이내", 1), ("1~2년", 0), ("2~3년", 1), ("3~5년", 0), ("5년 넘음", 1), ("미래", 1)]);
        assert!((top.unwrap() - 3.0 / 8.0).abs() < 1e-9, "{top:?}");
        assert_eq!(super::mtime_stats(&[], now), (vec![], None));
    }

    #[test]
    fn estimate_scales_each_format_by_found_over_processed() {
        use crate::report::FormatCount;
        let f = |ext: &str, found, total, stats_docs, chunks, chunk_tokens| FormatCount { ext: ext.into(), found, total, stats_docs, chunks, chunk_tokens, ..Default::default() };
        // hwp: 표본 10개 중 청크 40 → 찾은 100개면 400. txt: 표본이 전부(5개) → 그대로 7.
        let (c, tk) = super::estimate_all(&[f("hwp", 100, 10, 8, 40, 12_000), f("txt", 5, 5, 5, 7, 700)]).unwrap();
        assert_eq!((c, tk), (407.0, 120_700.0));
        assert_eq!(super::estimate_all(&[f("hwp", 3, 3, 0, 0, 0)]), None, "통계를 낸 문서가 없으면 추정하지 않는다");
    }

    /// 형식별 청크 수·청크 입력 토큰(제목 접두어 포함)을 쌓고, 실패 문서는 청크 0 으로 표본에 남는다.
    #[test]
    fn chunk_totals_accumulate_per_format() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["a", "b", "bad"] {
            std::fs::write(dir.path().join(format!("{n}.hwp")), n).unwrap();
        }
        let para = |s: &str| doc::Block { kind: doc::BlockKind::Paragraph, text: s.into() };
        let fake = |p: &std::path::Path, _: bool| match std::fs::read_to_string(p).unwrap().as_str() {
            "bad" => extract::runner::Outcome::Failed { kind: extract::FailKind::Corrupt, magic: String::new() },
            _ => extract::runner::Outcome::Ok {
                doc: doc::Document { title: Some("제목".into()), blocks: vec![para(&"가".repeat(250)), para(&"나".repeat(250))] },
                distribution: false,
                coverage: None,
                stats: Default::default(),
            },
        };
        let chars = |s: &str| Some(s.chars().count());
        let x = super::run_with(&[dir.path().to_path_buf()], Some(&chars), &fake, 0, &mut |_| {});
        let f = &x.by_format[0];
        // 문단 250자 둘 → 300 목표에서 청크 2개씩, 문서 2개 = 4
        assert_eq!((f.total, f.stats_docs, f.chunks), (3, 2, 4));
        let one = embed::profile::ModelProfile::embeddinggemma2().render_doc(Some("제목"), &"가".repeat(250)).chars().count() as u64;
        assert_eq!(f.chunk_tokens, one * 4);
        assert_eq!(x.chunks_per_doc_mean, Some(2.0));
        assert_eq!(x.est_chunks_all, Some(4.0));
        assert_eq!(x.mtime_age.iter().map(|b| b.1).sum::<usize>(), 3, "찾은 파일 모두의 수정일");
    }

    #[test]
    fn huge_document_is_over_stats_cap() {
        let mk = |n: usize| doc::Document { title: None, blocks: vec![doc::Block { kind: doc::BlockKind::Paragraph, text: "가".repeat(n) }] };
        assert!(super::within_stats_cap(&mk(super::STATS_MAX_CHARS - 10)));
        assert!(!super::within_stats_cap(&mk(5_000_000)));
    }

    /// 모델이 있으면: 줄바꿈 없는 거대한 문단이 통계 계산 때문에 멈추지 않는다(KI-001).
    #[test]
    fn huge_single_paragraph_does_not_stall() {
        let models = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../models"));
        let Some(m) = crate::bench::list_models(&models).into_iter().next() else {
            eprintln!("skip: no model");
            return;
        };
        let Ok(tk) = embed::engine::Tokenizer::load(&m, None) else { return };
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        let fake = |_: &std::path::Path, _: bool| extract::runner::Outcome::Ok {
            doc: doc::Document { title: None, blocks: vec![doc::Block { kind: doc::BlockKind::Paragraph, text: "로그 줄 ".repeat(1_000_000) }] },
            distribution: false,
            coverage: None,
            stats: Default::default(),
        };
        let t = std::time::Instant::now();
        let count = |s: &str| tk.count(s).ok();
        let x = super::run_with(&[dir.path().to_path_buf()], Some(&count), &fake, 0, &mut |_| {});
        assert!(t.elapsed().as_secs() < 20, "{:?}", t.elapsed());
        assert_eq!((x.ok, x.stats_skipped_huge), (1, 1));
    }
}
