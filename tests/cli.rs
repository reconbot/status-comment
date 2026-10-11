//! End-to-end tests that run the binary the way a workflow would.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use mockito::Matcher;
use predicates::prelude::*;
use serde_json::json;
use tempfile::TempDir;

/// A fake GitHub Actions job: its temp dir, outputs and summary files.
struct Job {
    dir: TempDir,
    run_id: String,
    vars: Vec<(String, String)>,
}

impl Job {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("runner_temp")).unwrap();
        Job {
            dir,
            run_id: "100".into(),
            vars: Vec::new(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn with_pull_request(mut self, api_url: &str) -> Self {
        let event = self.path("event.json");
        fs::write(
            &event,
            r#"{"pull_request":{"number":5,"html_url":"https://github.com/o/r/pull/5"}}"#,
        )
        .unwrap();
        self.vars
            .push(("GITHUB_EVENT_PATH".into(), event.display().to_string()));
        self.vars.push(("GITHUB_API_URL".into(), api_url.into()));
        self.vars.push(("GITHUB_TOKEN".into(), "t0k".into()));
        self
    }

    fn cmd(&self) -> Command {
        let mut cmd = Command::cargo_bin("status-comment").unwrap();
        cmd.env_clear()
            .env("PATH", std::env::var("PATH").unwrap())
            .env("GITHUB_WORKFLOW", "Terraform")
            .env("GITHUB_JOB", "plan")
            .env("GITHUB_RUN_ID", &self.run_id)
            .env("GITHUB_RUN_ATTEMPT", "1")
            .env("GITHUB_REPOSITORY", "o/r")
            .env("GITHUB_SHA", "abcdef1234567890")
            .env("GITHUB_ACTOR", "octocat")
            .env("RUNNER_TEMP", self.path("runner_temp"))
            .env("GITHUB_OUTPUT", self.path("github_output"))
            .env("GITHUB_STEP_SUMMARY", self.path("step_summary"));
        for (k, v) in &self.vars {
            cmd.env(k, v);
        }
        cmd
    }

    /// The local stand-in for the Status Comment when there's no PR.
    fn comment(&self) -> String {
        fs::read_to_string(self.path("runner_temp/status-comment/Terraform_plan/comment.md"))
            .unwrap()
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).unwrap_or_default()
    }
}

#[test]
fn full_local_run() {
    let job = Job::new();
    job.cmd()
        .args(["--steps", "init,plan,apply"])
        .assert()
        .success();
    assert!(job.comment().contains("| Apply | ⬜ Pending |"));

    job.cmd()
        .args(["--step-name", "init"])
        .write_stdin("Terraform initialized\n")
        .assert()
        .success();

    job.cmd()
        .args([
            "--step-name",
            "plan",
            "--exit-code-map",
            "0=success,1=failure,2=changes",
        ])
        .args(["--execute", "echo 'Plan: 1 to add'; exit 2"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Plan: 1 to add"));

    job.cmd().arg("--finalize").assert().success();

    let comment = job.comment();
    assert!(comment.contains("| Init | ✅ Success |"), "{comment}");
    assert!(comment.contains("| Plan | changes |"), "{comment}");
    assert!(comment.contains("| Apply | ⏭️ Skipped |"), "{comment}");
    assert!(comment.contains("| 🎉 | All done |"), "{comment}");
    assert!(comment.contains("Terraform initialized"), "{comment}");
    assert!(comment.contains("Plan: 1 to add"), "{comment}");

    assert_eq!(
        job.read("github_output"),
        "status=success\nstatus=changes\n"
    );
    let summary = job.read("step_summary");
    assert!(summary.contains("| Plan | changes |"), "{summary}");
    assert!(
        !summary.contains("### Terraform"),
        "summary omits the comment header: {summary}"
    );
}

#[test]
fn execute_failure_is_failing_and_exits_with_command_code() {
    let job = Job::new();
    job.cmd()
        .args([
            "--step-name",
            "validate",
            "--execute",
            "echo bad >&2; exit 3",
        ])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("bad"));
    assert!(job.comment().contains("| Validate | ❌ Failed |"));
    assert_eq!(job.read("github_output"), "status=failure\n");
}

#[test]
fn mapped_failure_exits_non_zero() {
    let job = Job::new();
    job.cmd()
        .args([
            "--step-name",
            "x",
            "--exit-code-map",
            "0=failure",
            "--execute",
            "true",
        ])
        .assert()
        .code(1);
}

#[test]
fn custom_status_from_exit_code_exits_zero() {
    let job = Job::new();
    job.cmd()
        .args([
            "--step-name",
            "x",
            "--exit-code-map",
            "0=success,2=changes",
            "--execute",
            "exit 2",
        ])
        .assert()
        .success();
}

#[test]
fn piped_mode_records_status_and_never_fails() {
    let job = Job::new();
    job.cmd()
        .args(["--step-name", "bar", "--status", "failure"])
        .write_stdin("oops")
        .assert()
        .success();
    assert!(job.comment().contains("#### Bar output"));
    job.cmd()
        .args(["--step-name", "party", "--status", "canceled"])
        .assert()
        .success();
    assert!(job.comment().contains("| Party | 🚫 Cancelled |"));
}

#[test]
fn finalize_cancels_running_steps_and_warns() {
    let job = Job::new();
    job.cmd()
        .args(["--step-name", "apply", "--status", "running"])
        .assert()
        .success();
    job.cmd().arg("--finalize").assert().success();
    let comment = job.comment();
    assert!(comment.contains("| Apply | 🚫 Cancelled |"), "{comment}");
    assert!(comment.contains("> [!WARNING]"), "{comment}");
}

#[test]
fn new_run_resets_steps() {
    let mut job = Job::new();
    job.cmd().args(["--step-name", "plan"]).assert().success();
    job.run_id = "101".into();
    job.cmd().args(["--step-name", "apply"]).assert().success();
    let comment = job.comment();
    assert!(comment.contains("| Apply |"), "{comment}");
    assert!(!comment.contains("| Plan |"), "{comment}");
}

#[test]
fn template_is_remembered_for_the_job() {
    let job = Job::new();
    let template = job.path("t.md.j2");
    fs::write(
        &template,
        "{% for s in steps %}* {{ s.name }}: {{ s.status }}\n{% endfor %}",
    )
    .unwrap();
    job.cmd()
        .arg("--template")
        .arg(&template)
        .assert()
        .success();
    job.cmd()
        .args(["--step-name", "plan", "--status", "changes"])
        .assert()
        .success();
    assert!(
        job.comment().starts_with("* plan: changes\n"),
        "{}",
        job.comment()
    );
}

#[test]
fn invalid_template_is_rejected() {
    let job = Job::new();
    let template = job.path("bad.md.j2");
    fs::write(&template, "{% for %}").unwrap();
    job.cmd()
        .arg("--template")
        .arg(&template)
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid"));
}

#[test]
fn comment_key_override_uses_a_separate_comment() {
    let job = Job::new();
    job.cmd()
        .args(["--comment-key", "prod", "--step-name", "plan"])
        .assert()
        .success();
    assert!(Path::new(&job.path("runner_temp/status-comment/prod/comment.md")).exists());
}

#[test]
fn step_name_is_required_to_record() {
    let job = Job::new();
    job.cmd().args(["--execute", "true"]).assert().code(2);
    job.cmd().args(["--status", "success"]).assert().code(2);
    job.cmd().assert().code(2);
}

#[test]
fn needs_a_comment_key_outside_github_actions() {
    let job = Job::new();
    job.cmd()
        .env_remove("GITHUB_WORKFLOW")
        .args(["--step-name", "x"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--comment-key"));
}

#[test]
fn pull_request_creates_the_status_comment() {
    let mut server = mockito::Server::new();
    server
        .mock("GET", "/repos/o/r/issues/5/comments")
        .match_query(Matcher::Any)
        .with_body("[]")
        .create();
    let create = server
        .mock("POST", "/repos/o/r/issues/5/comments")
        .match_header("authorization", "Bearer t0k")
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex("Plan".into()),
            Matcher::Regex("status-comment:key Terraform%2Fplan".into()),
        ]))
        .with_status(201)
        .with_body(r#"{"id":77,"body":""}"#)
        .create();

    let job = Job::new().with_pull_request(&server.url());
    job.cmd().args(["--step-name", "plan"]).assert().success();
    create.assert();
}

#[test]
fn pull_request_updates_existing_comment_keeping_other_steps() {
    // Build a realistic existing comment by running locally first.
    let local = Job::new();
    local
        .cmd()
        .args(["--step-name", "init"])
        .write_stdin("init log")
        .assert()
        .success();
    let existing = local.comment();

    let mut server = mockito::Server::new();
    server
        .mock("GET", "/repos/o/r/issues/5/comments")
        .match_query(Matcher::Any)
        .with_body(json!([{ "id": 1, "body": "hi" }, { "id": 2, "body": existing }]).to_string())
        .create();
    let update = server
        .mock("PATCH", "/repos/o/r/issues/comments/2")
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex(r"\| Init \| ✅ Success \|".into()),
            Matcher::Regex("init log".into()),
            Matcher::Regex(r"\| Plan \| ✅ Success \|".into()),
        ]))
        .with_body(r#"{"id":2,"body":""}"#)
        .create();

    let job = Job::new().with_pull_request(&server.url());
    job.cmd().args(["--step-name", "plan"]).assert().success();
    update.assert();
}

#[test]
fn github_api_failure_warns_without_changing_exit_code() {
    let mut server = mockito::Server::new();
    server
        .mock("GET", "/repos/o/r/issues/5/comments")
        .match_query(Matcher::Any)
        .with_status(403)
        .create();
    let job = Job::new().with_pull_request(&server.url());
    job.cmd()
        .args(["--step-name", "plan", "--execute", "echo ok"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "::warning::status-comment: could not read the Status Comment",
        ));
    assert_eq!(job.read("github_output"), "status=success\n");
}
