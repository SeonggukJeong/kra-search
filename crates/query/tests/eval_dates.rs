//! eval/*/queries.tsv 의 date 질의: 5번째 열(기간)과 해석 결과가 같아야 하고, 6번째 열 `recurring` 은 yearless 표현에만 있어야 한다(기준일 2026-10-15).

use query::calendar::{format_ymd, parse_ymd};
use query::DateTable;
use std::path::Path;

#[test]
fn eval_date_queries_parse_to_their_range() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../eval");
    let today = parse_ymd("2026-10-15").unwrap();
    let t = DateTable::builtin();
    let mut n = 0;
    for set in ["public", "kra"] {
        let text = std::fs::read_to_string(root.join(set).join("queries.tsv")).unwrap();
        for line in text.lines().skip(1) {
            let c: Vec<&str> = line.split('\t').collect();
            if c.len() < 5 || c[2] != "date" {
                continue;
            }
            n += 1;
            let spans = t.find(c[1], today);
            if c[4] == "future" {
                assert!(spans.iter().all(|s| s.range.is_none()) && !spans.is_empty(), "{}: {:?}", c[0], spans);
                continue;
            }
            let chips: Vec<_> = spans.iter().filter(|s| s.range.is_some()).collect();
            assert_eq!(chips.len(), 1, "{} '{}': {:?}", c[0], c[1], spans);
            let r = chips[0].range.unwrap();
            let got = format!("{}..{}", format_ymd(r.start), format_ymd(r.end));
            assert_eq!(got, c[4], "{} '{}'", c[0], c[1]);
            // 6번째 열 `recurring`(해마다 같은 시기의 문서도 정답) = 해석기의 yearless.
            assert_eq!(chips[0].yearless, c.get(5) == Some(&"recurring"), "{} '{}': recurring 표시와 yearless 가 다름", c[0], c[1]);
        }
    }
    assert_eq!(n, 30, "date 질의 30개(Task 3)");
}
