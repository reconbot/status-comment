//! What the tool learns from where it runs: GitHub Actions, or Local Mode.

use std::fmt::Display;
use std::path::PathBuf;

use serde::Deserialize;

use crate::render::GithubView;

/// The run ID, and default Comment Key, everywhere in Local Mode.
const LOCAL: &str = "local";

#[derive(Debug)]
pub struct Env {
    /// False means Local Mode: never talk to GitHub, preview instead.
    pub on_github: bool,
    /// Holds config.json, and comment.md when there's no PR, unless `--state-path` says otherwise.
    pub state_dir: PathBuf,
    /// Workflow name plus job id; `None` when GitHub didn't say.
    pub default_comment_key: Option<String>,
    pub run_id: String,
    pub attempt: u32,
    /// The workflow run's page; `None` in Local Mode.
    pub run_url: Option<String>,
    pub workflow: String,
    pub job: String,
    pub actor: String,
    pub repository: Option<String>,
    pub sha: String,
    pub server_url: String,
    pub api_url: String,
    pub graphql_url: String,
    pub token: Option<String>,
    pub pull_request: Option<PullRequest>,
    pub github_output: Option<PathBuf>,
    pub step_summary: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub html_url: Option<String>,
}

#[derive(Deserialize)]
struct Event {
    pull_request: Option<PullRequest>,
}

impl Env {
    pub fn from_process() -> Self {
        Self::from_vars(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
    }

    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Self {
        let on_github = var("GITHUB_ACTIONS").as_deref() == Some("true");
        let server_url = var("GITHUB_SERVER_URL").unwrap_or_else(|| "https://github.com".into());
        let repository = var("GITHUB_REPOSITORY");
        let workflow = var("GITHUB_WORKFLOW");
        let job = var("GITHUB_JOB");
        let run_id = var("GITHUB_RUN_ID");
        let (state_dir, default_comment_key, run_id, run_url, workflow) = if on_github {
            (
                var("RUNNER_TEMP")
                    .map(PathBuf::from)
                    .unwrap_or_else(std::env::temp_dir)
                    .join("status-comment"),
                workflow
                    .as_ref()
                    .zip(job.as_ref())
                    .map(|(w, j)| format!("{w}/{j}")),
                run_id.clone().unwrap_or_else(|| "unknown".into()),
                repository
                    .as_ref()
                    .zip(run_id)
                    .map(|(repo, id)| format!("{server_url}/{repo}/actions/runs/{id}")),
                workflow.unwrap_or_default(),
            )
        } else {
            (
                PathBuf::from("."),
                Some(LOCAL.into()),
                LOCAL.into(),
                None,
                workflow.unwrap_or_else(|| "Local run".into()),
            )
        };
        Env {
            on_github,
            state_dir,
            default_comment_key,
            run_id,
            attempt: var("GITHUB_RUN_ATTEMPT")
                .and_then(|a| a.parse().ok())
                .unwrap_or(1),
            run_url,
            workflow,
            job: job.unwrap_or_default(),
            actor: var("GITHUB_ACTOR").unwrap_or_default(),
            repository,
            sha: var("GITHUB_SHA").unwrap_or_default(),
            server_url,
            api_url: var("GITHUB_API_URL").unwrap_or_else(|| "https://api.github.com".into()),
            graphql_url: var("GITHUB_GRAPHQL_URL")
                .unwrap_or_else(|| "https://api.github.com/graphql".into()),
            token: var("GITHUB_TOKEN"),
            pull_request: var("GITHUB_EVENT_PATH")
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|json| serde_json::from_str::<Event>(&json).ok())
                .and_then(|e| e.pull_request),
            github_output: var("GITHUB_OUTPUT").map(PathBuf::from),
            step_summary: var("GITHUB_STEP_SUMMARY").map(PathBuf::from),
        }
    }

    /// What a Template sees as `github`.
    pub fn view(&self) -> GithubView {
        GithubView {
            workflow: self.workflow.clone(),
            job: self.job.clone(),
            actor: self.actor.clone(),
            repository: self.repository.clone().unwrap_or_default(),
            sha: self.sha.clone(),
            short_sha: self.sha.chars().take(7).collect(),
            pr_number: self.pull_request.as_ref().map(|pr| pr.number),
            pr_url: self
                .pull_request
                .as_ref()
                .and_then(|pr| pr.html_url.clone()),
            server_url: self.server_url.clone(),
            updated_at: humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string(),
        }
    }

    /// A problem that doesn't stop the call.
    pub fn warn(&self, message: impl Display) {
        self.annotate("warning", message);
    }

    /// A failure that ends status-comment's own work without changing the exit code.
    pub fn error(&self, message: impl Display) {
        self.annotate("error", message);
    }

    /// A GitHub annotation in a workflow, a plain line on stderr in Local Mode.
    fn annotate(&self, level: &str, message: impl Display) {
        // `{:#}` flattens anyhow's context chain onto one line.
        let message = format!("{message:#}").replace('\n', " ");
        if self.on_github {
            println!("::{level}::status-comment: {message}");
        } else {
            eprintln!("status-comment: {level}: {message}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_mode_fills_in_what_github_would_say() {
        let env = Env::from_vars(|k| (k == "GITHUB_RUN_ID").then(|| "7".into()));
        assert_eq!(env.state_dir, PathBuf::from("."));
        assert_eq!(env.default_comment_key.as_deref(), Some("local"));
        assert_eq!(env.run_id, "local");
        assert_eq!(env.run_url, None);
        assert_eq!(env.workflow, "Local run");
    }
}
