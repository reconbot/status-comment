mod cli;
mod comment;
mod config;
mod env;
mod execute;
mod exit_code_map;
mod render;
mod state;
mod status;
mod store;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    match cli::run(cli::Cli::parse(), &env::Env::from_process()) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(e) => {
            eprintln!("status-comment: {e:#}");
            ExitCode::FAILURE
        }
    }
}
