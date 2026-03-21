use std::time::Duration;

use reqwest::Client;
use serde_json::{Value, json};

use crate::credentials::AtlassianCredentials;

const COMMENT_PAGE_SIZE: u32 = 100;
const HTTP_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct JiraClient {
    http: Client,
    api_root: String,
    email: String,
    token: String,
}

impl JiraClient {
    pub fn new(http: Client, creds: &AtlassianCredentials) -> Self {
        Self {
            http,
            api_root: creds.jira_rest_v3_root(),
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
        if include_adf
            && let Some(obj) = out.as_object_mut()
        {
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

fn slim_search_issue(issue: &serde_json::Map<String, Value>) -> Value {
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
