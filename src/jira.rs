use std::time::Duration;

use reqwest::Client;
use reqwest::multipart;
use serde_json::{Value, json};

use crate::credentials::AtlassianCredentials;

const COMMENT_PAGE_SIZE: u32 = 100;
const HTTP_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct JiraClient {
    http: Client,
    api_root: String,
    agile_root: String,
    email: String,
    token: String,
}

impl JiraClient {
    pub fn new(http: Client, creds: &AtlassianCredentials) -> Self {
        Self {
            http,
            api_root: creds.jira_rest_v3_root(),
            agile_root: creds.agile_root(),
            email: creds.email.clone(),
            token: creds.api_token.clone(),
        }
    }

    pub fn build_http_client() -> Result<Client, String> {
        Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(concat!("atlassian-mcp/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())
    }

    pub async fn get_issue_for_ai(
        &self,
        issue_key: &str,
        include_adf: bool,
    ) -> Result<Value, String> {
        let key = issue_key.trim();
        if key.is_empty() {
            return Err("issue_key must not be empty".into());
        }

        let enc = encode_path_segment(key);
        let issue_url = format!(
            "{}/issue/{}?fields=summary,description,attachment,status",
            self.api_root, enc
        );

        let issue: Value = self.get_json(&issue_url).await?;

        let issue_key_out = issue
            .get("key")
            .and_then(|k| k.as_str())
            .unwrap_or(key)
            .to_string();

        let fields = issue
            .get("fields")
            .and_then(|f| f.as_object())
            .ok_or_else(|| "Jira issue response missing fields".to_string())?;

        let summary = fields
            .get("summary")
            .and_then(|s| s.as_str())
            .map(str::to_string);

        let description_raw = fields.get("description").filter(|d| !d.is_null());
        let description = description_raw.and_then(adf_to_plain);
        let description_adf = if include_adf {
            description_raw.cloned()
        } else {
            None
        };

        let status = fields.get("status").map(slim_status);

        let attachments = fields
            .get("attachment")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| a.as_object().map(slim_attachment))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let mut slim_comments: Vec<Value> = Vec::new();
        let mut start_at: u32 = 0;
        loop {
            let page_url = format!(
                "{}/issue/{}/comment?startAt={}&maxResults={}",
                self.api_root, enc, start_at, COMMENT_PAGE_SIZE
            );
            let page: Value = self.get_json(&page_url).await?;
            let comments = page
                .get("comments")
                .and_then(|c| c.as_array())
                .cloned()
                .unwrap_or_default();
            let total = page.get("total").and_then(|t| t.as_u64());
            let batch_len = comments.len() as u32;

            for c in comments {
                if let Some(obj) = c.as_object() {
                    slim_comments.push(slim_comment(obj, include_adf));
                }
            }

            if batch_len == 0 {
                break;
            }

            let fetched = u64::from(start_at) + u64::from(batch_len);
            match total {
                Some(t) if fetched >= t => break,
                None if batch_len < COMMENT_PAGE_SIZE => break,
                _ => {}
            }

            start_at += batch_len;
        }

        let mut out = json!({
            "key": issue_key_out,
            "summary": summary,
            "description": description,
            "status": status,
            "attachments": attachments,
            "comments": {
                "total": slim_comments.len(),
                "comments": slim_comments,
            },
        });
        if include_adf && let Some(obj) = out.as_object_mut() {
            obj.insert(
                "description_adf".into(),
                description_adf.unwrap_or(Value::Null),
            );
        }

        Ok(out)
    }

    pub async fn add_comment_with_body(
        &self,
        issue_key: &str,
        body: Value,
    ) -> Result<Value, String> {
        let key = issue_key.trim();
        if key.is_empty() {
            return Err("issue_key must not be empty".into());
        }

        let url = format!(
            "{}/issue/{}/comment",
            self.api_root,
            encode_path_segment(key)
        );
        let payload = json!({ "body": body });

        self.post_json(&url, &payload).await
    }

    pub async fn update_description_with_adf(
        &self,
        issue_key: &str,
        description: Value,
    ) -> Result<Value, String> {
        let key = issue_key.trim();
        if key.is_empty() {
            return Err("issue_key must not be empty".into());
        }

        let url = format!("{}/issue/{}", self.api_root, encode_path_segment(key));
        let payload = json!({
            "fields": {
                "description": description
            }
        });

        self.put_json(&url, &payload).await
    }

    /// Replace the issue summary (`PUT /rest/api/3/issue/{key}`).
    pub async fn update_summary(&self, issue_key: &str, summary: &str) -> Result<Value, String> {
        let key = issue_key.trim();
        if key.is_empty() {
            return Err("issue_key must not be empty".into());
        }

        let url = format!("{}/issue/{}", self.api_root, encode_path_segment(key));
        let payload = summary_payload(summary)?;

        self.put_json(&url, &payload).await
    }

    /// Create a Jira issue (`POST /rest/api/3/issue`). Description is omitted when `None`.
    pub async fn create_issue(
        &self,
        project_key: &str,
        issue_type_name: &str,
        summary: &str,
        description: Option<Value>,
    ) -> Result<Value, String> {
        let project_key = project_key.trim();
        if project_key.is_empty() {
            return Err("project_key must not be empty".into());
        }
        let issue_type_name = issue_type_name.trim();
        if issue_type_name.is_empty() {
            return Err("issue_type must not be empty".into());
        }
        let summary = summary.trim();
        if summary.is_empty() {
            return Err("summary must not be empty".into());
        }

        let url = format!("{}/issue", self.api_root);
        let mut fields = serde_json::Map::new();
        fields.insert("project".into(), json!({ "key": project_key }));
        fields.insert("issuetype".into(), json!({ "name": issue_type_name }));
        fields.insert("summary".into(), json!(summary));
        if let Some(desc) = description {
            fields.insert("description".into(), desc);
        }

        let payload = json!({ "fields": Value::Object(fields) });
        self.post_json(&url, &payload).await
    }

    /// Search issues with JQL via `POST /rest/api/3/search/jql` (replaces removed GET `/search`).
    /// Paginates with `nextPageToken` so `start_at` still works.
    pub async fn search_jql(
        &self,
        jql: &str,
        max_results: u32,
        start_at: u32,
    ) -> Result<Value, String> {
        let jql = jql.trim();
        if jql.is_empty() {
            return Err("jql must not be empty".into());
        }
        let max = max_results.clamp(1, 100);
        let url = format!("{}/search/jql", self.api_root);

        const PAGE_SIZE: u32 = 100;
        const MAX_PAGES: u32 = 50;

        let mut collected: Vec<Value> = Vec::new();
        let mut next_cursor: Option<String> = None;
        let mut global_index: u32 = 0;
        let mut pages = 0u32;
        let mut last_raw: Option<Value> = None;

        while collected.len() < max as usize && pages < MAX_PAGES {
            pages += 1;
            let mut body = serde_json::Map::new();
            body.insert("jql".into(), json!(jql));
            body.insert("maxResults".into(), json!(PAGE_SIZE.min(100)));
            body.insert(
                "fields".into(),
                json!(vec![
                    "key",
                    "summary",
                    "status",
                    "issuetype",
                    "assignee",
                    "updated"
                ]),
            );
            if let Some(ref t) = next_cursor {
                body.insert("nextPageToken".into(), json!(t));
            }

            let raw = self
                .post_json(&url, &Value::Object(body))
                .await
                .map_err(|e| format!("Jira search/jql failed: {e}"))?;

            let issues = raw
                .get("issues")
                .and_then(|i| i.as_array())
                .cloned()
                .unwrap_or_default();

            if issues.is_empty() {
                last_raw = Some(raw);
                break;
            }

            for issue in issues {
                if global_index >= start_at
                    && collected.len() < max as usize
                    && let Some(obj) = issue.as_object()
                {
                    collected.push(slim_search_issue(obj));
                }
                global_index += 1;
            }

            let is_last = raw.get("isLast").and_then(|v| v.as_bool()).unwrap_or(true);
            next_cursor = raw
                .get("nextPageToken")
                .and_then(|t| t.as_str())
                .map(str::to_string);

            last_raw = Some(raw);

            if collected.len() >= max as usize || is_last {
                break;
            }
            if next_cursor.is_none() {
                break;
            }
        }

        let last = last_raw.unwrap_or(json!({}));
        let total = last
            .get("total")
            .cloned()
            .or_else(|| last.get("totalIssueCount").cloned());

        let is_last_out = last.get("isLast").and_then(|v| v.as_bool()).unwrap_or(true);
        let next_out = last
            .get("nextPageToken")
            .and_then(|t| t.as_str())
            .map(str::to_string);

        Ok(json!({
            "startAt": start_at,
            "maxResults": max,
            "total": total,
            "issues": collected,
            "isLast": is_last_out,
            "nextPageToken": next_out,
        }))
    }

    /// List Jira Agile boards, optionally filtered to one project. GET /rest/agile/1.0/board.
    pub async fn list_boards(
        &self,
        project_key: Option<&str>,
        max_results: u32,
        start_at: u32,
    ) -> Result<Value, String> {
        let max = max_results.clamp(1, 50);
        let mut url = format!(
            "{}/board?maxResults={}&startAt={}",
            self.agile_root, max, start_at
        );
        if let Some(pk) = project_key {
            let pk = pk.trim();
            if !pk.is_empty() {
                url.push_str(&format!("&projectKeyOrId={}", encode_path_segment(pk)));
            }
        }
        self.get_json(&url).await
    }

    /// `/board/{id}/issue` also includes backlog issues (Jira API quirk).
    /// Returns (issues, truncated) -- truncated means MAX_PAGES cut it off before `total`.
    pub async fn paginate_board_endpoint(
        &self,
        board_id: u64,
        endpoint: &str,
        jql: Option<&str>,
    ) -> Result<(Vec<Value>, bool), String> {
        const PAGE_SIZE: u32 = 100;
        const MAX_PAGES: u32 = 50;

        let mut collected: Vec<Value> = Vec::new();
        let mut start_at: u32 = 0;
        let mut total: Option<u64> = None;

        for _ in 0..MAX_PAGES {
            let mut url = format!(
                "{}/board/{}/{}?maxResults={}&startAt={}&fields=key,summary,status,issuetype,assignee,updated",
                self.agile_root, board_id, endpoint, PAGE_SIZE, start_at
            );
            if let Some(j) = jql {
                url.push_str(&format!("&jql={}", encode_path_segment(j)));
            }

            let raw: Value = self.get_json(&url).await?;
            if total.is_none() {
                total = raw.get("total").and_then(|t| t.as_u64());
            }
            let issues = raw
                .get("issues")
                .and_then(|i| i.as_array())
                .cloned()
                .unwrap_or_default();
            let page_len = issues.len() as u32;
            collected.extend(issues);

            if page_len == 0 {
                break;
            }
            start_at += page_len;

            if total.is_some_and(|t| collected.len() as u64 >= t) {
                break;
            }
        }

        let truncated = total.is_some_and(|t| (collected.len() as u64) < t);

        Ok((collected, truncated))
    }

    /// Move issues from the backlog onto a board. POST /rest/agile/1.0/board/{id}/issue. 204 on success.
    pub async fn move_issues_to_board(
        &self,
        board_id: u64,
        issue_keys: &[String],
    ) -> Result<Value, String> {
        let url = format!("{}/board/{}/issue", self.agile_root, board_id);
        let payload = json!({ "issues": issue_keys });
        self.post_json_no_content(&url, &payload).await
    }

    /// Move issues from a board into its backlog. Board-scoped POST only; the boardless
    /// `/backlog/issue` endpoint is scrum-only.
    pub async fn move_issues_to_backlog(
        &self,
        board_id: u64,
        issue_keys: &[String],
    ) -> Result<Value, String> {
        let url = format!("{}/backlog/{}/issue", self.agile_root, board_id);
        let payload = json!({ "issues": issue_keys });
        self.post_json_no_content(&url, &payload).await
    }

    /// Search for Jira users by name or email. Returns accountId (needed for ADF mentions), displayName, emailAddress, and active status.
    pub async fn search_users(&self, query: &str, max_results: u32) -> Result<Value, String> {
        let query = query.trim();
        if query.is_empty() {
            return Err("query must not be empty".into());
        }

        let url = format!("{}/user/search", self.api_root);
        let response = self
            .http
            .get(&url)
            .basic_auth(&self.email, Some(&self.token))
            .header("Accept", "application/json")
            .query(&[
                ("query", query),
                ("maxResults", &max_results.clamp(1, 50).to_string()),
            ])
            .send()
            .await
            .map_err(|e| format!("Jira request failed: {e}"))?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Jira response: {e}"))?;

        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(format!("Jira returned {status}: {msg}"));
        }

        let users: Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("Invalid JSON from Jira: {e}"))?;

        let slimmed = users
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|u| {
                        let obj = u.as_object()?;
                        Some(json!({
                            "accountId": obj.get("accountId")?,
                            "displayName": obj.get("displayName").unwrap_or(&Value::Null),
                            "emailAddress": obj.get("emailAddress").unwrap_or(&Value::Null),
                            "active": obj.get("active").unwrap_or(&Value::Null),
                        }))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(json!({
            "users": slimmed,
            "total": slimmed.len(),
        }))
    }

    /// List attachments on a Jira issue.
    pub async fn list_attachments(&self, issue_key: &str) -> Result<Value, String> {
        let key = issue_key.trim();
        if key.is_empty() {
            return Err("issue_key must not be empty".into());
        }

        let url = format!(
            "{}/issue/{}?fields=attachment",
            self.api_root,
            encode_path_segment(key)
        );
        let issue: Value = self.get_json(&url).await?;

        let attachments = issue
            .get("fields")
            .and_then(|f| f.get("attachment"))
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| a.as_object().map(slim_attachment))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(json!({
            "key": key,
            "attachments": attachments,
            "total": attachments.len(),
        }))
    }

    /// Upload a file attachment to a Jira issue. File content is provided as raw bytes.
    pub async fn add_attachment(
        &self,
        issue_key: &str,
        filename: &str,
        file_bytes: Vec<u8>,
    ) -> Result<Value, String> {
        let key = issue_key.trim();
        if key.is_empty() {
            return Err("issue_key must not be empty".into());
        }
        let filename = filename.trim();
        if filename.is_empty() {
            return Err("filename must not be empty".into());
        }

        let url = format!(
            "{}/issue/{}/attachments",
            self.api_root,
            encode_path_segment(key)
        );

        let part = multipart::Part::bytes(file_bytes)
            .file_name(filename.to_string())
            .mime_str("application/octet-stream")
            .map_err(|e| format!("Invalid MIME type: {e}"))?;

        let form = multipart::Form::new().part("file", part);

        let response = self
            .http
            .post(&url)
            .basic_auth(&self.email, Some(&self.token))
            .header("Accept", "application/json")
            .header("X-Atlassian-Token", "no-check")
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("Jira attachment upload failed: {e}"))?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Jira response: {e}"))?;

        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(format!("Jira returned {status}: {msg}"));
        }

        let attachments: Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("Invalid JSON from Jira: {e}"))?;

        // Jira returns an array of created attachments; slim them down
        let slimmed = attachments
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| a.as_object().map(slim_attachment))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(json!({
            "ok": true,
            "attachments": slimmed,
        }))
    }

    /// Delete a Jira attachment by its ID.
    pub async fn delete_attachment(&self, attachment_id: &str) -> Result<Value, String> {
        let id = attachment_id.trim();
        if id.is_empty() {
            return Err("attachment_id must not be empty".into());
        }

        let url = format!("{}/attachment/{}", self.api_root, encode_path_segment(id));

        let response = self
            .http
            .delete(&url)
            .basic_auth(&self.email, Some(&self.token))
            .send()
            .await
            .map_err(|e| format!("Jira request failed: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let bytes = response
                .bytes()
                .await
                .map_err(|e| format!("Failed to read Jira response: {e}"))?;
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(format!("Jira returned {status}: {msg}"));
        }

        Ok(json!({ "ok": true }))
    }

    /// List available transitions for a Jira issue.
    pub async fn get_transitions(&self, issue_key: &str) -> Result<Value, String> {
        let key = issue_key.trim();
        if key.is_empty() {
            return Err("issue_key must not be empty".into());
        }

        let url = format!(
            "{}/issue/{}/transitions",
            self.api_root,
            encode_path_segment(key)
        );
        let raw: Value = self.get_json(&url).await?;

        let transitions = raw
            .get("transitions")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| {
                        let obj = t.as_object()?;
                        Some(json!({
                            "id": obj.get("id").and_then(|x| x.as_str()),
                            "name": obj.get("name").and_then(|x| x.as_str()),
                            "to": obj.get("to").map(slim_status),
                        }))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(json!({
            "key": key,
            "transitions": transitions,
        }))
    }

    /// Fetch an issue's change history (`GET /rest/api/3/issue/{key}/changelog`).
    /// `field` filters to one changed field name (e.g. `status`); `None` returns every change.
    pub async fn get_changelog(
        &self,
        issue_key: &str,
        field: Option<&str>,
        max_results: u32,
        start_at: u32,
    ) -> Result<Value, String> {
        let key = issue_key.trim();
        if key.is_empty() {
            return Err("issue_key must not be empty".into());
        }
        let max = max_results.clamp(1, 100) as usize;
        let field_filter = field.map(|f| f.trim().to_ascii_lowercase());

        const PAGE_SIZE: u32 = 100;
        const MAX_PAGES: u32 = 50;

        let mut collected: Vec<Value> = Vec::new();
        // Raw changelog entries walked, filtered-out ones included, so the resume cursor stays exact.
        let mut consumed: u32 = 0;
        let mut pages = 0u32;
        let mut total: Option<u64> = None;
        let mut exhausted = false;

        'pages: while collected.len() < max && pages < MAX_PAGES {
            pages += 1;
            let url = format!(
                "{}/issue/{}/changelog?startAt={}&maxResults={}",
                self.api_root,
                encode_path_segment(key),
                start_at.saturating_add(consumed),
                PAGE_SIZE
            );
            let raw: Value = self.get_json(&url).await?;

            if total.is_none() {
                total = raw.get("total").and_then(|t| t.as_u64());
            }

            let values = raw
                .get("values")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let page_len = values.len() as u32;

            if page_len == 0 {
                exhausted = true;
                break;
            }

            for entry in &values {
                consumed += 1;
                let Some(obj) = entry.as_object() else {
                    continue;
                };
                let items = obj
                    .get("items")
                    .and_then(|i| i.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter(|it| match &field_filter {
                                None => true,
                                Some(want) => it
                                    .get("field")
                                    .and_then(|f| f.as_str())
                                    .is_some_and(|f| f.to_ascii_lowercase() == *want),
                            })
                            .map(slim_change_item)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                if !items.is_empty() {
                    collected.push(json!({
                        "id": obj.get("id").and_then(|x| x.as_str()),
                        "author": obj.get("author").map(slim_user).unwrap_or(Value::Null),
                        "created": obj.get("created").and_then(|x| x.as_str()),
                        "items": items,
                    }));
                }

                if collected.len() >= max {
                    break 'pages;
                }
            }

            let is_last = raw
                .get("isLast")
                .and_then(|l| l.as_bool())
                .unwrap_or(page_len < PAGE_SIZE);
            if is_last {
                exhausted = true;
                break;
            }
        }

        Ok(json!({
            "key": key,
            "field": field_filter,
            "total_changes": total,
            "returned": collected.len(),
            "next_start_at": if exhausted { Value::Null } else { json!(start_at.saturating_add(consumed)) },
            "history": collected,
        }))
    }

    /// Set the assignee for a Jira issue. `account_id`:
    /// - `Some("<accountId>")` assigns that user
    /// - `Some("-1")` uses the project's default assignee
    /// - `None` unassigns the issue (sends `null`)
    pub async fn set_assignee(
        &self,
        issue_key: &str,
        account_id: Option<&str>,
    ) -> Result<Value, String> {
        let key = issue_key.trim();
        if key.is_empty() {
            return Err("issue_key must not be empty".into());
        }

        let url = format!(
            "{}/issue/{}/assignee",
            self.api_root,
            encode_path_segment(key)
        );
        let account_value = match account_id.map(str::trim) {
            None | Some("") => Value::Null,
            Some(id) => Value::String(id.to_string()),
        };
        let payload = json!({ "accountId": account_value });

        self.put_json(&url, &payload).await
    }

    /// Transition a Jira issue to a new status.
    pub async fn transition_issue(
        &self,
        issue_key: &str,
        transition_id: &str,
    ) -> Result<Value, String> {
        let key = issue_key.trim();
        if key.is_empty() {
            return Err("issue_key must not be empty".into());
        }
        let tid = transition_id.trim();
        if tid.is_empty() {
            return Err("transition_id must not be empty".into());
        }

        let url = format!(
            "{}/issue/{}/transitions",
            self.api_root,
            encode_path_segment(key)
        );
        let payload = json!({
            "transition": { "id": tid }
        });

        self.post_json_no_content(&url, &payload).await
    }

    /// POST that accepts 204 No Content (used by transitions endpoint).
    async fn post_json_no_content(&self, url: &str, payload: &Value) -> Result<Value, String> {
        let response = self
            .http
            .post(url)
            .basic_auth(&self.email, Some(&self.token))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(payload)
            .send()
            .await
            .map_err(|e| format!("Jira request failed: {e}"))?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Jira response: {e}"))?;

        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(format!("Jira returned {status}: {msg}"));
        }

        if bytes.is_empty() {
            return Ok(json!({ "ok": true }));
        }

        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid JSON from Jira: {e}"))
    }

    async fn get_json(&self, url: &str) -> Result<Value, String> {
        let response = self
            .http
            .get(url)
            .basic_auth(&self.email, Some(&self.token))
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| format!("Jira request failed: {e}"))?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Jira response: {e}"))?;

        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(format!("Jira returned {status}: {msg}"));
        }

        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid JSON from Jira: {e}"))
    }

    async fn post_json(&self, url: &str, payload: &Value) -> Result<Value, String> {
        let response = self
            .http
            .post(url)
            .basic_auth(&self.email, Some(&self.token))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(payload)
            .send()
            .await
            .map_err(|e| format!("Jira request failed: {e}"))?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Jira response: {e}"))?;

        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(format!("Jira returned {status}: {msg}"));
        }

        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid JSON from Jira: {e}"))
    }

    async fn put_json(&self, url: &str, payload: &Value) -> Result<Value, String> {
        let response = self
            .http
            .put(url)
            .basic_auth(&self.email, Some(&self.token))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(payload)
            .send()
            .await
            .map_err(|e| format!("Jira request failed: {e}"))?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Jira response: {e}"))?;

        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(format!("Jira returned {status}: {msg}"));
        }

        // PUT may return empty body on success
        if bytes.is_empty() {
            return Ok(json!({ "ok": true }));
        }

        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid JSON from Jira: {e}"))
    }
}

pub(crate) fn slim_search_issue(issue: &serde_json::Map<String, Value>) -> Value {
    let fields = issue.get("fields").and_then(|f| f.as_object());
    json!({
        "key": issue.get("key").and_then(|k| k.as_str()),
        "summary": fields.and_then(|f| f.get("summary")).and_then(|s| s.as_str()),
        "status": fields.and_then(|f| f.get("status")).map(slim_status),
        "issuetype": fields.and_then(|f| f.get("issuetype")).and_then(|t| t.as_object()).map(|t| {
            json!({
                "name": t.get("name").and_then(|n| n.as_str()),
            })
        }),
        "assignee": fields.and_then(|f| f.get("assignee")).map(slim_user),
        "updated": fields.and_then(|f| f.get("updated")).and_then(|u| u.as_str()),
    })
}

fn slim_comment(obj: &serde_json::Map<String, Value>, include_adf: bool) -> Value {
    let id = obj.get("id").cloned().unwrap_or(Value::Null);
    let plain = obj.get("body").and_then(adf_to_plain);
    if include_adf {
        json!({
            "id": id,
            "author": obj.get("author").map(slim_user).unwrap_or(Value::Null),
            "created": obj.get("created").and_then(|v| v.as_str()),
            "updated": obj.get("updated").and_then(|v| v.as_str()),
            "body": plain,
            "body_adf": obj.get("body").cloned().unwrap_or(Value::Null),
        })
    } else {
        json!({
            "id": id,
            "author": obj.get("author").map(slim_user).unwrap_or(Value::Null),
            "created": obj.get("created").and_then(|v| v.as_str()),
            "updated": obj.get("updated").and_then(|v| v.as_str()),
            "body": plain,
        })
    }
}

fn slim_user(v: &Value) -> Value {
    let Some(o) = v.as_object() else {
        return Value::Null;
    };
    json!({
        "displayName": o.get("displayName").and_then(|x| x.as_str()),
        "emailAddress": o.get("emailAddress").and_then(|x| x.as_str()),
    })
}

fn slim_change_item(v: &Value) -> Value {
    let Some(o) = v.as_object() else {
        return Value::Null;
    };
    json!({
        "field": o.get("field").and_then(|x| x.as_str()),
        "from": o.get("fromString").and_then(|x| x.as_str()),
        "to": o.get("toString").and_then(|x| x.as_str()),
    })
}

fn slim_status(v: &Value) -> Value {
    let Some(o) = v.as_object() else {
        return Value::Null;
    };
    let cat = o.get("statusCategory").and_then(|c| c.as_object());
    json!({
        "name": o.get("name").and_then(|x| x.as_str()),
        "id": o.get("id").and_then(|x| x.as_str()),
        "statusCategory": cat.map(|c| {
            json!({
                "key": c.get("key").and_then(|x| x.as_str()),
                "name": c.get("name").and_then(|x| x.as_str()),
            })
        }),
    })
}

fn slim_attachment(o: &serde_json::Map<String, Value>) -> Value {
    json!({
        "id": o.get("id").cloned().unwrap_or(Value::Null),
        "filename": o.get("filename").and_then(|x| x.as_str()),
        "mimeType": o.get("mimeType").and_then(|x| x.as_str()),
        "size": o.get("size"),
        "content": o.get("content").and_then(|x| x.as_str()),
        "created": o.get("created").and_then(|x| x.as_str()),
        "author": o.get("author").map(slim_user).unwrap_or(Value::Null),
    })
}

fn adf_to_plain(value: &Value) -> Option<String> {
    let obj = value.as_object()?;
    if obj.get("type")?.as_str()? != "doc" {
        return None;
    }
    let mut out = String::new();
    if let Some(content) = obj.get("content").and_then(|c| c.as_array()) {
        for block in content {
            adf_walk_adf(block, &mut out);
        }
    }
    let s = out.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn adf_walk_adf(node: &Value, out: &mut String) {
    let Some(obj) = node.as_object() else {
        return;
    };
    let ty = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match ty {
        "text" => {
            if let Some(t) = obj.get("text").and_then(|x| x.as_str()) {
                out.push_str(t);
            }
        }
        "hardBreak" => out.push('\n'),
        "mediaSingle" | "mediaGroup" | "media" | "embedCard" | "extension" => {
            out.push_str("[media]");
            if let Some(children) = obj.get("content").and_then(|c| c.as_array()) {
                for ch in children {
                    adf_walk_adf(ch, out);
                }
            }
        }
        "emoji" => {
            if let Some(s) = obj
                .get("attrs")
                .and_then(|a| a.get("shortName"))
                .and_then(|x| x.as_str())
            {
                out.push_str(s);
            }
        }
        "mention" => {
            if let Some(t) = obj
                .get("attrs")
                .and_then(|a| a.get("text"))
                .and_then(|x| x.as_str())
            {
                out.push('@');
                out.push_str(t);
            } else {
                out.push_str("@mention");
            }
        }
        _ => {
            if let Some(children) = obj.get("content").and_then(|c| c.as_array()) {
                for ch in children {
                    adf_walk_adf(ch, out);
                }
            }
            if matches!(
                ty,
                "paragraph" | "heading" | "listItem" | "codeBlock" | "blockquote"
            ) {
                out.push('\n');
            }
        }
    }
}

fn encode_path_segment(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for &b in key.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Parse a user-supplied JSON string as a Jira/Confluence **Atlassian Document Format** root document.
/// Expects `{"type":"doc","version":1,"content":[...]}` (same shape Jira REST API v3 uses for description and comments).
pub fn parse_adf_document_json(text: &str) -> Result<Value, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("ADF document JSON must not be empty".into());
    }
    let v: Value =
        serde_json::from_str(text).map_err(|e| format!("invalid JSON for ADF document: {e}"))?;
    let obj = v
        .as_object()
        .ok_or_else(|| "ADF document root must be a JSON object".to_string())?;
    let ty = obj
        .get("type")
        .and_then(|t| t.as_str())
        .ok_or_else(|| "ADF document must have string field \"type\"".to_string())?;
    if ty != "doc" {
        return Err(format!(
            "ADF document \"type\" must be \"doc\" (Jira Cloud REST v3), got {ty:?}"
        ));
    }
    if !obj.contains_key("version") {
        return Err("ADF document must include \"version\" (use 1 with Jira Cloud REST v3)".into());
    }
    if obj.get("version").and_then(|x| x.as_u64()).is_none()
        && obj.get("version").and_then(|x| x.as_i64()).is_none()
    {
        return Err("ADF document \"version\" must be a number".into());
    }
    Ok(v)
}

/// Build an ADF document from tool input: `plain` wraps lines as paragraphs; `adf` parses full ADF JSON.
pub fn document_from_content_format(content_format: &str, text: &str) -> Result<Value, String> {
    match content_format.trim().to_ascii_lowercase().as_str() {
        "plain" => {
            if text.trim().is_empty() {
                return Err("body text must not be empty when content_format is \"plain\"".into());
            }
            Ok(plain_text_to_adf(text))
        }
        "adf" => parse_adf_document_json(text),
        other => Err(format!(
            "content_format must be \"plain\" or \"adf\", got {other:?}"
        )),
    }
}

fn summary_payload(summary: &str) -> Result<Value, String> {
    let trimmed = summary.trim();
    if trimmed.is_empty() {
        return Err("summary must not be empty".into());
    }
    Ok(json!({ "fields": { "summary": trimmed } }))
}

fn plain_text_to_adf(text: &str) -> Value {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut paragraphs: Vec<Value> = Vec::new();

    for line in lines {
        if line.is_empty() {
            paragraphs.push(json!({
                "type": "paragraph",
                "content": []
            }));
            continue;
        }
        paragraphs.push(json!({
            "type": "paragraph",
            "content": [{ "type": "text", "text": line }]
        }));
    }

    if paragraphs.is_empty() {
        paragraphs.push(json!({
            "type": "paragraph",
            "content": []
        }));
    }

    json!({
        "type": "doc",
        "version": 1,
        "content": paragraphs
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slim_change_item_keeps_display_strings() {
        let item = json!({
            "field": "status",
            "fieldtype": "jira",
            "from": "10000",
            "fromString": "In Progress",
            "to": "10001",
            "toString": "Done"
        });
        assert_eq!(
            slim_change_item(&item),
            json!({ "field": "status", "from": "In Progress", "to": "Done" })
        );
    }

    #[test]
    fn summary_payload_wraps_trimmed_summary_in_fields() {
        assert_eq!(
            summary_payload("  New title  ").unwrap(),
            json!({"fields":{"summary":"New title"}})
        );
    }

    #[test]
    fn summary_payload_rejects_blank() {
        let err = summary_payload("   ").unwrap_err();
        assert!(err.contains("summary"));
    }

    #[test]
    fn encode_path_segment_allows_safe_chars() {
        assert_eq!(encode_path_segment("PROJ-123"), "PROJ-123");
    }

    #[test]
    fn encode_path_segment_percent_encodes_other_bytes() {
        assert_eq!(encode_path_segment("a/b"), "a%2Fb");
        assert_eq!(encode_path_segment("x y"), "x%20y");
    }

    #[test]
    fn plain_text_to_adf_single_line() {
        let v = plain_text_to_adf("hello");
        assert_eq!(v["type"], "doc");
        assert_eq!(v["content"][0]["type"], "paragraph");
        assert_eq!(v["content"][0]["content"][0]["text"], "hello");
    }

    #[test]
    fn adf_to_plain_round_trip_simple() {
        let adf = plain_text_to_adf("one\ntwo");
        let plain = adf_to_plain(&adf).expect("plain text");
        assert_eq!(plain, "one\ntwo");
    }

    #[test]
    fn parse_adf_document_json_accepts_minimal_doc() {
        let s = r#"{"type":"doc","version":1,"content":[]}"#;
        let v = parse_adf_document_json(s).expect("adf");
        assert_eq!(v["type"], "doc");
    }

    #[test]
    fn parse_adf_document_json_rejects_wrong_type() {
        let err = parse_adf_document_json(r#"{"type":"paragraph","version":1}"#).unwrap_err();
        assert!(err.contains("doc"));
    }

    #[test]
    fn document_from_content_format_adf_heading() {
        let s = r#"{"type":"doc","version":1,"content":[{"type":"heading","attrs":{"level":2},"content":[{"type":"text","text":"Hi"}]}]}"#;
        let v = document_from_content_format("adf", s).expect("doc");
        assert_eq!(v["content"][0]["type"], "heading");
    }
}
