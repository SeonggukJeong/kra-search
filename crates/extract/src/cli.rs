//! 파일 하나 추출. `extract-one` 바이너리, `kra-diag extract-one`, 상주 워커(worker.rs)가 함께 쓴다.

use crate::format::{Format, MAX_FILE_BYTES};
use crate::magic::magic_hex;
use crate::runner::Outcome;
use crate::FailKind;
use std::path::Path;

pub fn extract_one(path: &Path) -> Outcome {
    extract_one_with(path, false)
}

fn failed(kind: FailKind) -> Outcome {
    Outcome::Failed { kind, magic: String::new() }
}

/// `measure_coverage` 면 조판 텍스트 대비 커버리지도 잰다(HWP 만, 느림 — 진단·평가 전용).
pub fn extract_one_with(path: &Path, measure_coverage: bool) -> Outcome {
    let Some(format) = Format::from_path(path) else { return failed(FailKind::Unsupported) };
    match std::fs::metadata(path) {
        Err(_) => return failed(FailKind::Unreadable),
        Ok(m) if m.len() > MAX_FILE_BYTES => return failed(FailKind::TooLarge),
        Ok(_) => {}
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return failed(FailKind::Unreadable),
    };
    match std::panic::catch_unwind(|| crate::extract_bytes(format, &bytes)) {
        Ok(Ok(x)) => {
            let hwp = matches!(format, Format::Hwp | Format::Hwpx);
            let coverage = if measure_coverage && hwp { crate::coverage::measure(&bytes, &x.doc.blocks) } else { None };
            Outcome::Ok { doc: x.doc, distribution: x.distribution, coverage, stats: x.stats }
        }
        Ok(Err((kind, magic))) => Outcome::Failed { kind, magic },
        Err(_) => Outcome::Failed { kind: FailKind::Corrupt, magic: magic_hex(&bytes) },
    }
}

/// 결과를 stdout 에 "\n{MARKER}{json}" 한 줄로 출력한다. 파서가 표준 출력에 찍은 잡음과 구분하려고 워커와 같은 표지를 붙인다.
pub fn extract_one_main(path: &Path, measure_coverage: bool) {
    let json = serde_json::to_string(&extract_one_with(path, measure_coverage)).unwrap();
    println!("\n{}{json}", crate::worker::MARKER);
}
