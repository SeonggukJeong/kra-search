//! 성능 측정: 모델별 처리량·검색어 지연·메모리, 평면 벡터 검색 속도.

use crate::report::{BenchInfo, FlatBench, ModelBench};
use embed::chunk::{build_chunks, ChunkParams};
use embed::engine::{EmbedError, Embedder, EngineOptions};
use embed::profile::ModelProfile;
use index::flat::FlatIndex;
use std::path::{Path, PathBuf};
use std::time::Instant;
use sysinfo::{ProcessesToUpdate, System};

const CORPUS: &str = include_str!("../assets/bench/constitution.txt");
const CARD_TOKENS: f64 = 256.0;

fn corpus_doc() -> doc::Document {
    let blocks = CORPUS
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| doc::Block { kind: doc::BlockKind::Paragraph, text: l.to_string() })
        .collect();
    doc::Document { title: Some("대한민국헌법".into()), blocks }
}

/// 경로가 들어 있는 오류 메시지를 경로 없는 문장으로 바꾼다.
fn err_msg(e: &EmbedError) -> String {
    match e {
        EmbedError::ModelNotFound(_) => "모델 파일이 없습니다".into(),
        EmbedError::ModelLoad(_) => "모델을 불러오지 못했습니다".into(),
        other => other.to_string(),
    }
}

pub fn list_models(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("gguf")))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

pub fn rss_mb() -> u64 {
    let Ok(pid) = sysinfo::get_current_pid() else { return 0 };
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).map(|p| p.memory() / 1024 / 1024).unwrap_or(0)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

pub fn bench_model(path: &Path, backend_dir: Option<&Path>, physical: usize) -> ModelBench {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let opts = |t: usize| EngineOptions { threads: t, max_tokens: 1024, backend_dir: backend_dir.map(Path::to_path_buf) };
    let mut r = ModelBench { model_file: name, ..Default::default() };
    let t = Instant::now();
    let mut m = match Embedder::load(path, &opts(physical.max(1))) {
        Ok(m) => m,
        Err(e) => {
            r.error = Some(err_msg(&e));
            return r;
        }
    };
    r.load_ms = ms(t) as u64;
    let p = ModelProfile::embeddinggemma2();

    let t = Instant::now();
    if let Err(e) = m.embed(&[p.render_query("대통령의 임기와 선거")]) {
        r.error = Some(err_msg(&e));
        return r;
    }
    r.first_query_ms = ms(t);
    r.rss_mb_after_load = rss_mb();

    let d = corpus_doc();
    let raw_chunks = {
        let count = |s: &str| m.token_count(s).unwrap_or_else(|_| s.chars().count());
        build_chunks(&d, &ChunkParams { target_tokens: 400, max_tokens: 500, max_chunks: 2000 }, &count)
    };
    let inputs: Vec<String> = raw_chunks.iter().map(|c| p.render_doc(d.title.as_deref(), c)).collect();
    r.bench_tokens = inputs.iter().map(|s| m.token_count(s).unwrap_or(0)).sum();

    let mut lat: Vec<f64> = raw_chunks
        .iter()
        .take(50)
        .map(|c| {
            let q: String = c.chars().take(20).collect();
            let t = Instant::now();
            let _ = m.embed(&[p.render_query(&q)]);
            ms(t)
        })
        .collect();
    lat.sort_by(f64::total_cmp);
    r.query_ms_p50 = percentile(&lat, 0.5);
    r.query_ms_p95 = percentile(&lat, 0.95);
    r.rss_mb_peak = rss_mb();
    drop(m);

    let mut threads = vec![1, 2, physical.max(1)];
    threads.sort();
    threads.dedup();
    for th in threads {
        let Ok(mut e) = Embedder::load(path, &opts(th)) else { continue };
        let _ = e.embed(&inputs[..inputs.len().min(2)]);
        let t = Instant::now();
        let mut ok = true;
        for batch in inputs.chunks(8) {
            if e.embed(batch).is_err() {
                ok = false;
                break;
            }
        }
        if ok {
            r.tokens_per_sec.push((th, r.bench_tokens as f64 / t.elapsed().as_secs_f64()));
        }
        r.rss_mb_peak = r.rss_mb_peak.max(rss_mb());
    }
    let best = r.tokens_per_sec.iter().map(|x| x.1).fold(0.0, f64::max);
    if best > 0.0 {
        r.card_minutes_per_10k = 10_000.0 * CARD_TOKENS / best / 60.0;
    }
    r
}

fn lcg(seed: &mut u64) -> f32 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((*seed >> 33) as f32 / (1u64 << 31) as f32) - 0.5
}

fn median_ms(
    ix: &FlatIndex,
    queries: &[Vec<f32>],
    checksum: &mut u64,
    f: impl Fn(&FlatIndex, &[f32]) -> Vec<(u32, f32)>,
) -> f64 {
    let mut v: Vec<f64> = queries
        .iter()
        .map(|q| {
            let t = Instant::now();
            let res = f(ix, std::hint::black_box(q));
            let elapsed = ms(t);
            *checksum += res.first().map(|x| x.0 as u64 + 1).unwrap_or(0);
            elapsed
        })
        .collect();
    v.sort_by(f64::total_cmp);
    percentile(&v, 0.5)
}

pub fn flat_bench(n: usize) -> FlatBench {
    let mut seed = 42u64;
    let queries: Vec<Vec<f32>> = (0..20).map(|_| (0..768).map(|_| lcg(&mut seed)).collect()).collect();
    let build = |bit_dim: usize| {
        let mut s = 7u64;
        let mut ix = FlatIndex::new(256, bit_dim);
        let mut v = vec![0f32; 768];
        for _ in 0..n {
            v.iter_mut().for_each(|x| *x = lcg(&mut s));
            ix.push(&v);
        }
        ix
    };
    let mut r = FlatBench { n, ..Default::default() };
    let mut sum = 0u64;
    let ix = build(256);
    r.exact_ms = median_ms(&ix, &queries, &mut sum, |ix, q| ix.search_exact(q, 10));
    r.hamming256_ms = median_ms(&ix, &queries, &mut sum, |ix, q| ix.search_hamming(q, 1000, 10));
    r.asym256_ms = median_ms(&ix, &queries, &mut sum, |ix, q| ix.search_asym(q, 1000, 10));
    drop(ix);
    let ix = build(768);
    r.hamming768_ms = median_ms(&ix, &queries, &mut sum, |ix, q| ix.search_hamming(q, 1000, 10));
    r.result_checksum = sum;
    r
}

/// 모델 하나를 자식 프로세스(`kra-diag bench-one`)에서 측정한다.
/// 메모리를 깨끗하게 재고, 특정 CPU 명령어 문제로 비정상 종료돼도 진단 전체는 계속된다.
fn bench_in_child(model: &Path, threads: usize) -> ModelBench {
    use std::io::Read;
    use wait_timeout::ChildExt;
    let name = model.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let fail = |msg: String| ModelBench { model_file: name.clone(), error: Some(msg), ..Default::default() };
    let Ok(exe) = std::env::current_exe() else { return fail("실행 파일 경로를 알 수 없습니다".into()) };
    let mut child = match std::process::Command::new(exe)
        .args(["bench-one", "--threads", &threads.to_string()])
        .arg(model)
        .stdout(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return fail("측정 프로세스를 시작하지 못했습니다".into()),
    };
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    match child.wait_timeout(std::time::Duration::from_secs(3600)) {
        Ok(Some(st)) => {
            let out = reader.join().unwrap_or_default();
            serde_json::from_str(out.trim())
                .unwrap_or_else(|_| fail(format!("측정 프로세스가 비정상 종료되었습니다(code {:?})", st.code())))
        }
        _ => {
            let _ = child.kill();
            fail("측정 시간이 1시간을 넘어 중단했습니다".into())
        }
    }
}

pub fn run(models_dir: &Path, skip_flat: bool, flat_n: usize) -> BenchInfo {
    let physical = System::physical_core_count().unwrap_or(2);
    let models = list_models(models_dir);
    let mut b = BenchInfo::default();
    if models.is_empty() {
        b.models.push(ModelBench {
            model_file: "(없음)".into(),
            error: Some("모델 파일을 찾지 못했습니다(models 폴더)".into()),
            ..Default::default()
        });
    }
    for m in &models {
        eprintln!("모델 측정 중: {}", m.file_name().unwrap_or_default().to_string_lossy());
        b.models.push(bench_in_child(m, physical));
    }
    if !skip_flat {
        eprintln!("벡터 검색 측정 중({flat_n}개)…");
        b.flat = flat_bench(flat_n);
    }
    b
}
