//! `Searcher::open` 의 MapMeta 만들기 비용(내용 5만) — `from_sets` 가 하는 세 읽기를 그대로 잰다(벡터 적재는 따로 잰다).
//! 실행: cargo test -p app --release --test bench_open -- --include-ignored --nocapture

use index::meta::{ContentData, Meta};
use std::collections::HashMap;
use std::time::Instant;

#[test]
#[ignore = "벤치(내용 5만 행 만들기 포함)"]
fn map_meta_build_cost_at_50k_contents() {
    let dir = tempfile::tempdir().unwrap();
    let mut meta = Meta::open(&dir.path().join("meta.db"), &[7u8; 32]).unwrap();
    const N: usize = 50_000;
    let t = Instant::now();
    let mut ids = Vec::with_capacity(N);
    for i in 0..N {
        // 약 1/3 은 제목에 연도("2025년 …")
        let title = if i % 3 == 0 { format!("{}년 경마시행계획 {i}", 2015 + i % 12) } else { format!("민원 처리 결과 보고 {i}") };
        let d = ContentData { fail: None, title: Some(title), distribution: false, doc: None };
        ids.push(meta.insert_content(i as u128 + 1, &d).unwrap());
    }
    for (i, id) in ids.iter().enumerate() {
        let path = format!("D:\\문서\\부서{}\\2025\\보고서_{i}.hwp", i % 40);
        meta.upsert_file(&path, 1000, 1_700_000_000_000_000_000 + i as i64 * 86_400_000_000, *id).unwrap();
    }
    // 청크가 있는 내용(슬롯 있음) — 대부분
    let rows: Vec<(i64, usize, Option<u64>)> = ids.iter().enumerate().map(|(i, id)| (*id, 0usize, Some(i as u64))).collect();
    meta.add_chunks(&rows, N as u64).unwrap();
    println!("5만 내용 만들기(측정 대상 아님): {:.0}ms", t.elapsed().as_secs_f64() * 1000.0);

    let t0 = Instant::now();
    let texts = meta.title_texts().unwrap();
    let t_texts = t0.elapsed().as_secs_f64() * 1000.0;
    let t1 = Instant::now();
    let title_years: HashMap<i64, Vec<i32>> = texts
        .into_iter()
        .filter_map(|(id, t)| {
            let ys = query::title_years(&t);
            (!ys.is_empty()).then_some((id, ys))
        })
        .collect();
    let t_years = t1.elapsed().as_secs_f64() * 1000.0;
    let t2 = Instant::now();
    let dates: HashMap<i64, i32> = meta.newest_mtimes().unwrap().into_iter().map(|(id, ns)| (id, query::calendar::day_of_unix_ns(ns))).collect();
    let t_mtimes = t2.elapsed().as_secs_f64() * 1000.0;
    let t3 = Instant::now();
    let chunked = meta.chunked_contents().unwrap();
    let t_chunked = t3.elapsed().as_secs_f64() * 1000.0;
    let total = t0.elapsed().as_secs_f64() * 1000.0;
    println!(
        "MapMeta 만들기 내용 5만: title_texts {t_texts:.0}ms · title_years {t_years:.0}ms · newest_mtimes {t_mtimes:.0}ms · chunked_contents {t_chunked:.0}ms · 합계 {total:.0}ms"
    );
    assert_eq!(dates.len(), N);
    assert_eq!(chunked.len(), N);
    assert!(title_years.len() > N / 4, "연도 있는 제목 {}", title_years.len());
}
