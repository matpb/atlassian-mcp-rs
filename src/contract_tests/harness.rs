use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::jira::JiraClient;
use crate::upload::UploadTickets;

/// One request that reached the fake Atlassian server.
#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: http::HeaderMap,
    pub body: Vec<u8>,
}

impl Recorded {
    /// Parses the body as JSON; panics with the body text if it is not JSON.
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "recorded body is not JSON ({e}): {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// Decoded query parameters in order; a key without `=` gets an empty value.
    pub fn query_pairs(&self) -> Vec<(String, String)> {
        let Some(query) = self.query.as_deref() else {
            return Vec::new();
        };
        query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                (percent_decode(k), percent_decode(v))
            })
            .collect()
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Clone)]
struct Route {
    status: u16,
    content_type: &'static str,
    body: String,
}

#[derive(Default)]
struct Store {
    routes: HashMap<(String, String), Route>,
    requests: Vec<Recorded>,
}

type SharedStore = Arc<Mutex<Store>>;

/// In-process stand-in for Atlassian Cloud, bound to 127.0.0.1 on a random port.
#[derive(Clone)]
pub struct FakeAtlassian {
    pub base_url: String,
    store: SharedStore,
}

impl FakeAtlassian {
    pub async fn start() -> FakeAtlassian {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake Atlassian");
        let addr = listener.local_addr().expect("fake Atlassian address");
        let store: SharedStore = Arc::new(Mutex::new(Store::default()));
        let app = axum::Router::new()
            .fallback(handle)
            .with_state(store.clone());
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve fake Atlassian");
        });
        FakeAtlassian {
            base_url: format!("http://{addr}"),
            store,
        }
    }

    /// Answers `method path` with a JSON body. A later call for the same pair replaces it.
    pub fn on(&self, method: &str, path: &str, status: u16, body: Value) {
        self.add_route(method, path, status, "application/json", body.to_string());
    }

    /// Answers `method path` with a raw text body; use "" for an empty 204.
    pub fn on_raw(&self, method: &str, path: &str, status: u16, body: &str) {
        self.add_route(
            method,
            path,
            status,
            "text/plain; charset=utf-8",
            body.to_string(),
        );
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.store().requests.clone()
    }

    /// Returns the only recorded request; panics unless exactly one was recorded.
    pub fn only_request(&self) -> Recorded {
        let mut requests = self.requests();
        let seen: Vec<String> = requests
            .iter()
            .map(|r| format!("{} {}", r.method, r.path))
            .collect();
        assert_eq!(requests.len(), 1, "expected one fake request, saw {seen:?}");
        requests.remove(0)
    }

    fn add_route(
        &self,
        method: &str,
        path: &str,
        status: u16,
        content_type: &'static str,
        body: String,
    ) {
        let key = (method.to_ascii_uppercase(), path.to_string());
        self.store().routes.insert(
            key,
            Route {
                status,
                content_type,
                body,
            },
        );
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().expect("fake Atlassian store lock")
    }
}

async fn handle(State(store): State<SharedStore>, req: Request<Body>) -> Response {
    let (parts, body) = req.into_parts();
    let Ok(bytes) = to_bytes(body, 64 * 1024 * 1024).await else {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            "fake request body over 64 MiB",
        )
            .into_response();
    };
    let method = parts.method.as_str().to_string();
    let path = parts.uri.path().to_string();
    let route = {
        let mut store = store.lock().expect("fake Atlassian store lock");
        let route = store.routes.get(&(method.clone(), path.clone())).cloned();
        store.requests.push(Recorded {
            method: method.clone(),
            path: path.clone(),
            query: parts.uri.query().map(str::to_string),
            headers: parts.headers,
            body: bytes.to_vec(),
        });
        route
    };
    match route {
        Some(r) => respond(r.status, r.content_type, r.body),
        None => respond(
            599,
            "text/plain; charset=utf-8",
            format!("no fake route for {method} {path}"),
        ),
    }
}

fn respond(status: u16, content_type: &'static str, body: String) -> Response {
    let status = StatusCode::from_u16(status).expect("fake route status is 100-999");
    (status, [(http::header::CONTENT_TYPE, content_type)], body).into_response()
}

/// The real MCP app, with outgoing Atlassian calls pointed at `fake`.
pub struct Mcp {
    pub base_url: String,
    pub fake: FakeAtlassian,
    client: reqwest::Client,
}

impl Mcp {
    pub async fn start() -> Mcp {
        Mcp::start_with_max_upload(1024 * 1024).await
    }

    pub async fn start_with_max_upload(bytes: usize) -> Mcp {
        let fake = FakeAtlassian::start().await;
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind MCP server");
        let base_url = format!("http://{}", listener.local_addr().expect("MCP address"));
        let http = JiraClient::build_http_client().expect("build HTTP client");
        let app = crate::build_app(
            http,
            UploadTickets::default(),
            Some(base_url.clone()),
            bytes,
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve MCP");
        });
        Mcp {
            base_url,
            fake,
            client: reqwest::Client::new(),
        }
    }

    /// Sends a JSON-RPC request with all credential headers pointing at the fake.
    pub async fn rpc(&self, method: &str, params: Value) -> Value {
        self.post_rpc(method, params, true).await
    }

    /// Sends a JSON-RPC request with no credential headers.
    pub async fn rpc_without_creds(&self, method: &str, params: Value) -> Value {
        self.post_rpc(method, params, false).await
    }

    /// Calls a tool and returns the parsed JSON of its text content; panics on error.
    pub async fn call(&self, tool: &str, args: Value) -> Value {
        let text = self.call_text(tool, args).await;
        serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("tool {tool} output is not JSON ({e}): {text}"))
    }

    /// Calls a tool and returns its raw text content; panics on a JSON-RPC or tool error.
    pub async fn call_text(&self, tool: &str, args: Value) -> String {
        let resp = self.call_raw(tool, args).await;
        if resp.get("error").is_some() || resp["result"]["isError"] == true {
            panic!("tool {tool} failed: {resp:#}");
        }
        resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("tool {tool} returned no text content: {resp:#}"))
            .to_string()
    }

    /// Calls a tool expecting failure; returns the JSON-RPC error object or the isError result.
    pub async fn call_err(&self, tool: &str, args: Value) -> Value {
        let resp = self.call_raw(tool, args).await;
        if let Some(err) = resp.get("error") {
            return err.clone();
        }
        if resp["result"]["isError"] == true {
            return resp["result"].clone();
        }
        panic!("tool {tool} succeeded but an error was expected: {resp:#}");
    }

    async fn call_raw(&self, tool: &str, args: Value) -> Value {
        self.rpc("tools/call", json!({"name": tool, "arguments": args}))
            .await
    }

    async fn post_rpc(&self, method: &str, params: Value, with_creds: bool) -> Value {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let mut req = self
            .client
            .post(format!("{}/mcp", self.base_url))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&body);
        if with_creds {
            let fake = &self.fake.base_url;
            req = req
                .header("X-Atlassian-Site-Url", fake)
                .header("X-Atlassian-Email", "tester@example.com")
                .header("X-Atlassian-Api-Token", "test-token")
                .header("X-Bitbucket-Workspace", "ws")
                .header("X-Bitbucket-Username", "tester@example.com")
                .header("X-Bitbucket-Api-Token", "bb-token")
                .header("X-Bitbucket-Base-Url", format!("{fake}/2.0"));
        }
        let response = req.send().await.expect("send MCP request");
        let status = response.status();
        let text = response.text().await.expect("read MCP response");
        serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("MCP response is not JSON (HTTP {status}, {e}): {text}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorded_with_query(query: Option<&str>) -> Recorded {
        Recorded {
            method: "GET".into(),
            path: "/x".into(),
            query: query.map(str::to_string),
            headers: http::HeaderMap::new(),
            body: Vec::new(),
        }
    }

    #[test]
    fn query_pairs_decode_percent_and_plus() {
        let r = recorded_with_query(Some("a=b%20c&x=1+2&flag&q=%2F%zz"));
        assert_eq!(
            r.query_pairs(),
            vec![
                ("a".to_string(), "b c".to_string()),
                ("x".to_string(), "1 2".to_string()),
                ("flag".to_string(), String::new()),
                ("q".to_string(), "/%zz".to_string()),
            ]
        );
    }

    #[test]
    fn query_pairs_empty_without_query() {
        assert!(recorded_with_query(None).query_pairs().is_empty());
    }

    #[test]
    fn recorded_json_parses_body() {
        let mut r = recorded_with_query(None);
        r.body = br#"{"a":[1,"x"]}"#.to_vec();
        assert_eq!(r.json(), json!({"a": [1, "x"]}));
    }

    #[tokio::test]
    async fn later_route_replaces_earlier_and_unmatched_is_599() {
        let fake = FakeAtlassian::start().await;
        fake.on("get", "/a", 200, json!({"v": 1}));
        fake.on("GET", "/a", 201, json!({"v": 2}));
        let client = reqwest::Client::new();

        let hit = client
            .get(format!("{}/a?ignored=1", fake.base_url))
            .send()
            .await
            .expect("GET /a");
        assert_eq!(hit.status().as_u16(), 201);
        assert_eq!(hit.text().await.expect("body"), r#"{"v":2}"#);

        let miss = client
            .delete(format!("{}/nope", fake.base_url))
            .send()
            .await
            .expect("DELETE /nope");
        assert_eq!(miss.status().as_u16(), 599);
        assert_eq!(
            miss.text().await.expect("body"),
            "no fake route for DELETE /nope"
        );

        let seen: Vec<(String, String)> = fake
            .requests()
            .into_iter()
            .map(|r| (r.method, r.path))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("GET".to_string(), "/a".to_string()),
                ("DELETE".to_string(), "/nope".to_string()),
            ]
        );
    }
}
