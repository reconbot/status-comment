//! What the tool learns from the GitHub Actions environment.

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug)]
pub struct GithubEnv {
    /// False means Local Mode: never talk to GitHub, preview instead.
    pub on_github: bool,
    pub workflow: Option<String>,
    pub job: Option<String>,
    pub run_id: Option<String>,
    pub run_attempt: Option<String>,
    pub repository: Option<String>,
    pub sha: Option<String>,
    pub actor: Option<String>,
    pub server_url: String,
    pub api_url: String,
    pub token: Option<String>,
    pub pull_request: Option<PullRequest>,
    pub runner_temp: PathBuf,
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

impl GithubEnv {
    pub fn from_process() -> Self {
        Self::from_vars(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
    }

    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Self {
        let pull_request = var("GITHUB_EVENT_PATH")
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|json| serde_json::from_str::<Event>(&json).ok())
            .and_then(|e| e.pull_request);
        GithubEnv {
            on_github: var("GITHUB_ACTIONS").as_deref() == Some("true"),
            workflow: var("GITHUB_WORKFLOW"),
            job: var("GITHUB_JOB"),
            run_id: var("GITHUB_RUN_ID"),
            run_attempt: var("GITHUB_RUN_ATTEMPT"),
            repository: var("GITHUB_REPOSITORY"),
            sha: var("GITHUB_SHA"),
            actor: var("GITHUB_ACTOR"),
            server_url: var("GITHUB_SERVER_URL").unwrap_or_else(|| "https://github.com".into()),
            api_url: var("GITHUB_API_URL").unwrap_or_else(|| "https://api.github.com".into()),
            token: var("GITHUB_TOKEN"),
            pull_request,
            runner_temp: var("RUNNER_TEMP")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir),
            github_output: var("GITHUB_OUTPUT").map(PathBuf::from),
            step_summary: var("GITHUB_STEP_SUMMARY").map(PathBuf::from),
        }
    }

    /// The default Comment Key: workflow name plus job id.
    pub fn default_comment_key(&self) -> Option<String> {
        Some(format!(
            "{}/{}",
            self.workflow.as_ref()?,
            self.job.as_ref()?
        ))
    }

    pub fn run_url(&self) -> Option<String> {
        Some(format!(
            "{}/{}/actions/runs/{}",
            self.server_url,
            self.repository.as_ref()?,
            self.run_id.as_ref()?
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> GithubEnv {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        GithubEnv::from_vars(|k| map.get(k).cloned())
    }

    #[test]
    fn reads_pull_request_from_event_payload() {
        let dir = tempfile::tempdir().unwrap();
        let event = dir.path().join("event.json");
        std::fs::write(
            &event,
            r#"{"pull_request":{"number":12,"html_url":"https://github.com/o/r/pull/12"}}"#,
        )
        .unwrap();
        let env = env(&[("GITHUB_EVENT_PATH", event.to_str().unwrap())]);
        assert_eq!(
            env.pull_request,
            Some(PullRequest {
                number: 12,
                html_url: Some("https://github.com/o/r/pull/12".into())
            })
        );
    }

    #[test]
    fn default_comment_key_needs_workflow_and_job() {
        assert_eq!(
            env(&[("GITHUB_WORKFLOW", "Terraform"), ("GITHUB_JOB", "plan")])
                .default_comment_key()
                .as_deref(),
            Some("Terraform/plan")
        );
        assert_eq!(
            env(&[("GITHUB_WORKFLOW", "Terraform")]).default_comment_key(),
            None
        );
    }

    #[test]
    fn detects_github_actions() {
        assert!(env(&[("GITHUB_ACTIONS", "true")]).on_github);
        assert!(!env(&[]).on_github);
    }
}
