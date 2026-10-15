//! kra-search 명령줄. `kra-search index`, `kra-search search 검색어`, `kra-search status`, `kra-search eval`.

use app::embedding::{embed_line, run_embedding, EmbedOptions};
use app::model::{self, MODEL_FILE};
use app::{fail_label, lock_index, open_store, open_store_readonly, reset_store, run_index, IndexError, IndexSummary, Store};
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
    #[arg(long, global = true, value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..))]
    threads: Option<usize>,
    /// (평가·시험용) 기준 시점 YYYY-MM-DD — 날짜 표현("작년")을 이 날 기준으로 읽는다
    #[arg(long, global = true, hide = true)]
    now: Option<String>,
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
    /// 검색 품질 평가: --set 평가 세트(개발 PC), --queries 내 질의 파일(폐쇄망 확인)
    Eval {
        /// 평가 세트 폴더(manifest.toml·queries.tsv·docs/). 여러 번 지정 가능. --home 을 따로 지정해야 한다.
        #[arg(long)]
        set: Vec<PathBuf>,
        /// 내 질의 파일: 한 줄에 `질의<탭>정답 파일 이름[,정답 파일 이름…]`(UTF-8)
        #[arg(long)]
        queries: Option<PathBuf>,
        /// 보고서 파일(기본: --set 은 화면, --queries 는 kra-search-eval-report.txt)
        #[arg(long)]
        out: Option<PathBuf>,
        /// 질의별 순위 JSON(일치 검사용)
        #[arg(long, hide = true)]
        dump: Option<PathBuf>,
        /// 모델 없이 평가 경로만 시험(시험용)
        #[arg(long, hide = true)]
        keyword_only: bool,
    },
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
    let (fp, cache) = match model::fingerprint_of(&path, store.meta.kv(model::MODEL_HASH_KV)?.as_deref()) {
        Ok(v) => v,
        Err(x) => {
            // 모델을 읽지 못해도 키워드 색인은 끝났다 — index 를 실패시키지 않는다(M2 설계 §3.5).
            eprintln!("모델 파일을 읽지 못해 키워드 검색만 준비했습니다: {x}");
            return Ok(());
        }
    };
    store.meta.set_kv(model::MODEL_HASH_KV, &cache)?;
    let profile = model::profile();
    let mut m = match Embedder::load(&path, &EngineOptions { threads, max_tokens: 1024, backend_dir: model::exe_dir() }) {
        Ok(m) => m,
        Err(x) => {
            eprintln!("의미 검색 모델을 불러오지 못해 키워드 검색만 준비했습니다: {x}");
            return Ok(());
        }
    };
    // 처음부터 색인인가: 이 실행을 시작할 때 파일도 벡터도 없었다(stat_full 은 키워드 단계가 "파일이 없었는가"로 먼저 적어 둔다).
    let ec = store.meta.embed_counts(embed::plan::DATA_HEAD)?;
    let vectors_empty = ec.cards_done == 0 && ec.main_done == 0 && ec.tail_done == 0;
    let files_were_empty = store.meta.kv("stat_full")?.as_deref() == Some("1");
    store.meta.set_kv("stat_full", if files_were_empty && vectors_empty { "1" } else { "0" })?;
    let s = run_embedding(store, home, &mut m, &fp, &profile, &EmbedOptions { data_tail: tail, stop_after_append: None }, &mut |c| eprint!("\r{}", embed_line(c)))?;
    eprintln!();
    for (k, v) in [("stat_cards_secs", s.secs_cards), ("stat_body_secs", s.secs_body)] {
        store.meta.set_kv(k, &format!("{v:.1}"))?;
    }
    store.meta.set_kv("stat_threads", &threads.to_string())?;
    store.meta.set_kv("stat_cards_new", &s.cards.to_string())?;
    store.meta.set_kv("stat_chunks_new", &s.chunks.to_string())?;
    let mut line = format!("의미 검색: 빠른 {}개 · 정밀 {}개 새로 만듦 ({threads}스레드)", s.cards, s.chunks);
    if s.failed > 0 {
        line += &format!(" · 만들지 못함 {}개", s.failed);
    }
    println!("{line}");
    Ok(())
}

/// `index` 와 `eval --set` 이 같이 쓰는 색인 한 번(키워드 → 의미 검색 → meta 검사, 손상이면 한 번 다시).
fn index_command(cli: &Cli, home: &Home, analyzers: &Analyzers, pdfium: Option<PathBuf>, mut roots: Vec<PathBuf>, data_tail: Option<&str>) -> Result<(), String> {
    let _lock = lock_index(home)?;
    let mut store = open_store(home, analyzers)?;
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
        let files_were_empty = store.meta.counts().is_ok_and(|c| c.files == 0);
        let r = index_once(&mut store, &mut worker, &mut roots, timeout, pdf_reader).and_then(|s| {
            println!("{}", summary_line(&s));
            store.meta.set_kv("stat_keyword_secs", &format!("{:.1}", t.elapsed().as_secs_f64()))?;
            // 이번 실행의 처리량 — 시간만 있으면 "0.3초" 가 얼마를 처리한 것인지 알 수 없다. 의미 검색 단계가 못 돌면 0 으로 남는다.
            store.meta.set_kv("stat_extracted", &s.extracted.to_string())?;
            store.meta.set_kv("stat_full", if files_were_empty { "1" } else { "0" })?;
            for k in ["stat_cards_secs", "stat_body_secs", "stat_cards_new", "stat_chunks_new"] {
                store.meta.set_kv(k, "0")?;
            }
            index_semantic(cli, home, &mut store, data_tail)?;
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
                reset_store(home)?;
                store = open_store(home, analyzers)?;
            }
            r => return r.map_err(|e| e.to_string()),
        }
    }
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

/// `eval` 의 검색기. `--keyword-only`(시험용)가 아니면 의미 검색이 실제로 쓰여야 한다 — 모델 파일이 있어도 불러오지 못하면
/// 색인은 키워드만 만들고 끝나고(`index_semantic` 은 성공), 검색기는 알림 없이 키워드만 쓴다. 그 숫자를 평가로 내지 않는다.
/// `--set` 과 `--queries`(Task 18)가 같이 쓴다.
fn eval_searcher(cli: &Cli, home: &Home, store: &Store, keyword_only: bool) -> Result<app::search::Searcher, String> {
    let threads = cli.threads.unwrap_or_else(platform::cpu::default_embed_threads);
    let searcher = app::search::Searcher::open(store, home, model::model_path(&cli.model).as_deref(), threads)?;
    if keyword_only {
        return Ok(searcher);
    }
    if let Some(n) = &searcher.notice {
        return Err(format!("평가에는 의미 검색이 있어야 합니다: {n}"));
    }
    if !searcher.semantic_ready() {
        return Err("평가에는 의미 검색이 있어야 합니다: 의미 검색 색인을 만들지 못했습니다(모델을 불러오지 못함 — 위 메시지 참고).".into());
    }
    Ok(searcher)
}

/// `p` 가 기본 색인 위치(`Home::default_root`)인가. 절대 경로로 바꿔 구성 요소로 비교하고(끝의 `/`·`.` 무시),
/// 둘 다 있으면 실제 경로(링크 풀기)로도 비교한다. Windows 는 대소문자를 가리지 않는다.
fn is_default_home(p: &Path) -> bool {
    let norm = |x: &Path| -> PathBuf { absolute(x).components().filter(|c| !matches!(c, std::path::Component::CurDir)).collect() };
    let (a, b) = (norm(p), norm(&Home::default_root()));
    let same = if cfg!(windows) { a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase() } else { a == b };
    same || matches!((std::fs::canonicalize(&a), std::fs::canonicalize(&b)), (Ok(x), Ok(y)) if x == y)
}

fn today(now: &Option<String>) -> Result<query::Day, String> {
    match now {
        Some(s) => query::calendar::parse_ymd(s).ok_or_else(|| format!("--now 는 YYYY-MM-DD 형식입니다: {s}")),
        None => Ok(query::calendar::today()),
    }
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
            // 지정한 루트(없으면 index_once 가 저장된 루트를 읽어 채운다). 다시 만들 때 이 값을 다시 저장한다.
            let roots: Vec<PathBuf> = root.iter().map(|r| absolute(r)).collect();
            index_command(&cli, &home, &analyzers, pdfium, roots, data_tail.as_deref())?;
        }
        Cmd::Eval { set, queries, out, dump, keyword_only } => {
            if !set.is_empty() {
                // 평가 색인은 내 색인과 섞이면 안 된다 — --home 이 없거나 기본 위치(같은 폴더를 다른 꼴로 쓴 것 포함)면 거절.
                if cli.home.as_deref().is_none_or(is_default_home) {
                    return Err("평가 세트는 --home 에 내 색인과 다른 폴더를 지정하세요(내 색인과 섞이지 않게).".into());
                }
                // 오래 걸리는 복사·색인 전에 확인한다 — 모델 없이 만든 순위는 제품 순위가 아니다.
                if model::model_path(&cli.model).is_none() && !keyword_only {
                    return Err(format!("평가에는 의미 검색 모델이 있어야 합니다({MODEL_FILE}, --model)."));
                }
                let now = today(&cli.now)?;
                let docs_dir = absolute(&home.root().join("eval-docs"));
                eprintln!("평가 문서 {}개 준비", app::evalset::materialize(set, &docs_dir)?);
                // 제품과 같은 색인 흐름(DEC-052). 루트는 복사본 폴더 하나 — 세트 폴더 이름이 경로에 남아 m0eval 의 번호 순서와 같아진다(P2).
                let t = Instant::now();
                // 평가는 데이터 뒷부분(본문 ③) 켬으로 정의한다(DEC-058) — 다시 쓰는 평가 폴더에 남은 설정(kv data_tail)이 결과를 바꾸지 않게 매번 켠다.
                index_command(&cli, &home, &analyzers, pdfium, vec![docs_dir], Some("on"))?;
                eprintln!("색인 {:.1}초", t.elapsed().as_secs_f64());
                let store = open_store_readonly(&home, &analyzers)?;
                let mut searcher = eval_searcher(&cli, &home, &store, *keyword_only)?;
                let mut qs = Vec::new();
                for d in set {
                    qs.extend(app::evalset::queries(d)?);
                }
                let t = Instant::now();
                let ranks = app::eval::rank_set(&store, &mut searcher, &qs, now)?;
                eprintln!("질의 {}개 검색 {:.1}초", qs.len(), t.elapsed().as_secs_f64());
                let report = app::eval::set_report(&qs, &ranks);
                match out {
                    Some(p) => std::fs::write(p, &report).map_err(|e| format!("{}: {e}", p.display()))?,
                    None => println!("{report}"),
                }
                if let Some(d) = dump {
                    std::fs::write(d, serde_json::to_string(&ranks).expect("json")).map_err(|e| format!("{}: {e}", d.display()))?;
                }
            } else if let Some(tsv) = queries {
                // 내 색인을 그대로 읽는다 — 색인을 만들거나 바꾸지 않는다(그래서 기본 위치 거절·데이터 뒷부분 강제도 없다).
                let store = open_store_readonly(&home, &analyzers)?;
                let bytes = std::fs::read(tsv).map_err(|e| format!("질의 파일을 읽지 못했습니다: {e}"))?;
                let text = String::from_utf8(bytes).map_err(|_| "질의 파일을 UTF-8 로 저장해 주세요(메모장: 다른 이름으로 저장 → 인코딩 UTF-8).".to_string())?;
                let (qs, skipped_lines) = app::eval::parse_user_queries(&text);
                if qs.is_empty() {
                    return Err("질의가 없습니다 — 한 줄에 `질의<탭>정답 파일 이름`".into());
                }
                let threads = cli.threads.unwrap_or_else(platform::cpu::default_embed_threads);
                let t = Instant::now();
                // --set 과 같은 확인: 모델을 불러오지 못한 채 키워드 숫자를 하이브리드로 내지 않는다.
                let mut s = eval_searcher(&cli, &home, &store, *keyword_only)?;
                let model_ms = t.elapsed().as_secs_f64() * 1000.0;
                let rs = app::eval::run_user_queries(&store, &mut s, &qs, today(&cli.now)?)?;
                let me = |e: index::meta::MetaError| IndexError::from(e).to_string();
                let c = store.meta.counts().map_err(me)?;
                let stats = app::eval::IndexStats::from_kv(|k| store.meta.kv(k).ok().flatten());
                let ix = app::eval::IndexInfo { files: c.files, contents: c.contents, failed: c.failed, embed: store.meta.embed_counts(embed::plan::DATA_HEAD).map_err(me)?, stats };
                let split = platform::cpu::core_split();
                let pc = app::eval::PcInfo {
                    perf: split.map(|x| x.0),
                    eff: split.map(|x| x.1),
                    logical: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
                    threads,
                };
                // 설정 지문: 직렬화한 검색 설정의 xxh3 앞 8자리(사용자 내용 없음).
                let params_id = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(serde_json::to_string(&s.params).expect("json").as_bytes()))[..8].to_string();
                let run = app::eval::RunInfo { version: env!("CARGO_PKG_VERSION").to_string(), params_id, skipped_lines };
                let report = app::eval::user_report(&rs, &ix, &pc, &run, model_ms);
                // 화면에 먼저 — 파일을 못 써도 이 실행의 결과는 남는다.
                println!("{report}");
                let path = out.clone().unwrap_or_else(|| PathBuf::from("kra-search-eval-report.txt"));
                std::fs::write(&path, &report).map_err(|e| format!("보고서 파일을 쓰지 못했습니다(위 화면 내용을 복사해 가세요): {e}"))?;
                // 경로는 이 PC 화면에만 나온다 — 보고서 파일에는 없다.
                eprintln!("보고서: {}", path.display());
            } else {
                return Err("--set 또는 --queries 를 주세요.".into());
            }
        }
        Cmd::Search { limit, query } => {
            let store = open_store_readonly(&home, &analyzers)?;
            let threads = cli.threads.unwrap_or_else(platform::cpu::default_embed_threads);
            let mut s = app::search::Searcher::open(&store, &home, model::model_path(&cli.model).as_deref(), threads)?;
            let (pq, hits) = s.search(&store, &query.join(" "), today(&cli.now)?, *limit)?;
            if let Some(n) = &s.notice {
                eprintln!("{n}");
            }
            for c in pq.chips() {
                let r = c.range.expect("칩");
                println!("📅 {} → {} ~ {} (이 기간 문서를 앞으로)", &pq.original[c.start..c.end], query::calendar::format_ymd(r.start), query::calendar::format_ymd(r.end));
            }
            if hits.is_empty() {
                println!("결과 없음");
            }
            for (i, h) in hits.iter().enumerate() {
                println!("{:>3}. {}", i + 1, h.hit.title);
                println!("     {}", h.hit.paths[0]);
                if h.hit.paths.len() > 1 {
                    println!("     {}", duplicate_line(&h.hit.paths[1..]));
                }
                if let Some(f) = &h.hit.fail {
                    println!("     ※ {}", fail_label(f));
                }
                let w = app::search::why_line(&h.why);
                if !w.is_empty() {
                    println!("     {w}");
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
