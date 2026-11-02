use reqwest::Client;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
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

    // --- Tags ---

    pub async fn list_tags(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pagelen: Option<u32>,
        page: Option<u32>,
        sort: Option<&str>,
        q: Option<&str>,
    ) -> Result<Value, String> {
        let req = self.list_tags_request(workspace_override, repo_slug, pagelen, page, sort, q)?;
        self.send_json_request(req).await
    }

    fn list_tags_request(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pagelen: Option<u32>,
        page: Option<u32>,
        sort: Option<&str>,
        q: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let req = Self::paged(
            self.authed(self.http.get(format!("{base}/refs/tags"))),
            pagelen,
            page,
        );
        let req = query_nonblank(req, "sort", sort);
        Ok(query_nonblank(req, "q", q))
    }

    // --- Pull requests ---

    pub async fn list_pull_requests(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        state: Option<&str>,
        pagelen: Option<u32>,
        page: Option<u32>,
        sort: Option<&str>,
    ) -> Result<Value, String> {
        let req = self.list_pull_requests_request(
            workspace_override,
            repo_slug,
            state,
            pagelen,
            page,
            sort,
        )?;
        self.send_json_request(req).await
    }

    fn list_pull_requests_request(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        state: Option<&str>,
        pagelen: Option<u32>,
        page: Option<u32>,
        sort: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, String> {
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
        Ok(query_nonblank(req, "sort", sort))
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
        reviewers: Option<&[String]>,
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
        if let Some(revs) = reviewers.filter(|r| !r.is_empty()) {
            body.insert(
                "reviewers".into(),
                json!(
                    revs.iter()
                        .map(|uuid| json!({"uuid": uuid}))
                        .collect::<Vec<_>>()
                ),
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

    pub async fn update_pull_request(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pull_request_id: u32,
        title: Option<&str>,
        description: Option<&str>,
    ) -> Result<Value, String> {
        let title = title.map(|t| require_non_empty(t, "title")).transpose()?;
        if title.is_none() && description.is_none() {
            return Err("at least one of title or description must be provided".to_string());
        }

        // Bitbucket's PUT replaces the fields it receives and resets the ones it doesn't, so
        // every mutable field has to be carried over from the current PR, not just the edited one.
        let current = self
            .get_pull_request(workspace_override, repo_slug, pull_request_id)
            .await?;

        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pullrequests/{pull_request_id}");

        let mut body = serde_json::Map::new();
        let title = match title {
            Some(t) => t,
            None => current
                .get("title")
                .and_then(Value::as_str)
                .ok_or("could not read the current pull request title")?
                .to_string(),
        };
        body.insert("title".into(), json!(title));

        let description = match description {
            Some(d) => d.to_string(),
            None => current
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        };
        body.insert("description".into(), json!(description));

        if let Some(revs) = current.get("reviewers").and_then(Value::as_array) {
            let mut reviewers = Vec::with_capacity(revs.len());
            for r in revs {
                let uuid = r.get("uuid").and_then(Value::as_str).ok_or(
                    "a current reviewer has no uuid; refusing to update and drop reviewers",
                )?;
                reviewers.push(json!({ "uuid": uuid }));
            }
            body.insert("reviewers".into(), json!(reviewers));
        }

        if let Some(dest) = current
            .pointer("/destination/branch/name")
            .and_then(Value::as_str)
        {
            body.insert("destination".into(), json!({ "branch": { "name": dest } }));
        }
        if let Some(close) = current.get("close_source_branch").and_then(Value::as_bool) {
            body.insert("close_source_branch".into(), json!(close));
        }

        let req = self
            .http
            .put(&url)
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

    // --- Files ---

    pub async fn get_file(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        commit: &str,
        path: &str,
    ) -> Result<String, String> {
        let response = self
            .get_file_request(workspace_override, repo_slug, commit, path)?
            .send()
            .await
            .map_err(|e| format!("Bitbucket request failed: {e}"))?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Bitbucket response: {e}"))?;
        let text = String::from_utf8_lossy(&bytes).into_owned();

        if !status.is_success() {
            return Err(format!("Bitbucket returned {status}: {text}"));
        }
        Ok(text)
    }

    fn get_file_request(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        commit: &str,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!(
            "{base}/src/{}/{}",
            commit_path_segment(commit)?,
            encode_file_path(path)?
        );
        Ok(self
            .http
            .get(url)
            .basic_auth(&self.username, Some(&self.app_password))
            .header("Accept", "*/*"))
    }

    pub async fn file_history(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        commit: &str,
        path: &str,
        pagelen: Option<u32>,
        page: Option<&str>,
    ) -> Result<Value, String> {
        let req =
            self.file_history_request(workspace_override, repo_slug, commit, path, pagelen, page)?;
        self.send_json_request(req).await
    }

    fn file_history_request(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        commit: &str,
        path: &str,
        pagelen: Option<u32>,
        page: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!(
            "{base}/filehistory/{}/{}",
            commit_path_segment(commit)?,
            encode_file_path(path)?
        );
        let req = Self::paged(self.authed(self.http.get(url)), pagelen, None);
        Ok(query_nonblank(req, "page", page))
    }

    // --- Commits ---

    #[allow(clippy::too_many_arguments)]
    pub async fn list_commits(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        revision: Option<&str>,
        include: &[String],
        exclude: &[String],
        pagelen: Option<u32>,
        page: Option<u32>,
        ctx: Option<&str>,
    ) -> Result<Value, String> {
        let req = self.list_commits_request(
            workspace_override,
            repo_slug,
            revision,
            include,
            exclude,
            pagelen,
            page,
            ctx,
        )?;
        self.send_json_request(req).await
    }

    #[allow(clippy::too_many_arguments)]
    fn list_commits_request(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        revision: Option<&str>,
        include: &[String],
        exclude: &[String],
        pagelen: Option<u32>,
        page: Option<u32>,
        ctx: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let rev = revision.map(str::trim).filter(|s| !s.is_empty());
        let has_range = include.iter().chain(exclude).any(|v| !v.trim().is_empty());
        let url = match (rev, has_range) {
            (Some(rev), _) => format!("{base}/commits/{rev}"),
            (None, false) => format!("{base}/commits/HEAD"),
            (None, true) => format!("{base}/commits"),
        };
        let mut req = self.authed(self.http.get(url));
        for v in include {
            req = query_nonblank(req, "include", Some(v.as_str()));
        }
        for v in exclude {
            req = query_nonblank(req, "exclude", Some(v.as_str()));
        }
        req = Self::paged(req, pagelen, page);
        Ok(query_nonblank(req, "ctx", ctx))
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

    // --- Workspace members (user search) ---

    pub async fn search_workspace_members(
        &self,
        workspace_override: Option<&str>,
        query: Option<&str>,
        pagelen: Option<u32>,
        page: Option<u32>,
    ) -> Result<Value, String> {
        let ws = self.workspace(workspace_override);
        let url = format!(
            "{}/workspaces/{}/members",
            self.api_root,
            encode_path_segment(ws)
        );
        let mut req = self
            .http
            .get(&url)
            .basic_auth(&self.username, Some(&self.app_password))
            .header("Accept", "application/json");
        if let Some(q) = query.map(str::trim).filter(|s| !s.is_empty()) {
            // Bitbucket workspace members supports q= filtering on user fields
            req = req.query(&[(
                "q",
                format!("user.display_name ~ \"{q}\" OR user.nickname ~ \"{q}\""),
            )]);
        }
        if let Some(n) = pagelen {
            req = req.query(&[("pagelen", n.clamp(1, 100).to_string())]);
        }
        if let Some(p) = page {
            req = req.query(&[("page", p.to_string())]);
        }
        self.send_json_request(req).await
    }

    // --- Pipelines ---

    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.basic_auth(&self.username, Some(&self.app_password))
            .header("Accept", "application/json")
    }

    fn paged(
        mut req: reqwest::RequestBuilder,
        pagelen: Option<u32>,
        page: Option<u32>,
    ) -> reqwest::RequestBuilder {
        if let Some(n) = pagelen {
            req = req.query(&[("pagelen", n.clamp(1, 100).to_string())]);
        }
        if let Some(p) = page {
            req = req.query(&[("page", p.to_string())]);
        }
        req
    }

    pub async fn list_pipelines(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pagelen: Option<u32>,
        page: Option<u32>,
        target_branch: Option<&str>,
        raw: bool,
    ) -> Result<Value, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let req = self
            .authed(self.http.get(format!("{base}/pipelines")))
            .query(&[("sort", "-created_on")]);
        let value = self
            .send_json_request(Self::paged(req, pagelen, page))
            .await?;
        if raw {
            return Ok(value);
        }
        let branch = target_branch.map(str::trim).filter(|s| !s.is_empty());
        let items: Vec<Value> = value
            .get("values")
            .and_then(Value::as_array)
            .map(|v| v.iter().map(summarize_pipeline).collect())
            .unwrap_or_default();
        let items: Vec<Value> = match branch {
            Some(b) => items
                .into_iter()
                .filter(|p| p.get("ref_name").and_then(Value::as_str) == Some(b))
                .collect(),
            None => items,
        };
        Ok(json!({
            "page": value.get("page"),
            "pagelen": value.get("pagelen"),
            "size": value.get("size"),
            "filtered_by_branch": branch,
            "pipelines": items,
        }))
    }

    pub async fn get_pipeline(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pipeline: &str,
        raw: bool,
    ) -> Result<Value, String> {
        let pipeline = require_non_empty(pipeline, "pipeline")?;
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pipelines/{}", encode_path_segment(&pipeline));
        let value = self
            .send_json_request(self.authed(self.http.get(url)))
            .await?;
        Ok(if raw {
            value
        } else {
            summarize_pipeline(&value)
        })
    }

    pub async fn list_pipeline_steps(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pipeline: &str,
        raw: bool,
    ) -> Result<Value, String> {
        let pipeline = require_non_empty(pipeline, "pipeline")?;
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!("{base}/pipelines/{}/steps", encode_path_segment(&pipeline));
        let req = Self::paged(self.authed(self.http.get(url)), Some(100), None);
        let value = self.send_json_request(req).await?;
        if raw {
            return Ok(value);
        }
        let steps: Vec<Value> = value
            .get("values")
            .and_then(Value::as_array)
            .map(|v| v.iter().map(summarize_step).collect())
            .unwrap_or_default();
        Ok(json!({ "pipeline": pipeline, "steps": steps }))
    }

    pub async fn get_pipeline_step_log(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pipeline: &str,
        step_uuid: &str,
        tail_chars: Option<usize>,
    ) -> Result<Value, String> {
        let pipeline = require_non_empty(pipeline, "pipeline")?;
        let step_uuid = require_non_empty(step_uuid, "step_uuid")?;
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!(
            "{base}/pipelines/{}/steps/{}/log",
            encode_path_segment(&pipeline),
            encode_path_segment(&step_uuid)
        );
        let response = self
            .http
            .get(url)
            .basic_auth(&self.username, Some(&self.app_password))
            .header("Accept", "application/octet-stream, text/plain, */*;q=0.1")
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
        let total_chars = text.chars().count();
        let keep = tail_chars.unwrap_or(20_000).max(1);
        let log = if total_chars > keep {
            let skip = total_chars - keep;
            text.chars().skip(skip).collect::<String>()
        } else {
            text
        };
        Ok(json!({
            "step_uuid": step_uuid,
            "total_chars": total_chars,
            "returned_chars": log.chars().count(),
            "truncated_head": total_chars > keep,
            "log": log,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn trigger_pipeline(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        ref_name: &str,
        ref_type: Option<&str>,
        commit_hash: Option<&str>,
        custom_pipeline: Option<&str>,
        variables: Option<&[PipelineVariable]>,
    ) -> Result<Value, String> {
        let ref_name = require_non_empty(ref_name, "ref_name")?;
        let ref_type = ref_type
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("branch");
        if ref_type != "branch" && ref_type != "tag" {
            return Err("ref_type must be `branch` or `tag`".to_string());
        }
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;

        let mut target = serde_json::Map::new();
        target.insert("ref_type".into(), json!(ref_type));
        target.insert("ref_name".into(), json!(ref_name));
        match commit_hash.map(str::trim).filter(|s| !s.is_empty()) {
            Some(hash) => {
                target.insert("type".into(), json!("pipeline_commit_target"));
                target.insert("commit".into(), json!({ "type": "commit", "hash": hash }));
            }
            None => {
                target.insert("type".into(), json!("pipeline_ref_target"));
            }
        }
        if let Some(name) = custom_pipeline.map(str::trim).filter(|s| !s.is_empty()) {
            target.insert(
                "selector".into(),
                json!({ "type": "custom", "pattern": name }),
            );
        }

        let mut body = serde_json::Map::new();
        body.insert("target".into(), Value::Object(target));
        if let Some(vars) = variables.filter(|v| !v.is_empty()) {
            body.insert("variables".into(), json!(vars));
        }

        let req = self
            .authed(self.http.post(format!("{base}/pipelines")))
            .header("Content-Type", "application/json")
            .json(&Value::Object(body));
        let value = self.send_json_request(req).await?;
        Ok(summarize_pipeline(&value))
    }

    pub async fn stop_pipeline(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pipeline: &str,
    ) -> Result<Value, String> {
        let pipeline = require_non_empty(pipeline, "pipeline")?;
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let url = format!(
            "{base}/pipelines/{}/stopPipeline",
            encode_path_segment(&pipeline)
        );
        self.send_json_request_allow_empty(self.authed(self.http.post(url)))
            .await
    }

    // --- Deployments ---

    #[allow(clippy::too_many_arguments)]
    pub async fn list_deployments(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pagelen: Option<u32>,
        page: Option<u32>,
        raw: bool,
        environment: Option<&str>,
        sort: Option<&str>,
    ) -> Result<Value, String> {
        let req = self.list_deployments_request(
            workspace_override,
            repo_slug,
            pagelen,
            page,
            environment,
            sort,
        )?;
        let value = self.send_json_request(req).await?;
        if raw {
            return Ok(value);
        }
        let items: Vec<Value> = value
            .get("values")
            .and_then(Value::as_array)
            .map(|v| v.iter().map(summarize_deployment).collect())
            .unwrap_or_default();
        Ok(json!({
            "page": value.get("page"),
            "pagelen": value.get("pagelen"),
            "size": value.get("size"),
            "deployments": items,
        }))
    }

    fn list_deployments_request(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        pagelen: Option<u32>,
        page: Option<u32>,
        environment: Option<&str>,
        sort: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let sort = sort
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("-state.started_on");
        let req = self
            .authed(self.http.get(format!("{base}/deployments")))
            .query(&[("sort", sort)]);
        let req = query_nonblank(req, "environment", environment);
        Ok(Self::paged(req, pagelen, page))
    }

    pub async fn list_environments(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        raw: bool,
    ) -> Result<Value, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        let req = Self::paged(
            self.authed(self.http.get(format!("{base}/environments"))),
            Some(100),
            None,
        );
        let value = self.send_json_request(req).await?;
        if raw {
            return Ok(value);
        }
        let items: Vec<Value> = value
            .get("values")
            .and_then(Value::as_array)
            .map(|v| {
                v.iter()
                    .map(|e| {
                        json!({
                            "uuid": e.get("uuid"),
                            "name": e.get("name"),
                            "slug": e.get("slug"),
                            "type": e.pointer("/environment_type/name"),
                            "rank": e.get("rank"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(json!({ "environments": items }))
    }

    /// Accepts an environment UUID (`{...}`) or a name/slug and returns the UUID.
    async fn resolve_environment_uuid(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        environment: &str,
    ) -> Result<String, String> {
        let wanted = require_non_empty(environment, "environment")?;
        if wanted.starts_with('{') {
            return Ok(wanted);
        }
        let envs = self
            .list_environments(workspace_override, repo_slug, false)
            .await?;
        let matches: Vec<&Value> = envs
            .get("environments")
            .and_then(Value::as_array)
            .map(|v| {
                v.iter()
                    .filter(|e| {
                        ["name", "slug"].iter().any(|k| {
                            e.get(*k)
                                .and_then(Value::as_str)
                                .is_some_and(|s| s.eq_ignore_ascii_case(&wanted))
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        match matches.as_slice() {
            [one] => one
                .get("uuid")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| "environment has no uuid".to_string()),
            [] => Err(format!(
                "no deployment environment named `{wanted}`; use bitbucket_list_deployment_environments"
            )),
            _ => Err(format!(
                "several deployment environments match `{wanted}`; pass the UUID instead"
            )),
        }
    }

    // --- Pipeline variables (repository or deployment scope) ---

    async fn variables_root(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        environment: Option<&str>,
    ) -> Result<String, String> {
        let base = self.repo_root(self.workspace(workspace_override), repo_slug)?;
        match environment.map(str::trim).filter(|s| !s.is_empty()) {
            None => Ok(format!("{base}/pipelines_config/variables")),
            Some(env) => {
                let uuid = self
                    .resolve_environment_uuid(workspace_override, repo_slug, env)
                    .await?;
                Ok(format!(
                    "{base}/deployments_config/environments/{}/variables",
                    encode_path_segment(&uuid)
                ))
            }
        }
    }

    pub async fn list_variables(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        environment: Option<&str>,
        pagelen: Option<u32>,
        page: Option<u32>,
    ) -> Result<Value, String> {
        let root = self
            .variables_root(workspace_override, repo_slug, environment)
            .await?;
        let req = Self::paged(self.authed(self.http.get(&root)), pagelen, page);
        let value = self.send_json_request(req).await?;
        let items: Vec<Value> = value
            .get("values")
            .and_then(Value::as_array)
            .map(|v| v.iter().map(summarize_variable).collect())
            .unwrap_or_default();
        Ok(json!({
            "scope": environment.map(str::trim).filter(|s| !s.is_empty()).map_or("repository", |_| "deployment"),
            "environment": environment,
            "page": value.get("page"),
            "pagelen": value.get("pagelen"),
            "size": value.get("size"),
            "variables": items,
        }))
    }

    pub async fn create_variable(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        environment: Option<&str>,
        key: &str,
        value: &str,
        secured: bool,
    ) -> Result<Value, String> {
        let key = require_non_empty(key, "key")?;
        let root = self
            .variables_root(workspace_override, repo_slug, environment)
            .await?;
        let req = self
            .authed(self.http.post(&root))
            .header("Content-Type", "application/json")
            .json(&json!({ "key": key, "value": value, "secured": secured }));
        let created = self.send_json_request(req).await?;
        Ok(summarize_variable(&created))
    }

    /// `variable` is a UUID (`{...}`) or a key name; scans the scope's variable pages.
    async fn find_variable(&self, root: &str, variable: &str) -> Result<Value, String> {
        let wanted = require_non_empty(variable, "variable")?;
        let by_uuid = wanted.starts_with('{');
        // A just-created variable can take a moment to show up in the listing.
        for attempt in 0..3u8 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            }
            if let Some(found) = self.scan_variables(root, &wanted, by_uuid).await? {
                return Ok(found);
            }
        }
        Err(format!(
            "no pipeline variable matching `{wanted}` in this scope"
        ))
    }

    async fn scan_variables(
        &self,
        root: &str,
        wanted: &str,
        by_uuid: bool,
    ) -> Result<Option<Value>, String> {
        let mut page = 1u32;
        loop {
            let req = Self::paged(self.authed(self.http.get(root)), Some(100), Some(page));
            let value = self.send_json_request(req).await?;
            let values = value
                .get("values")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(found) = values.iter().find(|v| {
                let field = if by_uuid { "uuid" } else { "key" };
                v.get(field).and_then(Value::as_str) == Some(wanted)
            }) {
                return Ok(Some(found.clone()));
            }
            if values.is_empty() || value.get("next").is_none() || page >= 20 {
                return Ok(None);
            }
            page += 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_variable(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        environment: Option<&str>,
        variable: &str,
        key: Option<&str>,
        value: Option<&str>,
        secured: Option<bool>,
    ) -> Result<Value, String> {
        if key.is_none() && value.is_none() && secured.is_none() {
            return Err("at least one of key, value or secured must be provided".to_string());
        }
        let root = self
            .variables_root(workspace_override, repo_slug, environment)
            .await?;
        let current = self.find_variable(&root, variable).await?;
        let uuid = current
            .get("uuid")
            .and_then(Value::as_str)
            .ok_or("current variable has no uuid")?
            .to_string();
        let currently_secured = current
            .get("secured")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let mut body = serde_json::Map::new();
        let new_key = match key.map(|k| require_non_empty(k, "key")).transpose()? {
            Some(k) => k,
            None => current
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        };
        body.insert("key".into(), json!(new_key));
        body.insert(
            "secured".into(),
            json!(secured.unwrap_or(currently_secured)),
        );
        match value {
            Some(v) => {
                body.insert("value".into(), json!(v));
            }
            None if currently_secured => {
                return Err(
                    "this variable is secured, so Bitbucket cannot return its value; pass `value` to update it"
                        .to_string(),
                );
            }
            None => {
                body.insert(
                    "value".into(),
                    current.get("value").cloned().unwrap_or(json!("")),
                );
            }
        }

        let req = self
            .authed(
                self.http
                    .put(format!("{root}/{}", encode_path_segment(&uuid))),
            )
            .header("Content-Type", "application/json")
            .json(&Value::Object(body));
        let updated = self.send_json_request(req).await?;
        Ok(summarize_variable(&updated))
    }

    pub async fn delete_variable(
        &self,
        workspace_override: Option<&str>,
        repo_slug: &str,
        environment: Option<&str>,
        variable: &str,
    ) -> Result<Value, String> {
        let root = self
            .variables_root(workspace_override, repo_slug, environment)
            .await?;
        let current = self.find_variable(&root, variable).await?;
        let uuid = current
            .get("uuid")
            .and_then(Value::as_str)
            .ok_or("current variable has no uuid")?
            .to_string();
        let req = self.authed(
            self.http
                .delete(format!("{root}/{}", encode_path_segment(&uuid))),
        );
        self.send_json_request_allow_empty(req).await?;
        Ok(json!({
            "ok": true,
            "deleted": true,
            "uuid": uuid,
            "key": current.get("key"),
        }))
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

/// Variable passed to a triggered pipeline (`variables` in POST .../pipelines).
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct PipelineVariable {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub secured: bool,
}

fn summarize_pipeline(p: &Value) -> Value {
    json!({
        "uuid": p.get("uuid"),
        "build_number": p.get("build_number"),
        "state": p.pointer("/state/name"),
        "stage": p.pointer("/state/stage/name"),
        "result": p.pointer("/state/result/name"),
        "ref_type": p.pointer("/target/ref_type"),
        "ref_name": p.pointer("/target/ref_name"),
        "custom_pipeline": p.pointer("/target/selector/pattern"),
        "commit": p.pointer("/target/commit/hash"),
        "trigger": p.pointer("/trigger/name"),
        "creator": p.pointer("/creator/display_name"),
        "created_on": p.get("created_on"),
        "completed_on": p.get("completed_on"),
        "duration_in_seconds": p.get("duration_in_seconds"),
        "build_seconds_used": p.get("build_seconds_used"),
        "has_variables": p.get("has_variables"),
        "url": p.pointer("/repository/links/html/href").and_then(Value::as_str)
            .zip(p.get("build_number").and_then(Value::as_u64))
            .map(|(repo, n)| format!("{repo}/pipelines/results/{n}")),
    })
}

fn summarize_step(s: &Value) -> Value {
    json!({
        "uuid": s.get("uuid"),
        "name": s.get("name"),
        "state": s.pointer("/state/name"),
        "stage": s.pointer("/state/stage/name"),
        "result": s.pointer("/state/result/name"),
        "deployment_environment": s.pointer("/deployment_environment/name"),
        "trigger": s.pointer("/trigger/type"),
        "started_on": s.get("started_on"),
        "completed_on": s.get("completed_on"),
        "duration_in_seconds": s.get("duration_in_seconds"),
        "run_number": s.get("run_number"),
        "max_time": s.get("maxTime"),
    })
}

fn summarize_deployment(d: &Value) -> Value {
    json!({
        "uuid": d.get("uuid"),
        "environment": d.pointer("/environment/name"),
        "environment_uuid": d.pointer("/environment/uuid"),
        "state": d.pointer("/state/name"),
        "status": d.pointer("/state/status/name"),
        "started_on": d.pointer("/state/started_on"),
        "completed_on": d.pointer("/state/completed_on"),
        "pipeline_uuid": d.pointer("/release/pipeline/uuid"),
        "build_number": d
            .pointer("/release/name")
            .and_then(Value::as_str)
            .and_then(|n| n.trim_start_matches('#').parse::<u64>().ok()),
        "commit": d.pointer("/release/commit/hash"),
        "deployer": d.pointer("/state/deployer/display_name"),
        "url": d.pointer("/release/url"),
    })
}

fn summarize_variable(v: &Value) -> Value {
    json!({
        "uuid": v.get("uuid"),
        "key": v.get("key"),
        "value": v.get("value"),
        "secured": v.get("secured"),
        "scope": v.get("scope"),
    })
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

/// Percent-encode a repository file path per segment, keeping the `/` separators.
fn encode_file_path(path: &str) -> Result<String, String> {
    let trimmed = path.trim().trim_matches('/');
    if trimmed.is_empty() {
        return Err("path must not be empty".into());
    }
    let mut segments = Vec::new();
    for segment in trimmed.split('/') {
        if segment.is_empty() {
            return Err("path must not contain empty segments".into());
        }
        reject_dot_segment(segment, "path segment")?;
        segments.push(encode_path_segment(segment));
    }
    Ok(segments.join("/"))
}

/// Trim and percent-encode a commit, tag or branch for use as one path segment.
fn commit_path_segment(commit: &str) -> Result<String, String> {
    let commit = require_non_empty(commit, "commit")?;
    reject_dot_segment(&commit, "commit")?;
    Ok(encode_path_segment(&commit))
}

/// Rejects `.` and `..`, which URL parsing would resolve out of the endpoint path.
fn reject_dot_segment(segment: &str, name: &str) -> Result<(), String> {
    if segment == "." || segment == ".." {
        Err(format!("{name} must not be . or .."))
    } else {
        Ok(())
    }
}

/// Add `key=value` to the query when `value` is present and not blank.
fn query_nonblank(
    req: reqwest::RequestBuilder,
    key: &str,
    value: Option<&str>,
) -> reqwest::RequestBuilder {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => req.query(&[(key, v)]),
        None => req,
    }
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

    #[test]
    fn pipeline_summary_builds_web_url() {
        let p = json!({
            "build_number": 42,
            "state": {"name": "COMPLETED", "result": {"name": "SUCCESSFUL"}},
            "repository": {"links": {"html": {"href": "https://bitbucket.org/ws/repo"}}}
        });
        let s = summarize_pipeline(&p);
        assert_eq!(
            s["url"],
            "https://bitbucket.org/ws/repo/pipelines/results/42"
        );
        assert_eq!(s["result"], "SUCCESSFUL");
        assert!(s["stage"].is_null());
    }

    #[test]
    fn deployment_summary_parses_build_number_from_release_name() {
        let d = json!({"release": {"name": "#1164"}, "state": {"name": "COMPLETED"}});
        assert_eq!(summarize_deployment(&d)["build_number"], 1164);
    }

    fn test_client() -> BitbucketClient {
        let creds = BitbucketCredentials::new(
            "https://api.bitbucket.org/2.0".into(),
            "ws".into(),
            "stub_bb_user_001".into(),
            "stub_bb_token_001".into(),
        );
        BitbucketClient::new(Client::new(), &creds)
    }

    fn query_pairs(url: &reqwest::Url) -> Vec<(String, String)> {
        url.query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    #[test]
    fn list_tags_request_builds_query() {
        let req = test_client()
            .list_tags_request(None, "r", Some(2), Some(3), Some("-target.date"), None)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            req.url().as_str(),
            "https://api.bitbucket.org/2.0/repositories/ws/r/refs/tags?pagelen=2&page=3&sort=-target.date"
        );
    }

    #[test]
    fn list_tags_request_passes_q_and_skips_blank_sort() {
        let req = test_client()
            .list_tags_request(None, "r", None, None, Some("  "), Some("name ~ \"3.\""))
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            req.url().as_str(),
            "https://api.bitbucket.org/2.0/repositories/ws/r/refs/tags?q=name+%7E+%223.%22"
        );
        assert_eq!(
            query_pairs(req.url()),
            vec![("q".to_string(), "name ~ \"3.\"".to_string())]
        );
    }

    #[test]
    fn encode_file_path_keeps_slashes_and_escapes_segments() {
        assert_eq!(
            encode_file_path("app/Info.plist"),
            Ok("app/Info.plist".to_string())
        );
        assert_eq!(
            encode_file_path("/docs/my file#1.md"),
            Ok("docs/my%20file%231.md".to_string())
        );
    }

    #[test]
    fn encode_file_path_rejects_empty_segments() {
        assert_eq!(
            encode_file_path("a//b"),
            Err("path must not contain empty segments".to_string())
        );
        assert_eq!(
            encode_file_path("  "),
            Err("path must not be empty".to_string())
        );
    }

    #[test]
    fn encode_file_path_rejects_dot_segments() {
        assert_eq!(
            encode_file_path("a/../b"),
            Err("path segment must not be . or ..".to_string())
        );
        assert_eq!(
            encode_file_path("./a"),
            Err("path segment must not be . or ..".to_string())
        );
    }

    #[test]
    fn get_file_request_builds_url() {
        let req = test_client()
            .get_file_request(None, "r", "3.1.0", "app/Info.plist")
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            req.url().as_str(),
            "https://api.bitbucket.org/2.0/repositories/ws/r/src/3.1.0/app/Info.plist"
        );
        assert_eq!(req.headers()["accept"].to_str().unwrap(), "*/*");
    }

    #[test]
    fn get_file_request_rejects_dot_commit() {
        let err = test_client()
            .get_file_request(None, "r", "..", "a.txt")
            .unwrap_err();
        assert_eq!(err, "commit must not be . or ..");
    }

    #[test]
    fn file_history_request_passes_opaque_page_cursor() {
        let c = test_client();
        let req = c
            .file_history_request(None, "r", "dev", ".gitignore", Some(2), Some("E8Ux"))
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            req.url().as_str(),
            "https://api.bitbucket.org/2.0/repositories/ws/r/filehistory/dev/.gitignore?pagelen=2&page=E8Ux"
        );
        let plain = c
            .file_history_request(None, "r", "dev", "a", Some(500), None)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            plain.url().as_str(),
            "https://api.bitbucket.org/2.0/repositories/ws/r/filehistory/dev/a?pagelen=100"
        );
    }

    #[test]
    fn list_commits_request_unchanged_without_include() {
        let c = test_client();
        let head = c
            .list_commits_request(None, "r", None, &[], &[], None, None, None)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            head.url().as_str(),
            "https://api.bitbucket.org/2.0/repositories/ws/r/commits/HEAD"
        );
        let dev = c
            .list_commits_request(None, "r", Some("dev"), &[], &[], Some(5), Some(2), None)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            dev.url().as_str(),
            "https://api.bitbucket.org/2.0/repositories/ws/r/commits/dev?pagelen=5&page=2"
        );
        let blank = c
            .list_commits_request(
                None,
                "r",
                Some(" "),
                &[" ".to_string()],
                &[],
                None,
                None,
                None,
            )
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            blank.url().as_str(),
            "https://api.bitbucket.org/2.0/repositories/ws/r/commits/HEAD"
        );
    }

    #[test]
    fn list_commits_request_with_include_exclude_uses_bare_commits_path() {
        let req = test_client()
            .list_commits_request(
                None,
                "r",
                None,
                &["dev".to_string()],
                &["master".to_string(), "3.0.0".to_string()],
                Some(2),
                Some(2),
                Some("abc"),
            )
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            req.url().as_str(),
            "https://api.bitbucket.org/2.0/repositories/ws/r/commits?include=dev&exclude=master&exclude=3.0.0&pagelen=2&page=2&ctx=abc"
        );
    }

    #[test]
    fn list_pull_requests_request_adds_sort() {
        let c = test_client();
        let sorted = c
            .list_pull_requests_request(
                None,
                "r",
                Some("MERGED"),
                Some(50),
                None,
                Some("-updated_on"),
            )
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            sorted.url().as_str(),
            "https://api.bitbucket.org/2.0/repositories/ws/r/pullrequests?state=MERGED&pagelen=50&sort=-updated_on"
        );
        let unsorted = c
            .list_pull_requests_request(None, "r", Some("MERGED"), Some(50), None, None)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            unsorted.url().as_str(),
            "https://api.bitbucket.org/2.0/repositories/ws/r/pullrequests?state=MERGED&pagelen=50"
        );
    }

    #[test]
    fn list_deployments_request_default_sort_unchanged() {
        let req = test_client()
            .list_deployments_request(None, "r", None, None, None, None)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            req.url().as_str(),
            "https://api.bitbucket.org/2.0/repositories/ws/r/deployments?sort=-state.started_on"
        );
    }

    #[test]
    fn list_deployments_request_with_environment() {
        let req = test_client()
            .list_deployments_request(
                None,
                "r",
                Some(5),
                None,
                Some("{abc}"),
                Some("-state.started_on"),
            )
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            req.url().as_str(),
            "https://api.bitbucket.org/2.0/repositories/ws/r/deployments?sort=-state.started_on&environment=%7Babc%7D&pagelen=5"
        );
    }
}
