//! Where a Status Comment's body is read from and written to.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;

use crate::comment;

pub trait CommentStore {
    fn load(&mut self) -> Result<Option<String>>;
    fn save(&mut self, body: &str) -> Result<()>;
}

/// The fallback when there's no PR: the "comment" is a file local to the job.
pub struct FileStore {
    pub path: PathBuf,
}

impl CommentStore for FileStore {
    fn load(&mut self) -> Result<Option<String>> {
        match fs::read_to_string(&self.path) {
            Ok(body) => Ok(Some(body)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", self.path.display())),
        }
    }

    fn save(&mut self, body: &str) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&self.path, body).with_context(|| format!("writing {}", self.path.display()))
    }
}

#[derive(Deserialize)]
struct IssueComment {
    id: u64,
    body: Option<String>,
}

/// The Status Comment on a pull request, found by its Comment Key marker.
pub struct GithubStore {
    agent: ureq::Agent,
    api_url: String,
    repository: String,
    pr_number: u64,
    token: String,
    key: String,
    comment_id: Option<u64>,
}

const PER_PAGE: usize = 100;

impl GithubStore {
    pub fn new(api_url: &str, repository: &str, pr_number: u64, token: &str, key: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .build()
            .new_agent();
        GithubStore {
            agent,
            api_url: api_url.trim_end_matches('/').to_string(),
            repository: repository.to_string(),
            pr_number,
            token: token.to_string(),
            key: key.to_string(),
            comment_id: None,
        }
    }

    fn with_headers<B>(&self, request: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        request
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "status-comment")
    }
}

impl CommentStore for GithubStore {
    /// Comments are listed oldest first, so the first match is the oldest.
    fn load(&mut self) -> Result<Option<String>> {
        for page in 1.. {
            let url = format!(
                "{}/repos/{}/issues/{}/comments?per_page={PER_PAGE}&page={page}",
                self.api_url, self.repository, self.pr_number
            );
            let comments: Vec<IssueComment> = self
                .with_headers(self.agent.get(&url))
                .call()
                .context("listing PR comments")?
                .body_mut()
                .read_json()
                .context("parsing PR comments")?;
            for c in &comments {
                if let Some(body) = &c.body
                    && comment::has_key_marker(body, &self.key)
                {
                    self.comment_id = Some(c.id);
                    return Ok(Some(body.clone()));
                }
            }
            if comments.len() < PER_PAGE {
                break;
            }
        }
        Ok(None)
    }

    fn save(&mut self, body: &str) -> Result<()> {
        let payload = json!({ "body": body });
        match self.comment_id {
            Some(id) => {
                let url = format!(
                    "{}/repos/{}/issues/comments/{id}",
                    self.api_url, self.repository
                );
                self.with_headers(self.agent.patch(&url))
                    .send_json(&payload)
                    .context("updating the Status Comment")?;
            }
            None => {
                let url = format!(
                    "{}/repos/{}/issues/{}/comments",
                    self.api_url, self.repository, self.pr_number
                );
                let created: IssueComment = self
                    .with_headers(self.agent.post(&url))
                    .send_json(&payload)
                    .context("creating the Status Comment")?
                    .body_mut()
                    .read_json()?;
                self.comment_id = Some(created.id);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;
    use mockito::Matcher;

    fn body_for(key: &str) -> String {
        comment::compose("table", &State::new("1"), key)
    }

    #[test]
    fn file_store_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = FileStore {
            path: dir.path().join("a/b/comment.md"),
        };
        assert_eq!(store.load().unwrap(), None);
        store.save("hello").unwrap();
        assert_eq!(store.load().unwrap().as_deref(), Some("hello"));
    }

    #[test]
    fn finds_oldest_matching_comment_and_updates_it() {
        let mut server = mockito::Server::new();
        let comments = json!([
            { "id": 1, "body": "unrelated" },
            { "id": 2, "body": body_for("wf/plan") },
            { "id": 3, "body": body_for("wf/plan") },
        ]);
        let list = server
            .mock("GET", "/repos/o/r/issues/5/comments")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("per_page".into(), "100".into()),
                Matcher::UrlEncoded("page".into(), "1".into()),
            ]))
            .match_header("authorization", "Bearer t0k")
            .with_body(comments.to_string())
            .create();
        let update = server
            .mock("PATCH", "/repos/o/r/issues/comments/2")
            .match_body(Matcher::Json(json!({ "body": "new" })))
            .with_body(r#"{"id":2,"body":"new"}"#)
            .create();

        let mut store = GithubStore::new(&server.url(), "o/r", 5, "t0k", "wf/plan");
        assert!(store.load().unwrap().unwrap().contains("table"));
        store.save("new").unwrap();
        list.assert();
        update.assert();
    }

    #[test]
    fn pages_through_comments() {
        let mut server = mockito::Server::new();
        let page1: Vec<_> = (0..100).map(|i| json!({ "id": i, "body": "x" })).collect();
        server
            .mock("GET", "/repos/o/r/issues/5/comments")
            .match_query(Matcher::UrlEncoded("page".into(), "1".into()))
            .with_body(serde_json::Value::from(page1).to_string())
            .create();
        let page2 = server
            .mock("GET", "/repos/o/r/issues/5/comments")
            .match_query(Matcher::UrlEncoded("page".into(), "2".into()))
            .with_body(json!([{ "id": 500, "body": body_for("k") }]).to_string())
            .create();
        let mut store = GithubStore::new(&server.url(), "o/r", 5, "t", "k");
        assert!(store.load().unwrap().is_some());
        assert_eq!(store.comment_id, Some(500));
        page2.assert();
    }

    #[test]
    fn creates_comment_when_none_matches() {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/repos/o/r/issues/5/comments")
            .match_query(Matcher::Any)
            .with_body("[]")
            .create();
        let create = server
            .mock("POST", "/repos/o/r/issues/5/comments")
            .match_body(Matcher::Json(json!({ "body": "first" })))
            .with_status(201)
            .with_body(r#"{"id":9,"body":"first"}"#)
            .create();
        let update = server
            .mock("PATCH", "/repos/o/r/issues/comments/9")
            .with_body(r#"{"id":9,"body":"second"}"#)
            .create();

        let mut store = GithubStore::new(&server.url(), "o/r", 5, "t", "k");
        assert_eq!(store.load().unwrap(), None);
        store.save("first").unwrap();
        store.save("second").unwrap();
        create.assert();
        update.assert();
    }

    #[test]
    fn api_errors_surface() {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/repos/o/r/issues/5/comments")
            .match_query(Matcher::Any)
            .with_status(403)
            .create();
        let mut store = GithubStore::new(&server.url(), "o/r", 5, "t", "k");
        assert!(store.load().is_err());
    }
}
