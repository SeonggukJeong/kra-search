//! 시험용 상주 워커. 파일 이름 `__hang__`/`__crash__`/`__noise__`/`__binnoise__` 로 멈춤·비정상 종료·표준 출력 잡음(UTF-8 이 아닌 바이트 포함)을 흉내 낸다.
//! 제품 워커는 `kra-search worker`(app 크레이트)이며 이런 훅이 없다.

use std::io::Write;
use std::path::Path;

fn main() {
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        if a == "--pdfium" {
            let p = args.next().expect("--pdfium <경로>");
            extract::pdf::init(Path::new(&p)).expect("pdfium");
        }
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    extract::worker::serve(stdin.lock(), stdout.lock(), |p| {
        match p.file_name().and_then(|n| n.to_str()) {
            Some("__hang__") => std::thread::sleep(std::time::Duration::from_secs(600)),
            Some("__crash__") => std::process::abort(),
            Some("__noise__") => {
                // 파서가 줄바꿈 없이 표준 출력에 찍는 경우를 흉내
                print!("garbage from a parser");
                let _ = std::io::stdout().flush();
            }
            Some("__binnoise__") => {
                // C 라이브러리(pdfium 등)가 UTF-8 이 아닌 바이트를 줄 단위로 찍는 경우를 흉내
                let mut o = std::io::stdout();
                let _ = o.write_all(b"\xff\xfe junk\n\x80\x81 more");
                let _ = o.flush();
            }
            _ => {}
        }
        extract::cli::extract_one(p)
    })
    .expect("serve");
}
