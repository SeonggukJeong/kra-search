use extract::hwp::FailKind;
use extract::runner::{run_command, run_isolated, Outcome};
use std::path::PathBuf;
use std::time::Duration;

fn exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_extract-one"))
}
fn fx(name: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/")).join(name)
}
fn ok_doc(name: &str) -> (doc::Document, bool) {
    match run_isolated(&exe(), &fx(name), Duration::from_secs(30)) {
        Outcome::Ok { doc, distribution } => (doc, distribution),
        other => panic!("{name}: {other:?}"),
    }
}

#[test]
fn normal_hwp_yields_text() {
    for name in ["schedule.hwp", "schedule.hwpx", "form.hwp", "table-text.hwpx"] {
        let (d, _) = ok_doc(name);
        assert!(d.blocks.iter().map(|b| b.text.len()).sum::<usize>() > 0, "{name} empty");
    }
}

#[test]
fn tables_become_header_and_rows() {
    let (d, _) = ok_doc("table-text.hwpx");
    assert!(d.blocks.iter().any(|b| b.kind == doc::BlockKind::TableHeader), "{:?}", d.blocks);
    assert!(d.blocks.iter().any(|b| b.kind == doc::BlockKind::TableRow && b.text.contains(": ")));
}

#[test]
fn distribution_flag_detected() {
    let (d, dist) = ok_doc("distribution.hwpx");
    assert!(dist);
    assert!(!d.blocks.is_empty());
}

#[test]
fn corrupt_file_classified() {
    match run_isolated(&exe(), &fx("corrupt.hwp"), Duration::from_secs(30)) {
        Outcome::Failed { kind, .. } => assert!(matches!(kind, FailKind::Corrupt | FailKind::Unsupported)),
        Outcome::Crashed { .. } => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn non_container_reports_magic() {
    match run_isolated(&exe(), &fx("not_container.hwp"), Duration::from_secs(30)) {
        Outcome::Failed { kind: FailKind::NotContainer, magic } => assert_eq!(magic, "4641534f4f2d5445"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn fasoo_drm_detected() {
    match run_isolated(&exe(), &fx("drm_fasoo.hwp"), Duration::from_secs(30)) {
        Outcome::Failed { kind: FailKind::Drm, .. } => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn hang_is_killed_by_timeout() {
    // 환경변수는 자식 프로세스에만 준다(병렬 테스트에 영향 없음)
    let mut cmd = std::process::Command::new(exe());
    cmd.arg(fx("corrupt.hwp")).env("KRA_TEST_HANG", "1");
    let r = run_command(cmd, Duration::from_secs(2));
    assert!(matches!(r, Outcome::Timeout), "{r:?}");
}
