fn main() {
    if let Err(error) = codex_usage::cli::run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
