use owo_colors::OwoColorize;

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("create tokio runtime");
    if let Err(err) = runtime.block_on(Box::pin(codex_helper::run_cli())) {
        eprintln!("{}", err.to_string().red());
        std::process::exit(1);
    }
}
