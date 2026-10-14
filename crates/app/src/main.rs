//! kra-search 명령줄. `kra-search index`, `kra-search search 검색어`, `kra-search status`.

use app::{fail_label, lock_index, open_store, open_store_readonly, reset_store, run_index, search, IndexError, IndexSummary, Store};
use clap::{Parser, Subcommand};
use extract::worker::Worker;
use index::tokenize::Analyzers;
use platform::paths::Home;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "kra-search", version, about = "KRA 문서검색 (M1 명령줄)")]
struct Cli {
    /// 색인 저장 위치(기본: %LOCALAPPDATA%\KraSearch)
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    /// pdfium 라이브러리 경로(기본: 실행 파일 옆). 상대 경로는 절대 경로로 바꿔 쓴다.
    #[arg(long, global = true, hide = true)]
    pdfium: Option<PathBuf>,
    /// 파일당 제한 시간(초)
    #[arg(long, global = true, hide = true, default_value_t = 30)]
    timeout_secs: u64,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 문서를 읽어 색인한다(바뀐 파일만)
    Index {
        /// 색인할 폴더(여러 번 지정 가능). 지정하면 저장된 폴더 목록을 바꾼다.
        #[arg(long)]
        root: Vec<PathBuf>,
    },
    /// 색인에서 찾는다. 낱말 여러 개를 따옴표 없이 써도 된다(`--limit` 은 검색어 앞에).
    Search {
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(required = true, num_args = 1.., allow_hyphen_values = true, trailing_var_arg = true)]
        query: Vec<String>,
    },
    /// 색인 현황
    Status,
    /// (내부용) 상주 추출 워커
    #[command(hide = true)]
    Worker,
}

/// 경로를 절대 경로로. canonicalize 는 Windows 에서 `\\?\C:\…` 꼴을 돌려줘 저장·출력·접두어 비교를 어긋나게 한다(DEC-040).
fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// pdfium 은 명시한 절대 경로만 쓴다: --pdfium(상대 경로면 절대로 — LoadLibrary 가 검색 경로를 뒤지지 않게), 없으면 실행 파일과 같은 폴더(스펙 §7.6).
fn pdfium_path(arg: &Option<PathBuf>) -> Option<PathBuf> {
    match arg {
        Some(p) => Some(absolute(p)),
        None => {
            let p = std::env::current_exe().ok()?.parent()?.join(extract::pdf::LIB_NAME);
            p.exists().then_some(p)
        }
    }
}

fn default_roots() -> Vec<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    home.map(|h| ["Documents", "Desktop", "Downloads"].iter().map(|d| h.join(d)).filter(|p| p.is_dir()).collect()).unwrap_or_default()
}

fn worker_cmd(pdfium: Option<PathBuf>) -> impl Fn() -> Command + Send + 'static {
    move || {
        let mut c = Command::new(std::env::current_exe().expect("실행 파일 경로"));
        c.arg("worker");
        if let Some(p) = &pdfium {
            c.arg("--pdfium").arg(p);
        }
        c
    }
}

fn main() {
    // 가장 먼저: DLL 검색 경로 제한·오류 창 끄기·WER 힙 제외(워커에 상속, DEC-042). 레지스트리는 바꾸지 않는다.
    platform::process::harden();
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("오류: {e}");
        std::process::exit(1);
    }
}

fn to_strings(roots: &[PathBuf]) -> Vec<String> {
    roots.iter().map(|r| r.to_string_lossy().into_owned()).collect()
}

/// 루트를 정해 저장하고 한 번 색인한다. `roots` 가 비어 있으면 저장된 루트(그것도 없으면 기본 폴더)로 채운다.
/// meta 손상은 루트를 읽거나 바꾸다가 드러나도 `MetaCorrupt` 로 돌려준다 — 부르는 쪽이 한 번 다시 만든다.
/// 저장된 루트를 읽기 전에 손상이 드러나면 루트를 알 수 없어 다시 만들 때 기본 폴더를 쓴다(KI-030).
fn index_once(store: &mut Store, worker: &mut Worker, roots: &mut Vec<PathBuf>, timeout: Duration) -> Result<IndexSummary, IndexError> {
    if roots.is_empty() {
        *roots = store.meta.roots()?.into_iter().map(PathBuf::from).collect();
    }
    if roots.is_empty() {
        *roots = default_roots();
    }
    store.meta.set_roots(&to_strings(roots))?;
    run_index(store, worker, roots, timeout)
}

fn run(cli: Cli) -> Result<(), String> {
    let pdfium = pdfium_path(&cli.pdfium);
    if let Cmd::Worker = cli.cmd {
        if let Some(p) = &pdfium {
            // 실패해도 워커는 돈다 — PDF 만 "열 수 없는 파일"이 된다.
            let _ = extract::pdf::init(p);
        }
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        return extract::worker::serve(stdin.lock(), stdout.lock(), extract::cli::extract_one).map_err(|e| e.to_string());
    }
    let home = Home::new(cli.home.clone().unwrap_or_else(Home::default_root));
    let analyzers = Analyzers::load()?;
    match cli.cmd {
        Cmd::Index { root } => {
            let _lock = lock_index(&home)?;
            let mut store = open_store(&home, &analyzers)?;
            // 지정한 루트(없으면 index_once 가 저장된 루트를 읽어 채운다). 다시 만들 때 이 값을 다시 저장한다.
            let mut roots: Vec<PathBuf> = root.iter().map(|r| absolute(r)).collect();
            let mut worker = Worker::new(worker_cmd(pdfium));
            let timeout = Duration::from_secs(cli.timeout_secs);
            let s = match index_once(&mut store, &mut worker, &mut roots, timeout) {
                // 열 때는 멀쩡했지만 루트 목록·색인 도중 meta.db 손상이 드러남 — 한 번만 처음부터 다시 만든다.
                Err(IndexError::MetaCorrupt) => {
                    eprintln!();
                    eprintln!("색인 데이터가 손상되어 처음부터 다시 만듭니다.");
                    drop(store);
                    reset_store(&home)?;
                    store = open_store(&home, &analyzers)?;
                    index_once(&mut store, &mut worker, &mut roots, timeout).map_err(|e| e.to_string())?
                }
                r => r.map_err(|e| e.to_string())?,
            };
            let mut line = format!(
                "파일 {}개 확인 · 바뀐 파일 {}개 · 지운 파일 {}개 · 새로 읽음 {}개 · 같은 내용 재사용 {}개 · 다시 시도 {}개 · 못 읽음 {}개(다시 시도 포함) · 접근 못 함 {}개",
                s.scanned, s.changed, s.removed, s.extracted, s.reused, s.retried, s.failed, s.unreadable
            );
            if s.cloud_only > 0 {
                line += &format!(" · 클라우드 전용이라 건너뜀 {}개", s.cloud_only);
            }
            if s.changed_while_reading > 0 {
                line += &format!(" · 읽는 중 바뀌어 다음에 다시 {}개", s.changed_while_reading);
            }
            println!("{line}");
        }
        Cmd::Search { limit, query } => {
            let store = open_store_readonly(&home, &analyzers)?;
            let hits = search(&store, &query.join(" "), limit)?;
            if hits.is_empty() {
                println!("결과 없음");
            }
            for (i, h) in hits.iter().enumerate() {
                println!("{:>3}. {}  [{:.2}]", i + 1, h.title, h.score);
                println!("     {}", h.paths[0]);
                if h.paths.len() > 1 {
                    println!("     같은 내용 {}개 더: {}", h.paths.len() - 1, h.paths[1..].join(" | "));
                }
                if let Some(f) = &h.fail {
                    println!("     ※ {}", fail_label(f));
                }
            }
        }
        Cmd::Status => {
            let store = open_store_readonly(&home, &analyzers)?;
            let c = store.meta.counts().map_err(|e| IndexError::from(e).to_string())?;
            let roots = store.meta.roots().map_err(|e| IndexError::from(e).to_string())?;
            println!("색인 폴더: {}", if roots.is_empty() { "(없음)".to_string() } else { roots.join(", ") });
            println!("파일 {} · 내용 {} · 내용을 못 읽은 것 {} · 반영 대기 {} · 검색에 반영된 문서 {}", c.files, c.contents, c.failed, c.pending, store.keyword.num_docs());
            println!("저장 위치: {}", home.root().display());
        }
        Cmd::Worker => unreachable!(),
    }
    Ok(())
}
