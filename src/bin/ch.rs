use owo_colors::OwoColorize;

const CH_CLI_STACK_SIZE: usize = 8 * 1024 * 1024;

fn main() {
    let result = std::thread::Builder::new()
        .name("ch-cli".to_string())
        .stack_size(CH_CLI_STACK_SIZE)
        .spawn(run_ch_cli)
        .expect("spawn ch CLI thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));

    if let Err(err) = result {
        eprintln!("{}", err.to_string().red());
        std::process::exit(1);
    }
}

fn run_ch_cli() -> codex_helper::CliResult<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("create tokio runtime");
    runtime.block_on(Box::pin(codex_helper::run_ch_cli()))
}
