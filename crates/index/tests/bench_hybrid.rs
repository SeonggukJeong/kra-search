//! 스펙 §13 "의미 포함 최종 결과 p95 ≤ 500ms(본문 청크 50만)" 중 검색 코어 몫(벡터 스캔 + 결합).
//! 검색어 임베딩(폐쇄망 Q8 p95 62ms, results.md §5.2)은 여기서 재지 않는다 — 코어는 400ms 안이어야 한다.
//! 실행: cargo test -p index --release --test bench_hybrid -- --include-ignored --nocapture

use index::hybrid::{fuse, semantic, HybridParams, MapMeta, VectorSet};
use query::calendar::parse_ymd;
use query::{parse, DateTable};
use std::collections::HashSet;
use std::time::Instant;
use vector::record::{Layout, Record};

fn rand_vecs(n: usize, seed: u64) -> impl Iterator<Item = Vec<f32>> {
    let mut s = seed;
    (0..n).map(move |_| {
        (0..768)
            .map(|_| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((s >> 33) as f32 / (1u64 << 31) as f32) - 0.5
            })
            .collect()
    })
}

#[test]
#[ignore = "벤치(약 1분, 메모리 약 400MB)"]
fn semantic_and_fuse_p95_under_400ms_at_500k_chunks() {
    let (mut cards, mut chunks) = (VectorSet::new(Layout::M0), VectorSet::new(Layout::M0));
    for (i, v) in rand_vecs(10_000, 1).enumerate() {
        cards.push(i as i64, &Record::from_full(&v, Layout::M0).unwrap());
    }
    for (i, v) in rand_vecs(500_000, 2).enumerate() {
        chunks.push((i / 50) as i64, &Record::from_full(&v, Layout::M0).unwrap());
    }
    let today = parse_ymd("2026-10-15").unwrap();
    let pq = parse("작년 내부통제 규정 개정 이유", today, &DateTable::builtin());
    let p = HybridParams::default(); // 제품 기본값(DEC-058)
    let meta = MapMeta { dates: (0..10_000).map(|i| (i, today - (i as i32 % 2000))).collect(), chunked: (0..10_000).collect(), title_years: Default::default() };
    let keyword: Vec<(i64, f32)> = (0..200).map(|i| (i * 37 % 10_000, 10.0 - i as f32 * 0.01)).collect();
    let mut ms = Vec::new();
    let mut checksum = 0usize;
    for q in rand_vecs(40, 3) {
        let t = Instant::now();
        let sem = semantic(&cards, &chunks, &q, &p);
        let hits = fuse(&keyword, &sem, &pq, &p, today, &meta, &HashSet::new());
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
        // 의미 경로가 조용히 비어(빨리 끝나) 지연 기준을 통과하지 않게 — 한 번만 확인(재는 시간 밖)
        if ms.len() == 1 {
            assert!(!sem.cards.is_empty() && !sem.chunks.is_empty(), "의미 검색 결과가 비었다(카드 {} · 청크 {})", sem.cards.len(), sem.chunks.len());
        }
        checksum += hits.len();
    }
    ms.sort_by(f64::total_cmp);
    let p95 = ms[(ms.len() - 1) * 95 / 100];
    println!("청크 50만 · 카드 1만: p50 {:.1}ms · p95 {:.1}ms (결과 합 {checksum})", ms[ms.len() / 2], p95);
    assert!(checksum > 0);
    assert!(p95 < 400.0, "p95 {p95:.1}ms");
}
