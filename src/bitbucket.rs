use reqwest::Client;
use serde_json::{Value, json};

use crate::credentials::BitbucketCredentials;

#[derive(Clone)]
pub struct BitbucketClient {
    http: Client,
    api_root: String,
    workspace: String,
    username: String,
    app_password: String,
}

impl BitbucketClient {
    pub fn new(http: Client, creds: &BitbucketCredentials) -> Self {
        Self {
            http,
            api_root: creds.api_root_trimmed(),
            workspace: creds.workspace.clone(),
            username: creds.username.clone(),
            app_password: creds.app_password.clone(),
        }
    }

    fn workspace<'a>(&'a self, override_ws: Option<&'a str>) -> &'a str {
        let o = override_ws.map(str::trim).filter(|s| !s.is_empty());
        o.unwrap_or(self.workspace.as_str())
    }

    fn repo_root(&self, workspace: &str, repo_slug: &str) -> Result<String, String> {
        let repo = require_non_empty(repo_slug, "repo_slug")?;
        Ok(format!(
            "{}/repositories/{}/{}",
            self.api_root,
            encode_path_segment(workspace),
            encode_path_segment(&repo)
        ))
    }

    // --- Repositories ---

    pub async fn list_repositories(
        &self,
        workspace_override: Option<&str>,
        pagelen: Option<u32>,
        page: Option<u32>,
        role: Option<&str>,
    ) -> Result<Value, String> {
        let ws = self.workspace(workspace_override);
        let url = format!("{}/repositories/{}", self.api_root, encode_path_segment(ws));
        let mut req = self
            .http
            .get(&url)
            .basic_auth(&self.username, Some(&self.app_password));
        req = req.header("Accept", "application/json");
        if let Some(n) = pagelen {
            req = req.query(&[("pagelen", n.clamp(1, 100).to_string())]);
        }
        if let Some(p) = page {
            req = req.query(&[("page", p.to_string())]);
        }
        if let Some(r) = role.map(str::trim).filter(|s| !s.is_empty()) {
            req = req.query(&[("role", r)]);
        }
        self.send_json_request(req).await
    }

    pub async fn get_repository(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
    ) -> Result<Value, String> {
        let url = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let req = self
            .http
            .get(&url)
            .basic_auth(&self.username, Some(&self.app_password))
            .header("Accept", "application/json");
        self.send_json_request(req).await
    }

    pub async fn list_branches(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pagelen: Option<u32>,
        page: Option<u32>,
        name_filter: Option<&str>,
    ) -> Result<Value, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/refs/branches");
        let mut req = self
            .http
            .get(&url)
            .basic_auth(&self.username, Some(&self.app_password));
        req = req.header("Accept", "application/json");
        if let Some(n) = pagelen {
            req = req.query(&[("pagelen", n.clamp(1, 100).to_string())]);
        }
        if let Some(p) = page {
            req = req.query(&[("page", p.to_string())]);
        }
        if let Some(q) = name_filter.map(str::trim).filter(|s| !s.is_empty()) {
            req = req.query(&[("q", format!("name~\"{q}\""))]);
        }
        self.send_json_request(req).await
    }

    // --- Pull requests ---

    pub async fn list_pull_requests(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        state: Option<&str>,
        pagelen: Option<u32>,
        page: Option<u32>,
    ) -> Result<Value, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pullrequests");
        let mut req = self
            .http
            .get(&url)
            .basic_auth(&self.username, Some(&self.app_password));
        req = req.header("Accept", "application/json");
        if let Some(s) = state.map(str::trim).filter(|s| !s.is_empty()) {
            req = req.query(&[("state", s)]);
        }
        if let Some(n) = pagelen {
            req = req.query(&[("pagelen", n.clamp(1, 100).to_string())]);
        }
        if let Some(p) = page {
            req = req.query(&[("page", p.to_string())]);
        }
        self.send_json_request(req).await
    }

    pub async fn get_pull_request(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pull_request_id: u32,
    ) -> Result<Value, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pullrequests/{pull_request_id}");
        let req = self
            .http
            .get(&url)
            .basic_auth(&self.username, Some(&self.app_password))
            .header("Accept", "application/json");
        self.send_json_request(req).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_pull_request(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        title: &str,
        description: Option<&str>,
        source_branch: &str,
        dest_branch: &str,
        close_source_branch: bool,
    ) -> Result<Value, String> {
        let title = require_non_empty(title, "title")?;
        let source_branch = require_non_empty(source_branch, "source_branch")?;
        let dest_branch = require_non_empty(dest_branch, "destination_branch")?;

        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pullrequests");

        let mut body = serde_json::Map::new();
        body.insert("title".into(), json!(title));
        if let Some(d) = description.map(str::trim).filter(|s| !s.is_empty()) {
            body.insert("description".into(), json!(d));
        }
        body.insert(
            "source".into(),
            json!({ "branch": { "name": source_branch } }),
        );
        body.insert(
            "destination".into(),
            json!({ "branch": { "name": dest_branch } }),
        );
        body.insert("close_source_branch".into(), json!(close_source_branch));

        let req = self
            .http
            .post(&url)
            .basic_auth(&self.username, Some(&self.app_password))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&Value::Object(body));

        self.send_json_request(req).await
    }

    pub async fn get_pull_request_diff(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pull_request_id: u32,
    ) -> Result<String, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pullrequests/{pull_request_id}/diff");
        let response = self
            .http
            .get(&url)
            .basic_auth(&self.username, Some(&self.app_password))
            .header(
                "Accept",
                "text/x-diff, text/plain, application/json, */*;q=0.1",
            )
            .send()
            .await
            .map_err(|e| format!("Bitbucket request failed: {e}"))?;

        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| format!("Failed to read Bitbucket response: {e}"))?;

        if !status.is_success() {
            return Err(format!("Bitbucket returned {status}: {text}"));
        }
        Ok(text)
    }

    pub async fn get_pull_request_diffstat(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pull_request_id: u32,
        pagelen: Option<u32>,
        page: Option<u32>,
    ) -> Result<Value, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pullrequests/{pull_request_id}/diffstat");
        let mut req = self
            .http
            .get(&url)
            .basic_auth(&self.username, Some(&self.app_password));
        req = req.header("Accept", "application/json");
        if let Some(n) = pagelen {
            req = req.query(&[("pagelen", n.clamp(1, 100).to_string())]);
        }
        if let Some(p) = page {
            req = req.query(&[("page", p.to_string())]);
        }
        self.send_json_request(req).await
    }

    pub async fn list_pull_request_comments(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pull_request_id: u32,
        pagelen: Option<u32>,
        page: Option<u32>,
    ) -> Result<Value, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pullrequests/{pull_request_id}/comments");
        let mut req = self
            .http
            .get(&url)
            .basic_auth(&self.username, Some(&self.app_password));
        req = req.header("Accept", "application/json");
        if let Some(n) = pagelen {
            req = req.query(&[("pagelen", n.clamp(1, 100).to_string())]);
        }
        if let Some(p) = page {
            req = req.query(&[("page", p.to_string())]);
        }
        self.send_json_request(req).await
    }

    pub async fn get_pull_request_comment(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pull_request_id: u32,
        comment_id: u64,
    ) -> Result<Value, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pullrequests/{pull_request_id}/comments/{comment_id}");
        let req = self
            .http
            .get(&url)
            .basic_auth(&self.username, Some(&self.app_password))
            .header("Accept", "application/json");
        self.send_json_request(req).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_pull_request_comment(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pull_request_id: u32,
        content_raw: &str,
        parent_comment_id: Option<u64>,
        inline_path: Option<&str>,
        inline_to: Option<u32>,
    ) -> Result<Value, String> {
        let content_raw = require_non_empty(content_raw, "content")?;

        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pullrequests/{pull_request_id}/comments");

        let mut body = serde_json::Map::new();
        body.insert("content".into(), json!({ "raw": content_raw }));
        if let Some(pid) = parent_comment_id {
            body.insert("parent".into(), json!({ "id": pid }));
        }
        if let (Some(path), Some(to)) = (
            inline_path.map(str::trim).filter(|s| !s.is_empty()),
            inline_to,
        ) {
            body.insert(
                "inline".into(),
                json!({
                    "path": path,
                    "to": to,
                }),
            );
        }

        let req = self
            .http
            .post(&url)
            .basic_auth(&self.username, Some(&self.app_password))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&Value::Object(body));

        self.send_json_request(req).await
    }

    pub async fn update_pull_request_comment(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pull_request_id: u32,
        comment_id: u64,
        content_raw: &str,
    ) -> Result<Value, String> {
        let content_raw = require_non_empty(content_raw, "content")?;

        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pullrequests/{pull_request_id}/comments/{comment_id}");
        let body = json!({ "content": { "raw": content_raw } });

        let req = self
            .http
            .put(&url)
            .basic_auth(&self.username, Some(&self.app_password))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&body);

        self.send_json_request(req).await
    }

    pub async fn delete_pull_request_comment(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pull_request_id: u32,
        comment_id: u64,
    ) -> Result<Value, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pullrequests/{pull_request_id}/comments/{comment_id}");
        let req = self
            .http
            .delete(&url)
            .basic_auth(&self.username, Some(&self.app_password))
            .header("Accept", "application/json");

        let response = req
            .send()
            .await
            .map_err(|e| format!("Bitbucket request failed: {e}"))?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Bitbucket response: {e}"))?;

        if status == reqwest::StatusCode::NO_CONTENT || bytes.is_empty() {
            return Ok(json!({ "ok": true, "deleted": true }));
        }
        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(format!("Bitbucket returned {status}: {msg}"));
        }
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid JSON from Bitbucket: {e}"))
    }

    // --- Commits ---

    pub async fn list_commits(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        revision: Option<&str>,
        pagelen: Option<u32>,
        page: Option<u32>,
    ) -> Result<Value, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let rev = revision
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("HEAD");
        let url = format!("{base}/commits/{rev}");
        let mut req = self
            .http
            .get(&url)
            .basic_auth(&self.username, Some(&self.app_password));
        req = req.header("Accept", "application/json");
        if let Some(n) = pagelen {
            req = req.query(&[("pagelen", n.clamp(1, 100).to_string())]);
        }
        if let Some(p) = page {
            req = req.query(&[("page", p.to_string())]);
        }
        self.send_json_request(req).await
    }

    pub async fn get_commit(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        commit: &str,
    ) -> Result<Value, String> {
        let commit = require_non_empty(commit, "commit")?;
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/commit/{commit}");
        let req = self
            .http
            .get(&url)
            .basic_auth(&self.username, Some(&self.app_password))
            .header("Accept", "application/json");
        self.send_json_request(req).await
    }

    // --- PR workflow ---

    pub async fn approve_pull_request(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pull_request_id: u32,
    ) -> Result<Value, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pullrequests/{pull_request_id}/approve");
        let req = self
            .http
            .post(&url)
            .basic_auth(&self.username, Some(&self.app_password))
            .header("Accept", "application/json");
        self.send_json_request(req).await
    }

    pub async fn unapprove_pull_request(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pull_request_id: u32,
    ) -> Result<Value, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pullrequests/{pull_request_id}/approve");
        let req = self
            .http
            .delete(&url)
            .basic_auth(&self.username, Some(&self.app_password))
            .header("Accept", "application/json");
        self.send_json_request_allow_empty(req).await
    }

    pub async fn decline_pull_request(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pull_request_id: u32,
        message: Option<&str>,
    ) -> Result<Value, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pullrequests/{pull_request_id}/decline");
        let body = message
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|m| json!({ "message": m }))
            .unwrap_or_else(|| json!({}));

        let req = self
            .http
            .post(&url)
            .basic_auth(&self.username, Some(&self.app_password))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&body);

        self.send_json_request(req).await
    }

    pub async fn merge_pull_request(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pull_request_id: u32,
        merge_strategy: Option<&str>,
        close_source_branch: Option<bool>,
        message: Option<&str>,
    ) -> Result<Value, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pullrequests/{pull_request_id}/merge");

        let mut body = serde_json::Map::new();
        if let Some(ms) = merge_strategy.map(str::trim).filter(|s| !s.is_empty()) {
            body.insert("merge_strategy".into(), json!(ms));
        }
        if let Some(c) = close_source_branch {
            body.insert("close_source_branch".into(), json!(c));
        }
        if let Some(m) = message.map(str::trim).filter(|s| !s.is_empty()) {
            body.insert("message".into(), json!(m));
        }

        let req = self
            .http
            .post(&url)
            .basic_auth(&self.username, Some(&self.app_password))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&Value::Object(body));

        self.send_json_request(req).await
    }

    pub async fn list_pull_request_activity(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pull_request_id: u32,
        pagelen: Option<u32>,
        page: Option<u32>,
    ) -> Result<Value, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pullrequests/{pull_request_id}/activity");
        let mut req = self
            .http
            .get(&url)
            .basic_auth(&self.username, Some(&self.app_password));
        req = req.header("Accept", "application/json");
        if let Some(n) = pagelen {
            req = req.query(&[("pagelen", n.clamp(1, 100).to_string())]);
        }
        if let Some(p) = page {
            req = req.query(&[("page", p.to_string())]);
        }
        self.send_json_request(req).await
    }

    async fn send_json_request(&self, req: reqwest::RequestBuilder) -> Result<Value, String> {
        let response = req
            .send()
            .await
            .map_err(|e| format!("Bitbucket request failed: {e}"))?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Bitbucket response: {e}"))?;

        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(format!("Bitbucket returned {status}: {msg}"));
        }

        if bytes.is_empty() {
            return Ok(json!({ "ok": true }));
        }

        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid JSON from Bitbucket: {e}"))
    }

    async fn send_json_request_allow_empty(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<Value, String> {
        let response = req
            .send()
            .await
            .map_err(|e| format!("Bitbucket request failed: {e}"))?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Bitbucket response: {e}"))?;

        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(format!("Bitbucket returned {status}: {msg}"));
        }

        if bytes.is_empty() {
            return Ok(json!({ "ok": true }));
        }

        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid JSON from Bitbucket: {e}"))
    }
}

fn require_non_empty(s: &str, name: &str) -> Result<String, String> {
    let t = s.trim();
    if t.is_empty() {
        Err(format!("{name} must not be empty"))
    } else {
        Ok(t.to_string())
    }
}

/// Percent-encode path segments for Bitbucket workspace and repo slugs.
fn encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_allows_slug_chars() {
        assert_eq!(encode_path_segment("my-workspace"), "my-workspace");
        assert_eq!(encode_path_segment("my.repo"), "my.repo");
    }

    #[test]
    fn encode_escapes_slash() {
        assert_eq!(encode_path_segment("a/b"), "a%2Fb");
    }
}
