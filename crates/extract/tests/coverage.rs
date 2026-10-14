//! 추출 누락 회귀 방지: rhwp 페이지 텍스트(조판 결과)에 있는 글자가 우리 블록에 빠짐없이 들어 있는지.
//! rhwp 를 올리거나 walk.rs 를 고친 뒤 이 테스트가 떨어지면 docs/engineering/incidents.md INC-011 을 본다.
use extract::coverage::measure;
use extract::hwp::extract_hwp;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/").to_string() + name).unwrap()
}

#[test]
fn fixtures_are_fully_covered() {
    for name in [
        "schedule.hwp",
        "schedule.hwpx",
        "form.hwp",
        "table-text.hwpx",
        "distribution.hwpx",
        "notice_clauses.hwpx",
        "press_binary.hwp",
        "press_textbox.hwpx",
    ] {
        let bytes = fixture(name);
        let doc = extract_hwp(&bytes).unwrap_or_else(|e| panic!("{name}: {e:?}")).doc;
        let c = measure(&bytes, &doc.blocks).unwrap_or_else(|| panic!("{name}: no page text"));
        assert!(c >= 0.98, "{name}: coverage {c:.3}");
    }
}

#[test]
fn dropped_blocks_lower_coverage() {
    let bytes = fixture("schedule.hwp");
    let doc = extract_hwp(&bytes).unwrap().doc;
    let half = &doc.blocks[..doc.blocks.len() / 2];
    let c = measure(&bytes, half).unwrap();
    assert!(c < 0.9, "coverage {c:.3} should drop when half the blocks are missing");
}
