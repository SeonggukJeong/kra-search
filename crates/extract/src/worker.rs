//! 상주 워커: 앱이 워커 프로세스 하나를 띄워 두고 파일 경로를 한 줄씩 보낸다(스펙 §5.1).
//! 워커가 멈추거나 죽어도 앱은 영향받지 않는다 — 그 파일만 시간 초과/비정상 종료로 기록하고 워커를 다시 띄운다.
//! 응답 줄은 MARKER 로 시작한다. 파서(rhwp·pdfium)가 표준 출력에 찍는 잡음이 섞여도 표지 뒤만 읽는다.
//! 잡음은 UTF-8 이 아닐 수 있으므로 앱 쪽은 줄을 바이트로 읽고, 표지 뒤만 글자로 바꾼다.

use crate::runner::Outcome;
use crate::FailKind;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Request {
    pub id: u64,
    pub path: PathBuf,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Response {
    pub id: u64,
    pub outcome: Outcome,
}

pub const MARKER: &str = "\u{1}KRA ";
pub const MAX_LINE_BYTES: usize = 256 * 1024 * 1024;

/// 바이트 줄에서 MARKER 뒤 부분을 글자로 돌려준다(없으면 None).
fn after_marker(line: &[u8]) -> Option<String> {
    let m = MARKER.as_bytes();
    let i = line.windows(m.len()).position(|w| w == m)?;
    let tail = &line[i + m.len()..];
    let tail = tail.strip_suffix(b"\n").unwrap_or(tail);
    let tail = tail.strip_suffix(b"\r").unwrap_or(tail);
    Some(String::from_utf8_lossy(tail).into_owned())
}

/// 한 줄을 `buf` 에 읽는다(상한 `MAX_LINE_BYTES`). 상한을 넘은 줄은 끝까지 읽어 버리고 `Ok(Some(false))`.
/// 입력이 끝나면 `Ok(None)`.
fn read_line_capped(r: &mut impl BufRead, buf: &mut Vec<u8>) -> std::io::Result<Option<bool>> {
    buf.clear();
    let n = r.by_ref().take(MAX_LINE_BYTES as u64 + 1).read_until(b'\n', buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.len() > MAX_LINE_BYTES && buf.last() != Some(&b'\n') {
        // 너무 긴 줄 — 줄바꿈까지 버린다
        let mut sink = Vec::new();
        loop {
            sink.clear();
            let k = r.by_ref().take(1 << 20).read_until(b'\n', &mut sink)?;
            if k == 0 || sink.last() == Some(&b'\n') {
                break;
            }
        }
        buf.clear();
        return Ok(Some(false));
    }
    Ok(Some(true))
}

pub fn serve(input: impl BufRead, mut output: impl Write, extract: impl Fn(&Path) -> Outcome) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        let Ok(req) = serde_json::from_str::<Request>(&line) else { continue };
        let outcome = extract(&req.path);
        let json = serde_json::to_string(&Response { id: req.id, outcome }).expect("응답 직렬화");
        // 앞에 줄바꿈을 넣어 파서가 남긴 미완성 줄과 섞이지 않게 한다.
        writeln!(output, "\n{MARKER}{json}")?;
        output.flush()?;
    }
    Ok(())
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
}

pub struct Worker {
    make_cmd: Box<dyn Fn() -> Command + Send>,
    running: Option<Running>,
    next_id: u64,
    restarts: u32,
}

impl Worker {
    pub fn new(make_cmd: impl Fn() -> Command + Send + 'static) -> Worker {
        Worker { make_cmd: Box::new(make_cmd), running: None, next_id: 1, restarts: 0 }
    }

    pub fn restarts(&self) -> u32 {
        self.restarts
    }

    fn spawn(&self) -> std::io::Result<Running> {
        let mut cmd = (self.make_cmd)();
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            let mut buf = Vec::new();
            // 입력이 끝나거나(워커 종료) 읽기 오류일 때만 끝낸다. UTF-8 이 아닌 잡음은 그 줄만 무시된다.
            while let Ok(Some(ok)) = read_line_capped(&mut r, &mut buf) {
                if !ok {
                    continue;
                }
                if let Some(json) = after_marker(&buf) {
                    if tx.send(json).is_err() {
                        break;
                    }
                }
            }
        });
        Ok(Running { child, stdin, rx })
    }

    /// 지금 워커를 죽이고 다음 요청 때 새로 띄우게 한다.
    fn discard(&mut self) -> Option<i32> {
        let mut r = self.running.take()?;
        let _ = r.child.kill();
        let code = r.child.wait().ok().and_then(|s| s.code());
        self.restarts += 1;
        code
    }

    pub fn extract(&mut self, path: &Path, timeout: Duration) -> Outcome {
        let id = self.next_id;
        self.next_id += 1;
        // UTF-8 이 아닌 경로는 JSON 으로 보낼 수 없다 — 앱을 멈추지 않고 그 파일만 읽을 수 없음으로.
        let Ok(req) = serde_json::to_string(&Request { id, path: path.to_path_buf() }) else {
            return Outcome::Failed { kind: FailKind::Unreadable, magic: String::new() };
        };
        // 쓰기가 실패하면(이전에 죽은 워커) 한 번 다시 띄워 보낸다.
        let mut sent = false;
        for _ in 0..2 {
            if self.running.is_none() {
                match self.spawn() {
                    Ok(r) => self.running = Some(r),
                    Err(_) => return Outcome::Crashed { code: None },
                }
            }
            let r = self.running.as_mut().expect("running");
            if writeln!(r.stdin, "{req}").and_then(|_| r.stdin.flush()).is_ok() {
                sent = true;
                break;
            }
            self.discard();
        }
        if !sent {
            return Outcome::Crashed { code: None };
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let r = self.running.as_mut().expect("running");
            match r.rx.recv_timeout(left) {
                Ok(json) => match serde_json::from_str::<Response>(&json) {
                    Ok(resp) if resp.id == id => return resp.outcome,
                    _ => continue, // 이전 요청의 늦은 응답·깨진 줄은 버린다
                },
                Err(RecvTimeoutError::Timeout) => {
                    self.discard();
                    return Outcome::Timeout;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let code = self.discard();
                    return Outcome::Crashed { code };
                }
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(mut r) = self.running.take() {
            let _ = r.child.kill();
            let _ = r.child.wait();
        }
    }
}
