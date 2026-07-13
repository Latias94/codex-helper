use std::io::{self, Read};

use codex_helper_desktop_contract_schema::{ExtractRequest, extract_contract_schema};

fn main() {
    if let Err(error) = run() {
        eprintln!("desktop contract schema extraction failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|error| format!("failed to read extraction request: {error}"))?;
    let request: ExtractRequest = serde_json::from_str(&input)
        .map_err(|error| format!("invalid extraction request: {error}"))?;
    let response = extract_contract_schema(request)?;
    serde_json::to_writer(io::stdout(), &response)
        .map_err(|error| format!("failed to write extraction response: {error}"))?;
    Ok(())
}
