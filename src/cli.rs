use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};

use crate::comment;
use crate::config::{Config, Setting};
use crate::execute::execute;
use crate::exit_code_map::ExitCodeMap;
use crate::github_env::GithubEnv;
use crate::render::{self, GithubView, Renderer, RunInfo, Target};
use crate::state::{State, StepSpec};
use crate::status::Status;
use crate::store::{CommentStore, FileStore, GithubStore};

const STATUSES_HELP: &str = "\
Built-in Statuses:
  pending    Not started: new Steps from `steps set`, and every Step after `steps reset`
  running    In progress: set by `exec` while the command runs
  success    Finished cleanly: exit code 0 by default, and `step` by default
  failure    Finished with an error: any exit code not in --exit-code-map  [Failing]
  skipped    Never ran: `finalize` turns pending into skipped
  cancelled  Interrupted: `finalize` turns running into cancelled  [Failing]

Any other value (e.g. `changes`) is a Custom Status: never Failing, rendered by the Template.";

/// Keeps a pull request comment up to date with the Steps of a GitHub Actions job.
#[derive(Debug, Parser)]
#[command(
    version,
    about,
    arg_required_else_help = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Directory for config.json, and comment.md when there's no PR [default: $RUNNER_TEMP/status-comment; `.` in Local Mode]
    #[arg(long, global = true)]
    pub state_path: Option<PathBuf>,

    /// GitHub token [default: $GITHUB_TOKEN]
    #[arg(long, global = true)]
    pub token: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Set or reset the Run's Steps.
    #[command(subcommand)]
    Steps(StepsCommand),

    /// Set a Step's Status. Piped stdin becomes its Output.
    #[command(after_help = STATUSES_HELP, arg_required_else_help = true)]
    Step {
        /// The Step's name.
        name: String,
        /// The Status to set.
        #[arg(long, default_value = "success")]
        status: Status,
    },

    /// Mark a Step running, run a command under `bash -o pipefail`, and set its
    /// Status from the exit code.
    #[command(after_help = STATUSES_HELP, arg_required_else_help = true)]
    Exec {
        /// The Step's name.
        name: String,
        /// The command, as one string; pipes and `&&` work.
        command: String,
        /// Exit code → Status, e.g. `0=success,2=changes`. Unmapped codes are `failure`.
        #[arg(long, default_value = "0=success")]
        exit_code_map: ExitCodeMap,
    },

    /// Close out the Run: pending → skipped, running → cancelled; write the job summary.
    Finalize,

    /// Change or list the config. Changes print the result.
    #[command(arg_required_else_help = true)]
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommand>,
        /// Set a value: `template=<file>` or `comment-key=<key>`. Repeatable.
        #[arg(long, value_name = "KEY=VALUE")]
        set: Vec<Setting>,
        /// Replace the config with this JSON file (applied before --set).
        #[arg(long, value_name = "FILE")]
        load: Option<PathBuf>,
    },

    /// Print the stored comment, or the job summary, without changing anything.
    Preview {
        /// `comment`: the stored comment exactly as saved. `summary`: the job summary it would produce.
        #[arg(long, value_enum, default_value = "comment")]
        target: PreviewTarget,
    },

    /// Print the built-in Template, as a starting point for your own.
    DefaultTemplate,
}

#[derive(Debug, Subcommand)]
pub enum StepsCommand {
    /// Set the Run's Steps, in order. Unlisted Steps are removed; listed ones keep
    /// their Status; new ones start pending.
    Set {
        /// `name` or `name=Display Name`.
        #[arg(required = true, value_name = "STEP")]
        steps: Vec<StepSpec>,
    },
    /// Set every Step back to pending and clear its Output.
    Reset,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the current config.
    List,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum PreviewTarget {
    Comment,
    Summary,
}

fn warn(message: impl std::fmt::Display) {
    // `{:#}` flattens anyhow's context chain onto one annotation line.
    println!(
        "::warning::status-comment: {}",
        format!("{message:#}").replace('\n', " ")
    );
}

/// Holds config.json, and comment.md when there's no PR.
fn state_dir(env: &GithubEnv, cli: &Cli) -> PathBuf {
    match &cli.state_path {
        Some(path) => path.clone(),
        None if env.on_github => env.runner_temp.join("status-comment"),
        None => PathBuf::from("."),
    }
}

fn open_store(env: &GithubEnv, cli: &Cli, key: &str) -> Box<dyn CommentStore> {
    let file_store = || {
        Box::new(FileStore {
            path: state_dir(env, cli).join("comment.md"),
            key: key.to_string(),
        })
    };
    if !env.on_github {
        return file_store();
    }
    let token = cli.token.clone().or_else(|| env.token.clone());
    match (&env.pull_request, &env.repository, token) {
        (Some(pr), Some(repo), Some(token)) => {
            return Box::new(GithubStore::new(&env.api_url, repo, pr.number, &token, key));
        }
        (Some(_), _, None) => {
            warn("no GitHub token (set GITHUB_TOKEN or pass --token); skipping the Status Comment")
        }
        _ => {}
    }
    file_store()
}

fn load_template(config: &Config) -> String {
    let Some(path) = &config.template else {
        return render::DEFAULT_TEMPLATE.to_string();
    };
    fs::read_to_string(path).unwrap_or_else(|e| {
        warn(format!(
            "could not read Template {}: {e}; using the default",
            path.display()
        ));
        render::DEFAULT_TEMPLATE.to_string()
    })
}

/// Local Mode has no run ID, so a Run ends at Finalize: the next call that
/// sets or records Steps starts a new one.
fn local_run_id(previous: Option<&State>, command: &Command) -> String {
    let starts_steps = matches!(
        command,
        Command::Steps(StepsCommand::Set { .. }) | Command::Step { .. } | Command::Exec { .. }
    );
    match previous {
        Some(state) if state.finalized && starts_steps => {
            let n: u64 = state
                .run_id
                .strip_prefix("local-")
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            format!("local-{}", n + 1)
        }
        Some(state) => state.run_id.clone(),
        None => "local-1".into(),
    }
}

fn append_line(path: &Path, line: &str) -> Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// Runs one invocation and returns the process exit code.
pub fn run(cli: Cli, env: &GithubEnv, stdin: Option<String>) -> Result<i32> {
    let config_path = state_dir(env, &cli).join("config.json");
    match &cli.command {
        Command::DefaultTemplate => {
            print!("{}", render::DEFAULT_TEMPLATE);
            return Ok(0);
        }
        Command::Config { set, load, .. } => {
            let mut config = match load {
                Some(path) => Config::from_file(path)?,
                None => Config::load(&config_path)?,
            };
            for setting in set {
                config.set(setting)?;
            }
            if load.is_some() || !set.is_empty() {
                config.save(&config_path)?;
            }
            println!("{}", serde_json::to_string_pretty(&config)?);
            return Ok(0);
        }
        _ => {}
    }

    let config = Config::load(&config_path)?;
    let default_key = if env.on_github {
        env.default_comment_key()
    } else {
        Some("local".to_string())
    };
    let Some(key) = config.comment_key.clone().or(default_key) else {
        bail!(
            "cannot work out a Comment Key (GITHUB_WORKFLOW or GITHUB_JOB is unset); set one with `config --set comment-key=<key>`"
        );
    };
    let template = load_template(&config);
    let github = github_view(env);
    let run_info = RunInfo {
        url: env.run_url().filter(|_| env.on_github),
        attempt: env.run_attempt.clone().unwrap_or_else(|| "1".into()),
    };
    let renderer = Renderer {
        template: &template,
        github: &github,
        run: &run_info,
    };

    let mut store = open_store(env, &cli, &key);
    // Writing a comment we couldn't read would wipe it.
    let (body, mut comment_ok) = match store.load() {
        Ok(body) => (body, true),
        Err(e) => {
            warn(e.context("could not read the Status Comment; it will not be updated"));
            (None, false)
        }
    };
    let previous = body.as_deref().and_then(comment::load);

    if let Command::Preview { target } = cli.command {
        match target {
            // The whole stored comment, hidden markers included.
            PreviewTarget::Comment => match body {
                Some(body) => print!("{body}"),
                None => eprintln!("status-comment: no Status Comment yet"),
            },
            PreviewTarget::Summary => {
                let state = previous.unwrap_or_default();
                print!(
                    "{}",
                    renderer.render(&state, Target::Summary, render::SUMMARY_LIMIT, 0)?
                );
            }
        }
        return Ok(0);
    }

    let run_id = if env.on_github {
        env.run_id.clone().unwrap_or_else(|| "unknown".into())
    } else {
        local_run_id(previous.as_ref(), &cli.command)
    };
    let mut state = State::for_run(previous, &run_id);
    let mut publish = |state: &State| -> Result<()> {
        if !comment_ok {
            return Ok(());
        }
        let reserve = comment::compose("", state, &key).len();
        let body = renderer.render(state, Target::Comment, render::COMMENT_LIMIT, reserve)?;
        if let Err(e) = store.save(&comment::compose(&body, state, &key)) {
            warn(e.context("could not update the Status Comment"));
            comment_ok = false;
        }
        Ok(())
    };

    let mut exit_code = 0;
    let recorded = match &cli.command {
        Command::Steps(StepsCommand::Set { steps }) => {
            state.set_steps(steps);
            None
        }
        Command::Steps(StepsCommand::Reset) => {
            state.reset();
            None
        }
        Command::Finalize => {
            state.finalize();
            None
        }
        Command::Step { name, status } => {
            let output = stdin.filter(|s| !s.is_empty());
            state.record(name, status.clone(), output);
            Some((name, status.clone()))
        }
        Command::Exec {
            name,
            command,
            exit_code_map,
        } => {
            state.record(name, Status::Running, None);
            publish(&state)?;
            let executed = execute(command, &mut std::io::stdout())?;
            let status = exit_code_map
                .get(executed.exit_code)
                .cloned()
                .unwrap_or(Status::Failure);
            if status.is_failing() {
                exit_code = if executed.exit_code == 0 {
                    1
                } else {
                    executed.exit_code
                };
            }
            state.record(name, status.clone(), Some(executed.output));
            Some((name, status))
        }
        Command::Config { .. } | Command::Preview { .. } | Command::DefaultTemplate => {
            unreachable!("handled above")
        }
    };
    if let Some((name, status)) = recorded {
        eprintln!("status-comment: {name} → {status}");
        if let Some(path) = &env.github_output {
            append_line(path, &format!("status={status}")).context("writing to GITHUB_OUTPUT")?;
        }
    }
    publish(&state)?;

    if !env.on_github {
        let preview = renderer.render(&state, Target::Comment, render::COMMENT_LIMIT, 0)?;
        eprintln!(
            "\n── status-comment preview (Local Mode, {}) ──\n{}\n── end preview ──",
            state_dir(env, &cli).join("comment.md").display(),
            preview.trim_end()
        );
    }

    if matches!(cli.command, Command::Finalize)
        && let Some(path) = &env.step_summary
    {
        let summary = renderer.render(&state, Target::Summary, render::SUMMARY_LIMIT, 0)?;
        fs::write(path, summary).context("writing the job summary")?;
    }

    Ok(exit_code)
}

fn github_view(env: &GithubEnv) -> GithubView {
    let sha = env.sha.clone().unwrap_or_default();
    GithubView {
        workflow: env.workflow.clone().unwrap_or_else(|| {
            if env.on_github {
                String::new()
            } else {
                "Local run".into()
            }
        }),
        job: env.job.clone().unwrap_or_default(),
        actor: env.actor.clone().unwrap_or_default(),
        repository: env.repository.clone().unwrap_or_default(),
        short_sha: sha.chars().take(7).collect(),
        sha,
        pr_number: env.pull_request.as_ref().map(|pr| pr.number),
        pr_url: env.pull_request.as_ref().and_then(|pr| pr.html_url.clone()),
        server_url: env.server_url.clone(),
        updated_at: humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string(),
    }
}
