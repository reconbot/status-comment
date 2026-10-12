//! Where a Status Comment's body is read from and written to.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;

use crate::comment;

pub trait CommentStore {
    fn load(&mut self) -> Result<Option<String>>;
    fn save(&mut self, body: &str) -> Result<()>;
}

/// The "comment" as a file: used without a PR, and in Local Mode.
pub struct FileStore {
    pub path: PathBuf,
    pub key: String,
}

impl CommentStore for FileStore {
    /// A file holding another Comment Key's comment counts as empty.
    fn load(&mut self) -> Result<Option<String>> {
        match fs::read_to_string(&self.path) {
            Ok(body) => Ok(Some(body).filter(|b| comment::has_key_marker(b, &self.key))),
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
}

/// Lists a PR's comments, oldest first. `viewerDidAuthor` is whether the token
/// in use wrote the comment, which works for every kind of token.
const COMMENTS_QUERY: &str =
    "query($owner: String!, $repo: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      comments(first: 100, after: $cursor) {
        nodes { databaseId body viewerDidAuthor }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}";

#[derive(Deserialize)]
struct GraphqlResponse {
    data: Option<serde_json::Value>,
    errors: Option<Vec<GraphqlError>>,
}

#[derive(Deserialize)]
struct GraphqlError {
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentPage {
    nodes: Vec<CommentNode>,
    page_info: PageInfo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentNode {
    database_id: u64,
    body: String,
    viewer_did_author: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

/// The Status Comment on a pull request: the oldest comment this token wrote
/// that carries the Comment Key marker. Comments by anyone else are ignored, so
/// a copied marker can't feed it someone else's state.
pub struct GithubStore {
    agent: ureq::Agent,
    api_url: String,
    graphql_url: String,
    repository: String,
    pr_number: u64,
    token: String,
    key: String,
    comment_id: Option<u64>,
}

impl GithubStore {
    pub fn new(
        api_url: &str,
        graphql_url: &str,
        repository: &str,
        pr_number: u64,
        token: &str,
        key: &str,
    ) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .build()
            .new_agent();
        GithubStore {
            agent,
            api_url: api_url.trim_end_matches('/').to_string(),
            graphql_url: graphql_url.to_string(),
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

    fn comment_page(&self, cursor: Option<&str>) -> Result<CommentPage> {
        let (owner, repo) = self
            .repository
            .split_once('/')
            .context("GITHUB_REPOSITORY should be owner/repo")?;
        let request = json!({
            "query": COMMENTS_QUERY,
            "variables": { "owner": owner, "repo": repo, "number": self.pr_number, "cursor": cursor },
        });
        let response: GraphqlResponse = self
            .with_headers(self.agent.post(&self.graphql_url))
            .send_json(&request)
            .context("listing PR comments")?
            .body_mut()
            .read_json()
            .context("parsing PR comments")?;
        if let Some(error) = response.errors.as_ref().and_then(|e| e.first()) {
            bail!("listing PR comments: {}", error.message);
        }
        let page = response
            .data
            .and_then(|d| d.pointer("/repository/pullRequest/comments").cloned())
            .context("listing PR comments: no pull request in the response")?;
        serde_json::from_value(page).context("parsing PR comments")
    }
}

impl CommentStore for GithubStore {
    fn load(&mut self) -> Result<Option<String>> {
        let mut cursor = None;
        loop {
            let page = self.comment_page(cursor.as_deref())?;
            if let Some(found) = page
                .nodes
                .into_iter()
                .find(|c| c.viewer_did_author && comment::has_key_marker(&c.body, &self.key))
            {
                self.comment_id = Some(found.database_id);
                return Ok(Some(found.body));
            }
            if !page.page_info.has_next_page {
                return Ok(None);
            }
            cursor = page.page_info.end_cursor;
        }
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
        comment::compose("table", &State::new("1", 1), key)
    }

    #[test]
    fn file_store_ignores_another_keys_comment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("comment.md");
        fs::write(&path, body_for("other")).unwrap();
        let mut store = FileStore {
            path: path.clone(),
            key: "mine".into(),
        };
        assert_eq!(store.load().unwrap(), None);
        store.key = "other".into();
        assert!(store.load().unwrap().is_some());
    }

    /// A GraphQL page of comments: `(id, body, written by us)`.
    fn page(comments: &[(u64, &str, bool)], next: Option<&str>) -> String {
        let nodes: Vec<_> = comments
            .iter()
            .map(|(id, body, ours)| json!({ "databaseId": id, "body": body, "viewerDidAuthor": ours }))
            .collect();
        json!({ "data": { "repository": { "pullRequest": { "comments": {
            "nodes": nodes,
            "pageInfo": { "hasNextPage": next.is_some(), "endCursor": next },
        } } } } })
        .to_string()
    }

    fn store(server: &mockito::Server, key: &str) -> GithubStore {
        GithubStore::new(
            &server.url(),
            &format!("{}/graphql", server.url()),
            "o/r",
            5,
            "t0k",
            key,
        )
    }

    #[test]
    fn pages_through_comments() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/graphql")
            .match_body(Matcher::PartialJson(
                json!({ "variables": { "cursor": null } }),
            ))
            .with_body(page(&[(1, "x", true)], Some("c1")))
            .create();
        let second = server
            .mock("POST", "/graphql")
            .match_body(Matcher::PartialJson(
                json!({ "variables": { "cursor": "c1" } }),
            ))
            .with_body(page(&[(500, &body_for("k"), true)], None))
            .create();
        let mut store = store(&server, "k");
        assert!(store.load().unwrap().is_some());
        assert_eq!(store.comment_id, Some(500));
        second.assert();
    }
}
