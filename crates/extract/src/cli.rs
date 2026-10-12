//! 추출 자식 프로세스의 본체. `extract-one` 바이너리와 `kra-diag extract-one` 이 함께 쓴다.

use crate::hwp::{extract_hwp, FailKind};
use crate::magic::magic_hex;
use crate::runner::Outcome;
use std::path::Path;

pub fn extract_one(path: &Path) -> Outcome {
    if std::env::var("KRA_TEST_HANG").as_deref() == Ok("1") {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
    match std::fs::read(path) {
        Err(_) => Outcome::Failed { kind: FailKind::Corrupt, magic: String::new() },
        Ok(bytes) => match std::panic::catch_unwind(|| extract_hwp(&bytes)) {
            Ok(Ok(x)) => Outcome::Ok { doc: x.doc, distribution: x.distribution },
            Ok(Err((kind, magic))) => Outcome::Failed { kind, magic },
            Err(_) => Outcome::Failed { kind: FailKind::Corrupt, magic: magic_hex(&bytes) },
        },
    }
}

/// 결과를 stdout 한 줄 JSON으로 출력한다.
pub fn extract_one_main(path: &Path) {
    println!("{}", serde_json::to_string(&extract_one(path)).unwrap());
}
