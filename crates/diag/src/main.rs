//! kra-diag: 폐쇄망 PC 진단 도구. 문서 본문·경로·파일명은 결과에 남기지 않는다.

mod bench;
mod env;
#[cfg(windows)]
mod env_windows;
mod extract_stats;
mod pathhash;
mod report;

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "kra-diag", version, about = "KRA 문서검색 M0 진단 도구")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// 환경·성능·문서 추출 통계를 모두 측정
    All {
        #[arg(long)]
        root: Vec<PathBuf>,
        #[arg(long)]
        models: Option<PathBuf>,
        #[arg(long)]
        skip_flat: bool,
        #[arg(long, default_value_t = 500_000)]
        flat_n: usize,
    },
    /// PC 환경만 점검
    Env,
    /// 모델·벡터 검색 성능만 측정
    Bench {
        #[arg(long)]
        models: Option<PathBuf>,
        #[arg(long)]
        skip_flat: bool,
        #[arg(long, default_value_t = 500_000)]
        flat_n: usize,
    },
    /// HWP/HWPX 추출 통계만 측정
    ExtractStats {
        #[arg(long)]
        root: Vec<PathBuf>,
        #[arg(long)]
        models: Option<PathBuf>,
    },
    /// (내부용) 파일 하나를 추출해 JSON 한 줄 출력
    #[command(hide = true)]
    ExtractOne { file: PathBuf },
    /// (내부용) 모델 하나를 측정해 JSON 한 줄 출력
    #[command(hide = true)]
    BenchOne {
        model: PathBuf,
        #[arg(long)]
        threads: usize,
    },
}

fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(windows)]
fn harden_dll_search() {
    use windows::Win32::System::LibraryLoader::{
        SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };
    unsafe {
        let _ = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_APPLICATION_DIR);
    }
}

fn first_model(dir: &Path, backend: &Path) -> Option<embed::engine::Embedder> {
    let m = bench::list_models(dir).into_iter().next()?;
    let opts = embed::engine::EngineOptions { threads: 2, max_tokens: 1024, backend_dir: Some(backend.to_path_buf()) };
    embed::engine::Embedder::load(&m, &opts).ok()
}

fn roots_or_default(r: Vec<PathBuf>) -> Vec<PathBuf> {
    if r.is_empty() { extract_stats::default_roots() } else { r }
}

fn main() {
    #[cfg(windows)]
    harden_dll_search();
    let cli = Cli::parse();
    let interactive = cli.cmd.is_none();
    let dir = exe_dir();
    let cmd = cli.cmd.unwrap_or(Cmd::All { root: vec![], models: None, skip_flat: false, flat_n: 500_000 });
    match &cmd {
        Cmd::ExtractOne { file } => {
            extract::cli::extract_one_main(file);
            return;
        }
        Cmd::BenchOne { model, threads } => {
            let r = bench::bench_model(model, Some(&dir), *threads);
            println!("{}", serde_json::to_string(&r).unwrap());
            return;
        }
        _ => {}
    }
    let mut r = report::Report::new();
    match cmd {
        Cmd::All { root, models, skip_flat, flat_n } => {
            let models = models.unwrap_or_else(|| dir.join("models"));
            r.bench = Some(bench::run(&models, skip_flat, flat_n));
            r.env = Some(env::collect());
            let tk = first_model(&models, &dir);
            r.extract = Some(extract_stats::run(&roots_or_default(root), tk.as_ref()));
        }
        Cmd::Env => {
            let _ = embed::engine::init(Some(&dir));
            r.env = Some(env::collect());
        }
        Cmd::Bench { models, skip_flat, flat_n } => {
            r.bench = Some(bench::run(&models.unwrap_or_else(|| dir.join("models")), skip_flat, flat_n));
        }
        Cmd::ExtractStats { root, models } => {
            let tk = first_model(&models.unwrap_or_else(|| dir.join("models")), &dir);
            r.extract = Some(extract_stats::run(&roots_or_default(root), tk.as_ref()));
        }
        Cmd::ExtractOne { .. } | Cmd::BenchOne { .. } => unreachable!(),
    }
    if let Some(e) = r.env.as_mut() {
        e.llama_system_info = embed::engine::system_info();
    }
    let cwd = std::env::current_dir().unwrap_or(dir);
    if let Err(e) = report::write(&mut r, &cwd) {
        eprintln!("보고서를 저장하지 못했습니다: {e}");
        std::process::exit(1);
    }
    if interactive {
        println!("\n완료되었습니다. kra-diag-summary.txt 와 kra-diag-report.json 을 확인해 주세요. Enter 키를 누르면 닫힙니다.");
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
    }
}
