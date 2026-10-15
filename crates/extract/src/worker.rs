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

// 워커 재활용(정상 워커를 내려놓고 다음 요청 때 새로 띄움). 파서(rhwp·pdfium)가 잡은 메모리가 프로세스 안에 쌓여
// 공개 평가 세트 1,000개에서 상주 워커가 490MB 까지 올랐다(문서 하나만 읽으면 최대 158MB). 늘어나는 원인은 응답 크기가 아니라
// 큰 입력 파일이다 — 그림이 많은 10~48MB HWPX 하나마다 워커가 파일 크기의 약 3배씩 늘고 돌려주지 않았다(응답은 수 KB).
// 기준별 실측(같은 세트, 워커 최대 RSS · 프로세스 수 · 색인 시간): 재활용 없음 490MB·1·4.8초, 1,000개마다 490MB(세트가 1,000개),
// 200개마다 403MB·5, 읽은 크기 합 128MB 315MB·13, **64MB 181MB·23·5.1초**, 32MB 104~156MB·38.

/// 상주 워커 하나가 이만큼 파일을 처리하면 새 프로세스로 바꾼다(작은 파일만 아주 많을 때의 안전망).
pub const RECYCLE_AFTER_FILES: u32 = 1_000;
/// 지금 프로세스가 읽은 파일 크기 합이 이만큼을 넘으면 새 프로세스로 바꾼다(주된 기준).
pub const RECYCLE_AFTER_INPUT_BYTES: u64 = 64 * 1024 * 1024;
/// 응답 줄이 이보다 크면(본문이 큰 문서) 그 응답 직후 새 프로세스로 바꾼다. 큰 응답을 읽느라 커진 앱 쪽 읽기 버퍼도 함께 사라진다.
pub const RECYCLE_AFTER_LINE_BYTES: usize = 16 * 1024 * 1024;

pub struct Worker {
    make_cmd: Box<dyn Fn() -> Command + Send>,
    running: Option<Running>,
    next_id: u64,
    /// 시간 초과·비정상 종료로 죽여서 다시 띄운 횟수
    restarts: u32,
    /// 메모리를 돌려주려고 정상 워커를 바꾼 횟수(restarts 와 따로 센다)
    recycles: u32,
    /// 지금 프로세스가 응답한 파일 수와 읽은 파일 크기 합
    served: u32,
    served_bytes: u64,
    recycle_files: u32,
    recycle_input_bytes: u64,
    recycle_line_bytes: usize,
}

impl Worker {
    pub fn new(make_cmd: impl Fn() -> Command + Send + 'static) -> Worker {
        Worker {
            make_cmd: Box::new(make_cmd),
            running: None,
            next_id: 1,
            restarts: 0,
            recycles: 0,
            served: 0,
            served_bytes: 0,
            recycle_files: RECYCLE_AFTER_FILES,
            recycle_input_bytes: RECYCLE_AFTER_INPUT_BYTES,
            recycle_line_bytes: RECYCLE_AFTER_LINE_BYTES,
        }
    }

    /// 재활용 기준을 바꾼다(시험·측정용): 파일 수, 읽은 파일 크기 합, 응답 줄 크기.
    /// 기본은 `RECYCLE_AFTER_FILES`·`RECYCLE_AFTER_INPUT_BYTES`·`RECYCLE_AFTER_LINE_BYTES`.
    pub fn recycle_after(mut self, files: u32, input_bytes: u64, line_bytes: usize) -> Worker {
        self.recycle_files = files.max(1);
        self.recycle_input_bytes = input_bytes;
        self.recycle_line_bytes = line_bytes;
        self
    }

    pub fn restarts(&self) -> u32 {
        self.restarts
    }

    pub fn recycles(&self) -> u32 {
        self.recycles
    }

    /// 지금 떠 있는 워커 프로세스 id(없으면 None — 다음 요청 때 띄운다).
    pub fn pid(&self) -> Option<u32> {
        self.running.as_ref().map(|r| r.child.id())
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
    fn stop(&mut self) -> Option<i32> {
        let mut r = self.running.take()?;
        self.served = 0;
        self.served_bytes = 0;
        let _ = r.child.kill();
        r.child.wait().ok().and_then(|s| s.code())
    }

    /// 멈추거나 죽은 워커를 버린다(restarts).
    fn discard(&mut self) -> Option<i32> {
        let code = self.stop();
        self.restarts += 1;
        code
    }

    /// 정상 응답 뒤: 정한 개수·입력 크기를 채웠거나 응답이 컸으면 워커를 내려놓는다(recycles). 다음 요청 때 새로 띄운다.
    fn after_response(&mut self, path: &Path, line_bytes: usize) {
        self.served += 1;
        self.served_bytes += std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if self.served >= self.recycle_files || self.served_bytes > self.recycle_input_bytes || line_bytes > self.recycle_line_bytes {
            self.stop();
            self.recycles += 1;
        }
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
                    Ok(resp) if resp.id == id => {
                        self.after_response(path, json.len());
                        return resp.outcome;
                    }
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
