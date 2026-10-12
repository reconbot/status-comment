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
        self.vars
            .push(("GITHUB_GRAPHQL_URL".into(), format!("{api_url}/graphql")));
        self.vars.push(("GITHUB_TOKEN".into(), "t0k".into()));
        self
    }
}

/// A GraphQL response listing PR comments: `(id, body, written by our token)`.
fn comments_page(comments: &[(u64, &str, bool)]) -> String {
    let nodes: Vec<_> = comments
        .iter()
        .map(|(id, body, ours)| json!({ "databaseId": id, "body": body, "viewerDidAuthor": ours }))
        .collect();
    json!({ "data": { "repository": { "pullRequest": { "comments": {
        "nodes": nodes,
        "pageInfo": { "hasNextPage": false, "endCursor": null },
    } } } } })
    .to_string()
}

impl Job {
    fn cmd(&self) -> Command {
        let mut cmd = Command::cargo_bin("status-comment").unwrap();
        cmd.env_clear()
            .env("PATH", std::env::var("PATH").unwrap())
            .env("GITHUB_ACTIONS", "true")
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

    /// The Status Comment when there's no PR: comment.md in the state path.
    fn comment(&self) -> String {
        fs::read_to_string(self.path("runner_temp/status-comment/comment.md")).unwrap()
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).unwrap_or_default()
    }
}

#[test]
fn full_run_without_a_pull_request() {
    let job = Job::new();
    job.cmd()
        .args(["steps", "set", "init", "plan", "apply"])
        .assert()
        .success();
    assert!(job.comment().contains("| Apply | ⬜ Pending |"));

    job.cmd()
        .args(["step", "init"])
        .write_stdin("Terraform initialized <resource> & \"x\"\n")
        .assert()
        .success();

    job.cmd()
        .args([
            "exec",
            "plan",
            "--exit-code-map",
            "0=success,1=failure,2=changes",
        ])
        .arg("echo 'Plan: 1 to add'; exit 2")
        .assert()
        .success()
        .stdout(predicate::str::contains("Plan: 1 to add"));

    let comment = job.comment();
    assert!(
        comment.contains("### Terraform · plan\n\n<sub>"),
        "{comment}"
    );
    assert!(
        !comment.contains("🎉"),
        "no celebration before Finalize: {comment}"
    );

    job.cmd().arg("finalize").assert().success();

    let comment = job.comment();
    assert!(comment.contains("| Init | ✅ Success |"), "{comment}");
    assert!(comment.contains("| Plan | changes |"), "{comment}");
    assert!(comment.contains("| Apply | ⏭️ Skipped |"), "{comment}");
    assert!(comment.contains("| 🎉 | All done |"), "{comment}");
    assert!(
        comment.contains("<details><summary>Init output</summary>"),
        "{comment}"
    );
    assert!(
        comment.contains("Terraform initialized <resource> & \"x\""),
        "Output is not HTML-escaped: {comment}"
    );
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
fn exec_failure_is_failing_and_exits_with_command_code() {
    let job = Job::new();
    job.cmd()
        .args(["exec", "validate", "echo bad >&2; exit 3"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("bad"));
    assert!(job.comment().contains("| Validate | ❌ Failed |"));
    assert_eq!(job.read("github_output"), "status=failure\n");
}

#[test]
fn a_failing_status_from_exit_code_zero_exits_one() {
    let job = Job::new();
    job.cmd()
        .args(["exec", "x", "--exit-code-map", "2=changes", "true"])
        .assert()
        .code(1);
    assert!(job.comment().contains("| X | ❌ Failed |"));
    job.cmd()
        .args(["exec", "y", "--exit-code-map", "0=failure", "true"])
        .assert()
        .code(1);
}

#[test]
fn step_sets_status_and_never_fails() {
    let job = Job::new();
    job.cmd()
        .args(["step", "bar", "--status", "failure"])
        .write_stdin("oops")
        .assert()
        .success();
    assert!(job.comment().contains("#### Bar output"));
    job.cmd()
        .args(["step", "apply", "--status", "cancelled"])
        .assert()
        .success();
    assert!(job.comment().contains("| Apply | 🚫 Cancelled |"));
}

#[test]
fn finalize_cancels_running_steps_and_warns() {
    let job = Job::new();
    job.cmd()
        .args(["step", "apply", "--status", "running"])
        .assert()
        .success();
    job.cmd().arg("finalize").assert().success();
    let comment = job.comment();
    assert!(comment.contains("| Apply | 🚫 Cancelled |"), "{comment}");
    assert!(comment.contains("> [!WARNING]"), "{comment}");
}

#[test]
fn finalize_without_steps_in_this_run_changes_nothing() {
    let mut job = Job::new();
    job.cmd()
        .arg("finalize")
        .assert()
        .success()
        .stderr(predicate::str::contains("nothing to finalize"));
    assert!(!job.path("runner_temp/status-comment/comment.md").exists());
    assert_eq!(job.read("step_summary"), "");

    // A later Run that never records a Step leaves the last Run's comment alone.
    job.cmd().args(["step", "plan"]).assert().success();
    let before = job.comment();
    job.run_id = "101".into();
    job.cmd().arg("finalize").assert().success();
    assert_eq!(job.comment(), before);
}

#[test]
fn new_run_resets_steps() {
    let mut job = Job::new();
    job.cmd().args(["step", "plan"]).assert().success();
    job.run_id = "101".into();
    job.cmd().args(["step", "apply"]).assert().success();
    let comment = job.comment();
    assert!(comment.contains("| Apply |"), "{comment}");
    assert!(!comment.contains("| Plan |"), "{comment}");
}

#[test]
fn steps_set_rewrites_the_list_and_reopens_the_run() {
    let job = Job::new();
    job.cmd()
        .args(["steps", "set", "init", "plan", "apply"])
        .assert()
        .success();
    job.cmd().args(["step", "init"]).assert().success();
    job.cmd().arg("finalize").assert().success();
    assert!(job.comment().contains("🎉"));

    job.cmd()
        .args(["config", "set", "step.rules-check=OPA Rules"])
        .assert()
        .success();
    job.cmd()
        .args(["steps", "set", "rules-check", "init"])
        .assert()
        .success();
    let comment = job.comment();
    assert!(
        comment.contains("| OPA Rules | ⬜ Pending |\n| Init | ✅ Success |\n"),
        "{comment}"
    );
    assert!(!comment.contains("| Plan |"), "{comment}");
    assert!(
        !comment.contains("🎉"),
        "reopened Run must not celebrate: {comment}"
    );
}

#[test]
fn status_labels_come_from_the_config() {
    let job = Job::new();
    job.cmd()
        .args([
            "config",
            "set",
            "status.changes=🔍 Changes detected",
            "status.failure=💥 Broken",
        ])
        .assert()
        .success();
    job.cmd()
        .args(["step", "plan", "--status", "changes"])
        .assert()
        .success();
    job.cmd()
        .args(["step", "lint", "--status", "failure"])
        .assert()
        .success();
    let comment = job.comment();
    assert!(
        comment.contains("| Plan | 🔍 Changes detected |"),
        "{comment}"
    );
    assert!(comment.contains("| Lint | 💥 Broken |"), "{comment}");
}

#[test]
fn an_empty_value_resets_a_label() {
    let job = Job::new();
    job.cmd()
        .args([
            "config",
            "set",
            "step.plan=Terraform Plan",
            "status.failure=💥 Broken",
        ])
        .assert()
        .success();
    job.cmd()
        .args(["config", "set", "step.plan=", "status.failure="])
        .assert()
        .success()
        .stdout(predicate::str::contains("Terraform Plan").not())
        .stdout(predicate::str::contains("❌ Failed"));
    job.cmd()
        .args(["step", "plan", "--status", "failure"])
        .assert()
        .success();
    let comment = job.comment();
    assert!(comment.contains("| Plan | ❌ Failed |"), "{comment}");
}

#[test]
fn an_empty_value_resets_the_template_and_comment_key() {
    let job = Job::new();
    let template = job.path("t.md.j2");
    fs::write(&template, "custom").unwrap();
    job.cmd()
        .args(["config", "set", "comment-key=prod"])
        .arg(format!("template={}", template.display()))
        .assert()
        .success();
    job.cmd()
        .args(["config", "set", "comment-key=", "template="])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""template": "default""#))
        .stdout(predicate::str::contains(
            r#""comment-key": "Terraform/plan""#,
        ));
}

#[test]
fn a_later_label_relabels_the_comment() {
    let job = Job::new();
    job.cmd().args(["step", "plan"]).assert().success();
    assert!(job.comment().contains("| Plan | ✅ Success |"));
    job.cmd()
        .args(["config", "set", "step.plan=Terraform Plan"])
        .assert()
        .success();
    job.cmd().args(["step", "apply"]).assert().success();
    let comment = job.comment();
    assert!(
        comment.contains("| Terraform Plan | ✅ Success |\n| Apply | ✅ Success |\n"),
        "{comment}"
    );
}

#[test]
fn reset_status_clears_temporary_work_and_keeps_going() {
    let job = Job::new();
    job.cmd()
        .args(["steps", "set", "plan", "apply"])
        .assert()
        .success();
    job.cmd()
        .args(["exec", "plan", "echo first try; exit 1"])
        .assert()
        .code(1);
    job.cmd().args(["step", "scratch"]).assert().success();

    // Drop the temporary Step and start the rest over, in one call.
    job.cmd()
        .args(["steps", "set", "plan", "apply", "--reset-status"])
        .assert()
        .success();
    let comment = job.comment();
    assert!(
        comment.contains("| Plan | ⬜ Pending |\n| Apply | ⬜ Pending |\n"),
        "{comment}"
    );
    assert!(!comment.contains("Scratch"), "{comment}");
    assert!(!comment.contains("first try"), "{comment}");

    job.cmd().args(["step", "plan"]).assert().success();
    assert!(job.comment().contains("| Plan | ✅ Success |"));

    // On its own, --reset-status keeps the list.
    job.cmd()
        .args(["steps", "--reset-status"])
        .assert()
        .success();
    assert!(
        job.comment()
            .contains("| Plan | ⬜ Pending |\n| Apply | ⬜ Pending |\n")
    );
}

#[test]
fn config_set_template_saves_its_path() {
    let job = Job::new();
    let template = job.path("t.md.j2");
    fs::write(&template, "first").unwrap();
    job.cmd()
        .args(["config", "set"])
        .arg(format!("template={}", template.display()))
        .assert()
        .success()
        .stdout(predicate::str::contains("t.md.j2"));
    assert!(
        job.read("runner_temp/status-comment/config.json")
            .contains("t.md.j2")
    );

    // The path is saved, not the contents: later edits are picked up.
    fs::write(
        &template,
        "{% for s in steps %}* {{ s.name }}: {{ s.status }}\n{% endfor %}",
    )
    .unwrap();
    job.cmd()
        .args(["step", "plan", "--status", "changes"])
        .assert()
        .success();
    assert!(
        job.comment().starts_with("* plan: changes\n"),
        "{}",
        job.comment()
    );
}

#[test]
fn config_set_comment_key_marks_the_comment() {
    let job = Job::new();
    job.cmd()
        .args(["config", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""template": "default""#))
        .stdout(predicate::str::contains(
            r#""comment-key": "Terraform/plan""#,
        ))
        .stdout(predicate::str::contains(r#""failure": "❌ Failed""#));
    job.cmd()
        .args(["config", "set", "comment-key=prod"])
        .assert()
        .success();
    job.cmd()
        .args(["config", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""comment-key": "prod""#));
    job.cmd().args(["step", "plan"]).assert().success();
    assert!(job.comment().contains("<!-- status-comment:key prod -->"));
}

#[test]
fn config_rejects_unknown_keys() {
    let job = Job::new();
    job.cmd()
        .args(["config", "set", "colour=blue"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown config key"));
}

#[test]
fn config_load_replaces_the_saved_config() {
    let job = Job::new();
    fs::create_dir(job.path("ci")).unwrap();
    fs::write(job.path("ci/t.md.j2"), "custom").unwrap();
    fs::write(
        job.path("ci/status.json"),
        r#"{"template": "t.md.j2", "comment-key": "ci"}"#,
    )
    .unwrap();
    job.cmd()
        .args(["config", "load"])
        .arg(job.path("ci/status.json"))
        .assert()
        .success();
    job.cmd().args(["step", "plan"]).assert().success();
    let comment = job.comment();
    assert!(comment.starts_with("custom"), "{comment}");
    assert!(comment.contains("status-comment:key ci "), "{comment}");
}

#[test]
fn missing_template_file_falls_back_to_the_default_with_a_warning() {
    let job = Job::new();
    let template = job.path("t.md.j2");
    fs::write(&template, "custom").unwrap();
    job.cmd()
        .args(["config", "set"])
        .arg(format!("template={}", template.display()))
        .assert()
        .success();
    fs::remove_file(&template).unwrap();
    job.cmd()
        .args(["step", "plan"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "::warning::status-comment: could not read Template",
        ));
    assert!(job.comment().contains("| Plan | ✅ Success |"));
}

#[test]
fn state_path_moves_config_and_comment() {
    let job = Job::new();
    let state = job.path("state");
    job.cmd()
        .arg("--state-path")
        .arg(&state)
        .args(["config", "set", "comment-key=moved"])
        .assert()
        .success();
    // Global: accepted after the command too.
    job.cmd()
        .args(["step", "plan", "--state-path"])
        .arg(&state)
        .assert()
        .success();
    assert!(state.join("config.json").exists());
    assert!(
        fs::read_to_string(state.join("comment.md"))
            .unwrap()
            .contains("status-comment:key moved ")
    );
    assert!(!job.path("runner_temp/status-comment").exists());
}

#[test]
fn preview_prints_without_changing_anything() {
    let job = Job::new();
    job.cmd()
        .args(["step", "plan", "--status", "running"])
        .assert()
        .success();
    let before = job.comment();
    job.cmd()
        .arg("preview")
        .assert()
        .success()
        .stdout(before.clone());
    job.cmd()
        .args(["preview", "--target", "summary"])
        .assert()
        .success()
        .stdout(predicate::str::contains("| Plan |"))
        .stdout(predicate::str::contains("### Terraform").not());
    assert_eq!(job.comment(), before);
    assert_eq!(job.read("github_output"), "status=running\n");
}

#[test]
fn every_command_writes_the_defaults_to_the_config() {
    let job = Job::new();
    job.cmd().arg("preview").assert().success();
    let config = job.read("runner_temp/status-comment/config.json");
    assert!(config.contains(r#""template": "default""#), "{config}");
    assert!(
        config.contains(r#""comment-key": "Terraform/plan""#),
        "{config}"
    );

    // A saved default isn't re-derived: later env changes don't move the comment.
    job.cmd()
        .env("GITHUB_JOB", "other")
        .args(["step", "plan"])
        .assert()
        .success();
    assert!(
        job.comment()
            .contains("status-comment:key Terraform%2Fplan ")
    );
}

#[test]
fn preview_without_a_comment_says_so() {
    let job = Job::new();
    job.cmd()
        .arg("preview")
        .assert()
        .success()
        .stdout("")
        .stderr(predicate::str::contains("no Status Comment yet"));
}

#[test]
fn needs_a_comment_key_when_workflow_env_is_missing() {
    let job = Job::new();
    job.cmd()
        .env_remove("GITHUB_WORKFLOW")
        .args(["step", "x"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("comment-key"));
}

#[test]
fn pull_request_creates_the_status_comment() {
    let mut server = mockito::Server::new();
    server
        .mock("POST", "/graphql")
        .with_body(comments_page(&[]))
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
    job.cmd().args(["step", "plan"]).assert().success();
    create.assert();
}

#[test]
fn pull_request_updates_existing_comment_keeping_other_steps() {
    // Build a realistic existing comment without a PR first.
    let local = Job::new();
    local
        .cmd()
        .args(["step", "init"])
        .write_stdin("init log")
        .assert()
        .success();
    let existing = local.comment();

    let mut server = mockito::Server::new();
    server
        .mock("POST", "/graphql")
        .with_body(comments_page(&[(1, "hi", true), (2, &existing, true)]))
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
    job.cmd().args(["step", "plan"]).assert().success();
    update.assert();
}

#[test]
fn a_comment_from_a_newer_run_is_not_overwritten() {
    let mut newer = Job::new();
    newer.run_id = "200".into();
    newer.cmd().args(["step", "plan"]).assert().success();
    let existing = newer.comment();

    let mut server = mockito::Server::new();
    server
        .mock("POST", "/graphql")
        .with_body(comments_page(&[(2, &existing, true)]))
        .create();
    let update = server
        .mock("PATCH", "/repos/o/r/issues/comments/2")
        .expect(0)
        .create();
    let job = Job::new().with_pull_request(&server.url()); // run 100
    job.cmd()
        .args(["exec", "plan", "exit 2", "--exit-code-map", "2=changes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already shows run 200"));
    update.assert();
    assert_eq!(job.read("github_output"), "status=changes\n");
}

#[test]
fn a_comment_from_an_older_release_keeps_its_steps() {
    let old = "| Plan | ✅ |\n\n<!-- status-comment:state {\"run_id\":\"100\",\"steps\":[{\"name\":\"old\",\"display_name\":\"Old\",\"status\":\"success\"}]} -->\n<!-- status-comment:key Terraform%2Fplan -->\n";
    let mut server = mockito::Server::new();
    server
        .mock("POST", "/graphql")
        .with_body(comments_page(&[(2, old, true)]))
        .create();
    let update = server
        .mock("PATCH", "/repos/o/r/issues/comments/2")
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex(r"\| Old \| ✅ Success \|".into()),
            Matcher::Regex(r"\| Plan \| ✅ Success \|".into()),
            Matcher::Regex(format!(
                r#"\\"version\\":\\"{}\\""#,
                env!("CARGO_PKG_VERSION")
            )),
        ]))
        .with_body(r#"{"id":2}"#)
        .create();
    let job = Job::new().with_pull_request(&server.url());
    job.cmd().args(["step", "plan"]).assert().success();
    update.assert();
}

#[test]
fn comments_by_other_authors_are_ignored() {
    let local = Job::new();
    local.cmd().args(["step", "forged"]).assert().success();
    let forged = local.comment();

    let mut server = mockito::Server::new();
    server
        .mock("POST", "/graphql")
        .with_body(comments_page(&[(1, &forged, false)]))
        .create();
    let create = server
        .mock("POST", "/repos/o/r/issues/5/comments")
        .match_body(Matcher::Regex("Plan".into()))
        .with_status(201)
        .with_body(r#"{"id":9}"#)
        .create();
    let job = Job::new().with_pull_request(&server.url());
    job.cmd().args(["step", "plan"]).assert().success();
    create.assert();
}

#[test]
fn github_api_failure_warns_without_changing_exit_code() {
    let mut server = mockito::Server::new();
    server.mock("POST", "/graphql").with_status(403).create();
    let job = Job::new().with_pull_request(&server.url());
    job.cmd()
        .args(["exec", "plan", "echo ok"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "::warning::status-comment: could not read the Status Comment",
        ));
    assert_eq!(job.read("github_output"), "status=success\n");
}

/// Local Mode: run on a laptop with no GitHub Actions environment at all.
fn local_cmd(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("status-comment").unwrap();
    cmd.env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .current_dir(dir);
    cmd
}

/// Local Mode keeps the comment in the current directory.
fn local_comment(dir: &Path) -> String {
    fs::read_to_string(dir.join("comment.md")).unwrap()
}

#[test]
fn local_mode_previews_the_comment() {
    let dir = tempfile::tempdir().unwrap();
    local_cmd(dir.path())
        .args(["steps", "set", "init", "plan"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "status-comment preview (Local Mode",
        ))
        .stderr(predicate::str::contains("### Local run\n\n<sub>"))
        .stderr(predicate::str::contains("| Plan | ⬜ Pending |"))
        .stderr(predicate::str::contains("<!-- status-comment:state "));
    // The same bytes as the saved comment.md, hidden markers included.
    let saved = local_comment(dir.path());
    local_cmd(dir.path())
        .arg("preview")
        .assert()
        .success()
        .stdout(saved);
    local_cmd(dir.path())
        .args(["exec", "plan", "echo hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hi"))
        .stderr(predicate::str::contains("| Plan | ✅ Success |"));
}

#[test]
fn default_template_works_as_a_template_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = local_cmd(dir.path())
        .arg("default-template")
        .output()
        .unwrap();
    assert!(!dir.path().join("config.json").exists());
    fs::write(dir.path().join("mine.md.j2"), out.stdout).unwrap();
    // A relative path is saved as absolute, in ./config.json.
    local_cmd(dir.path())
        .args(["config", "set", "template=mine.md.j2"])
        .assert()
        .success();
    let config = fs::read_to_string(dir.path().join("config.json")).unwrap();
    let saved = fs::canonicalize(dir.path().join("mine.md.j2")).unwrap();
    assert!(config.contains(saved.to_str().unwrap()), "{config}");
    local_cmd(dir.path())
        .args(["step", "plan"])
        .assert()
        .success()
        .stderr(predicate::str::contains("| Plan | ✅ Success |"));
}

#[test]
fn local_mode_never_calls_github() {
    // A token and a PR event lying around locally must not trigger API calls.
    let mut server = mockito::Server::new();
    let reads = server.mock("GET", Matcher::Any).expect(0).create();
    let writes = server.mock("POST", Matcher::Any).expect(0).create();
    let dir = tempfile::tempdir().unwrap();
    let event = dir.path().join("event.json");
    fs::write(&event, r#"{"pull_request":{"number":5}}"#).unwrap();
    local_cmd(dir.path())
        .env("GITHUB_TOKEN", "t")
        .env("GITHUB_REPOSITORY", "o/r")
        .env("GITHUB_EVENT_PATH", &event)
        .env("GITHUB_API_URL", server.url())
        .args(["step", "plan"])
        .assert()
        .success();
    reads.assert();
    writes.assert();
}

#[test]
fn local_mode_starts_a_new_run_after_finalize() {
    let dir = tempfile::tempdir().unwrap();
    local_cmd(dir.path())
        .args(["step", "plan"])
        .assert()
        .success();
    local_cmd(dir.path()).arg("finalize").assert().success();
    // Finalizing again keeps the same Run.
    local_cmd(dir.path()).arg("finalize").assert().success();
    assert!(local_comment(dir.path()).contains("| Plan |"));

    local_cmd(dir.path())
        .args(["step", "apply"])
        .assert()
        .success();
    let comment = local_comment(dir.path());
    assert!(comment.contains("| Apply |"), "{comment}");
    assert!(!comment.contains("| Plan |"), "{comment}");
    assert!(comment.contains(r#""run_id":"local-2""#), "{comment}");
}
