fn main() {
    let mut args = std::env::args_os().skip(1);
    let path = args.next().expect("usage: extract-one <file> [--coverage]");
    let coverage = args.any(|a| a == "--coverage");
    extract::cli::extract_one_main(std::path::Path::new(&path), coverage);
}
