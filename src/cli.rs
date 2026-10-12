use std::fs::{self, OpenOptions};
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};

use crate::comment;
use crate::config::{Config, Setting};
use crate::env::Env;
use crate::execute::execute;
use crate::exit_code_map::ExitCodeMap;
use crate::render::{self, GithubView, Renderer, Target};
use crate::state::State;
use crate::status::Status;
use crate::store::{CommentStore, FileStore, GithubStore};

const STATUSES_HELP: &str = "\
Built-in Statuses:
  pending    Not started: new Steps from `steps set`, and every Step after `steps --reset-status`
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
    /// Set the Run's Steps, or reset their Statuses.
    #[command(arg_required_else_help = true)]
    Steps {
        /// Set every Step back to pending and clear its Output (after `set`, when both are given).
        #[arg(long, global = true)]
        reset_status: bool,
        #[command(subcommand)]
        command: Option<StepsCommand>,
    },

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

    /// List or change the config.
    #[command(
        subcommand,
        after_help = "\
Defaults, as the equivalent commands:
  config set template=default
  config set comment-key=\"$GITHUB_WORKFLOW/$GITHUB_JOB\"   (on GitHub)
  config set comment-key=local                           (in Local Mode)
  config set status.success=\"✅ Success\"                  (and so on for each Built-in Status)"
    )]
    Config(ConfigCommand),

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
        /// Step names. Display names come from `config set step.<name>=…`.
        #[arg(required = true, value_name = "STEP")]
        steps: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the current config.
    List,
    /// Set values and print the result: `template=<file|default>`, `comment-key=<key>`,
    /// `status.<status>=<label>`, `step.<step>=<display name>`.
    Set {
        #[arg(required = true, value_name = "KEY=VALUE")]
        settings: Vec<Setting>,
    },
    /// Replace the config with this JSON file and print the result. Relative
    /// `template` paths resolve against the file's directory.
    Load { file: PathBuf },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum PreviewTarget {
    Comment,
    Summary,
}

/// Runs one invocation and returns the process exit code. Errors in the
/// invocation itself (usage, config) fail it; once the Status Comment is being
/// read or written, an error is reported and ends that work, but leaves the
/// exit code alone.
pub fn run(cli: Cli, env: &Env) -> Result<i32> {
    let state_dir = cli.state_path.unwrap_or_else(|| env.state_dir.clone());
    let open = || -> Result<StatusComment> {
        let config = prepare_config(env, &state_dir, None)?;
        StatusComment::open(env, config, &state_dir, cli.token.clone())
    };
    let command = cli.command;
    match &command {
        Command::DefaultTemplate => print!("{}", render::DEFAULT_TEMPLATE),
        Command::Config(config_command) => {
            let config = prepare_config(env, &state_dir, Some(config_command))?;
            println!("{}", serde_json::to_string_pretty(&config)?);
        }
        Command::Preview { target } => report(env, open()?.preview(*target)),
        Command::Steps {
            reset_status,
            command: set,
        } => report(
            env,
            apply(open()?, &command, None, |state| {
                if let Some(StepsCommand::Set { steps }) = set {
                    state.set_steps(steps);
                }
                if *reset_status {
                    state.reset();
                }
            }),
        ),
        Command::Finalize => report(env, apply(open()?, &command, None, State::finalize)),
        Command::Step { name, status } => {
            let output = piped_stdin()?;
            report(
                env,
                apply(open()?, &command, Some((name, status)), |state| {
                    state.record(name, status.clone(), output)
                }),
            );
        }
        Command::Exec {
            name,
            command: script,
            exit_code_map,
        } => {
            let comment = open()?;
            // Show the Step running first. A failure ends the comment work,
            // never the command.
            let running = comment.begin(&command).and_then(|update| {
                let Some(mut update) = update else {
                    return Ok(None);
                };
                update.state.record(name, Status::Running, None);
                update.save()?;
                Ok(Some(update))
            });
            let running = running.unwrap_or_else(|e| {
                env.error(e);
                None
            });
            let executed = execute(script, std::io::stdout(), std::io::stderr())?;
            let status = exit_code_map.status_for(executed.exit_code);
            if let Some(mut update) = running {
                update
                    .state
                    .record(name, status.clone(), Some(executed.output));
                report(env, update.finish(Some((name, &status))));
            }
            return Ok(match (status.is_failing(), executed.exit_code) {
                (false, _) => 0,
                (true, 0) => 1,
                (true, code) => code,
            });
        }
    }
    Ok(0)
}

/// Reads the Run's state, changes it, and writes it back.
fn apply(
    comment: StatusComment,
    command: &Command,
    recorded: Option<(&str, &Status)>,
    change: impl FnOnce(&mut State),
) -> Result<()> {
    let Some(mut update) = comment.begin(command)? else {
        return Ok(());
    };
    change(&mut update.state);
    update.finish(recorded)
}

/// Reads the Config (or the file `config load` names), applies `config set`,
/// fills in the defaults and saves it.
fn prepare_config(env: &Env, state_dir: &Path, command: Option<&ConfigCommand>) -> Result<Config> {
    let path = state_dir.join("config.json");
    let mut config = match command {
        Some(ConfigCommand::Load { file }) => Config::load(file)?,
        _ => Config::read(&path)?,
    };
    // Settings first, so an emptied key gets its default back.
    if let Some(ConfigCommand::Set { settings }) = command {
        for setting in settings {
            config.set(setting)?;
        }
    }
    config.fill_defaults(env.default_comment_key.clone());
    config.save(&path)?;
    Ok(config)
}

/// Piped stdin, which `step` takes as Output.
fn piped_stdin() -> Result<Option<String>> {
    if std::io::stdin().is_terminal() {
        return Ok(None);
    }
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("reading stdin")?;
    Ok(Some(input).filter(|s| !s.is_empty()))
}

/// Reports an error in the Status Comment work, which never changes the exit code.
fn report(env: &Env, result: Result<()>) {
    if let Err(e) = result {
        env.error(e);
    }
}

fn append_line(path: &Path, line: &str) -> Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// Everything a command needs to read and write the Status Comment.
struct StatusComment<'a> {
    env: &'a Env,
    config: Config,
    template: String,
    github: GithubView,
    state_dir: PathBuf,
    /// `None` once writing would replace a newer Run's results.
    store: Option<Box<dyn CommentStore>>,
}

impl<'a> StatusComment<'a> {
    fn open(env: &'a Env, config: Config, state_dir: &Path, token: Option<String>) -> Result<Self> {
        if config.comment_key.is_empty() {
            bail!(
                "cannot work out a Comment Key (GITHUB_WORKFLOW or GITHUB_JOB is unset); set one with `config set comment-key=<key>`"
            );
        }
        let template = match config.template_file() {
            None => render::DEFAULT_TEMPLATE.to_string(),
            Some(path) => fs::read_to_string(path).unwrap_or_else(|e| {
                env.warn(format!(
                    "could not read Template {}: {e}; using the default",
                    path.display()
                ));
                render::DEFAULT_TEMPLATE.to_string()
            }),
        };
        let store = open_store(env, state_dir, token, &config.comment_key);
        Ok(StatusComment {
            env,
            github: env.view(),
            state_dir: state_dir.to_path_buf(),
            config,
            template,
            store: Some(store),
        })
    }

    fn renderer(&self) -> Renderer<'_> {
        Renderer {
            template: &self.template,
            github: &self.github,
            run_url: self.env.run_url.as_deref(),
            config: &self.config,
        }
    }

    fn load(&mut self) -> Result<Option<String>> {
        let store = self.store.as_mut().expect("opened with a store");
        store.load().context("could not read the Status Comment")
    }

    /// Prints the whole stored comment, hidden markers included, or the job
    /// summary it would produce.
    fn preview(mut self, target: PreviewTarget) -> Result<()> {
        let body = self.load()?;
        match target {
            PreviewTarget::Comment => match body {
                Some(body) => print!("{body}"),
                None => eprintln!("status-comment: no Status Comment yet"),
            },
            PreviewTarget::Summary => {
                let state = body
                    .as_deref()
                    .and_then(comment::load)
                    .unwrap_or_else(|| State::new("", 1));
                print!("{}", self.renderer().render(&state, Target::Summary, 0)?);
            }
        }
        Ok(())
    }

    /// This call's view of the Run: the stored state if it's this Run, else a
    /// fresh one. `None` means `finalize` found no Steps in this Run to close out.
    fn begin(mut self, command: &Command) -> Result<Option<Update<'a>>> {
        let finalize = matches!(command, Command::Finalize);
        let starts_steps = matches!(
            command,
            Command::Steps {
                command: Some(_),
                ..
            } | Command::Step { .. }
                | Command::Exec { .. }
        );
        let env = self.env;
        let previous = self.load()?.as_deref().and_then(comment::load);
        let previous = match previous {
            Some(p) if p.is_newer_than(&env.run_id, env.attempt) => {
                env.warn(format!(
                    "the Status Comment already shows run {} attempt {}, which is newer; not overwriting it",
                    p.run_id, p.attempt
                ));
                self.store = None;
                None
            }
            // Local Mode has no run ID, so a Run ends at Finalize.
            Some(p) if !env.on_github && p.finalized && starts_steps => None,
            other => other,
        };
        // Nothing was started in this Run, so there's nothing to close out; this
        // keeps an unused install from posting, or wiping, a comment at job end.
        if finalize && previous.as_ref().is_none_or(|p| p.run_id != env.run_id) {
            eprintln!("status-comment: no Steps in this Run; nothing to finalize");
            return Ok(None);
        }
        let mut state = State::for_run(previous, &env.run_id, env.attempt);
        // Finalize closes the Run; anything else reopens it.
        state.finalized = false;
        Ok(Some(Update {
            comment: self,
            state,
        }))
    }
}

/// A Run's state on its way back to the Status Comment.
struct Update<'a> {
    comment: StatusComment<'a>,
    state: State,
}

impl Update<'_> {
    /// Writes the state to the Status Comment and returns what was written.
    fn save(&mut self) -> Result<Option<String>> {
        if self.comment.store.is_none() {
            return Ok(None);
        }
        let body = comment::build(
            &self.comment.renderer(),
            &self.state,
            &self.comment.config.comment_key,
        )?;
        let store = self.comment.store.as_mut().expect("checked above");
        store
            .save(&body)
            .context("could not update the Status Comment")?;
        Ok(Some(body))
    }

    /// Saves, reports the recorded Status, shows the comment in Local Mode, and
    /// writes the job summary when the Run is finalized.
    fn finish(mut self, recorded: Option<(&str, &Status)>) -> Result<()> {
        let env = self.comment.env;
        let body = self.save()?;
        if let Some((name, status)) = recorded {
            eprintln!("status-comment: {name} → {status}");
            if let Some(path) = &env.github_output {
                append_line(path, &format!("status={status}"))
                    .context("writing to GITHUB_OUTPUT")?;
            }
        }
        if let (false, Some(body)) = (env.on_github, body) {
            eprintln!(
                "\n── status-comment preview (Local Mode, {}) ──\n{}\n── end preview ──",
                self.comment.state_dir.join("comment.md").display(),
                body.trim_end()
            );
        }
        if self.state.finalized
            && let Some(path) = &env.step_summary
        {
            let summary = self
                .comment
                .renderer()
                .render(&self.state, Target::Summary, 0)?;
            fs::write(path, summary).context("writing the job summary")?;
        }
        Ok(())
    }
}

fn open_store(
    env: &Env,
    state_dir: &Path,
    token: Option<String>,
    key: &str,
) -> Box<dyn CommentStore> {
    let file_store = || {
        Box::new(FileStore {
            path: state_dir.join("comment.md"),
            key: key.to_string(),
        })
    };
    if !env.on_github {
        return file_store();
    }
    match (
        &env.pull_request,
        &env.repository,
        token.or_else(|| env.token.clone()),
    ) {
        (Some(pr), Some(repo), Some(token)) => {
            return Box::new(GithubStore::new(
                &env.api_url,
                &env.graphql_url,
                repo,
                pr.number,
                &token,
                key,
            ));
        }
        (Some(_), _, None) => env.warn(
            "no GitHub token (set GITHUB_TOKEN or pass --token); skipping the Status Comment",
        ),
        _ => {}
    }
    file_store()
}
