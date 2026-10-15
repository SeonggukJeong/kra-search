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
#[command(name = "kra-diag", version, about = "KRA 문서검색 진단 도구")]
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
    /// 문서(형식별) 추출 통계만 측정
    ExtractStats {
        #[arg(long)]
        root: Vec<PathBuf>,
        #[arg(long)]
        models: Option<PathBuf>,
    },
    /// (내부용) 파일 하나를 추출해 JSON 한 줄 출력
    #[command(hide = true)]
    ExtractOne {
        file: PathBuf,
        /// 추출 커버리지도 잰다
        #[arg(long)]
        coverage: bool,
        /// pdfium 절대 경로(부모가 실행 파일 옆 것을 넘긴다)
        #[arg(long)]
        pdfium: Option<PathBuf>,
    },
    /// (내부용) 모델 하나를 측정해 JSON 한 줄 출력
    #[command(hide = true)]
    BenchOne {
        model: PathBuf,
        #[arg(long)]
        threads: usize,
        /// 논리 코어 수(스레드 수 측정 범위)
        #[arg(long, default_value_t = 0)]
        logical: usize,
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
    use windows::Win32::System::Diagnostics::Debug::{SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX};
    use windows::Win32::System::ErrorReporting::{WerSetFlags, WER_FAULT_REPORTING, WER_FAULT_REPORTING_FLAG_NOHEAP, WER_FAULT_REPORTING_NO_UI};
    unsafe {
        let _ = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_APPLICATION_DIR);
        // 자식 프로세스가 비정상 종료해도 오류 창을 띄우지 않고(자식에게 상속됨), 오류 보고 덤프에 힙(문서 텍스트)을 넣지 않는다.
        // 레지스트리를 바꾸는 WerAddExcludedApplication 은 PC에 흔적을 남기므로 진단 도구에서는 쓰지 않는다.
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
        let _ = WerSetFlags(WER_FAULT_REPORTING(WER_FAULT_REPORTING_NO_UI | WER_FAULT_REPORTING_FLAG_NOHEAP.0));
    }
}

/// 토큰 통계용 토크나이저(가중치 없이 어휘만 로드 — 부모 프로세스가 수 시간 동안 모델 전체를 들고 있지 않게).
fn first_tokenizer(dir: &Path, backend: &Path) -> Option<embed::engine::Tokenizer> {
    let m = bench::list_models(dir).into_iter().next()?;
    embed::engine::Tokenizer::load(&m, Some(backend)).ok()
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
        Cmd::ExtractOne { file, coverage, pdfium } => {
            if let Some(p) = pdfium {
                let _ = extract::pdf::init(p);
            }
            extract::cli::extract_one_main(file, *coverage);
            return;
        }
        Cmd::BenchOne { model, threads, logical } => {
            let r = bench::bench_model(model, Some(&dir), *threads, (*logical).max(*threads));
            println!("{}", serde_json::to_string(&r).unwrap());
            return;
        }
        _ => {}
    }
    let out = report::output_dir(&std::env::current_dir().unwrap_or_else(|_| dir.clone()), &dir);
    if interactive {
        // 더블클릭 실행: 오류가 나도 창이 바로 닫히지 않게 한다.
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            default_hook(info);
            pause("오류로 중단되었습니다. 지금까지의 결과는 kra-diag-summary.txt 에 저장되어 있습니다.");
        }));
    }
    let mut r = report::Report::new();
    let save = |r: &mut report::Report, last: bool| {
        if let Err(e) = report::write(r, &out, last) {
            eprintln!("보고서를 저장하지 못했습니다({}): {e}", out.display());
            if interactive {
                pause("Enter 키를 누르면 닫힙니다.");
            }
            std::process::exit(1);
        }
    };
    match cmd {
        Cmd::All { root, models, skip_flat, flat_n } => {
            let models = models.unwrap_or_else(|| dir.join("models"));
            r.bench = Some(bench::run(&models, skip_flat, flat_n));
            save(&mut r, false);
            r.env = Some(env::collect());
            if let Some(e) = r.env.as_mut() {
                e.llama_system_info = embed::engine::system_info();
            }
            save(&mut r, false);
            let tk = first_tokenizer(&models, &dir);
            let x = extract_stats::run(&roots_or_default(root), tk.as_ref(), &mut |partial| {
                r.extract = Some(clone_extract(partial));
                save(&mut r, false);
            });
            r.extract = Some(x);
        }
        Cmd::Env => {
            let _ = embed::engine::init(Some(&dir));
            r.env = Some(env::collect());
        }
        Cmd::Bench { models, skip_flat, flat_n } => {
            r.bench = Some(bench::run(&models.unwrap_or_else(|| dir.join("models")), skip_flat, flat_n));
        }
        Cmd::ExtractStats { root, models } => {
            let tk = first_tokenizer(&models.unwrap_or_else(|| dir.join("models")), &dir);
            let x = extract_stats::run(&roots_or_default(root), tk.as_ref(), &mut |partial| {
                r.extract = Some(clone_extract(partial));
                save(&mut r, false);
            });
            r.extract = Some(x);
        }
        Cmd::ExtractOne { .. } | Cmd::BenchOne { .. } => unreachable!(),
    }
    if let Some(e) = r.env.as_mut() {
        e.llama_system_info = embed::engine::system_info();
    }
    save(&mut r, true);
    println!("\n저장 위치: {}", out.display());
    if interactive {
        pause("완료되었습니다. kra-diag-summary.txt 와 kra-diag-report.json 을 확인해 주세요. Enter 키를 누르면 닫힙니다.");
    }
}

fn pause(msg: &str) {
    println!("\n{msg}");
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
}

fn clone_extract(x: &report::ExtractInfo) -> report::ExtractInfo {
    serde_json::from_value(serde_json::to_value(x).unwrap()).unwrap()
}
