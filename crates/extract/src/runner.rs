//! 추출을 별도 프로세스에서 실행해 파서의 멈춤·비정상 종료를 격리한다.

use crate::hwp::FailKind;
use doc::Document;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Outcome {
    /// `coverage`: 추출 커버리지(0~1, `extract::coverage`) — 요청했을 때만.
    Ok {
        doc: Document,
        distribution: bool,
        #[serde(default)]
        coverage: Option<f32>,
    },
    Failed { kind: FailKind, magic: String },
    Timeout,
    Crashed { code: Option<i32> },
}

/// `exe file` 을 실행한다(`extract-one` 바이너리, 커버리지 측정 없음).
pub fn run_isolated(exe: &Path, file: &Path, timeout: Duration) -> Outcome {
    let mut cmd = Command::new(exe);
    cmd.arg(file);
    run_command(cmd, timeout)
}

/// 임의의 자식 명령을 실행하고 stdout 한 줄의 `Outcome` JSON을 읽는다. 시간 초과면 종료시킨다.
pub fn run_command(mut cmd: Command, timeout: Duration) -> Outcome {
    let mut child = match cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(c) => c,
        Err(_) => return Outcome::Crashed { code: None },
    };
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    match child.wait_timeout(timeout) {
        Ok(Some(status)) => {
            let out = reader.join().unwrap_or_default();
            serde_json::from_str(out.trim()).unwrap_or(Outcome::Crashed { code: status.code() })
        }
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            Outcome::Timeout
        }
    }
}
