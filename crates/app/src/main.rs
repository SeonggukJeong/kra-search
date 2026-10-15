//! kra-search 명령줄. `kra-search index`, `kra-search search 검색어`, `kra-search status`.

use app::embedding::{embed_line, run_embedding, EmbedOptions};
use app::model::{self, MODEL_FILE};
use app::{fail_label, lock_index, open_store, open_store_readonly, reset_store, run_index, search, IndexError, IndexSummary, Store};
use clap::{Parser, Subcommand};
use embed::engine::{Embedder, EngineOptions};
use extract::worker::Worker;
use index::tokenize::Analyzers;
use platform::paths::Home;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

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
    /// 의미 검색 모델(GGUF). 기본: 실행 파일 옆 models\embeddinggemma-2-Q8_0.gguf
    #[arg(long, global = true)]
    model: Option<PathBuf>,
    /// 의미 검색 색인 스레드 수(기본: 성능 코어 수, 최대 4)
    #[arg(long, global = true)]
    threads: Option<usize>,
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
        /// 텍스트·CSV·엑셀 파일 뒷부분까지 정밀 의미 검색(on/off, 저장됨)
        #[arg(long, value_parser = ["on", "off"])]
        data_tail: Option<String>,
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
fn index_once(store: &mut Store, worker: &mut Worker, roots: &mut Vec<PathBuf>, timeout: Duration, pdf_reader: bool) -> Result<IndexSummary, IndexError> {
    if roots.is_empty() {
        *roots = store.meta.roots()?.into_iter().map(PathBuf::from).collect();
    }
    if roots.is_empty() {
        *roots = default_roots();
    }
    store.meta.set_roots(&to_strings(roots))?;
    run_index(store, worker, roots, timeout, pdf_reader)
}

/// 키워드 색인 뒤 의미 검색 색인(DEC-046). 모델이 없거나 못 불러오면 키워드만으로 끝낸다(M2 설계 §3.5).
/// meta 손상은 `MetaCorrupt` 로 돌려준다 — 부르는 쪽이 키워드 단계와 같이 한 번 처음부터 다시 만든다.
fn index_semantic(cli: &Cli, home: &Home, store: &mut Store, data_tail: Option<&str>) -> Result<(), IndexError> {
    if let Some(v) = data_tail {
        store.meta.set_kv("data_tail", v)?;
    }
    let tail = store.meta.kv("data_tail")?.as_deref() != Some("off");
    let Some(path) = model::model_path(&cli.model) else {
        eprintln!("의미 검색 모델({MODEL_FILE})을 찾지 못해 키워드 검색만 준비했습니다.");
        return Ok(());
    };
    let threads = cli.threads.unwrap_or_else(platform::cpu::default_embed_threads);
    let (fp, cache) = model::fingerprint_of(&path, store.meta.kv(model::MODEL_HASH_KV)?.as_deref())
        .map_err(|x| IndexError::Other(format!("모델 파일을 읽지 못했습니다: {x}")))?;
    store.meta.set_kv(model::MODEL_HASH_KV, &cache)?;
    let profile = model::profile();
    let mut m = match Embedder::load(&path, &EngineOptions { threads, max_tokens: 1024, backend_dir: model::exe_dir() }) {
        Ok(m) => m,
        Err(x) => {
            eprintln!("의미 검색 모델을 불러오지 못해 키워드 검색만 준비했습니다: {x}");
            return Ok(());
        }
    };
    let s = run_embedding(store, home, &mut m, &fp, &profile, &EmbedOptions { data_tail: tail, stop_after_append: None }, &mut |c| eprint!("\r{}", embed_line(c)))?;
    eprintln!();
    for (k, v) in [("stat_cards_secs", s.secs_cards), ("stat_body_secs", s.secs_body)] {
        store.meta.set_kv(k, &format!("{v:.1}"))?;
    }
    store.meta.set_kv("stat_threads", &threads.to_string())?;
    let mut line = format!("의미 검색: 빠른 {}개 · 정밀 {}개 새로 만듦 ({threads}스레드)", s.cards, s.chunks);
    if s.failed > 0 {
        line += &format!(" · 만들지 못함 {}개", s.failed);
    }
    println!("{line}");
    Ok(())
}

fn summary_line(s: &IndexSummary) -> String {
    let mut line = format!(
        "파일 {}개 확인 · 바뀐 파일 {}개 · 지운 파일 {}개 · 새로 읽음 {}개 · 같은 내용 재사용 {}개 · 다시 시도 {}개 · 열 수 없는 파일 {}개(다시 시도 포함) · 접근 못 함 {}개",
        s.scanned, s.changed, s.removed, s.extracted, s.reused, s.retried, s.failed, s.unreadable
    );
    if s.cloud_only > 0 {
        line += &format!(" · 클라우드 전용이라 건너뜀 {}개", s.cloud_only);
    }
    if s.pdf_skipped > 0 {
        line += &format!(" · PDF 건너뜀 {}개(다음에 다시)", s.pdf_skipped);
    }
    if s.changed_while_reading > 0 {
        line += &format!(" · 읽는 중 바뀌어 다음에 다시 {}개", s.changed_while_reading);
    }
    line
}

/// 경로를 3개까지 보이고 나머지는 개수로.
const SHOWN_DUPLICATE_PATHS: usize = 3;

fn duplicate_line(others: &[String]) -> String {
    let shown = others[..others.len().min(SHOWN_DUPLICATE_PATHS)].join(" | ");
    let rest = others.len().saturating_sub(SHOWN_DUPLICATE_PATHS);
    let more = if rest > 0 { format!(" 외 {rest}개") } else { String::new() };
    format!("같은 내용 {}개 더: {shown}{more}", others.len())
}

fn run(cli: Cli) -> Result<(), String> {
    let pdfium = pdfium_path(&cli.pdfium);
    if let Cmd::Worker = &cli.cmd {
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
    match &cli.cmd {
        Cmd::Index { root, data_tail } => {
            let _lock = lock_index(&home)?;
            let mut store = open_store(&home, &analyzers)?;
            // 지정한 루트(없으면 index_once 가 저장된 루트를 읽어 채운다). 다시 만들 때 이 값을 다시 저장한다.
            let mut roots: Vec<PathBuf> = root.iter().map(|r| absolute(r)).collect();
            // PDF 읽기 도구를 이 프로세스에서 한 번 불러 본다(DLL 을 절대 경로로 읽기만 한다). 못 불러오면 PDF 는 기록하지 않고 건너뛴다.
            // 워커에게 물어보는 방식보다 단순하고, 워커와 같은 경로·같은 불러오기 코드라 결과가 같다.
            let pdf_reader = pdfium.as_deref().is_some_and(|p| extract::pdf::init(p).is_ok());
            if !pdf_reader {
                eprintln!("PDF 읽기 도구({})를 찾지 못해 PDF 는 이번에 건너뜁니다.", extract::pdf::LIB_NAME);
            }
            let mut worker = Worker::new(worker_cmd(pdfium));
            let timeout = Duration::from_secs(cli.timeout_secs);
            let mut rebuilt = false;
            loop {
                let t = Instant::now();
                let r = index_once(&mut store, &mut worker, &mut roots, timeout, pdf_reader).and_then(|s| {
                    println!("{}", summary_line(&s));
                    store.meta.set_kv("stat_keyword_secs", &format!("{:.1}", t.elapsed().as_secs_f64()))?;
                    index_semantic(&cli, &home, &mut store, data_tail.as_deref())?;
                    // 검색이 읽는 쪽까지 모두 확인(DEC-056) — 손상이면 아래에서 한 번 다시 만든다.
                    Ok(store.meta.check()?)
                });
                match r {
                    // 열 때는 멀쩡했지만 색인 도중(키워드든 의미 검색이든) meta.db 손상이 드러남 — 한 번만 처음부터 다시 만든다.
                    Err(IndexError::MetaCorrupt) if !rebuilt => {
                        rebuilt = true;
                        eprintln!();
                        eprintln!("색인 데이터가 손상되어 처음부터 다시 만듭니다.");
                        drop(store);
                        reset_store(&home)?;
                        store = open_store(&home, &analyzers)?;
                    }
                    r => {
                        r.map_err(|e| e.to_string())?;
                        break;
                    }
                }
            }
        }
        Cmd::Search { limit, query } => {
            let store = open_store_readonly(&home, &analyzers)?;
            let hits = search(&store, &query.join(" "), *limit)?;
            if hits.is_empty() {
                println!("결과 없음");
            }
            for (i, h) in hits.iter().enumerate() {
                println!("{:>3}. {}  [{:.2}]", i + 1, h.title, h.score);
                println!("     {}", h.paths[0]);
                if h.paths.len() > 1 {
                    println!("     {}", duplicate_line(&h.paths[1..]));
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
            let ec = store.meta.embed_counts(embed::plan::DATA_HEAD).map_err(|e| IndexError::from(e).to_string())?;
            let tail = if store.meta.kv("data_tail").map_err(|e| IndexError::from(e).to_string())?.as_deref() == Some("off") { "끔" } else { "켬" };
            println!("{} (텍스트·CSV·엑셀 파일 뒷부분: {tail})", embed_line(&ec));
            println!("저장 위치: {}", home.root().display());
        }
        Cmd::Worker => unreachable!(),
    }
    Ok(())
}
