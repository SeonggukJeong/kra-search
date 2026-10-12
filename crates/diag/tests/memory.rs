//! 모델을 해제했다가 다시 불러와도 메모리가 계속 늘지 않아야 한다(제품: 5분 미사용 시 해제 후 재로드).

use embed::engine::{Embedder, EngineOptions};
use sysinfo::{ProcessesToUpdate, System};

fn rss_mb() -> u64 {
    let pid = sysinfo::get_current_pid().unwrap();
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).unwrap().memory() / 1024 / 1024
}

#[test]
#[ignore = "needs model file"]
fn reload_does_not_grow_memory() {
    let path = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../models/embeddinggemma-2-Q8_0.gguf"));
    let opts = EngineOptions { threads: 4, max_tokens: 1024, backend_dir: None };
    let text = vec!["title: none | text: ".to_string() + &"공공기관 업무 문서 본문입니다. ".repeat(60)];
    let mut samples = Vec::new();
    for _ in 0..5 {
        let mut m = Embedder::load(&path, &opts).unwrap();
        m.embed(&text).unwrap();
        drop(m);
        samples.push(rss_mb());
    }
    eprintln!("rss after each load/drop: {samples:?}");
    assert!(samples[4] < samples[1] + 150, "memory grows on reload: {samples:?}");
}

#[test]
#[ignore = "needs model file"]
fn bulk_embedding_does_not_reserve_vocab_sized_buffers() {
    let path = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../models/embeddinggemma-2-Q8_0.gguf"));
    for threads in [4usize] {
        let opts = EngineOptions { threads, max_tokens: 1024, backend_dir: None };
        let mut m = Embedder::load(&path, &opts).unwrap();
        let after_load = rss_mb();
        let batch: Vec<String> = (0..8).map(|i| format!("title: none | text: {}", "공공기관 업무 문서 본문 문장입니다. ".repeat(40 + i))).collect();
        let mut peak = 0;
        for _ in 0..3 {
            m.embed(&batch).unwrap();
            peak = peak.max(rss_mb());
        }
        eprintln!("threads={threads}: after_load={after_load}MB peak_during_embedding={peak}MB");
        // 어휘(262k) × 토큰 수 크기의 logits 버퍼(~1GB)가 잡히면 실패한다.
        assert!(peak < after_load + 300, "threads={threads}: {after_load}MB -> {peak}MB");
        drop(m);
    }
}
