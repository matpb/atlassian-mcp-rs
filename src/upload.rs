use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, to_bytes};
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use http::HeaderMap;
use rand::RngCore;
use reqwest::Client;

use crate::credentials::AtlassianCredentials;
use crate::jira::JiraClient;

pub const DEFAULT_TTL: Duration = Duration::from_secs(600);
pub const MAX_TTL: Duration = Duration::from_secs(3600);
pub const DEFAULT_MAX_UPLOAD_BYTES: usize = 100 * 1024 * 1024;

#[derive(Clone)]
pub struct UploadTicket {
    pub creds: AtlassianCredentials,
    pub issue_key: String,
    pub filename: String,
    pub expires_at: Instant,
}

/// One-shot upload tickets, held in memory only. Nothing is written to disk.
#[derive(Clone, Default)]
pub struct UploadTickets {
    inner: Arc<Mutex<HashMap<String, UploadTicket>>>,
}

impl UploadTickets {
    pub fn issue(
        &self,
        creds: AtlassianCredentials,
        issue_key: String,
        filename: String,
        ttl: Duration,
    ) -> String {
        let token = random_token();
        let ticket = UploadTicket {
            creds,
            issue_key,
            filename,
            expires_at: Instant::now() + ttl.min(MAX_TTL),
        };
        let mut map = self.inner.lock().unwrap();
        map.retain(|_, t| t.expires_at > Instant::now());
        map.insert(token.clone(), ticket);
        token
    }

    /// Removes and returns the ticket; expired tickets are dropped and yield None.
    pub fn take(&self, token: &str) -> Option<UploadTicket> {
        let mut map = self.inner.lock().unwrap();
        let ticket = map.remove(token)?;
        (ticket.expires_at > Instant::now()).then_some(ticket)
    }
}

fn random_token() -> String {
    let mut buf = [0u8; 32];
    rand::rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// Origin the agent must reach us at: MCP_PUBLIC_URL, else the proxy's forwarded headers, else Host.
pub fn public_origin(configured: Option<&str>, headers: &HeaderMap) -> Option<String> {
    if let Some(url) = configured.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(url.trim_end_matches('/').to_string());
    }
    let hdr = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let host = hdr("x-forwarded-host").or_else(|| hdr("host"))?;
    let scheme = hdr("x-forwarded-proto").unwrap_or_else(|| "http".to_string());
    Some(format!("{scheme}://{host}"))
}

pub fn upload_path(token: &str) -> String {
    format!("/upload/{token}")
}

#[derive(Clone)]
pub struct UploadState {
    pub http: Client,
    pub tickets: UploadTickets,
    pub max_bytes: usize,
}

/// PUT /upload/{token}: raw file body in, forwarded to Jira as multipart, never stored.
/// The token is checked before the body is read so junk requests cost no memory.
pub async fn handle_upload(
    State(state): State<UploadState>,
    Path(token): Path<String>,
    req: Request<Body>,
) -> Response {
    let Some(ticket) = state.tickets.take(&token) else {
        return (
            StatusCode::NOT_FOUND,
            "Unknown, expired or already used upload token. Call jira_get_attachment_upload_url again.",
        )
            .into_response();
    };
    let body = match to_bytes(req.into_body(), state.max_bytes).await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("Upload rejected ({e}); limit is {} bytes", state.max_bytes),
            )
                .into_response();
        }
    };
    if body.is_empty() {
        return (StatusCode::BAD_REQUEST, "Empty request body").into_response();
    }

    let jira = JiraClient::new(state.http, &ticket.creds);
    match jira
        .add_attachment(&ticket.issue_key, &ticket.filename, body.to_vec())
        .await
    {
        Ok(value) => (
            StatusCode::OK,
            [(http::header::CONTENT_TYPE, "application/json")],
            value.to_string(),
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds() -> AtlassianCredentials {
        AtlassianCredentials::new(
            "https://x.atlassian.net".into(),
            "a@b.c".into(),
            "tok".into(),
            "https://x.atlassian.net".into(),
        )
    }

    #[test]
    fn ticket_is_one_shot() {
        let store = UploadTickets::default();
        let token = store.issue(creds(), "SWS-1".into(), "a.zip".into(), DEFAULT_TTL);
        assert_eq!(token.len(), 43);
        let t = store.take(&token).expect("first take succeeds");
        assert_eq!(t.issue_key, "SWS-1");
        assert!(store.take(&token).is_none());
    }

    #[test]
    fn expired_ticket_is_gone() {
        let store = UploadTickets::default();
        let token = store.issue(creds(), "SWS-1".into(), "a.zip".into(), Duration::ZERO);
        assert!(store.take(&token).is_none());
    }

    #[test]
    fn origin_prefers_configured_url() {
        let mut h = HeaderMap::new();
        h.insert("host", "internal:8432".parse().unwrap());
        assert_eq!(
            public_origin(Some("https://mcp.example.com/"), &h).as_deref(),
            Some("https://mcp.example.com")
        );
    }

    #[test]
    fn origin_from_forwarded_headers() {
        let mut h = HeaderMap::new();
        h.insert("host", "10.0.0.1:8432".parse().unwrap());
        h.insert("x-forwarded-host", "mcp.example.com".parse().unwrap());
        h.insert("x-forwarded-proto", "https".parse().unwrap());
        assert_eq!(
            public_origin(None, &h).as_deref(),
            Some("https://mcp.example.com")
        );
    }

    #[test]
    fn origin_falls_back_to_host() {
        let mut h = HeaderMap::new();
        h.insert("host", "127.0.0.1:8432".parse().unwrap());
        assert_eq!(
            public_origin(None, &h).as_deref(),
            Some("http://127.0.0.1:8432")
        );
        assert!(public_origin(None, &HeaderMap::new()).is_none());
    }
}
