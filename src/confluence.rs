use reqwest::Client;
use serde_json::{Value, json};

use crate::credentials::AtlassianCredentials;
use crate::error::{ClientError, ClientResult, check_range};

/// Max characters of `body.storage.value` returned to the model (UTF-8 scalar values, not bytes).
const MAX_CONFLUENCE_BODY_CHARS: usize = 120_000;

#[derive(Clone)]
pub struct ConfluenceClient {
    http: Client,
    wiki_api_root: String,
    email: String,
    token: String,
}

impl ConfluenceClient {
    pub fn new(http: Client, creds: &AtlassianCredentials) -> Self {
        Self {
            http,
            wiki_api_root: creds.confluence_rest_root(),
            email: creds.email.clone(),
            token: creds.api_token.clone(),
        }
    }

    pub async fn search_cql(&self, cql: &str, limit: u32) -> ClientResult<Value> {
        let cql = cql.trim();
        if cql.is_empty() {
            return Err(ClientError::invalid("cql must not be empty"));
        }
        check_range("limit", u64::from(limit), 1, 100)?;
        let lim = limit;
        let url = format!("{}/search", self.wiki_api_root);
        let response = self
            .http
            .get(&url)
            .basic_auth(&self.email, Some(&self.token))
            .header("Accept", "application/json")
            .query(&[("cql", cql), ("limit", &lim.to_string())])
            .send()
            .await
            .map_err(|e| ClientError::upstream(format!("Confluence request failed: {e}")))?;

        let status = response.status();
        let bytes = response.bytes().await.map_err(|e| {
            ClientError::upstream(format!("Failed to read Confluence response: {e}"))
        })?;

        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(ClientError::upstream(format!(
                "Confluence returned {status}: {msg}"
            )));
        }

        let raw = parse_body(&bytes)?;

        let results = raw
            .get("results")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        let slim: Vec<Value> = results.iter().map(slim_search_hit).collect();

        Ok(json!({
            "size": raw.get("size"),
            "limit": lim,
            "results": slim,
        }))
    }

    pub async fn get_page(&self, page_id: &str) -> ClientResult<Value> {
        let id = page_id.trim();
        if id.is_empty() {
            return Err(ClientError::invalid("page_id must not be empty"));
        }
        let enc = encode_path_segment(id);
        let url = format!(
            "{}/content/{}?expand=body.storage,version,space,history.lastUpdated",
            self.wiki_api_root, enc
        );

        let raw: Value = self.get_json(&url).await?;
        Ok(slim_page(&raw))
    }

    /// Resolve a space key (e.g. "DOCS") to its numeric v2 space ID.
    /// If the input is already numeric, returns it as-is.
    async fn resolve_space_id(&self, space_id_or_key: &str) -> ClientResult<String> {
        // If it's already numeric, return as-is
        if space_id_or_key.chars().all(|c| c.is_ascii_digit()) {
            return Ok(space_id_or_key.to_string());
        }
        // Otherwise, look up by key via v2 API
        let url = format!(
            "{}/api/v2/spaces?keys={}",
            self.wiki_base_url(),
            encode_path_segment(space_id_or_key)
        );
        let raw: Value = self.get_json(&url).await?;
        let results = raw
            .get("results")
            .and_then(|r| r.as_array())
            .ok_or_else(|| {
                ClientError::upstream(format!("No spaces found for key \"{space_id_or_key}\""))
            })?;
        let space = results.first().ok_or_else(|| {
            ClientError::upstream(format!("Space \"{space_id_or_key}\" not found"))
        })?;
        let id = space
            .get("id")
            .and_then(|i| i.as_str())
            .ok_or_else(|| ClientError::upstream("Space response missing id field"))?;
        Ok(id.to_string())
    }

    pub async fn create_page(
        &self,
        space_id: &str,
        title: &str,
        body: &str,
        body_format: &str,
        parent_id: Option<&str>,
        status: &str,
    ) -> ClientResult<Value> {
        let space_id = space_id.trim();
        if space_id.is_empty() {
            return Err(ClientError::invalid("space_id must not be empty"));
        }
        let title = title.trim();
        if title.is_empty() {
            return Err(ClientError::invalid("title must not be empty"));
        }
        let representation = match body_format {
            "storage" | "wiki" => body_format,
            other => {
                return Err(ClientError::invalid(format!(
                    "body_format must be \"storage\" or \"wiki\", got \"{other}\""
                )));
            }
        };
        let space_id = self.resolve_space_id(space_id).await?;

        let url = format!("{}/api/v2/pages", self.wiki_base_url());

        let mut payload = json!({
            "spaceId": space_id,
            "status": status,
            "title": title,
            "body": {
                "representation": representation,
                "value": body,
            }
        });
        if let Some(pid) = parent_id {
            let pid = pid.trim();
            if !pid.is_empty() {
                payload
                    .as_object_mut()
                    .unwrap()
                    .insert("parentId".into(), json!(pid));
            }
        }

        let response = self
            .http
            .post(&url)
            .basic_auth(&self.email, Some(&self.token))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| ClientError::upstream(format!("Confluence request failed: {e}")))?;

        let status_code = response.status();
        let bytes = response.bytes().await.map_err(|e| {
            ClientError::upstream(format!("Failed to read Confluence response: {e}"))
        })?;

        if !status_code.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(ClientError::upstream(format!(
                "Confluence returned {status_code}: {msg}"
            )));
        }

        let raw = parse_body(&bytes)?;

        // Return a slim view: id, title, status, webui link
        let webui = raw.pointer("/_links/webui").and_then(|w| w.as_str());

        Ok(json!({
            "id": raw.get("id"),
            "title": raw.get("title"),
            "status": raw.get("status"),
            "webui": webui,
        }))
    }

    /// Return the wiki base URL (without /rest/api or /api/v2 suffix).
    fn wiki_base_url(&self) -> &str {
        // wiki_api_root ends with `/wiki/rest/api`; strip `/rest/api` to get the wiki root
        self.wiki_api_root
            .strip_suffix("/rest/api")
            .unwrap_or(&self.wiki_api_root)
    }

    async fn get_json(&self, url: &str) -> ClientResult<Value> {
        let response = self
            .http
            .get(url)
            .basic_auth(&self.email, Some(&self.token))
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| ClientError::upstream(format!("Confluence request failed: {e}")))?;

        let status = response.status();
        let bytes = response.bytes().await.map_err(|e| {
            ClientError::upstream(format!("Failed to read Confluence response: {e}"))
        })?;

        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(ClientError::upstream(format!(
                "Confluence returned {status}: {msg}"
            )));
        }

        parse_body(&bytes)
    }
}

/// Parses a 2xx body; a blank body (no content) becomes `{"ok": true}`, as in Jira and Bitbucket.
fn parse_body(bytes: &[u8]) -> ClientResult<Value> {
    if bytes.trim_ascii().is_empty() {
        return Ok(json!({ "ok": true }));
    }
    serde_json::from_slice(bytes).map_err(|e| ClientError::upstream(format!("Invalid JSON: {e}")))
}

/// An id as text: a JSON string, or a JSON number written out in full.
fn id_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn slim_search_hit(hit: &Value) -> Value {
    let content = hit.get("content").and_then(|c| c.as_object());
    let id = content
        .and_then(|c| c.get("id"))
        .and_then(id_text)
        .or_else(|| hit.get("id").and_then(id_text));
    let ctype = content
        .and_then(|c| c.get("type"))
        .and_then(|t| t.as_str())
        .or_else(|| hit.get("type").and_then(|t| t.as_str()));
    let status = content
        .and_then(|c| c.get("status"))
        .and_then(|s| s.as_str());
    let title = hit.get("title").and_then(|t| t.as_str()).or_else(|| {
        content
            .and_then(|c| c.get("title"))
            .and_then(|t| t.as_str())
    });
    let excerpt = hit.get("excerpt").and_then(|e| e.as_str());
    let webui = content
        .and_then(|c| c.get("_links"))
        .and_then(|l| l.get("webui"))
        .and_then(|w| w.as_str());

    json!({
        "id": id,
        "type": ctype,
        "status": status,
        "title": title,
        "excerpt": excerpt,
        "webui": webui,
    })
}

fn slim_page(raw: &Value) -> Value {
    let space = raw.get("space").and_then(|s| s.as_object()).map(|s| {
        json!({
            "key": s.get("key").and_then(|k| k.as_str()),
            "name": s.get("name").and_then(|n| n.as_str()),
        })
    });

    let version = raw.get("version").and_then(|v| v.as_object()).map(|v| {
        json!({
            "number": v.get("number"),
            "when": v.get("when").and_then(|w| w.as_str()),
            "message": v.get("message").and_then(|m| m.as_str()),
        })
    });

    let last_updated = raw
        .get("history")
        .and_then(|h| h.get("lastUpdated"))
        .and_then(|lu| lu.as_object())
        .map(|lu| {
            let by = lu.get("by").and_then(|b| b.as_object());
            json!({
                "when": lu.get("when").and_then(|w| w.as_str()),
                "by": by.map(|b| {
                    json!({
                        "displayName": b.get("displayName").and_then(|d| d.as_str()),
                        "email": b.get("email").and_then(|e| e.as_str()),
                    })
                }),
            })
        });

    let body_storage = raw
        .get("body")
        .and_then(|b| b.get("storage"))
        .and_then(|s| s.as_object());

    let rep = body_storage
        .and_then(|s| s.get("representation"))
        .and_then(|r| r.as_str());
    let full_value = body_storage
        .and_then(|s| s.get("value"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let (body_text, total_chars, truncated) = truncate_chars(full_value, MAX_CONFLUENCE_BODY_CHARS);

    let links = raw.get("_links").and_then(|l| l.as_object());
    let webui = links.and_then(|l| l.get("webui")).and_then(|w| w.as_str());
    let tinyui = links.and_then(|l| l.get("tinyui")).and_then(|t| t.as_str());

    let included_chars = if truncated {
        MAX_CONFLUENCE_BODY_CHARS
    } else {
        total_chars
    };

    json!({
        "id": raw.get("id").and_then(|i| i.as_str()),
        "type": raw.get("type").and_then(|t| t.as_str()),
        "title": raw.get("title").and_then(|t| t.as_str()),
        "status": raw.get("status").and_then(|s| s.as_str()),
        "space": space,
        "version": version,
        "lastUpdated": last_updated,
        "body": {
            "representation": rep,
            "value": body_text,
            "char_count_total": total_chars,
            "char_count_included": included_chars,
            "truncated": truncated,
        },
        "links": {
            "webui": webui,
            "tinyui": tinyui,
        },
    })
}

fn truncate_chars(s: &str, max: usize) -> (String, usize, bool) {
    let total = s.chars().count();
    if total <= max {
        return (s.to_string(), total, false);
    }
    let head: String = s.chars().take(max).collect();
    let note = format!("\n\n[… truncated for MCP: included {max} of {total} characters]");
    (head + &note, total, true)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn slim_page_drops_noise_and_truncates() {
        let raw = json!({
            "id": "123",
            "type": "page",
            "status": "current",
            "title": "T",
            "ari": "noise",
            "space": { "key": "S", "name": "Space", "ari": "x", "_links": {} },
            "version": { "number": 2, "when": "2024-01-01", "message": "edit", "by": {}, "_links": {} },
            "history": {
                "lastUpdated": {
                    "when": "2024-01-02",
                    "by": { "displayName": "Pat", "email": "p@e.com", "accountId": "acc" }
                }
            },
            "body": { "storage": { "representation": "storage", "value": "ab" } },
            "_links": { "webui": "/x", "self": "https://huge", "editui": "/e" },
            "_expandable": { "children": "" }
        });
        let slim = slim_page(&raw);
        assert!(slim.get("ari").is_none());
        assert_eq!(slim["body"]["char_count_total"], 2);
        assert_eq!(slim["body"]["truncated"], false);
        assert_eq!(slim["links"]["webui"], json!("/x"));
        assert!(slim.get("_expandable").is_none());
    }

    #[test]
    fn slim_page_truncation_flag() {
        let big = "x".repeat(MAX_CONFLUENCE_BODY_CHARS + 50);
        let raw = json!({
            "id": "1",
            "type": "page",
            "title": "Big",
            "body": { "storage": { "representation": "storage", "value": big } }
        });
        let slim = slim_page(&raw);
        assert_eq!(slim["body"]["truncated"], true);
        assert_eq!(
            slim["body"]["char_count_total"],
            MAX_CONFLUENCE_BODY_CHARS + 50
        );
    }

    #[test]
    fn slim_search_hit_prefers_content_and_hit_title() {
        let hit = json!({
            "content": {
                "id": "111",
                "type": "page",
                "status": "current",
                "title": "Inner",
                "_links": {"webui": "/spaces/OPS/pages/111"}
            },
            "title": "Outer",
            "excerpt": "Restart <span>queue</span>",
            "url": "/noise",
            "score": 0.5
        });
        assert_eq!(
            slim_search_hit(&hit),
            json!({
                "id": "111",
                "type": "page",
                "status": "current",
                "title": "Outer",
                "excerpt": "Restart <span>queue</span>",
                "webui": "/spaces/OPS/pages/111"
            })
        );
    }

    #[test]
    fn slim_search_hit_falls_back_to_top_level_fields() {
        let hit = json!({"id": "222", "type": "blogpost", "title": "Top", "excerpt": "e"});
        assert_eq!(
            slim_search_hit(&hit),
            json!({
                "id": "222",
                "type": "blogpost",
                "status": null,
                "title": "Top",
                "excerpt": "e",
                "webui": null
            })
        );
    }

    #[test]
    fn slim_search_hit_takes_title_from_content_when_hit_has_none() {
        let hit = json!({"content": {"id": "333", "title": "Inner only"}, "id": "999"});
        let slim = slim_search_hit(&hit);
        assert_eq!(slim["id"], "333");
        assert_eq!(slim["title"], "Inner only");
    }

    #[test]
    fn slim_search_hit_empty_object_gives_all_null() {
        assert_eq!(
            slim_search_hit(&json!({})),
            json!({
                "id": null,
                "type": null,
                "status": null,
                "title": null,
                "excerpt": null,
                "webui": null
            })
        );
    }

    #[test]
    fn slim_search_hit_numeric_id_becomes_string() {
        let slim = slim_search_hit(&json!({"content": {"id": 444}}));
        assert_eq!(slim["id"], json!("444"));
        let top = slim_search_hit(&json!({"id": 7}));
        assert_eq!(top["id"], json!("7"));
    }

    #[test]
    fn encode_path_segment_keeps_unreserved_and_encodes_the_rest() {
        assert_eq!(encode_path_segment("AZaz09-_"), "AZaz09-_");
        assert_eq!(encode_path_segment("DOCS"), "DOCS");
        assert_eq!(encode_path_segment("a b/c%"), "a%20b%2Fc%25");
        assert_eq!(encode_path_segment("é"), "%C3%A9");
        assert_eq!(encode_path_segment(""), "");
    }

    #[test]
    fn truncate_chars_returns_short_input_unchanged() {
        assert_eq!(truncate_chars("héllo", 10), ("héllo".to_string(), 5, false));
        assert_eq!(truncate_chars("héllo", 5), ("héllo".to_string(), 5, false));
        assert_eq!(truncate_chars("", 0), (String::new(), 0, false));
    }

    #[test]
    fn truncate_chars_cuts_on_char_boundary_for_multibyte_input() {
        let (text, total, truncated) = truncate_chars("héllo😀", 2);
        assert_eq!(
            text,
            "hé\n\n[… truncated for MCP: included 2 of 6 characters]"
        );
        assert_eq!(total, 6);
        assert!(truncated);

        let (text, total, truncated) = truncate_chars("😀😀😀", 1);
        assert_eq!(
            text,
            "😀\n\n[… truncated for MCP: included 1 of 3 characters]"
        );
        assert_eq!(total, 3);
        assert!(truncated);
    }

    #[test]
    fn truncate_chars_zero_max_keeps_only_the_note() {
        let (text, total, truncated) = truncate_chars("abc", 0);
        assert_eq!(
            text,
            "\n\n[… truncated for MCP: included 0 of 3 characters]"
        );
        assert_eq!(total, 3);
        assert!(truncated);
    }
}
