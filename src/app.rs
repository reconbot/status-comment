use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{ArgGroup, Parser};

use crate::comment;
use crate::execute::execute;
use crate::exit_code_map::ExitCodeMap;
use crate::github_env::GithubEnv;
use crate::render::{self, GithubView, Renderer, RunInfo, Target};
use crate::state::{State, StepUpdate};
use crate::status::Status;
use crate::store::{CommentStore, FileStore, GithubStore};

/// Keeps a pull request comment up to date with the Steps of a GitHub Actions job.
#[derive(Debug, Parser)]
#[command(version, about)]
#[command(group(ArgGroup::new("action").required(true).multiple(true)
    .args(["steps", "step_name", "template", "finalize"])))]
pub struct Cli {
    /// Declare Steps as `pending`, e.g. `init,validate,plan`.
    #[arg(long, value_delimiter = ',')]
    pub steps: Option<Vec<String>>,

    /// The Step this call records. Never read from the environment.
    #[arg(long)]
    pub step_name: Option<String>,

    /// The label shown for the Step (defaults to the title-cased name).
    #[arg(long, requires = "step_name")]
    pub display_name: Option<String>,

    /// Status to record in piped mode (default `success`).
    #[arg(long, requires = "step_name", conflicts_with = "execute")]
    pub status: Option<Status>,

    /// Run a command and record its Output and Status.
    #[arg(long, requires = "step_name")]
    pub execute: Option<String>,

    /// Map exit codes to Statuses, e.g. `0=success,1=failure,2=changes`.
    #[arg(long, requires = "execute")]
    pub exit_code_map: Option<ExitCodeMap>,

    /// A minijinja Template file to use for this job.
    #[arg(long)]
    pub template: Option<PathBuf>,

    /// Which Status Comment to update (default: workflow name + job id).
    #[arg(long)]
    pub comment_key: Option<String>,

    /// Close out the Run: pending → skipped, running → cancelled, and write the job summary.
    #[arg(long)]
    pub finalize: bool,

    /// GitHub token (default: the GITHUB_TOKEN env var).
    #[arg(long)]
    pub token: Option<String>,
}

fn warn(message: impl std::fmt::Display) {
    // `{:#}` flattens anyhow's context chain onto one annotation line.
    println!(
        "::warning::status-comment: {}",
        format!("{message:#}").replace('\n', " ")
    );
}

/// Local, per-job files for one Comment Key.
fn local_dir(env: &GithubEnv, key: &str) -> PathBuf {
    let safe: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    env.runner_temp.join("status-comment").join(safe)
}

fn open_store(env: &GithubEnv, cli: &Cli, key: &str) -> Box<dyn CommentStore> {
    let token = cli.token.clone().or_else(|| env.token.clone());
    match (&env.pull_request, &env.repository, token) {
        (Some(pr), Some(repo), Some(token)) => {
            Box::new(GithubStore::new(&env.api_url, repo, pr.number, &token, key))
        }
        (Some(_), _, None) => {
            warn("no GitHub token (set GITHUB_TOKEN or pass --token); skipping the Status Comment");
            Box::new(FileStore {
                path: local_dir(env, key).join("comment.md"),
            })
        }
        _ => Box::new(FileStore {
            path: local_dir(env, key).join("comment.md"),
        }),
    }
}

fn append_line(path: &Path, line: &str) -> Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// Runs one invocation and returns the process exit code.
pub fn run(cli: Cli, env: &GithubEnv, stdin: Option<String>) -> Result<i32> {
    let key = match cli
        .comment_key
        .clone()
        .or_else(|| env.default_comment_key())
    {
        Some(key) => key,
        None => bail!("cannot work out a Comment Key outside GitHub Actions; pass --comment-key"),
    };
    let dir = local_dir(env, &key);
    let template_path = dir.join("template.md.j2");

    if let Some(path) = &cli.template {
        let source = fs::read_to_string(path)
            .with_context(|| format!("reading Template {}", path.display()))?;
        minijinja::Environment::new()
            .template_from_str(&source)
            .with_context(|| format!("Template {} is invalid", path.display()))?;
        fs::create_dir_all(&dir)?;
        fs::write(&template_path, source)?;
    }
    if cli.steps.is_none() && cli.step_name.is_none() && !cli.finalize {
        return Ok(0);
    }

    let template =
        fs::read_to_string(&template_path).unwrap_or_else(|_| render::DEFAULT_TEMPLATE.to_string());
    let run_id = env.run_id.clone().unwrap_or_else(|| "local".into());
    let github = github_view(env);
    let run_info = RunInfo {
        url: env.run_url(),
        attempt: env.run_attempt.clone().unwrap_or_else(|| "1".into()),
    };
    let renderer = Renderer {
        template: &template,
        github: &github,
        run: &run_info,
    };

    let mut store = open_store(env, &cli, &key);
    // If the comment can't be read, writing would wipe it, so stop writing to it.
    let (previous, mut comment_ok) = match store.load() {
        Ok(body) => (body.as_deref().and_then(comment::load), true),
        Err(e) => {
            warn(e.context("could not read the Status Comment; it will not be updated"));
            (None, false)
        }
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

    if let Some(steps) = &cli.steps {
        state.declare(steps);
    }

    let mut exit_code = 0;
    if let Some(name) = &cli.step_name {
        let status = if let Some(command) = &cli.execute {
            state.record(
                name,
                StepUpdate {
                    status: Some(Status::Running),
                    display_name: cli.display_name.clone(),
                    output: None,
                },
            );
            publish(&state)?;

            let executed = execute(command, &mut std::io::stdout())?;
            let map = cli.exit_code_map.clone().unwrap_or_default();
            let status = match map.get(executed.exit_code) {
                Some(status) => {
                    if status.is_failing() {
                        exit_code = if executed.exit_code == 0 {
                            1
                        } else {
                            executed.exit_code
                        };
                    }
                    status.clone()
                }
                None => {
                    exit_code = if executed.exit_code == 0 {
                        1
                    } else {
                        executed.exit_code
                    };
                    Status::Failure
                }
            };
            state.record(
                name,
                StepUpdate {
                    status: Some(status.clone()),
                    display_name: None,
                    output: Some(executed.output),
                },
            );
            status
        } else {
            let status = cli.status.clone().unwrap_or(Status::Success);
            let output = stdin.filter(|s| !s.is_empty());
            state.record(
                name,
                StepUpdate {
                    status: Some(status.clone()),
                    display_name: cli.display_name.clone(),
                    output,
                },
            );
            status
        };
        eprintln!("status-comment: {name} → {status}");
        if let Some(path) = &env.github_output {
            append_line(path, &format!("status={status}")).context("writing to GITHUB_OUTPUT")?;
        }
    }

    if cli.finalize {
        state.finalize();
    }
    publish(&state)?;

    if cli.finalize
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
        workflow: env.workflow.clone().unwrap_or_default(),
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
