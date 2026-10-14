//! 개발용: 파일마다 추출 커버리지(rhwp 조판 텍스트 대비)를 출력한다.
//! `cargo run --release -p extract --example coverage -- eval/public/docs/*` → "파일\t커버리지"
fn main() {
    for a in std::env::args().skip(1) {
        let bytes = std::fs::read(&a).unwrap();
        let Ok(x) = extract::hwp::extract_hwp(&bytes) else { continue };
        let c = extract::coverage::measure(&bytes, &x.doc.blocks).map_or("-".to_string(), |c| format!("{c:.3}"));
        println!("{}\t{c}", std::path::Path::new(&a).file_name().unwrap().to_string_lossy());
    }
}
