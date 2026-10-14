fn main() {
    // 시험 전용: 시간 초과 처리 확인(tests/runner.rs). 출시 실행 파일에는 이 훅이 없다(KI-009).
    if std::env::var("KRA_TEST_HANG").as_deref() == Ok("1") {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
    let mut args = std::env::args_os().skip(1);
    let path = args.next().expect("usage: extract-one <file> [--coverage]");
    let coverage = args.any(|a| a == "--coverage");
    extract::cli::extract_one_main(std::path::Path::new(&path), coverage);
}
