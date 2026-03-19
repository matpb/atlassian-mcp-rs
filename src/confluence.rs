use reqwest::Client;
use serde_json::{Value, json};

use crate::credentials::AtlassianCredentials;

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

    pub async fn search_cql(&self, cql: &str, limit: u32) -> Result<Value, String> {
        let cql = cql.trim();
        if cql.is_empty() {
            return Err("cql must not be empty".into());
        }
        let lim = limit.clamp(1, 100);
        let url = format!("{}/search", self.wiki_api_root);
        let response = self
            .http
            .get(&url)
            .basic_auth(&self.email, Some(&self.token))
            .header("Accept", "application/json")
            .query(&[("cql", cql), ("limit", &lim.to_string())])
            .send()
            .await
            .map_err(|e| format!("Confluence request failed: {e}"))?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Confluence response: {e}"))?;

        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(format!("Confluence returned {status}: {msg}"));
        }

        let raw: Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("Invalid JSON: {e}"))?;

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

    pub async fn get_page(&self, page_id: &str) -> Result<Value, String> {
        let id = page_id.trim();
        if id.is_empty() {
            return Err("page_id must not be empty".into());
        }
        let enc = encode_path_segment(id);
        let url = format!(
            "{}/content/{}?expand=body.storage,version,space,history.lastUpdated",
            self.wiki_api_root, enc
        );

        let raw: Value = self.get_json(&url).await?;
        Ok(slim_page(&raw))
    }

    async fn get_json(&self, url: &str) -> Result<Value, String> {
        let response = self
            .http
            .get(url)
            .basic_auth(&self.email, Some(&self.token))
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| format!("Confluence request failed: {e}"))?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Confluence response: {e}"))?;

        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(format!("Confluence returned {status}: {msg}"));
        }

        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid JSON: {e}"))
    }
}

fn slim_search_hit(hit: &Value) -> Value {
    let content = hit.get("content").and_then(|c| c.as_object());
    let id = content
        .and_then(|c| c.get("id"))
        .and_then(|i| i.as_str())
        .or_else(|| hit.get("id").and_then(|i| i.as_str()));
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
}
