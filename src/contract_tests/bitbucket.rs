//! Contract tests for the Bitbucket tools: outgoing requests, slimmed outputs and input checks.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use super::harness::{Mcp, Recorded};

/// Expected query pairs, in the order the client sends them.
fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Each request the fake saw, as "METHOD path", in order.
fn trail(mcp: &Mcp) -> Vec<String> {
    mcp.fake
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect()
}

/// Asserts a tool failed with the internal-error code and exactly `message`.
fn assert_tool_error(err: &Value, message: &str) {
    assert_eq!(err["code"], -32603, "unexpected error object: {err}");
    assert_eq!(err["message"], message);
}

fn pipeline_142() -> Value {
    json!({
        "type": "pipeline",
        "uuid": "{pipe-142}",
        "build_number": 142,
        "creator": {"type": "user", "display_name": "Test User", "uuid": "acc-1"},
        "repository": {"type": "repository", "name": "r", "links": {"html": {"href": "http://127.0.0.1/ws/r"}}},
        "target": {
            "type": "pipeline_ref_target",
            "ref_type": "branch",
            "ref_name": "master",
            "commit": {"type": "commit", "hash": "abc123"}
        },
        "trigger": {"type": "pipeline_trigger_push", "name": "PUSH"},
        "state": {"name": "COMPLETED", "type": "pipeline_state_completed", "result": {"name": "SUCCESSFUL"}},
        "created_on": "2026-11-01T10:00:00.000000+00:00",
        "completed_on": "2026-11-01T10:01:35.000000+00:00",
        "duration_in_seconds": 95,
        "build_seconds_used": 190,
        "has_variables": false,
        "run_number": 300,
        "links": {"self": {"href": "http://127.0.0.1/unused"}}
    })
}

fn summary_142() -> Value {
    json!({
        "uuid": "{pipe-142}",
        "build_number": 142,
        "state": "COMPLETED",
        "stage": null,
        "result": "SUCCESSFUL",
        "ref_type": "branch",
        "ref_name": "master",
        "custom_pipeline": null,
        "commit": "abc123",
        "trigger": "PUSH",
        "creator": "Test User",
        "created_on": "2026-11-01T10:00:00.000000+00:00",
        "completed_on": "2026-11-01T10:01:35.000000+00:00",
        "duration_in_seconds": 95,
        "build_seconds_used": 190,
        "has_variables": false,
        "url": "http://127.0.0.1/ws/r/pipelines/results/142"
    })
}

fn pipeline_141() -> Value {
    json!({
        "uuid": "{pipe-141}",
        "build_number": 141,
        "state": {"name": "IN_PROGRESS", "stage": {"name": "RUNNING"}},
        "target": {
            "ref_type": "branch",
            "ref_name": "feature/x",
            "commit": {"hash": "def456"},
            "selector": {"type": "custom", "pattern": "deploy-prod"}
        },
        "creator": {"display_name": "Test User"},
        "created_on": "2026-11-01T11:00:00.000000+00:00",
        "has_variables": true
    })
}

fn summary_141() -> Value {
    json!({
        "uuid": "{pipe-141}",
        "build_number": 141,
        "state": "IN_PROGRESS",
        "stage": "RUNNING",
        "result": null,
        "ref_type": "branch",
        "ref_name": "feature/x",
        "custom_pipeline": "deploy-prod",
        "commit": "def456",
        "trigger": null,
        "creator": "Test User",
        "created_on": "2026-11-01T11:00:00.000000+00:00",
        "completed_on": null,
        "duration_in_seconds": null,
        "build_seconds_used": null,
        "has_variables": true,
        "url": null
    })
}

fn pipeline_140() -> Value {
    json!({
        "uuid": "{pipe-140}",
        "build_number": 140,
        "state": {"name": "COMPLETED", "result": {"name": "FAILED"}},
        "target": {"ref_type": "branch", "ref_name": "master", "commit": {"hash": "0fe1"}},
        "trigger": {"name": "MANUAL"},
        "created_on": "2026-10-31T09:00:00.000000+00:00",
        "completed_on": "2026-10-31T09:02:00.000000+00:00",
        "duration_in_seconds": 120,
        "has_variables": false
    })
}

fn summary_140() -> Value {
    json!({
        "uuid": "{pipe-140}",
        "build_number": 140,
        "state": "COMPLETED",
        "stage": null,
        "result": "FAILED",
        "ref_type": "branch",
        "ref_name": "master",
        "custom_pipeline": null,
        "commit": "0fe1",
        "trigger": "MANUAL",
        "creator": null,
        "created_on": "2026-10-31T09:00:00.000000+00:00",
        "completed_on": "2026-10-31T09:02:00.000000+00:00",
        "duration_in_seconds": 120,
        "build_seconds_used": null,
        "has_variables": false,
        "url": null
    })
}

fn pipelines_page() -> Value {
    json!({
        "page": 1,
        "pagelen": 10,
        "size": 3,
        "next": "http://127.0.0.1/unused?page=2",
        "values": [pipeline_142(), pipeline_141(), pipeline_140()]
    })
}

fn pipeline_143_response() -> Value {
    json!({
        "uuid": "{pipe-143}",
        "build_number": 143,
        "state": {"name": "PENDING"},
        "target": {"ref_type": "branch", "ref_name": "master"},
        "created_on": "2026-11-24T08:00:00.000000+00:00"
    })
}

fn summary_143() -> Value {
    json!({
        "uuid": "{pipe-143}",
        "build_number": 143,
        "state": "PENDING",
        "stage": null,
        "result": null,
        "ref_type": "branch",
        "ref_name": "master",
        "custom_pipeline": null,
        "commit": null,
        "trigger": null,
        "creator": null,
        "created_on": "2026-11-24T08:00:00.000000+00:00",
        "completed_on": null,
        "duration_in_seconds": null,
        "build_seconds_used": null,
        "has_variables": null,
        "url": null
    })
}

fn step_1() -> Value {
    json!({
        "type": "pipeline_step",
        "uuid": "{step-1}",
        "name": "Build",
        "state": {"name": "COMPLETED", "result": {"name": "SUCCESSFUL"}},
        "trigger": {"type": "pipeline_step_trigger_automatic"},
        "started_on": "2026-11-01T10:00:00.000000+00:00",
        "completed_on": "2026-11-01T10:01:00.000000+00:00",
        "duration_in_seconds": 60,
        "run_number": 1,
        "maxTime": 120,
        "image": {"name": "noise-image"},
        "script_commands": ["noise"]
    })
}

fn summary_step_1() -> Value {
    json!({
        "uuid": "{step-1}",
        "name": "Build",
        "state": "COMPLETED",
        "stage": null,
        "result": "SUCCESSFUL",
        "deployment_environment": null,
        "trigger": "pipeline_step_trigger_automatic",
        "started_on": "2026-11-01T10:00:00.000000+00:00",
        "completed_on": "2026-11-01T10:01:00.000000+00:00",
        "duration_in_seconds": 60,
        "run_number": 1,
        "max_time": 120
    })
}

fn step_2() -> Value {
    json!({
        "type": "pipeline_step",
        "uuid": "{step-2}",
        "name": "Deploy",
        "state": {"name": "PAUSED", "stage": {"name": "HALTED"}},
        "deployment_environment": {"name": "Production", "type": "deployment_environment"},
        "trigger": {"type": "pipeline_step_trigger_manual"},
        "started_on": "2026-11-01T10:02:00.000000+00:00"
    })
}

fn summary_step_2() -> Value {
    json!({
        "uuid": "{step-2}",
        "name": "Deploy",
        "state": "PAUSED",
        "stage": "HALTED",
        "result": null,
        "deployment_environment": "Production",
        "trigger": "pipeline_step_trigger_manual",
        "started_on": "2026-11-01T10:02:00.000000+00:00",
        "completed_on": null,
        "duration_in_seconds": null,
        "run_number": null,
        "max_time": null
    })
}

fn deployment_1() -> Value {
    json!({
        "type": "deployment",
        "uuid": "{dep-1}",
        "environment": {"name": "Production", "uuid": "{env-1}", "type": "deployment_environment"},
        "state": {
            "name": "COMPLETED",
            "status": {"name": "SUCCESSFUL"},
            "started_on": "2026-11-01T10:05:00.000000+00:00",
            "completed_on": "2026-11-01T10:07:00.000000+00:00",
            "deployer": {"display_name": "Test User"}
        },
        "release": {
            "type": "pipeline_release",
            "name": "#1164",
            "pipeline": {"uuid": "{pipe-1164}"},
            "commit": {"hash": "def456"},
            "url": "http://127.0.0.1/ws/r/pipelines/results/1164"
        },
        "links": {"self": {"href": "http://127.0.0.1/unused"}}
    })
}

fn summary_deployment_1() -> Value {
    json!({
        "uuid": "{dep-1}",
        "environment": "Production",
        "environment_uuid": "{env-1}",
        "state": "COMPLETED",
        "status": "SUCCESSFUL",
        "started_on": "2026-11-01T10:05:00.000000+00:00",
        "completed_on": "2026-11-01T10:07:00.000000+00:00",
        "pipeline_uuid": "{pipe-1164}",
        "build_number": 1164,
        "commit": "def456",
        "deployer": "Test User",
        "url": "http://127.0.0.1/ws/r/pipelines/results/1164"
    })
}

fn deployments_page() -> Value {
    let mut second = deployment_1();
    second["uuid"] = json!("{dep-2}");
    second["release"]["name"] = json!("hotfix-7");
    let mut third = deployment_1();
    third["uuid"] = json!("{dep-3}");
    third["release"]["name"] = json!("1163");
    json!({
        "page": 1,
        "pagelen": 5,
        "size": 3,
        "values": [deployment_1(), second, third]
    })
}

fn environments_page() -> Value {
    json!({
        "page": 1,
        "size": 2,
        "values": [
            {
                "type": "deployment_environment",
                "uuid": "{env-1}",
                "name": "Production",
                "slug": "production",
                "environment_type": {"name": "Production", "type": "deployment_environment_type"},
                "rank": 3,
                "deployment_gate_enabled": false
            },
            {
                "type": "deployment_environment",
                "uuid": "{env-2}",
                "name": "Staging",
                "slug": "staging",
                "environment_type": {"name": "Staging"},
                "rank": 2
            }
        ]
    })
}

fn repo_variables() -> Value {
    json!({
        "page": 2,
        "pagelen": 10,
        "size": 2,
        "values": [
            {"type": "pipeline_variable", "uuid": "{var-1}", "key": "LOG", "value": "info", "secured": false, "links": {"self": {"href": "http://127.0.0.1/unused"}}},
            {"type": "pipeline_variable", "uuid": "{var-2}", "key": "TOKEN", "secured": true, "links": {"self": {"href": "http://127.0.0.1/unused"}}}
        ]
    })
}

/// Routes keyed by (method, path, page), each answering (status, body text).
type Routes = HashMap<(String, String, String), (u16, String)>;

/// Fake Bitbucket that answers by method, path and `page` query value, which the harness cannot do.
#[derive(Clone, Default)]
struct PagedStore {
    routes: Arc<Mutex<Routes>>,
    seen: Arc<Mutex<Vec<Recorded>>>,
}

struct PagedFake {
    base_url: String,
    store: PagedStore,
}

impl PagedFake {
    async fn start() -> PagedFake {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind paged fake");
        let addr = listener.local_addr().expect("paged fake address");
        let store = PagedStore::default();
        let app = axum::Router::new()
            .fallback(paged_handle)
            .with_state(store.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve paged fake");
        });
        PagedFake {
            base_url: format!("http://{addr}"),
            store,
        }
    }

    /// Answers `method path` when the `page` query value is `page` ("" when absent).
    fn on(&self, method: &str, path: &str, page: &str, status: u16, body: &str) {
        self.store.routes.lock().expect("paged routes lock").insert(
            (method.to_string(), path.to_string(), page.to_string()),
            (status, body.to_string()),
        );
    }

    fn seen(&self) -> Vec<Recorded> {
        self.store.seen.lock().expect("paged seen lock").clone()
    }
}

async fn paged_handle(State(store): State<PagedStore>, req: Request<Body>) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = to_bytes(body, 1024 * 1024)
        .await
        .expect("read paged fake body");
    let method = parts.method.as_str().to_string();
    let path = parts.uri.path().to_string();
    let query = parts.uri.query().map(str::to_string);
    let page = query
        .as_deref()
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix("page="))
        .unwrap_or("")
        .to_string();
    let route = store
        .routes
        .lock()
        .expect("paged routes lock")
        .get(&(method.clone(), path.clone(), page))
        .cloned();
    store.seen.lock().expect("paged seen lock").push(Recorded {
        method: method.clone(),
        path: path.clone(),
        query,
        headers: parts.headers,
        body: bytes.to_vec(),
    });
    match route {
        Some((status, text)) => (
            StatusCode::from_u16(status).expect("paged status"),
            [(CONTENT_TYPE, "application/json")],
            text,
        )
            .into_response(),
        None => (
            StatusCode::from_u16(599).expect("599 is a valid status"),
            format!("no paged route for {method} {path}"),
        )
            .into_response(),
    }
}

/// Calls a Bitbucket tool through the real app, with its API root pointed at `api_root`.
async fn call_at(mcp: &Mcp, api_root: &str, tool: &str, args: Value) -> Value {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": tool, "arguments": args}
    });
    let resp: Value = reqwest::Client::new()
        .post(format!("{}/mcp", mcp.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("X-Bitbucket-Workspace", "ws")
        .header("X-Bitbucket-Username", "tester@example.com")
        .header("X-Bitbucket-Api-Token", "bb-token")
        .header("X-Bitbucket-Base-Url", api_root)
        .json(&body)
        .send()
        .await
        .expect("send MCP request")
        .json()
        .await
        .expect("MCP response is JSON");
    if resp.get("error").is_some() || resp["result"]["isError"] == true {
        panic!("tool {tool} failed: {resp:#}");
    }
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("tool {tool} returned no text: {resp:#}"));
    serde_json::from_str(text)
        .unwrap_or_else(|e| panic!("tool {tool} output is not JSON ({e}): {text}"))
}

// --- Repositories ---

#[tokio::test]
async fn list_repositories_sends_paging_and_role_and_passes_json_through() {
    let mcp = Mcp::start().await;
    let payload = json!({
        "size": 1,
        "page": 2,
        "pagelen": 50,
        "next": "http://127.0.0.1/unused",
        "values": [{"slug": "r", "name": "R", "links": {"self": {"href": "http://127.0.0.1/unused"}}}]
    });
    mcp.fake
        .on("GET", "/2.0/repositories/ws", 200, payload.clone());

    let out = mcp
        .call(
            "bitbucket_list_repositories",
            json!({"pagelen": 50, "page": 2, "role": " member "}),
        )
        .await;
    assert_eq!(out, payload);
    let req = mcp.fake.only_request();
    assert_eq!(req.path, "/2.0/repositories/ws");
    assert_eq!(
        req.query_pairs(),
        pairs(&[("pagelen", "50"), ("page", "2"), ("role", "member")])
    );
}

#[tokio::test]
async fn list_repositories_clamps_pagelen_and_blank_workspace_uses_header() {
    let mcp = Mcp::start().await;
    mcp.fake
        .on("GET", "/2.0/repositories/ws", 200, json!({"values": []}));

    mcp.call(
        "bitbucket_list_repositories",
        json!({"workspace": "  ", "pagelen": 500, "role": "  "}),
    )
    .await;
    mcp.call("bitbucket_list_repositories", json!({"pagelen": 0}))
        .await;

    let seen = mcp.fake.requests();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].path, "/2.0/repositories/ws");
    assert_eq!(seen[0].query_pairs(), pairs(&[("pagelen", "100")]));
    assert_eq!(seen[1].query_pairs(), pairs(&[("pagelen", "1")]));
}

#[tokio::test]
async fn get_repository_encodes_slug_and_honours_workspace_override() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/other-ws/a%20b%2Fc",
        200,
        json!({"slug": "a b/c", "name": "Odd"}),
    );

    let out = mcp
        .call(
            "bitbucket_get_repository",
            json!({"workspace": "other-ws", "repo_slug": "a b/c"}),
        )
        .await;
    assert_eq!(out, json!({"slug": "a b/c", "name": "Odd"}));
    let req = mcp.fake.only_request();
    assert_eq!(req.method, "GET");
    assert_eq!(req.path, "/2.0/repositories/other-ws/a%20b%2Fc");
    assert!(req.query.is_none());
}

#[tokio::test]
async fn get_repository_error_status_reports_code_and_body() {
    let mcp = Mcp::start().await;
    mcp.fake.on_raw(
        "GET",
        "/2.0/repositories/ws/r",
        403,
        r#"{"type":"error","error":{"message":"Forbidden"}}"#,
    );

    let err = mcp
        .call_err("bitbucket_get_repository", json!({"repo_slug": "r"}))
        .await;
    assert_tool_error(
        &err,
        r#"Bitbucket returned 403 Forbidden: {"type":"error","error":{"message":"Forbidden"}}"#,
    );
}

#[tokio::test]
async fn get_repository_blank_slug_is_rejected_before_any_request() {
    let mcp = Mcp::start().await;
    let err = mcp
        .call_err("bitbucket_get_repository", json!({"repo_slug": "  "}))
        .await;
    assert_tool_error(&err, "repo_slug must not be empty");
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn list_branches_sends_paging_and_quoted_name_filter() {
    let mcp = Mcp::start().await;
    let payload =
        json!({"values": [{"name": "feature/x", "target": {"hash": "abc123"}}], "next": "x"});
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/refs/branches",
        200,
        payload.clone(),
    );

    let out = mcp
        .call(
            "bitbucket_list_branches",
            json!({"repo_slug": "r", "pagelen": 10, "page": 2, "name_filter": "  feat "}),
        )
        .await;
    assert_eq!(out, payload);
    let req = mcp.fake.only_request();
    assert_eq!(req.path, "/2.0/repositories/ws/r/refs/branches");
    assert_eq!(
        req.query_pairs(),
        pairs(&[("pagelen", "10"), ("page", "2"), ("q", "name~\"feat\"")])
    );
}

#[tokio::test]
async fn list_tags_sends_sort_and_q_and_skips_blank_sort() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/refs/tags",
        200,
        json!({"values": []}),
    );

    mcp.call(
        "bitbucket_list_tags",
        json!({"repo_slug": "r", "pagelen": 2, "page": 3, "sort": "-target.date", "q": "name ~ \"3.\""}),
    )
    .await;
    mcp.call(
        "bitbucket_list_tags",
        json!({"repo_slug": "r", "sort": "  "}),
    )
    .await;

    let seen = mcp.fake.requests();
    assert_eq!(
        seen[0].query_pairs(),
        pairs(&[
            ("pagelen", "2"),
            ("page", "3"),
            ("sort", "-target.date"),
            ("q", "name ~ \"3.\""),
        ])
    );
    assert!(seen[1].query_pairs().is_empty());
}

#[tokio::test]
async fn list_pull_requests_sends_state_paging_and_sort() {
    let mcp = Mcp::start().await;
    let payload = json!({"values": [{"id": 7, "state": "MERGED"}]});
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pullrequests",
        200,
        payload.clone(),
    );

    let out = mcp
        .call(
            "bitbucket_list_pull_requests",
            json!({"repo_slug": "r", "state": "MERGED", "pagelen": 50, "page": 2, "sort": "-updated_on"}),
        )
        .await;
    assert_eq!(out, payload);
    mcp.call(
        "bitbucket_list_pull_requests",
        json!({"repo_slug": "r", "state": "  "}),
    )
    .await;

    let seen = mcp.fake.requests();
    assert_eq!(seen[0].path, "/2.0/repositories/ws/r/pullrequests");
    assert_eq!(
        seen[0].query_pairs(),
        pairs(&[
            ("state", "MERGED"),
            ("pagelen", "50"),
            ("page", "2"),
            ("sort", "-updated_on"),
        ])
    );
    assert!(seen[1].query_pairs().is_empty());
}

#[tokio::test]
async fn get_pull_request_passes_json_through() {
    let mcp = Mcp::start().await;
    let payload = json!({
        "id": 7,
        "title": "Add X",
        "state": "OPEN",
        "links": {"html": {"href": "http://127.0.0.1/ws/r/pull-requests/7"}}
    });
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pullrequests/7",
        200,
        payload.clone(),
    );

    let out = mcp
        .call(
            "bitbucket_get_pull_request",
            json!({"repo_slug": "r", "pull_request_id": 7}),
        )
        .await;
    assert_eq!(out, payload);
    let req = mcp.fake.only_request();
    assert_eq!(req.method, "GET");
    assert!(req.body.is_empty());
}

// --- Pull request create and update ---

#[tokio::test]
async fn create_pull_request_sends_full_body() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pullrequests",
        201,
        json!({"id": 8, "state": "OPEN"}),
    );

    let out = mcp
        .call(
            "bitbucket_create_pull_request",
            json!({
                "repo_slug": "r",
                "title": "  Add X  ",
                "description": " Body text ",
                "source_branch": "feature/x",
                "destination_branch": "master",
                "close_source_branch": true,
                "reviewers": ["acc-1"]
            }),
        )
        .await;
    assert_eq!(out, json!({"id": 8, "state": "OPEN"}));
    let req = mcp.fake.only_request();
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/2.0/repositories/ws/r/pullrequests");
    assert_eq!(
        req.json(),
        json!({
            "title": "Add X",
            "description": "Body text",
            "source": {"branch": {"name": "feature/x"}},
            "destination": {"branch": {"name": "master"}},
            "close_source_branch": true,
            "reviewers": [{"uuid": "acc-1"}]
        })
    );
}

#[tokio::test]
async fn create_pull_request_omits_blank_description_and_empty_reviewers() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pullrequests",
        201,
        json!({"id": 9}),
    );

    mcp.call(
        "bitbucket_create_pull_request",
        json!({
            "repo_slug": "r",
            "title": "Fix",
            "description": "   ",
            "source_branch": "fix/1",
            "destination_branch": "master",
            "reviewers": []
        }),
    )
    .await;
    assert_eq!(
        mcp.fake.only_request().json(),
        json!({
            "title": "Fix",
            "source": {"branch": {"name": "fix/1"}},
            "destination": {"branch": {"name": "master"}},
            "close_source_branch": false
        })
    );
}

#[tokio::test]
async fn create_pull_request_carries_1000_reviewers_in_order() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pullrequests",
        201,
        json!({"id": 10}),
    );
    let uuids: Vec<String> = (0..1000).map(|i| format!("acc-{i}")).collect();

    mcp.call(
        "bitbucket_create_pull_request",
        json!({
            "repo_slug": "r",
            "title": "Big",
            "source_branch": "big",
            "destination_branch": "master",
            "reviewers": uuids
        }),
    )
    .await;
    let body = mcp.fake.only_request().json();
    let expected: Vec<Value> = (0..1000)
        .map(|i| json!({"uuid": format!("acc-{i}")}))
        .collect();
    assert_eq!(body["reviewers"], Value::Array(expected));
}

#[tokio::test]
async fn create_pull_request_blank_required_fields_are_rejected() {
    let mcp = Mcp::start().await;

    let title = mcp
        .call_err(
            "bitbucket_create_pull_request",
            json!({"repo_slug": "r", "title": "  ", "source_branch": "a", "destination_branch": "master"}),
        )
        .await;
    assert_tool_error(&title, "title must not be empty");
    let source = mcp
        .call_err(
            "bitbucket_create_pull_request",
            json!({"repo_slug": "r", "title": "T", "source_branch": "", "destination_branch": "master"}),
        )
        .await;
    assert_tool_error(&source, "source_branch must not be empty");
    let dest = mcp
        .call_err(
            "bitbucket_create_pull_request",
            json!({"repo_slug": "r", "title": "T", "source_branch": "a", "destination_branch": " "}),
        )
        .await;
    assert_tool_error(&dest, "destination_branch must not be empty");
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn update_pull_request_description_sends_carried_fields() {
    let mcp = Mcp::start().await;
    let current = json!({
        "id": 7,
        "title": "Old title",
        "description": "Old body",
        "reviewers": [{"uuid": "acc-1", "display_name": "Test User"}],
        "destination": {"branch": {"name": "master"}},
        "close_source_branch": false,
        "state": "OPEN"
    });
    mcp.fake
        .on("GET", "/2.0/repositories/ws/r/pullrequests/7", 200, current);
    mcp.fake.on(
        "PUT",
        "/2.0/repositories/ws/r/pullrequests/7",
        200,
        json!({"id": 7}),
    );

    let out = mcp
        .call(
            "bitbucket_update_pull_request",
            json!({"repo_slug": "r", "pull_request_id": 7, "description": "New body"}),
        )
        .await;
    assert_eq!(out, json!({"id": 7}));
    assert_eq!(
        trail(&mcp),
        vec![
            "GET /2.0/repositories/ws/r/pullrequests/7",
            "PUT /2.0/repositories/ws/r/pullrequests/7",
        ]
    );
    assert_eq!(
        mcp.fake.requests()[1].json(),
        json!({
            "title": "Old title",
            "description": "New body",
            "reviewers": [{"uuid": "acc-1"}],
            "destination": {"branch": {"name": "master"}},
            "close_source_branch": false
        })
    );
}

#[tokio::test]
async fn update_pull_request_title_only_keeps_description_and_empty_description_clears() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pullrequests/7",
        200,
        json!({
            "id": 7,
            "title": "Old title",
            "description": "Old body",
            "destination": {"branch": {"name": "master"}},
            "close_source_branch": true
        }),
    );
    mcp.fake.on(
        "PUT",
        "/2.0/repositories/ws/r/pullrequests/7",
        200,
        json!({"id": 7}),
    );

    mcp.call(
        "bitbucket_update_pull_request",
        json!({"repo_slug": "r", "pull_request_id": 7, "title": " New title "}),
    )
    .await;
    mcp.call(
        "bitbucket_update_pull_request",
        json!({"repo_slug": "r", "pull_request_id": 7, "description": ""}),
    )
    .await;

    let seen = mcp.fake.requests();
    assert_eq!(seen.len(), 4);
    // Title is trimmed; no reviewers key because the current PR has none.
    assert_eq!(
        seen[1].json(),
        json!({
            "title": "New title",
            "description": "Old body",
            "destination": {"branch": {"name": "master"}},
            "close_source_branch": true
        })
    );
    assert_eq!(
        seen[3].json(),
        json!({
            "title": "Old title",
            "description": "",
            "destination": {"branch": {"name": "master"}},
            "close_source_branch": true
        })
    );
}

#[tokio::test]
async fn update_pull_request_refuses_reviewer_without_uuid() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pullrequests/7",
        200,
        json!({"title": "T", "description": "D", "reviewers": [{"display_name": "Test User"}]}),
    );

    let err = mcp
        .call_err(
            "bitbucket_update_pull_request",
            json!({"repo_slug": "r", "pull_request_id": 7, "description": "x"}),
        )
        .await;
    assert_tool_error(
        &err,
        "a current reviewer has no uuid; refusing to update and drop reviewers",
    );
    assert_eq!(
        trail(&mcp),
        vec!["GET /2.0/repositories/ws/r/pullrequests/7"]
    );
}

#[tokio::test]
async fn update_pull_request_title_missing_from_current_pr_is_an_error() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pullrequests/7",
        200,
        json!({"description": "D"}),
    );

    let err = mcp
        .call_err(
            "bitbucket_update_pull_request",
            json!({"repo_slug": "r", "pull_request_id": 7, "description": "x"}),
        )
        .await;
    assert_tool_error(&err, "could not read the current pull request title");
    assert_eq!(mcp.fake.requests().len(), 1);
}

#[tokio::test]
async fn update_pull_request_needs_title_or_description_before_any_request() {
    let mcp = Mcp::start().await;
    let err = mcp
        .call_err(
            "bitbucket_update_pull_request",
            json!({"repo_slug": "r", "pull_request_id": 7}),
        )
        .await;
    assert_tool_error(
        &err,
        "at least one of title or description must be provided",
    );

    let blank = mcp
        .call_err(
            "bitbucket_update_pull_request",
            json!({"repo_slug": "r", "pull_request_id": 7, "title": "  "}),
        )
        .await;
    assert_tool_error(&blank, "title must not be empty");
    assert!(mcp.fake.requests().is_empty());
}

// --- Pull request diff, diffstat, activity, comments ---

#[tokio::test]
async fn get_pull_request_diff_returns_text_in_diff_field() {
    let mcp = Mcp::start().await;
    let diff = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n";
    mcp.fake.on_raw(
        "GET",
        "/2.0/repositories/ws/r/pullrequests/7/diff",
        200,
        diff,
    );

    let out = mcp
        .call(
            "bitbucket_get_pull_request_diff",
            json!({"repo_slug": "r", "pull_request_id": 7}),
        )
        .await;
    assert_eq!(out, json!({"diff": diff}));
    let req = mcp.fake.only_request();
    assert_eq!(
        req.headers["accept"].to_str().expect("ascii accept"),
        "text/x-diff, text/plain, application/json, */*;q=0.1"
    );
}

#[tokio::test]
async fn get_pull_request_diff_error_status_reports_code_and_body() {
    let mcp = Mcp::start().await;
    mcp.fake.on_raw(
        "GET",
        "/2.0/repositories/ws/r/pullrequests/7/diff",
        404,
        "no such pull request",
    );

    let err = mcp
        .call_err(
            "bitbucket_get_pull_request_diff",
            json!({"repo_slug": "r", "pull_request_id": 7}),
        )
        .await;
    assert_tool_error(
        &err,
        "Bitbucket returned 404 Not Found: no such pull request",
    );
}

#[tokio::test]
async fn get_pull_request_diffstat_clamps_pagelen_and_sends_page() {
    let mcp = Mcp::start().await;
    let payload = json!({
        "size": 1,
        "values": [{
            "type": "diffstat",
            "status": "modified",
            "lines_added": 2,
            "lines_removed": 1,
            "old": {"path": "a.txt"},
            "new": {"path": "a.txt"}
        }]
    });
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pullrequests/7/diffstat",
        200,
        payload.clone(),
    );

    let out = mcp
        .call(
            "bitbucket_get_pull_request_diffstat",
            json!({"repo_slug": "r", "pull_request_id": 7, "pagelen": 1000, "page": 2}),
        )
        .await;
    assert_eq!(out, payload);
    assert_eq!(
        mcp.fake.only_request().query_pairs(),
        pairs(&[("pagelen", "100"), ("page", "2")])
    );
}

#[tokio::test]
async fn list_pull_request_comments_sends_paging_and_passes_json_through() {
    let mcp = Mcp::start().await;
    let payload = json!({"values": [{"id": 55, "content": {"raw": "Nice"}}], "page": 1});
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pullrequests/7/comments",
        200,
        payload.clone(),
    );

    let out = mcp
        .call(
            "bitbucket_list_pull_request_comments",
            json!({"repo_slug": "r", "pull_request_id": 7, "pagelen": 5, "page": 1}),
        )
        .await;
    assert_eq!(out, payload);
    assert_eq!(
        mcp.fake.only_request().query_pairs(),
        pairs(&[("pagelen", "5"), ("page", "1")])
    );
}

#[tokio::test]
async fn get_pull_request_comment_keeps_ids_above_2_pow_53() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pullrequests/7/comments/9007199254740993",
        200,
        json!({"id": 9007199254740993u64, "content": {"raw": "hi"}}),
    );

    let out = mcp
        .call(
            "bitbucket_get_pull_request_comment",
            json!({"repo_slug": "r", "pull_request_id": 7, "comment_id": 9007199254740993u64}),
        )
        .await;
    assert_eq!(out["id"], 9007199254740993u64);
    assert_eq!(out["content"]["raw"], "hi");
}

// --- Pull request comments ---

#[tokio::test]
async fn create_pull_request_comment_top_level_body_is_trimmed() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pullrequests/7/comments",
        201,
        json!({"id": 60}),
    );

    let out = mcp
        .call(
            "bitbucket_create_pull_request_comment",
            json!({"repo_slug": "r", "pull_request_id": 7, "content": "  Nice work \n"}),
        )
        .await;
    assert_eq!(out, json!({"id": 60}));
    // Pins current behaviour: content is trimmed, so leading indentation of the first line is lost.
    assert_eq!(
        mcp.fake.only_request().json(),
        json!({"content": {"raw": "Nice work"}})
    );
}

#[tokio::test]
async fn create_pull_request_comment_keeps_parent_id_zero() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pullrequests/7/comments",
        201,
        json!({"id": 61}),
    );

    mcp.call(
        "bitbucket_create_pull_request_comment",
        json!({"repo_slug": "r", "pull_request_id": 7, "content": "Reply", "parent_comment_id": 0}),
    )
    .await;
    assert_eq!(
        mcp.fake.only_request().json(),
        json!({"content": {"raw": "Reply"}, "parent": {"id": 0}})
    );
}

#[tokio::test]
async fn create_pull_request_comment_inline_anchor_is_trimmed() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pullrequests/7/comments",
        201,
        json!({"id": 62}),
    );

    mcp.call(
        "bitbucket_create_pull_request_comment",
        json!({
            "repo_slug": "r",
            "pull_request_id": 7,
            "content": "Fix this",
            "inline_path": "  app/Info.plist ",
            "inline_to": 12
        }),
    )
    .await;
    assert_eq!(
        mcp.fake.only_request().json(),
        json!({
            "content": {"raw": "Fix this"},
            "inline": {"path": "app/Info.plist", "to": 12}
        })
    );
}

#[tokio::test]
async fn create_pull_request_comment_partial_inline_anchor_is_dropped() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pullrequests/7/comments",
        201,
        json!({"id": 63}),
    );

    mcp.call(
        "bitbucket_create_pull_request_comment",
        json!({"repo_slug": "r", "pull_request_id": 7, "content": "A", "inline_path": "app/x.rs"}),
    )
    .await;
    mcp.call(
        "bitbucket_create_pull_request_comment",
        json!({"repo_slug": "r", "pull_request_id": 7, "content": "B", "inline_to": 3}),
    )
    .await;

    let seen = mcp.fake.requests();
    assert_eq!(seen[0].json(), json!({"content": {"raw": "A"}}));
    assert_eq!(seen[1].json(), json!({"content": {"raw": "B"}}));
}

#[tokio::test]
async fn create_pull_request_comment_blank_content_is_rejected() {
    let mcp = Mcp::start().await;
    let err = mcp
        .call_err(
            "bitbucket_create_pull_request_comment",
            json!({"repo_slug": "r", "pull_request_id": 7, "content": "   "}),
        )
        .await;
    assert_tool_error(&err, "content must not be empty");
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn reply_pull_request_comment_sets_parent() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pullrequests/7/comments",
        201,
        json!({"id": 64, "parent": {"id": 55}}),
    );

    let out = mcp
        .call(
            "bitbucket_reply_pull_request_comment",
            json!({"repo_slug": "r", "pull_request_id": 7, "parent_comment_id": 55, "content": "Thanks"}),
        )
        .await;
    assert_eq!(out, json!({"id": 64, "parent": {"id": 55}}));
    let req = mcp.fake.only_request();
    assert_eq!(req.path, "/2.0/repositories/ws/r/pullrequests/7/comments");
    assert_eq!(
        req.json(),
        json!({"content": {"raw": "Thanks"}, "parent": {"id": 55}})
    );
}

#[tokio::test]
async fn update_pull_request_comment_sends_raw_content() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "PUT",
        "/2.0/repositories/ws/r/pullrequests/7/comments/55",
        200,
        json!({"id": 55, "content": {"raw": "Edited"}}),
    );

    let out = mcp
        .call(
            "bitbucket_update_pull_request_comment",
            json!({"repo_slug": "r", "pull_request_id": 7, "comment_id": 55, "content": "Edited"}),
        )
        .await;
    assert_eq!(out, json!({"id": 55, "content": {"raw": "Edited"}}));
    let req = mcp.fake.only_request();
    assert_eq!(req.method, "PUT");
    assert_eq!(req.json(), json!({"content": {"raw": "Edited"}}));
}

#[tokio::test]
async fn update_pull_request_comment_blank_content_is_rejected() {
    let mcp = Mcp::start().await;
    let err = mcp
        .call_err(
            "bitbucket_update_pull_request_comment",
            json!({"repo_slug": "r", "pull_request_id": 7, "comment_id": 55, "content": ""}),
        )
        .await;
    assert_tool_error(&err, "content must not be empty");
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn delete_pull_request_comment_empty_204_is_ok_and_deleted() {
    let mcp = Mcp::start().await;
    mcp.fake.on_raw(
        "DELETE",
        "/2.0/repositories/ws/r/pullrequests/7/comments/55",
        204,
        "",
    );

    let out = mcp
        .call(
            "bitbucket_delete_pull_request_comment",
            json!({"repo_slug": "r", "pull_request_id": 7, "comment_id": 55}),
        )
        .await;
    assert_eq!(out, json!({"ok": true, "deleted": true}));
    assert_eq!(mcp.fake.only_request().method, "DELETE");
}

#[tokio::test]
async fn delete_pull_request_comment_json_200_is_passed_through() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "DELETE",
        "/2.0/repositories/ws/r/pullrequests/7/comments/55",
        200,
        json!({"id": 55, "note": "kept"}),
    );

    let out = mcp
        .call(
            "bitbucket_delete_pull_request_comment",
            json!({"repo_slug": "r", "pull_request_id": 7, "comment_id": 55}),
        )
        .await;
    assert_eq!(out, json!({"id": 55, "note": "kept"}));
}

#[tokio::test]
async fn delete_pull_request_comment_non_json_200_is_an_error() {
    let mcp = Mcp::start().await;
    mcp.fake.on_raw(
        "DELETE",
        "/2.0/repositories/ws/r/pullrequests/7/comments/55",
        200,
        "gone",
    );

    let err = mcp
        .call_err(
            "bitbucket_delete_pull_request_comment",
            json!({"repo_slug": "r", "pull_request_id": 7, "comment_id": 55}),
        )
        .await;
    assert_tool_error(
        &err,
        "Invalid JSON from Bitbucket: expected value at line 1 column 1",
    );
}

#[tokio::test]
async fn delete_pull_request_comment_403_reports_status_and_body() {
    let mcp = Mcp::start().await;
    mcp.fake.on_raw(
        "DELETE",
        "/2.0/repositories/ws/r/pullrequests/7/comments/55",
        403,
        "You do not have permission",
    );

    let err = mcp
        .call_err(
            "bitbucket_delete_pull_request_comment",
            json!({"repo_slug": "r", "pull_request_id": 7, "comment_id": 55}),
        )
        .await;
    assert_tool_error(
        &err,
        "Bitbucket returned 403 Forbidden: You do not have permission",
    );
}

// --- Files ---

#[tokio::test]
async fn get_file_encodes_commit_and_path_and_returns_raw_text() {
    let mcp = Mcp::start().await;
    let text = "<?xml version=\"1.0\"?>\n<plist/>\n";
    mcp.fake.on_raw(
        "GET",
        "/2.0/repositories/ws/r/src/feature%2Fa%20b/app/Info%20Plist%231.plist",
        200,
        text,
    );

    let out = mcp
        .call_text(
            "bitbucket_get_file",
            json!({"repo_slug": "r", "commit": "feature/a b", "path": "/app/Info Plist#1.plist/"}),
        )
        .await;
    assert_eq!(out, text);
    let req = mcp.fake.only_request();
    assert_eq!(req.headers["accept"].to_str().expect("ascii accept"), "*/*");
}

#[tokio::test]
async fn get_file_404_reports_status_and_body() {
    let mcp = Mcp::start().await;
    mcp.fake.on_raw(
        "GET",
        "/2.0/repositories/ws/r/src/3.1.0/missing.txt",
        404,
        "No such file",
    );

    let err = mcp
        .call_err(
            "bitbucket_get_file",
            json!({"repo_slug": "r", "commit": "3.1.0", "path": "missing.txt"}),
        )
        .await;
    assert_tool_error(&err, "Bitbucket returned 404 Not Found: No such file");
}

#[tokio::test]
async fn get_file_rejects_dot_segments_and_blank_inputs_before_any_request() {
    let mcp = Mcp::start().await;

    let dot = mcp
        .call_err(
            "bitbucket_get_file",
            json!({"repo_slug": "r", "commit": ".", "path": "a.txt"}),
        )
        .await;
    assert_tool_error(&dot, "commit must not be . or ..");
    let dotdot = mcp
        .call_err(
            "bitbucket_get_file",
            json!({"repo_slug": "r", "commit": "..", "path": "a.txt"}),
        )
        .await;
    assert_tool_error(&dotdot, "commit must not be . or ..");
    let blank_commit = mcp
        .call_err(
            "bitbucket_get_file",
            json!({"repo_slug": "r", "commit": "  ", "path": "a.txt"}),
        )
        .await;
    assert_tool_error(&blank_commit, "commit must not be empty");
    let dot_path = mcp
        .call_err(
            "bitbucket_get_file",
            json!({"repo_slug": "r", "commit": "main", "path": "a/../b"}),
        )
        .await;
    assert_tool_error(&dot_path, "path segment must not be . or ..");
    let blank_path = mcp
        .call_err(
            "bitbucket_get_file",
            json!({"repo_slug": "r", "commit": "main", "path": "  "}),
        )
        .await;
    assert_tool_error(&blank_path, "path must not be empty");
    let empty_segment = mcp
        .call_err(
            "bitbucket_get_file",
            json!({"repo_slug": "r", "commit": "main", "path": "a//b"}),
        )
        .await;
    assert_tool_error(&empty_segment, "path must not contain empty segments");
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn get_file_returns_1000x_content_unchanged() {
    let mcp = Mcp::start().await;
    let text: String = (0..20_000).map(|i| format!("line {i}\n")).collect();
    mcp.fake.on_raw(
        "GET",
        "/2.0/repositories/ws/r/src/3.1.0/big.txt",
        200,
        &text,
    );

    let out = mcp
        .call_text(
            "bitbucket_get_file",
            json!({"repo_slug": "r", "commit": "3.1.0", "path": "big.txt"}),
        )
        .await;
    assert_eq!(out.len(), text.len());
    assert!(out == text);
}

#[tokio::test]
async fn file_history_clamps_pagelen_and_passes_numeric_cursor() {
    let mcp = Mcp::start().await;
    let payload = json!({"values": [{"commit": {"hash": "abc123"}}], "next": "x"});
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/filehistory/dev/.gitignore",
        200,
        payload.clone(),
    );

    let out = mcp
        .call(
            "bitbucket_file_history",
            json!({"repo_slug": "r", "commit": "dev", "path": ".gitignore", "pagelen": 500, "page": 2}),
        )
        .await;
    assert_eq!(out, payload);
    assert_eq!(
        mcp.fake.only_request().query_pairs(),
        pairs(&[("pagelen", "100"), ("page", "2")])
    );
}

#[tokio::test]
async fn file_history_passes_string_cursor_and_skips_blank_cursor() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/filehistory/dev/a",
        200,
        json!({"values": []}),
    );

    mcp.call(
        "bitbucket_file_history",
        json!({"repo_slug": "r", "commit": "dev", "path": "a", "page": "E8Ux"}),
    )
    .await;
    mcp.call(
        "bitbucket_file_history",
        json!({"repo_slug": "r", "commit": "dev", "path": "a", "page": "  "}),
    )
    .await;

    let seen = mcp.fake.requests();
    assert_eq!(seen[0].query_pairs(), pairs(&[("page", "E8Ux")]));
    assert!(seen[1].query_pairs().is_empty());
}

// --- Commits ---

#[tokio::test]
async fn get_commit_passes_json_through_and_rejects_dot() {
    let mcp = Mcp::start().await;
    let payload = json!({"hash": "abc123", "message": "Fix\n"});
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/commit/abc123",
        200,
        payload.clone(),
    );

    let out = mcp
        .call(
            "bitbucket_get_commit",
            json!({"repo_slug": "r", "commit": "abc123"}),
        )
        .await;
    assert_eq!(out, payload);

    let err = mcp
        .call_err(
            "bitbucket_get_commit",
            json!({"repo_slug": "r", "commit": ".."}),
        )
        .await;
    assert_tool_error(&err, "commit must not be . or ..");
    assert_eq!(
        trail(&mcp),
        vec!["GET /2.0/repositories/ws/r/commit/abc123"]
    );
}

#[tokio::test]
async fn list_commits_defaults_to_head_and_encodes_revision() {
    let mcp = Mcp::start().await;
    let payload = json!({"values": [{"hash": "abc123"}], "next": "x"});
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/commits/HEAD",
        200,
        payload.clone(),
    );
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/commits/feature%2Fa%20b%3Fc",
        200,
        payload.clone(),
    );

    let out = mcp
        .call("bitbucket_list_commits", json!({"repo_slug": "r"}))
        .await;
    assert_eq!(out, payload);
    mcp.call(
        "bitbucket_list_commits",
        json!({"repo_slug": "r", "revision": "feature/a b?c"}),
    )
    .await;

    let seen = mcp.fake.requests();
    assert_eq!(seen[0].path, "/2.0/repositories/ws/r/commits/HEAD");
    assert!(seen[0].query.is_none());
    assert_eq!(
        seen[1].path,
        "/2.0/repositories/ws/r/commits/feature%2Fa%20b%3Fc"
    );
    assert!(seen[1].query.is_none());
}

#[tokio::test]
async fn list_commits_include_exclude_query_and_cursor() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/commits",
        200,
        json!({"values": []}),
    );

    mcp.call(
        "bitbucket_list_commits",
        json!({
            "repo_slug": "r",
            "include": "dev",
            "exclude": ["master", "3.0.0"],
            "pagelen": 2,
            "page": 2,
            "ctx": "abc"
        }),
    )
    .await;

    let req = mcp.fake.only_request();
    assert_eq!(req.path, "/2.0/repositories/ws/r/commits");
    assert_eq!(
        req.query_pairs(),
        pairs(&[
            ("include", "dev"),
            ("exclude", "master"),
            ("exclude", "3.0.0"),
            ("pagelen", "2"),
            ("page", "2"),
            ("ctx", "abc"),
        ])
    );
}

#[tokio::test]
async fn list_commits_null_include_and_exclude_are_absent() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/commits/HEAD",
        200,
        json!({"values": []}),
    );

    mcp.call(
        "bitbucket_list_commits",
        json!({"repo_slug": "r", "include": null, "exclude": null}),
    )
    .await;

    let req = mcp.fake.only_request();
    assert_eq!(req.path, "/2.0/repositories/ws/r/commits/HEAD");
    assert!(req.query.is_none());
}

// --- Pull request workflow ---

#[tokio::test]
async fn approve_pull_request_posts_and_passes_json_through() {
    let mcp = Mcp::start().await;
    let payload = json!({"approved": true, "user": {"display_name": "Test User"}});
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pullrequests/7/approve",
        200,
        payload.clone(),
    );

    let out = mcp
        .call(
            "bitbucket_approve_pull_request",
            json!({"repo_slug": "r", "pull_request_id": 7}),
        )
        .await;
    assert_eq!(out, payload);
    let req = mcp.fake.only_request();
    assert_eq!(req.method, "POST");
    assert!(req.body.is_empty());
}

#[tokio::test]
async fn approve_pull_request_empty_200_is_ok_true() {
    let mcp = Mcp::start().await;
    mcp.fake.on_raw(
        "POST",
        "/2.0/repositories/ws/r/pullrequests/8/approve",
        200,
        "",
    );

    let out = mcp
        .call(
            "bitbucket_approve_pull_request",
            json!({"repo_slug": "r", "pull_request_id": 8}),
        )
        .await;
    assert_eq!(out, json!({"ok": true}));
}

#[tokio::test]
async fn approve_pull_request_403_reports_status_and_body() {
    let mcp = Mcp::start().await;
    mcp.fake.on_raw(
        "POST",
        "/2.0/repositories/ws/r/pullrequests/7/approve",
        403,
        "You cannot approve your own pull request",
    );

    let err = mcp
        .call_err(
            "bitbucket_approve_pull_request",
            json!({"repo_slug": "r", "pull_request_id": 7}),
        )
        .await;
    assert_tool_error(
        &err,
        "Bitbucket returned 403 Forbidden: You cannot approve your own pull request",
    );
}

#[tokio::test]
async fn unapprove_pull_request_204_is_ok_true() {
    let mcp = Mcp::start().await;
    mcp.fake.on_raw(
        "DELETE",
        "/2.0/repositories/ws/r/pullrequests/7/approve",
        204,
        "",
    );

    let out = mcp
        .call(
            "bitbucket_unapprove_pull_request",
            json!({"repo_slug": "r", "pull_request_id": 7}),
        )
        .await;
    assert_eq!(out, json!({"ok": true}));
    assert_eq!(mcp.fake.only_request().method, "DELETE");
}

#[tokio::test]
async fn decline_pull_request_sends_trimmed_message() {
    let mcp = Mcp::start().await;
    let payload = json!({"id": 7, "state": "DECLINED"});
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pullrequests/7/decline",
        200,
        payload.clone(),
    );

    let out = mcp
        .call(
            "bitbucket_decline_pull_request",
            json!({"repo_slug": "r", "pull_request_id": 7, "message": "  Stale branch  "}),
        )
        .await;
    assert_eq!(out, payload);
    assert_eq!(
        mcp.fake.only_request().json(),
        json!({"message": "Stale branch"})
    );
}

#[tokio::test]
async fn decline_pull_request_without_message_sends_empty_object() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pullrequests/7/decline",
        200,
        json!({"id": 7, "state": "DECLINED"}),
    );

    mcp.call(
        "bitbucket_decline_pull_request",
        json!({"repo_slug": "r", "pull_request_id": 7}),
    )
    .await;
    mcp.call(
        "bitbucket_decline_pull_request",
        json!({"repo_slug": "r", "pull_request_id": 7, "message": "   "}),
    )
    .await;

    let seen = mcp.fake.requests();
    assert_eq!(seen[0].json(), json!({}));
    assert_eq!(seen[1].json(), json!({}));
}

#[tokio::test]
async fn merge_pull_request_sends_all_fields_including_false() {
    let mcp = Mcp::start().await;
    let payload = json!({"id": 7, "state": "MERGED"});
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pullrequests/7/merge",
        200,
        payload.clone(),
    );

    let out = mcp
        .call(
            "bitbucket_merge_pull_request",
            json!({
                "repo_slug": "r",
                "pull_request_id": 7,
                "merge_strategy": " squash ",
                "close_source_branch": false,
                "message": " Ship it "
            }),
        )
        .await;
    assert_eq!(out, payload);
    assert_eq!(
        mcp.fake.only_request().json(),
        json!({"merge_strategy": "squash", "close_source_branch": false, "message": "Ship it"})
    );
}

#[tokio::test]
async fn merge_pull_request_blank_optionals_are_omitted() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pullrequests/7/merge",
        200,
        json!({"id": 7}),
    );

    mcp.call(
        "bitbucket_merge_pull_request",
        json!({"repo_slug": "r", "pull_request_id": 7, "merge_strategy": "  ", "message": ""}),
    )
    .await;
    mcp.call(
        "bitbucket_merge_pull_request",
        json!({"repo_slug": "r", "pull_request_id": 7}),
    )
    .await;

    let seen = mcp.fake.requests();
    assert_eq!(seen[0].json(), json!({}));
    assert_eq!(seen[1].json(), json!({}));
}

#[tokio::test]
async fn list_pull_request_activity_sends_paging_and_passes_json_through() {
    let mcp = Mcp::start().await;
    let payload = json!({"values": [{"approval": {"user": {"display_name": "Test User"}}}]});
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pullrequests/7/activity",
        200,
        payload.clone(),
    );

    let out = mcp
        .call(
            "bitbucket_list_pull_request_activity",
            json!({"repo_slug": "r", "pull_request_id": 7, "pagelen": 20, "page": 3}),
        )
        .await;
    assert_eq!(out, payload);
    assert_eq!(
        mcp.fake.only_request().query_pairs(),
        pairs(&[("pagelen", "20"), ("page", "3")])
    );
}

#[tokio::test]
async fn search_users_sends_display_name_query_and_workspace_override() {
    let mcp = Mcp::start().await;
    let payload = json!({"values": [{"user": {"display_name": "Test User", "uuid": "acc-1"}}]});
    mcp.fake
        .on("GET", "/2.0/workspaces/ws/members", 200, payload.clone());
    mcp.fake.on(
        "GET",
        "/2.0/workspaces/other/members",
        200,
        json!({"values": []}),
    );

    let out = mcp
        .call(
            "bitbucket_search_users",
            json!({"query": "Test User", "pagelen": 10, "page": 1}),
        )
        .await;
    assert_eq!(out, payload);
    mcp.call(
        "bitbucket_search_users",
        json!({"workspace": "other", "query": "  "}),
    )
    .await;

    let seen = mcp.fake.requests();
    assert_eq!(
        seen[0].query_pairs(),
        pairs(&[
            (
                "q",
                "user.display_name ~ \"Test User\" OR user.nickname ~ \"Test User\"",
            ),
            ("pagelen", "10"),
            ("page", "1"),
        ])
    );
    assert_eq!(seen[1].path, "/2.0/workspaces/other/members");
    assert!(seen[1].query.is_none());
}

// --- Pipelines and deployments ---

#[tokio::test]
async fn list_pipelines_summarises_runs_and_sends_newest_first_query() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pipelines",
        200,
        pipelines_page(),
    );

    let out = mcp
        .call(
            "bitbucket_list_pipelines",
            json!({"repo_slug": "r", "pagelen": 10, "page": 1}),
        )
        .await;
    assert_eq!(
        out,
        json!({
            "page": 1,
            "pagelen": 10,
            "size": 3,
            "filtered_by_branch": null,
            "pipelines": [summary_142(), summary_141(), summary_140()]
        })
    );
    assert_eq!(
        mcp.fake.only_request().query_pairs(),
        pairs(&[("sort", "-created_on"), ("pagelen", "10"), ("page", "1")])
    );
}

#[tokio::test]
async fn list_pipelines_target_branch_filters_summaries() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pipelines",
        200,
        pipelines_page(),
    );

    let out = mcp
        .call(
            "bitbucket_list_pipelines",
            json!({"repo_slug": "r", "target_branch": "master"}),
        )
        .await;
    assert_eq!(
        out,
        json!({
            "page": 1,
            "pagelen": 10,
            "size": 3,
            "filtered_by_branch": "master",
            "pipelines": [summary_142(), summary_140()]
        })
    );
}

#[tokio::test]
async fn list_pipelines_raw_returns_full_payload() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pipelines",
        200,
        pipelines_page(),
    );

    let out = mcp
        .call(
            "bitbucket_list_pipelines",
            json!({"repo_slug": "r", "raw": true}),
        )
        .await;
    assert_eq!(out, pipelines_page());
}

#[tokio::test]
async fn get_pipeline_summarises_by_build_number() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pipelines/142",
        200,
        pipeline_142(),
    );

    let out = mcp
        .call(
            "bitbucket_get_pipeline",
            json!({"repo_slug": "r", "pipeline": "142"}),
        )
        .await;
    assert_eq!(out, summary_142());
    assert_eq!(
        mcp.fake.only_request().path,
        "/2.0/repositories/ws/r/pipelines/142"
    );
}

#[tokio::test]
async fn get_pipeline_uuid_is_encoded_and_raw_returns_full_object() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pipelines/%7Bpipe-142%7D",
        200,
        pipeline_142(),
    );

    let summary = mcp
        .call(
            "bitbucket_get_pipeline",
            json!({"repo_slug": "r", "pipeline": "{pipe-142}"}),
        )
        .await;
    assert_eq!(summary, summary_142());
    let raw = mcp
        .call(
            "bitbucket_get_pipeline",
            json!({"repo_slug": "r", "pipeline": "{pipe-142}", "raw": true}),
        )
        .await;
    assert_eq!(raw, pipeline_142());
    assert_eq!(
        trail(&mcp),
        vec![
            "GET /2.0/repositories/ws/r/pipelines/%7Bpipe-142%7D",
            "GET /2.0/repositories/ws/r/pipelines/%7Bpipe-142%7D",
        ]
    );
}

#[tokio::test]
async fn get_pipeline_blank_is_rejected_before_any_request() {
    let mcp = Mcp::start().await;
    let err = mcp
        .call_err(
            "bitbucket_get_pipeline",
            json!({"repo_slug": "r", "pipeline": "  "}),
        )
        .await;
    assert_tool_error(&err, "pipeline must not be empty");
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn list_pipeline_steps_summarises_and_maps_max_time() {
    let mcp = Mcp::start().await;
    let payload = json!({"page": 1, "size": 2, "values": [step_1(), step_2()]});
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pipelines/142/steps",
        200,
        payload,
    );

    let out = mcp
        .call(
            "bitbucket_list_pipeline_steps",
            json!({"repo_slug": "r", "pipeline": "142"}),
        )
        .await;
    assert_eq!(
        out,
        json!({"pipeline": "142", "steps": [summary_step_1(), summary_step_2()]})
    );
    assert_eq!(
        mcp.fake.only_request().query_pairs(),
        pairs(&[("pagelen", "100")])
    );
}

#[tokio::test]
async fn list_pipeline_steps_raw_returns_full_payload() {
    let mcp = Mcp::start().await;
    let payload = json!({"page": 1, "size": 2, "values": [step_1(), step_2()]});
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pipelines/%7Bpipe-142%7D/steps",
        200,
        payload.clone(),
    );

    let out = mcp
        .call(
            "bitbucket_list_pipeline_steps",
            json!({"repo_slug": "r", "pipeline": "{pipe-142}", "raw": true}),
        )
        .await;
    assert_eq!(out, payload);
}

#[tokio::test]
async fn get_pipeline_step_log_default_tail_returns_short_log_whole() {
    let mcp = Mcp::start().await;
    let text = "Step started\nerror: exit 1\n";
    mcp.fake.on_raw(
        "GET",
        "/2.0/repositories/ws/r/pipelines/142/steps/%7Bstep-1%7D/log",
        200,
        text,
    );

    let out = mcp
        .call(
            "bitbucket_get_pipeline_step_log",
            json!({"repo_slug": "r", "pipeline": "142", "step_uuid": "{step-1}"}),
        )
        .await;
    assert_eq!(
        out,
        json!({
            "step_uuid": "{step-1}",
            "total_chars": text.chars().count(),
            "returned_chars": text.chars().count(),
            "truncated_head": false,
            "log": text
        })
    );
    let req = mcp.fake.only_request();
    assert_eq!(
        req.headers["accept"].to_str().expect("ascii accept"),
        "application/octet-stream, text/plain, */*;q=0.1"
    );
}

#[tokio::test]
async fn get_pipeline_step_log_tail_chars_keeps_the_end() {
    let mcp = Mcp::start().await;
    mcp.fake.on_raw(
        "GET",
        "/2.0/repositories/ws/r/pipelines/142/steps/%7Bstep-1%7D/log",
        200,
        "abcdefghijklmnopqrstuvwxy",
    );

    let out = mcp
        .call(
            "bitbucket_get_pipeline_step_log",
            json!({"repo_slug": "r", "pipeline": "142", "step_uuid": "{step-1}", "tail_chars": 10}),
        )
        .await;
    assert_eq!(out["total_chars"], 25);
    assert_eq!(out["returned_chars"], 10);
    assert_eq!(out["truncated_head"], true);
    assert_eq!(out["log"], "pqrstuvwxy");
}

#[tokio::test]
async fn get_pipeline_step_log_zero_tail_keeps_one_char() {
    let mcp = Mcp::start().await;
    mcp.fake.on_raw(
        "GET",
        "/2.0/repositories/ws/r/pipelines/142/steps/%7Bstep-1%7D/log",
        200,
        "abcdefghijklmnopqrstuvwxy",
    );

    let out = mcp
        .call(
            "bitbucket_get_pipeline_step_log",
            json!({"repo_slug": "r", "pipeline": "142", "step_uuid": "{step-1}", "tail_chars": 0}),
        )
        .await;
    assert_eq!(out["returned_chars"], 1);
    assert_eq!(out["truncated_head"], true);
    assert_eq!(out["log"], "y");
}

#[tokio::test]
async fn get_pipeline_step_log_25000_chars_counts_characters_not_bytes() {
    let mcp = Mcp::start().await;
    let text: String = (0..25_000u32)
        .map(|i| {
            if i % 3 == 0 {
                'é'
            } else {
                char::from(b'a' + (i % 26) as u8)
            }
        })
        .collect();
    mcp.fake.on_raw(
        "GET",
        "/2.0/repositories/ws/r/pipelines/142/steps/%7Bstep-1%7D/log",
        200,
        &text,
    );

    let out = mcp
        .call(
            "bitbucket_get_pipeline_step_log",
            json!({"repo_slug": "r", "pipeline": "142", "step_uuid": "{step-1}"}),
        )
        .await;
    assert_eq!(out["total_chars"], 25_000);
    assert_eq!(out["returned_chars"], 20_000);
    assert_eq!(out["truncated_head"], true);
    let expected: String = text.chars().skip(5_000).collect();
    assert_eq!(out["log"], expected);
}

#[tokio::test]
async fn get_pipeline_step_log_404_and_blank_inputs() {
    let mcp = Mcp::start().await;
    mcp.fake.on_raw(
        "GET",
        "/2.0/repositories/ws/r/pipelines/142/steps/%7Bstep-1%7D/log",
        404,
        "log not available yet",
    );

    let err = mcp
        .call_err(
            "bitbucket_get_pipeline_step_log",
            json!({"repo_slug": "r", "pipeline": "142", "step_uuid": "{step-1}"}),
        )
        .await;
    assert_tool_error(
        &err,
        "Bitbucket returned 404 Not Found: log not available yet",
    );
    assert_eq!(mcp.fake.requests().len(), 1);

    let blank_step = mcp
        .call_err(
            "bitbucket_get_pipeline_step_log",
            json!({"repo_slug": "r", "pipeline": "142", "step_uuid": "  "}),
        )
        .await;
    assert_tool_error(&blank_step, "step_uuid must not be empty");
    let blank_pipeline = mcp
        .call_err(
            "bitbucket_get_pipeline_step_log",
            json!({"repo_slug": "r", "pipeline": " ", "step_uuid": "{step-1}"}),
        )
        .await;
    assert_tool_error(&blank_pipeline, "pipeline must not be empty");
    assert_eq!(mcp.fake.requests().len(), 1);
}

#[tokio::test]
async fn trigger_pipeline_default_branch_body_and_summary() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pipelines",
        201,
        pipeline_143_response(),
    );

    let out = mcp
        .call(
            "bitbucket_trigger_pipeline",
            json!({"repo_slug": "r", "ref_name": "master"}),
        )
        .await;
    assert_eq!(out, summary_143());
    assert_eq!(
        mcp.fake.only_request().json(),
        json!({"target": {"ref_type": "branch", "ref_name": "master", "type": "pipeline_ref_target"}})
    );
}

#[tokio::test]
async fn trigger_pipeline_tag_commit_selector_and_variables_body() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pipelines",
        201,
        pipeline_143_response(),
    );

    mcp.call(
        "bitbucket_trigger_pipeline",
        json!({
            "repo_slug": "r",
            "ref_name": "v1.2.0",
            "ref_type": "tag",
            "commit_hash": "abc123",
            "custom_pipeline": "deploy-prod",
            "variables": [
                {"key": "ENV", "value": "prod"},
                {"key": "TOKEN", "value": "stub-value", "secured": true}
            ]
        }),
    )
    .await;
    assert_eq!(
        mcp.fake.only_request().json(),
        json!({
            "target": {
                "ref_type": "tag",
                "ref_name": "v1.2.0",
                "type": "pipeline_commit_target",
                "commit": {"type": "commit", "hash": "abc123"},
                "selector": {"type": "custom", "pattern": "deploy-prod"}
            },
            "variables": [
                {"key": "ENV", "value": "prod", "secured": false},
                {"key": "TOKEN", "value": "stub-value", "secured": true}
            ]
        })
    );
}

#[tokio::test]
async fn trigger_pipeline_blank_optionals_and_empty_variables_are_omitted() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pipelines",
        201,
        pipeline_143_response(),
    );

    mcp.call(
        "bitbucket_trigger_pipeline",
        json!({
            "repo_slug": "r",
            "ref_name": "master",
            "ref_type": "  ",
            "commit_hash": " ",
            "custom_pipeline": "",
            "variables": []
        }),
    )
    .await;
    assert_eq!(
        mcp.fake.only_request().json(),
        json!({"target": {"ref_type": "branch", "ref_name": "master", "type": "pipeline_ref_target"}})
    );
}

#[tokio::test]
async fn trigger_pipeline_invalid_ref_type_and_blank_ref_are_rejected() {
    let mcp = Mcp::start().await;
    let bad_type = mcp
        .call_err(
            "bitbucket_trigger_pipeline",
            json!({"repo_slug": "r", "ref_name": "master", "ref_type": "commit"}),
        )
        .await;
    assert_tool_error(&bad_type, "ref_type must be `branch` or `tag`");
    let blank_ref = mcp
        .call_err(
            "bitbucket_trigger_pipeline",
            json!({"repo_slug": "r", "ref_name": "  "}),
        )
        .await;
    assert_tool_error(&blank_ref, "ref_name must not be empty");
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn stop_pipeline_204_is_ok_true() {
    let mcp = Mcp::start().await;
    mcp.fake.on_raw(
        "POST",
        "/2.0/repositories/ws/r/pipelines/142/stopPipeline",
        204,
        "",
    );

    let out = mcp
        .call(
            "bitbucket_stop_pipeline",
            json!({"repo_slug": "r", "pipeline": "142"}),
        )
        .await;
    assert_eq!(out, json!({"ok": true}));
    assert_eq!(mcp.fake.only_request().method, "POST");
}

#[tokio::test]
async fn stop_pipeline_json_200_is_passed_through_and_blank_is_rejected() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pipelines/143/stopPipeline",
        200,
        json!({"accepted": true}),
    );

    let out = mcp
        .call(
            "bitbucket_stop_pipeline",
            json!({"repo_slug": "r", "pipeline": "143"}),
        )
        .await;
    assert_eq!(out, json!({"accepted": true}));

    let err = mcp
        .call_err(
            "bitbucket_stop_pipeline",
            json!({"repo_slug": "r", "pipeline": ""}),
        )
        .await;
    assert_tool_error(&err, "pipeline must not be empty");
    assert_eq!(mcp.fake.requests().len(), 1);
}

#[tokio::test]
async fn list_deployments_summarises_build_numbers_and_sends_query() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/deployments",
        200,
        deployments_page(),
    );

    let out = mcp
        .call(
            "bitbucket_list_deployments",
            json!({"repo_slug": "r", "pagelen": 5, "page": 1, "environment": "{env-1}"}),
        )
        .await;
    assert_eq!(out["page"], 1);
    assert_eq!(out["pagelen"], 5);
    assert_eq!(out["size"], 3);
    assert_eq!(out["deployments"][0], summary_deployment_1());
    // "hotfix-7" has no build number; "1163" parses without the leading '#'.
    assert!(out["deployments"][1]["build_number"].is_null());
    assert_eq!(out["deployments"][2]["build_number"], 1163);
    assert_eq!(
        mcp.fake.only_request().query_pairs(),
        pairs(&[
            ("sort", "-state.started_on"),
            ("environment", "{env-1}"),
            ("pagelen", "5"),
            ("page", "1"),
        ])
    );
}

#[tokio::test]
async fn list_deployments_blank_sort_uses_default_and_clamps_pagelen() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/deployments",
        200,
        json!({"values": []}),
    );

    mcp.call(
        "bitbucket_list_deployments",
        json!({"repo_slug": "r", "sort": "  ", "pagelen": 1000}),
    )
    .await;
    mcp.call(
        "bitbucket_list_deployments",
        json!({"repo_slug": "r", "sort": "state.started_on"}),
    )
    .await;

    let seen = mcp.fake.requests();
    assert_eq!(
        seen[0].query_pairs(),
        pairs(&[("sort", "-state.started_on"), ("pagelen", "100")])
    );
    assert_eq!(
        seen[1].query_pairs(),
        pairs(&[("sort", "state.started_on")])
    );
}

#[tokio::test]
async fn list_deployments_raw_returns_full_payload() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/deployments",
        200,
        deployments_page(),
    );

    let out = mcp
        .call(
            "bitbucket_list_deployments",
            json!({"repo_slug": "r", "raw": true}),
        )
        .await;
    assert_eq!(out, deployments_page());
}

#[tokio::test]
async fn list_deployment_environments_summarises_and_sends_pagelen_100() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/environments",
        200,
        environments_page(),
    );

    let out = mcp
        .call(
            "bitbucket_list_deployment_environments",
            json!({"repo_slug": "r"}),
        )
        .await;
    assert_eq!(
        out,
        json!({
            "environments": [
                {"uuid": "{env-1}", "name": "Production", "slug": "production", "type": "Production", "rank": 3},
                {"uuid": "{env-2}", "name": "Staging", "slug": "staging", "type": "Staging", "rank": 2}
            ]
        })
    );
    assert_eq!(
        mcp.fake.only_request().query_pairs(),
        pairs(&[("pagelen", "100")])
    );
}

#[tokio::test]
async fn list_deployment_environments_raw_returns_full_payload() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/environments",
        200,
        environments_page(),
    );

    let out = mcp
        .call(
            "bitbucket_list_deployment_environments",
            json!({"repo_slug": "r", "raw": true}),
        )
        .await;
    assert_eq!(out, environments_page());
}

// --- Pipeline variables ---

#[tokio::test]
async fn list_pipeline_variables_repository_scope_summarises_and_hides_secured_value() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pipelines_config/variables",
        200,
        repo_variables(),
    );

    let out = mcp
        .call(
            "bitbucket_list_pipeline_variables",
            json!({"repo_slug": "r", "pagelen": 10, "page": 2}),
        )
        .await;
    assert_eq!(
        out,
        json!({
            "scope": "repository",
            "environment": null,
            "page": 2,
            "pagelen": 10,
            "size": 2,
            "variables": [
                {"uuid": "{var-1}", "key": "LOG", "value": "info", "secured": false, "scope": null},
                {"uuid": "{var-2}", "key": "TOKEN", "value": null, "secured": true, "scope": null}
            ]
        })
    );
    assert_eq!(
        mcp.fake.only_request().query_pairs(),
        pairs(&[("pagelen", "10"), ("page", "2")])
    );
}

#[tokio::test]
async fn list_pipeline_variables_environment_name_resolves_to_deployment_path() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/environments",
        200,
        environments_page(),
    );
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/deployments_config/environments/%7Benv-1%7D/variables",
        200,
        json!({"values": [{"uuid": "{var-3}", "key": "LOG", "value": "debug", "secured": false}]}),
    );

    let out = mcp
        .call(
            "bitbucket_list_pipeline_variables",
            json!({"repo_slug": "r", "environment": "Production"}),
        )
        .await;
    assert_eq!(
        out,
        json!({
            "scope": "deployment",
            "environment": "Production",
            "page": null,
            "pagelen": null,
            "size": null,
            "variables": [{"uuid": "{var-3}", "key": "LOG", "value": "debug", "secured": false, "scope": null}]
        })
    );
    assert_eq!(
        trail(&mcp),
        vec![
            "GET /2.0/repositories/ws/r/environments",
            "GET /2.0/repositories/ws/r/deployments_config/environments/%7Benv-1%7D/variables",
        ]
    );
}

#[tokio::test]
async fn list_pipeline_variables_environment_uuid_skips_lookup() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/deployments_config/environments/%7Benv-1%7D/variables",
        200,
        json!({"values": []}),
    );

    let out = mcp
        .call(
            "bitbucket_list_pipeline_variables",
            json!({"repo_slug": "r", "environment": "{env-1}"}),
        )
        .await;
    assert_eq!(out["scope"], "deployment");
    assert_eq!(out["environment"], "{env-1}");
    assert_eq!(trail(&mcp).len(), 1);
}

#[tokio::test]
async fn environment_slug_matches_case_insensitively() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/environments",
        200,
        environments_page(),
    );
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/deployments_config/environments/%7Benv-1%7D/variables",
        200,
        json!({"values": []}),
    );

    mcp.call(
        "bitbucket_list_pipeline_variables",
        json!({"repo_slug": "r", "environment": "PRODUCTION"}),
    )
    .await;
    assert_eq!(
        mcp.fake.requests()[1].path,
        "/2.0/repositories/ws/r/deployments_config/environments/%7Benv-1%7D/variables"
    );
}

#[tokio::test]
async fn environment_name_matching_several_environments_is_rejected() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/environments",
        200,
        json!({"values": [
            {"uuid": "{env-1}", "name": "Production", "slug": "production"},
            {"uuid": "{env-3}", "name": "production", "slug": "prod-eu"}
        ]}),
    );

    let err = mcp
        .call_err(
            "bitbucket_list_pipeline_variables",
            json!({"repo_slug": "r", "environment": "PRODUCTION"}),
        )
        .await;
    assert_tool_error(
        &err,
        "several deployment environments match `PRODUCTION`; pass the UUID instead",
    );
    assert_eq!(trail(&mcp), vec!["GET /2.0/repositories/ws/r/environments"]);
}

#[tokio::test]
async fn environment_name_with_no_match_is_rejected() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/environments",
        200,
        environments_page(),
    );

    let err = mcp
        .call_err(
            "bitbucket_list_pipeline_variables",
            json!({"repo_slug": "r", "environment": "QA"}),
        )
        .await;
    assert_tool_error(
        &err,
        "no deployment environment named `QA`; use bitbucket_list_deployment_environments",
    );
    assert_eq!(trail(&mcp), vec!["GET /2.0/repositories/ws/r/environments"]);
}

#[tokio::test]
async fn create_pipeline_variable_repository_scope_body_and_summary() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pipelines_config/variables",
        201,
        json!({"uuid": "{var-9}", "key": "LOG", "value": "info", "secured": false}),
    );

    let out = mcp
        .call(
            "bitbucket_create_pipeline_variable",
            json!({"repo_slug": "r", "key": "LOG", "value": "info"}),
        )
        .await;
    assert_eq!(
        out,
        json!({"uuid": "{var-9}", "key": "LOG", "value": "info", "secured": false, "scope": null})
    );
    let req = mcp.fake.only_request();
    assert_eq!(
        req.path,
        "/2.0/repositories/ws/r/pipelines_config/variables"
    );
    assert_eq!(
        req.json(),
        json!({"key": "LOG", "value": "info", "secured": false})
    );
}

#[tokio::test]
async fn create_pipeline_variable_secured_sends_value_but_output_hides_it() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/pipelines_config/variables",
        201,
        json!({"uuid": "{var-10}", "key": "TOKEN", "secured": true}),
    );

    let out = mcp
        .call(
            "bitbucket_create_pipeline_variable",
            json!({"repo_slug": "r", "key": "TOKEN", "value": "stub-secret-42", "secured": true}),
        )
        .await;
    assert_eq!(
        out,
        json!({"uuid": "{var-10}", "key": "TOKEN", "value": null, "secured": true, "scope": null})
    );
    assert!(!out.to_string().contains("stub-secret-42"));
    assert_eq!(
        mcp.fake.only_request().json(),
        json!({"key": "TOKEN", "value": "stub-secret-42", "secured": true})
    );
}

#[tokio::test]
async fn create_pipeline_variable_environment_name_resolves_to_deployment_path() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/environments",
        200,
        environments_page(),
    );
    mcp.fake.on(
        "POST",
        "/2.0/repositories/ws/r/deployments_config/environments/%7Benv-1%7D/variables",
        201,
        json!({"uuid": "{var-11}", "key": "LOG", "value": "info", "secured": false}),
    );

    mcp.call(
        "bitbucket_create_pipeline_variable",
        json!({"repo_slug": "r", "environment": "Production", "key": "LOG", "value": "info"}),
    )
    .await;
    assert_eq!(
        trail(&mcp),
        vec![
            "GET /2.0/repositories/ws/r/environments",
            "POST /2.0/repositories/ws/r/deployments_config/environments/%7Benv-1%7D/variables",
        ]
    );
    assert_eq!(
        mcp.fake.requests()[1].json(),
        json!({"key": "LOG", "value": "info", "secured": false})
    );
}

#[tokio::test]
async fn create_pipeline_variable_blank_key_is_rejected_before_environment_lookup() {
    let mcp = Mcp::start().await;
    let err = mcp
        .call_err(
            "bitbucket_create_pipeline_variable",
            json!({"repo_slug": "r", "environment": "Production", "key": "  ", "value": "x"}),
        )
        .await;
    assert_tool_error(&err, "key must not be empty");
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn update_pipeline_variable_by_key_sends_read_modify_write_body() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pipelines_config/variables",
        200,
        repo_variables(),
    );
    mcp.fake.on(
        "PUT",
        "/2.0/repositories/ws/r/pipelines_config/variables/%7Bvar-1%7D",
        200,
        json!({"uuid": "{var-1}", "key": "LOG", "value": "debug", "secured": false}),
    );

    let out = mcp
        .call(
            "bitbucket_update_pipeline_variable",
            json!({"repo_slug": "r", "variable": "LOG", "value": "debug"}),
        )
        .await;
    assert_eq!(
        out,
        json!({"uuid": "{var-1}", "key": "LOG", "value": "debug", "secured": false, "scope": null})
    );
    assert_eq!(
        trail(&mcp),
        vec![
            "GET /2.0/repositories/ws/r/pipelines_config/variables",
            "PUT /2.0/repositories/ws/r/pipelines_config/variables/%7Bvar-1%7D",
        ]
    );
    assert_eq!(
        mcp.fake.requests()[1].json(),
        json!({"key": "LOG", "secured": false, "value": "debug"})
    );
}

#[tokio::test]
async fn update_pipeline_variable_by_uuid_carries_current_value() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pipelines_config/variables",
        200,
        repo_variables(),
    );
    mcp.fake.on(
        "PUT",
        "/2.0/repositories/ws/r/pipelines_config/variables/%7Bvar-1%7D",
        200,
        json!({"uuid": "{var-1}", "key": "LOG", "value": "info", "secured": false}),
    );

    mcp.call(
        "bitbucket_update_pipeline_variable",
        json!({"repo_slug": "r", "variable": "{var-1}", "key": "LOG"}),
    )
    .await;
    assert_eq!(
        mcp.fake.requests()[1].json(),
        json!({"key": "LOG", "secured": false, "value": "info"})
    );
}

#[tokio::test]
async fn update_pipeline_variable_rename_and_secure_sends_all_fields() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pipelines_config/variables",
        200,
        repo_variables(),
    );
    mcp.fake.on(
        "PUT",
        "/2.0/repositories/ws/r/pipelines_config/variables/%7Bvar-1%7D",
        200,
        json!({"uuid": "{var-1}", "key": "LOG_V2", "secured": true}),
    );

    mcp.call(
        "bitbucket_update_pipeline_variable",
        json!({"repo_slug": "r", "variable": "LOG", "key": "LOG_V2", "secured": true, "value": "stub-2"}),
    )
    .await;
    assert_eq!(
        mcp.fake.requests()[1].json(),
        json!({"key": "LOG_V2", "secured": true, "value": "stub-2"})
    );
}

#[tokio::test]
async fn update_pipeline_variable_secured_without_value_is_refused() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pipelines_config/variables",
        200,
        repo_variables(),
    );

    let err = mcp
        .call_err(
            "bitbucket_update_pipeline_variable",
            json!({"repo_slug": "r", "variable": "TOKEN", "key": "TOKEN_2"}),
        )
        .await;
    assert_tool_error(
        &err,
        "this variable is secured, so Bitbucket cannot return its value; pass `value` to update it",
    );
    assert_eq!(
        trail(&mcp),
        vec!["GET /2.0/repositories/ws/r/pipelines_config/variables"]
    );
}

#[tokio::test]
async fn update_pipeline_variable_unsecuring_without_value_is_refused() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pipelines_config/variables",
        200,
        repo_variables(),
    );

    let err = mcp
        .call_err(
            "bitbucket_update_pipeline_variable",
            json!({"repo_slug": "r", "variable": "TOKEN", "secured": false}),
        )
        .await;
    assert_tool_error(
        &err,
        "this variable is secured, so Bitbucket cannot return its value; pass `value` to update it",
    );
    assert_eq!(mcp.fake.requests().len(), 1);
}

#[tokio::test]
async fn update_pipeline_variable_empty_value_is_sent_not_carried() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pipelines_config/variables",
        200,
        repo_variables(),
    );
    mcp.fake.on(
        "PUT",
        "/2.0/repositories/ws/r/pipelines_config/variables/%7Bvar-1%7D",
        200,
        json!({"uuid": "{var-1}", "key": "LOG", "value": "", "secured": false}),
    );

    mcp.call(
        "bitbucket_update_pipeline_variable",
        json!({"repo_slug": "r", "variable": "LOG", "value": ""}),
    )
    .await;
    assert_eq!(
        mcp.fake.requests()[1].json(),
        json!({"key": "LOG", "secured": false, "value": ""})
    );
}

#[tokio::test]
async fn update_pipeline_variable_needs_a_field_and_a_name_before_any_request() {
    let mcp = Mcp::start().await;
    let nothing = mcp
        .call_err(
            "bitbucket_update_pipeline_variable",
            json!({"repo_slug": "r", "variable": "LOG"}),
        )
        .await;
    assert_tool_error(
        &nothing,
        "at least one of key, value or secured must be provided",
    );

    let blank = mcp
        .call_err(
            "bitbucket_update_pipeline_variable",
            json!({"repo_slug": "r", "variable": "  ", "value": "x"}),
        )
        .await;
    assert_tool_error(&blank, "variable must not be empty");
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn update_pipeline_variable_finds_key_on_second_page() {
    let paged = PagedFake::start().await;
    let mcp = Mcp::start().await;
    let root = "/2.0/repositories/ws/r/pipelines_config/variables";
    paged.on(
        "GET",
        root,
        "1",
        200,
        &json!({
            "page": 1,
            "pagelen": 100,
            "size": 2,
            "next": format!("{}/2.0/repositories/ws/r/pipelines_config/variables?page=2", paged.base_url),
            "values": [{"uuid": "{var-1}", "key": "LOG", "value": "info", "secured": false}]
        })
        .to_string(),
    );
    paged.on(
        "GET",
        root,
        "2",
        200,
        &json!({
            "page": 2,
            "pagelen": 100,
            "size": 2,
            "values": [{"uuid": "{var-2}", "key": "TOKEN", "secured": true}]
        })
        .to_string(),
    );
    paged.on(
        "PUT",
        "/2.0/repositories/ws/r/pipelines_config/variables/%7Bvar-2%7D",
        "",
        200,
        &json!({"uuid": "{var-2}", "key": "TOKEN", "secured": true}).to_string(),
    );

    let out = call_at(
        &mcp,
        &format!("{}/2.0", paged.base_url),
        "bitbucket_update_pipeline_variable",
        json!({"repo_slug": "r", "variable": "TOKEN", "value": "stub-3"}),
    )
    .await;
    assert_eq!(out["uuid"], "{var-2}");
    let seen = paged.seen();
    assert_eq!(seen.len(), 3);
    assert_eq!(
        seen[0].query_pairs(),
        pairs(&[("pagelen", "100"), ("page", "1")])
    );
    assert_eq!(
        seen[1].query_pairs(),
        pairs(&[("pagelen", "100"), ("page", "2")])
    );
    assert_eq!(seen[2].method, "PUT");
    assert_eq!(
        seen[2].json(),
        json!({"key": "TOKEN", "secured": true, "value": "stub-3"})
    );
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn delete_pipeline_variable_by_key_returns_deleted_summary() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pipelines_config/variables",
        200,
        repo_variables(),
    );
    mcp.fake.on_raw(
        "DELETE",
        "/2.0/repositories/ws/r/pipelines_config/variables/%7Bvar-1%7D",
        204,
        "",
    );

    let out = mcp
        .call(
            "bitbucket_delete_pipeline_variable",
            json!({"repo_slug": "r", "variable": "LOG"}),
        )
        .await;
    assert_eq!(
        out,
        json!({"ok": true, "deleted": true, "uuid": "{var-1}", "key": "LOG"})
    );
    assert_eq!(
        trail(&mcp),
        vec![
            "GET /2.0/repositories/ws/r/pipelines_config/variables",
            "DELETE /2.0/repositories/ws/r/pipelines_config/variables/%7Bvar-1%7D",
        ]
    );
}

#[tokio::test]
async fn delete_secured_pipeline_variable_by_uuid_in_environment() {
    let mcp = Mcp::start().await;
    let root = "/2.0/repositories/ws/r/deployments_config/environments/%7Benv-1%7D/variables";
    mcp.fake.on(
        "GET",
        root,
        200,
        json!({"values": [{"uuid": "{var-2}", "key": "TOKEN", "secured": true}]}),
    );
    mcp.fake
        .on_raw("DELETE", &format!("{root}/%7Bvar-2%7D"), 204, "");

    let out = mcp
        .call(
            "bitbucket_delete_pipeline_variable",
            json!({"repo_slug": "r", "environment": "{env-1}", "variable": "{var-2}"}),
        )
        .await;
    assert_eq!(
        out,
        json!({"ok": true, "deleted": true, "uuid": "{var-2}", "key": "TOKEN"})
    );
    assert_eq!(
        trail(&mcp),
        vec![format!("GET {root}"), format!("DELETE {root}/%7Bvar-2%7D"),]
    );
}

#[tokio::test]
async fn delete_pipeline_variable_blank_name_is_rejected_before_any_request() {
    let mcp = Mcp::start().await;
    let err = mcp
        .call_err(
            "bitbucket_delete_pipeline_variable",
            json!({"repo_slug": "r", "variable": ""}),
        )
        .await;
    assert_tool_error(&err, "variable must not be empty");
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn delete_pipeline_variable_finds_uuid_on_second_page() {
    let paged = PagedFake::start().await;
    let mcp = Mcp::start().await;
    let root = "/2.0/repositories/ws/r/pipelines_config/variables";
    paged.on(
        "GET",
        root,
        "1",
        200,
        &json!({
            "page": 1,
            "pagelen": 100,
            "size": 2,
            "next": format!("{}/2.0/repositories/ws/r/pipelines_config/variables?page=2", paged.base_url),
            "values": [{"uuid": "{var-1}", "key": "LOG", "value": "info", "secured": false}]
        })
        .to_string(),
    );
    paged.on(
        "GET",
        root,
        "2",
        200,
        &json!({
            "page": 2,
            "pagelen": 100,
            "size": 2,
            "values": [{"uuid": "{var-2}", "key": "TOKEN", "secured": true}]
        })
        .to_string(),
    );
    paged.on(
        "DELETE",
        "/2.0/repositories/ws/r/pipelines_config/variables/%7Bvar-2%7D",
        "",
        204,
        "",
    );

    let out = call_at(
        &mcp,
        &format!("{}/2.0", paged.base_url),
        "bitbucket_delete_pipeline_variable",
        json!({"repo_slug": "r", "variable": "{var-2}"}),
    )
    .await;
    assert_eq!(
        out,
        json!({"ok": true, "deleted": true, "uuid": "{var-2}", "key": "TOKEN"})
    );
    let seen = paged.seen();
    assert_eq!(seen.len(), 3);
    assert_eq!(
        seen[0].query_pairs(),
        pairs(&[("pagelen", "100"), ("page", "1")])
    );
    assert_eq!(
        seen[1].query_pairs(),
        pairs(&[("pagelen", "100"), ("page", "2")])
    );
    assert_eq!(seen[2].method, "DELETE");
    assert_eq!(seen[2].path, format!("{root}/%7Bvar-2%7D"));
}

#[tokio::test]
async fn delete_pipeline_variable_403_reports_status_and_body() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r/pipelines_config/variables",
        200,
        repo_variables(),
    );
    mcp.fake.on_raw(
        "DELETE",
        "/2.0/repositories/ws/r/pipelines_config/variables/%7Bvar-1%7D",
        403,
        "required privilege scopes",
    );

    let err = mcp
        .call_err(
            "bitbucket_delete_pipeline_variable",
            json!({"repo_slug": "r", "variable": "LOG"}),
        )
        .await;
    assert_tool_error(
        &err,
        "Bitbucket returned 403 Forbidden: required privilege scopes",
    );
}

// --- Coverage index ---

#[tokio::test]
async fn bitbucket_tool_list_matches_the_tools_covered_here() {
    let mcp = Mcp::start().await;
    let resp = mcp.rpc("tools/list", json!({})).await;
    let mut names: Vec<String> = resp["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .filter(|n| n.starts_with("bitbucket_"))
        .map(str::to_string)
        .collect();
    names.sort();

    // Each name below has at least one test in this file; a new Bitbucket tool must add one.
    let mut covered = vec![
        "bitbucket_approve_pull_request",
        "bitbucket_create_pipeline_variable",
        "bitbucket_create_pull_request",
        "bitbucket_create_pull_request_comment",
        "bitbucket_decline_pull_request",
        "bitbucket_delete_pipeline_variable",
        "bitbucket_delete_pull_request_comment",
        "bitbucket_file_history",
        "bitbucket_get_commit",
        "bitbucket_get_file",
        "bitbucket_get_pipeline",
        "bitbucket_get_pipeline_step_log",
        "bitbucket_get_pull_request",
        "bitbucket_get_pull_request_comment",
        "bitbucket_get_pull_request_diff",
        "bitbucket_get_pull_request_diffstat",
        "bitbucket_get_repository",
        "bitbucket_list_branches",
        "bitbucket_list_commits",
        "bitbucket_list_deployment_environments",
        "bitbucket_list_deployments",
        "bitbucket_list_pipelines",
        "bitbucket_list_pipeline_steps",
        "bitbucket_list_pipeline_variables",
        "bitbucket_list_pull_request_activity",
        "bitbucket_list_pull_request_comments",
        "bitbucket_list_pull_requests",
        "bitbucket_list_repositories",
        "bitbucket_list_tags",
        "bitbucket_merge_pull_request",
        "bitbucket_reply_pull_request_comment",
        "bitbucket_search_users",
        "bitbucket_stop_pipeline",
        "bitbucket_trigger_pipeline",
        "bitbucket_unapprove_pull_request",
        "bitbucket_update_pipeline_variable",
        "bitbucket_update_pull_request",
        "bitbucket_update_pull_request_comment",
    ];
    covered.sort();
    assert_eq!(covered.len(), 38);
    assert_eq!(names, covered);
}
