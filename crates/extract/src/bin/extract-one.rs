fn main() {
    let path = std::env::args_os().nth(1).expect("usage: extract-one <file>");
    extract::cli::extract_one_main(std::path::Path::new(&path));
}
