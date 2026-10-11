mod app;
mod comment;
mod execute;
mod exit_code_map;
mod github_env;
mod render;
mod state;
mod status;
mod store;

use std::io::{IsTerminal, Read};
use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = app::Cli::parse();
    let env = github_env::GithubEnv::from_process();

    // Piped mode: Output comes from stdin. `--execute` never reads it.
    let stdin =
        if cli.step_name.is_some() && cli.execute.is_none() && !std::io::stdin().is_terminal() {
            let mut input = String::new();
            match std::io::stdin().read_to_string(&mut input) {
                Ok(_) => Some(input),
                Err(e) => {
                    eprintln!("status-comment: failed to read stdin: {e}");
                    return ExitCode::FAILURE;
                }
            }
        } else {
            None
        };

    match app::run(cli, &env, stdin) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(e) => {
            eprintln!("status-comment: {e:#}");
            ExitCode::FAILURE
        }
    }
}
