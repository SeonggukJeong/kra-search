//! 추출을 별도 프로세스에서 실행해 파서의 멈춤·비정상 종료를 격리한다.

use crate::{FailKind, Stats};
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
        #[serde(default)]
        stats: Stats,
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

/// 자식 표준 출력에서 마지막 표지(MARKER) 뒤의 JSON 을 읽는다. 표지가 없으면 출력 전체(옛 형식).
/// 잘못된 UTF-8 잡음은 대체 문자로 바꿔 읽는다(읽기 실패로 "비정상 종료"가 되지 않게).
pub fn parse_child_output(out: &[u8]) -> Option<Outcome> {
    let s = String::from_utf8_lossy(out);
    let m = crate::worker::MARKER;
    let tail = s.rfind(m).map(|i| &s[i + m.len()..]).unwrap_or(&s);
    serde_json::from_str(tail.trim()).ok()
}

/// 임의의 자식 명령을 실행하고 stdout 의 표지 뒤 `Outcome` JSON 을 읽는다(표지가 없으면 한 줄 JSON). 시간 초과면 종료시킨다.
pub fn run_command(mut cmd: Command, timeout: Duration) -> Outcome {
    let mut child = match cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(c) => c,
        Err(_) => return Outcome::Crashed { code: None },
    };
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stdout.read_to_end(&mut b);
        b
    });
    match child.wait_timeout(timeout) {
        Ok(Some(status)) => {
            let out = reader.join().unwrap_or_default();
            parse_child_output(&out).unwrap_or(Outcome::Crashed { code: status.code() })
        }
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            Outcome::Timeout
        }
    }
}
