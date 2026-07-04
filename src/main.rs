use std::path::PathBuf;
use std::process::ExitCode;

use tx_processor::engine::PaymentsEngine;
use tx_processor::io;

fn main() -> ExitCode {
    // stderr-only logging; default to `info` so skipped rows are visible.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Stderr)
        .init();

    let path = match parse_args() {
        Ok(path) => path,
        Err(msg) => {
            log::error!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = run(&path) {
        log::error!("failed to process {}: {e}", path.display());
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn parse_args() -> Result<PathBuf, String> {
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .ok_or("missing input file argument (usage: tx-processor <input.csv>)")?;
    if args.next().is_some() {
        return Err("too many arguments (usage: tx-processor <input.csv>)".to_string());
    }
    Ok(PathBuf::from(path))
}

fn run(path: &std::path::Path) -> std::io::Result<()> {
    let mut engine = PaymentsEngine::new();

    for record in io::read_transactions_from_path(path)? {
        match record {
            Ok(tx) => {
                if let Err(e) = engine.process(tx) {
                    log::warn!("skipping row: {e}");
                }
            }
            Err(e) => log::warn!("skipping malformed row: {e}"),
        }
    }

    let stdout = std::io::stdout();
    io::write_accounts(stdout.lock(), engine.accounts().iter())
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(())
}
