use owo_colors::OwoColorize;

#[tokio::main]
async fn main() {
    if let Err(err) = codex_helper::run_cli().await {
        eprintln!("{}", err.to_string().red());
        std::process::exit(1);
    }
}
