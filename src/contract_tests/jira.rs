use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::response::Response;
use base64::Engine;
use serde_json::{Value, json};
use tokio::net::TcpListener;

use super::harness::{Mcp, Recorded};

const ISSUE: &str = "/rest/api/3/issue/SWS-1";
const ISSUE_COMMENTS: &str = "/rest/api/3/issue/SWS-1/comment";
const SEARCH_JQL: &str = "/rest/api/3/search/jql";

fn q(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn requests_to(mcp: &Mcp, path: &str) -> Vec<Recorded> {
    mcp.fake
        .requests()
        .into_iter()
        .filter(|r| r.path == path)
        .collect()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn author_raw() -> Value {
    json!({
        "self": "https://example.invalid/rest/api/3/user?accountId=acc-1",
        "accountId": "acc-1",
        "displayName": "Test User",
        "emailAddress": "test.user@example.invalid",
        "active": true,
        "avatarUrls": {"48x48": "https://example.invalid/avatar.png"}
    })
}

/// ADF with a mention; Jira sends the mention text with its leading `@`.
fn description_doc() -> Value {
    json!({"type": "doc", "version": 1, "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "Login fails."}]},
        {"type": "paragraph", "content": [
            {"type": "text", "text": "Retry with "},
            {"type": "mention", "attrs": {"id": "acc-1", "text": "@Test User", "accessLevel": ""}},
            {"type": "text", "text": " later."}
        ]}
    ]})
}

fn comment_doc() -> Value {
    json!({"type": "doc", "version": 1, "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "Looks good"}]}
    ]})
}

/// Raw Jira search issue, with the noise fields Jira really sends.
fn raw_issue(key: &str, summary: &str) -> Value {
    json!({
        "expand": "operations,versionedRepresentations",
        "id": "10000",
        "key": key,
        "self": "https://example.invalid/rest/api/3/issue/10000",
        "fields": {
            "summary": summary,
            "created": "2026-10-01T09:00:00.000+0000",
            "updated": "2026-11-01T10:00:00.000+0000",
            "priority": {"name": "Medium", "id": "3"},
            "labels": ["backend"],
            "project": {"key": "SWS"},
            "status": {
                "self": "https://example.invalid/rest/api/3/status/10000",
                "name": "To Do",
                "id": "10000",
                "statusCategory": {"id": 2, "key": "new", "name": "To Do", "colorName": "blue-gray"}
            },
            "issuetype": {"name": "Task", "id": "10001", "subtask": false, "iconUrl": "https://example.invalid/t.png"},
            "assignee": author_raw()
        }
    })
}

/// The slim form `raw_issue` must become.
fn slim_issue(key: &str, summary: &str) -> Value {
    json!({
        "key": key,
        "summary": summary,
        "status": {"name": "To Do", "id": "10000", "statusCategory": {"key": "new", "name": "To Do"}},
        "issuetype": {"name": "Task"},
        "assignee": {"displayName": "Test User", "emailAddress": "test.user@example.invalid"},
        "updated": "2026-11-01T10:00:00.000+0000"
    })
}

// ---- scripted Jira: replies in order, plus response headers the fake cannot set ----

#[derive(Clone)]
struct Reply {
    status: u16,
    headers: Vec<(&'static str, &'static str)>,
    body: String,
}

fn reply(status: u16, body: Value) -> Reply {
    Reply {
        status,
        headers: Vec::new(),
        body: body.to_string(),
    }
}

#[derive(Default)]
struct Script {
    replies: VecDeque<Reply>,
    seen: Vec<Recorded>,
}

type SharedScript = Arc<Mutex<Script>>;

async fn scripted_handle(State(script): State<SharedScript>, req: Request<Body>) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = to_bytes(body, 1024 * 1024)
        .await
        .expect("read scripted body");
    let next = {
        let mut script = script.lock().expect("script lock");
        script.seen.push(Recorded {
            method: parts.method.as_str().to_string(),
            path: parts.uri.path().to_string(),
            query: parts.uri.query().map(str::to_string),
            headers: parts.headers,
            body: bytes.to_vec(),
        });
        script.replies.pop_front()
    };
    let r = next.unwrap_or(Reply {
        status: 599,
        headers: Vec::new(),
        body: "script exhausted".to_string(),
    });
    let mut builder = http::Response::builder()
        .status(r.status)
        .header(http::header::CONTENT_TYPE, "application/json");
    for (name, value) in r.headers {
        builder = builder.header(name, value);
    }
    builder.body(Body::from(r.body)).expect("scripted response")
}

/// Serves `replies` in order on 127.0.0.1 and returns the base URL and the recorded requests.
async fn scripted_jira(replies: Vec<Reply>) -> (String, SharedScript) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind scripted Jira");
    let addr = listener.local_addr().expect("scripted Jira address");
    let script: SharedScript = Arc::new(Mutex::new(Script {
        replies: VecDeque::from(replies),
        seen: Vec::new(),
    }));
    let app = axum::Router::new()
        .fallback(scripted_handle)
        .with_state(script.clone());
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve scripted Jira");
    });
    (format!("http://{addr}"), script)
}

fn seen_requests(script: &SharedScript) -> Vec<Recorded> {
    script.lock().expect("script lock").seen.clone()
}

/// Calls a tool through the real app with `X-Atlassian-Site-Url` pointing at `site`.
async fn call_on_site(mcp: &Mcp, site: &str, tool: &str, args: Value) -> Value {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": tool, "arguments": args}
    });
    let text = reqwest::Client::new()
        .post(format!("{}/mcp", mcp.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("X-Atlassian-Site-Url", site)
        .header("X-Atlassian-Email", "tester@example.com")
        .header("X-Atlassian-Api-Token", "test-token")
        .json(&body)
        .send()
        .await
        .expect("send MCP request")
        .text()
        .await
        .expect("read MCP response");
    serde_json::from_str(&text).expect("MCP response is JSON")
}

/// Parses the JSON text of a successful tool result; panics on an error.
fn result_json(resp: &Value) -> Value {
    assert!(resp.get("error").is_none(), "tool failed: {resp:#}");
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content: {resp:#}"));
    serde_json::from_str(text).expect("tool output is JSON")
}

// ---- jira_get_issue ----

#[tokio::test]
async fn get_issue_slims_realistic_issue_and_drops_noise() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        ISSUE,
        200,
        json!({
            "id": "10000",
            "key": "SWS-1",
            "self": "https://example.invalid/rest/api/3/issue/10000",
            "fields": {
                "summary": "Fix login",
                "description": description_doc(),
                "status": {
                    "self": "https://example.invalid/rest/api/3/status/3",
                    "name": "In Progress",
                    "id": "3",
                    "statusCategory": {"id": 4, "key": "indeterminate", "name": "In Progress", "colorName": "yellow"}
                },
                "attachment": [{
                    "id": "10001",
                    "filename": "screen.png",
                    "mimeType": "image/png",
                    "size": 2048,
                    "content": "https://example.invalid/attachment/10001",
                    "thumbnail": "https://example.invalid/thumb/10001",
                    "created": "2026-11-02T08:00:00.000+0000",
                    "author": author_raw()
                }],
                "priority": {"name": "High"},
                "labels": ["auth"],
                "reporter": author_raw(),
                "updated": "2026-11-03T09:00:00.000+0000"
            }
        }),
    );
    mcp.fake.on(
        "GET",
        ISSUE_COMMENTS,
        200,
        json!({
            "startAt": 0,
            "maxResults": 100,
            "total": 1,
            "comments": [{
                "id": "10100",
                "self": "https://example.invalid/rest/api/3/comment/10100",
                "author": author_raw(),
                "body": comment_doc(),
                "updateAuthor": author_raw(),
                "created": "2026-11-02T10:00:00.000+0000",
                "updated": "2026-11-02T10:05:00.000+0000",
                "visibility": {"type": "role", "value": "Developers"}
            }]
        }),
    );

    let out = mcp
        .call("jira_get_issue", json!({"issue_key": "SWS-1"}))
        .await;
    // Pins current behaviour: the mention text already starts with `@`, so the walker doubles it.
    assert_eq!(
        out,
        json!({
            "key": "SWS-1",
            "summary": "Fix login",
            "description": "Login fails.\nRetry with @@Test User later.",
            "status": {"name": "In Progress", "id": "3", "statusCategory": {"key": "indeterminate", "name": "In Progress"}},
            "attachments": [{
                "id": "10001",
                "filename": "screen.png",
                "mimeType": "image/png",
                "size": 2048,
                "content": "https://example.invalid/attachment/10001",
                "created": "2026-11-02T08:00:00.000+0000",
                "author": {"displayName": "Test User", "emailAddress": "test.user@example.invalid"}
            }],
            "comments": {
                "total": 1,
                "comments": [{
                    "id": "10100",
                    "author": {"displayName": "Test User", "emailAddress": "test.user@example.invalid"},
                    "created": "2026-11-02T10:00:00.000+0000",
                    "updated": "2026-11-02T10:05:00.000+0000",
                    "body": "Looks good"
                }]
            }
        })
    );
    let issue = requests_to(&mcp, ISSUE);
    assert_eq!(issue.len(), 1);
    assert_eq!(
        issue[0].query_pairs(),
        q(&[("fields", "summary,description,attachment,status")])
    );
}

#[tokio::test]
async fn get_issue_include_adf_adds_raw_documents() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        ISSUE,
        200,
        json!({"key": "SWS-1", "fields": {"summary": "s", "description": description_doc(), "status": null}}),
    );
    mcp.fake.on(
        "GET",
        ISSUE_COMMENTS,
        200,
        json!({"total": 1, "comments": [{"id": "10100", "body": comment_doc()}]}),
    );

    let out = mcp
        .call(
            "jira_get_issue",
            json!({"issue_key": "SWS-1", "include_adf": true}),
        )
        .await;
    assert_eq!(out["description_adf"], description_doc());
    assert_eq!(out["comments"]["comments"][0]["body"], "Looks good");
    assert_eq!(out["comments"]["comments"][0]["body_adf"], comment_doc());
    assert!(out["status"].is_null());
    let comments = requests_to(&mcp, ISSUE_COMMENTS);
    assert_eq!(
        comments[0].query_pairs(),
        q(&[("startAt", "0"), ("maxResults", "100")])
    );
}

#[tokio::test]
async fn get_issue_default_output_has_no_adf_keys() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        ISSUE,
        200,
        json!({"key": "SWS-1", "fields": {"summary": "s", "description": description_doc()}}),
    );
    mcp.fake.on(
        "GET",
        ISSUE_COMMENTS,
        200,
        json!({"total": 0, "comments": []}),
    );

    let out = mcp
        .call("jira_get_issue", json!({"issue_key": "SWS-1"}))
        .await;
    assert!(out.get("description_adf").is_none());
    assert_eq!(out["comments"], json!({"total": 0, "comments": []}));
}

#[tokio::test]
async fn get_issue_stops_paging_once_comment_total_is_reached() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        ISSUE,
        200,
        json!({"key": "SWS-1", "fields": {"summary": "s"}}),
    );
    mcp.fake.on(
        "GET",
        ISSUE_COMMENTS,
        200,
        json!({"total": 2, "comments": [
            {"id": "1", "body": comment_doc()},
            {"id": "2", "body": comment_doc()}
        ]}),
    );

    let out = mcp
        .call("jira_get_issue", json!({"issue_key": "SWS-1"}))
        .await;
    assert_eq!(out["comments"]["total"], 2);
    assert_eq!(requests_to(&mcp, ISSUE_COMMENTS).len(), 1);
}

#[tokio::test]
async fn get_issue_blank_key_is_rejected_before_any_request() {
    let mcp = Mcp::start().await;
    let err = mcp
        .call_err("jira_get_issue", json!({"issue_key": "   "}))
        .await;
    // Pins current behaviour: input validation returns internal error (-32603), not invalid params.
    assert_eq!(err["code"], -32603);
    assert_eq!(err["message"], "issue_key must not be empty");
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn get_issue_without_fields_object_is_an_error() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        ISSUE,
        200,
        json!({"key": "SWS-1", "errorMessages": []}),
    );
    let err = mcp
        .call_err("jira_get_issue", json!({"issue_key": "SWS-1"}))
        .await;
    assert_eq!(err["code"], -32603);
    assert_eq!(err["message"], "Jira issue response missing fields");
    assert!(requests_to(&mcp, ISSUE_COMMENTS).is_empty());
}

// ---- jira_add_comment ----

#[tokio::test]
async fn add_comment_plain_text_becomes_adf_paragraphs() {
    let mcp = Mcp::start().await;
    let created =
        json!({"id": "10200", "body": comment_doc(), "self": "https://example.invalid/c/10200"});
    mcp.fake.on("POST", ISSUE_COMMENTS, 201, created.clone());

    let out = mcp
        .call(
            "jira_add_comment",
            json!({"issue_key": "SWS-1", "comment": "First line\n\nLast line"}),
        )
        .await;
    assert_eq!(out, created);

    let req = mcp.fake.only_request();
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, ISSUE_COMMENTS);
    assert!(req.query.is_none());
    assert_eq!(
        req.json(),
        json!({"body": {"type": "doc", "version": 1, "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "First line"}]},
            {"type": "paragraph", "content": []},
            {"type": "paragraph", "content": [{"type": "text", "text": "Last line"}]}
        ]}})
    );
}

#[tokio::test]
async fn add_comment_adf_format_posts_document_unchanged() {
    let mcp = Mcp::start().await;
    mcp.fake
        .on("POST", ISSUE_COMMENTS, 201, json!({"id": "10200"}));
    let adf = comment_doc().to_string();

    mcp.call(
        "jira_add_comment",
        json!({"issue_key": "SWS-1", "comment": adf, "content_format": "ADF"}),
    )
    .await;
    assert_eq!(
        mcp.fake.only_request().json(),
        json!({"body": comment_doc()})
    );
}

#[tokio::test]
async fn add_comment_bad_format_or_blank_text_fails_before_request() {
    let mcp = Mcp::start().await;
    let bad_format = mcp
        .call_err(
            "jira_add_comment",
            json!({"issue_key": "SWS-1", "comment": "hi", "content_format": "xml"}),
        )
        .await;
    assert_eq!(bad_format["code"], -32602);
    assert_eq!(
        bad_format["message"],
        "content_format must be \"plain\" or \"adf\", got \"xml\""
    );

    let blank = mcp
        .call_err(
            "jira_add_comment",
            json!({"issue_key": "SWS-1", "comment": "   "}),
        )
        .await;
    assert_eq!(blank["code"], -32602);
    assert_eq!(
        blank["message"],
        "body text must not be empty when content_format is \"plain\""
    );
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn add_comment_large_plain_text_keeps_every_line() {
    let mcp = Mcp::start().await;
    mcp.fake
        .on("POST", ISSUE_COMMENTS, 201, json!({"id": "10200"}));
    let lines: Vec<String> = (1..=2000).map(|n| format!("line {n}")).collect();

    mcp.call(
        "jira_add_comment",
        json!({"issue_key": "SWS-1", "comment": lines.join("\n")}),
    )
    .await;
    let body = mcp.fake.only_request().json();
    let paragraphs = body["body"]["content"].as_array().expect("paragraphs");
    assert_eq!(paragraphs.len(), 2000);
    assert_eq!(paragraphs[0]["content"][0]["text"], "line 1");
    assert_eq!(paragraphs[1999]["content"][0]["text"], "line 2000");
}

// ---- jira_update_description ----

#[tokio::test]
async fn update_description_puts_adf_and_returns_ok_on_204() {
    let mcp = Mcp::start().await;
    mcp.fake.on_raw("PUT", ISSUE, 204, "");

    let out = mcp
        .call(
            "jira_update_description",
            json!({"issue_key": "SWS-1", "description": "Line one\n\nLine two"}),
        )
        .await;
    assert_eq!(out, json!({"ok": true}));

    let req = mcp.fake.only_request();
    assert_eq!(req.method, "PUT");
    assert_eq!(req.path, ISSUE);
    assert_eq!(
        req.json(),
        json!({"fields": {"description": {"type": "doc", "version": 1, "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "Line one"}]},
            {"type": "paragraph", "content": []},
            {"type": "paragraph", "content": [{"type": "text", "text": "Line two"}]}
        ]}}})
    );
}

#[tokio::test]
async fn update_description_rejects_non_doc_adf_before_request() {
    let mcp = Mcp::start().await;
    let err = mcp
        .call_err(
            "jira_update_description",
            json!({
                "issue_key": "SWS-1",
                "description": r#"{"type":"paragraph","version":1}"#,
                "content_format": "adf"
            }),
        )
        .await;
    assert_eq!(err["code"], -32602);
    assert_eq!(
        err["message"],
        "ADF document \"type\" must be \"doc\" (Jira Cloud REST v3), got \"paragraph\""
    );
    assert!(mcp.fake.requests().is_empty());
}

// ---- jira_update_summary ----

#[tokio::test]
async fn update_summary_trims_and_puts_fields_summary() {
    let mcp = Mcp::start().await;
    mcp.fake.on_raw("PUT", ISSUE, 204, "");

    let out = mcp
        .call(
            "jira_update_summary",
            json!({"issue_key": "SWS-1", "summary": "  New title  "}),
        )
        .await;
    assert_eq!(out, json!({"ok": true}));
    assert_eq!(
        mcp.fake.only_request().json(),
        json!({"fields": {"summary": "New title"}})
    );
}

#[tokio::test]
async fn update_summary_returns_jira_body_when_one_is_sent() {
    let mcp = Mcp::start().await;
    mcp.fake.on("PUT", ISSUE, 200, json!({"id": "10000"}));

    let out = mcp
        .call(
            "jira_update_summary",
            json!({"issue_key": "SWS-1", "summary": "New title"}),
        )
        .await;
    assert_eq!(out, json!({"id": "10000"}));
}

#[tokio::test]
async fn update_summary_blank_is_error_before_request() {
    let mcp = Mcp::start().await;
    let err = mcp
        .call_err(
            "jira_update_summary",
            json!({"issue_key": "SWS-1", "summary": "  "}),
        )
        .await;
    assert_eq!(err["code"], -32603);
    assert_eq!(err["message"], "summary must not be empty");
    assert!(mcp.fake.requests().is_empty());
}

// ---- jira_create_issue ----

#[tokio::test]
async fn create_issue_posts_fields_and_returns_jira_response() {
    let mcp = Mcp::start().await;
    let created = json!({"id": "10001", "key": "SWS-2", "self": "https://example.invalid/rest/api/3/issue/10001"});
    mcp.fake
        .on("POST", "/rest/api/3/issue", 201, created.clone());

    let out = mcp
        .call(
            "jira_create_issue",
            json!({
                "project_key": "SWS",
                "issue_type": "Task",
                "summary": "  Fix login  ",
                "description": "Steps",
            }),
        )
        .await;
    assert_eq!(out, created);
    assert_eq!(
        mcp.fake.only_request().json(),
        json!({"fields": {
            "project": {"key": "SWS"},
            "issuetype": {"name": "Task"},
            "summary": "Fix login",
            "description": {"type": "doc", "version": 1, "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "Steps"}]}
            ]}
        }})
    );
}

#[tokio::test]
async fn create_issue_blank_description_is_omitted() {
    let mcp = Mcp::start().await;
    mcp.fake
        .on("POST", "/rest/api/3/issue", 201, json!({"key": "SWS-2"}));

    mcp.call(
        "jira_create_issue",
        json!({"project_key": "SWS", "issue_type": "Task", "summary": "Fix", "description": "   "}),
    )
    .await;
    let fields = mcp.fake.only_request().json()["fields"].clone();
    assert!(fields.get("description").is_none());
    assert_eq!(fields["summary"], "Fix");
}

#[tokio::test]
async fn create_issue_blank_required_fields_fail_before_request() {
    let mcp = Mcp::start().await;
    let cases = [
        (
            json!({"project_key": " ", "issue_type": "Task", "summary": "s"}),
            "project_key must not be empty",
        ),
        (
            json!({"project_key": "SWS", "issue_type": " ", "summary": "s"}),
            "issue_type must not be empty",
        ),
        (
            json!({"project_key": "SWS", "issue_type": "Task", "summary": " "}),
            "summary must not be empty",
        ),
    ];
    for (args, message) in cases {
        let err = mcp.call_err("jira_create_issue", args).await;
        // Pins current behaviour: blank input is an internal error (-32603), not invalid params.
        assert_eq!(err["code"], -32603);
        assert_eq!(err["message"], message);
    }
    assert!(mcp.fake.requests().is_empty());
}

// ---- jira_get_transitions ----

#[tokio::test]
async fn get_transitions_slims_targets_and_encodes_slash_in_key() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/rest/api/3/issue/SWS%2F1/transitions",
        200,
        json!({"transitions": [
            {
                "id": "21",
                "name": "Start progress",
                "to": {
                    "self": "https://example.invalid/rest/api/3/status/3",
                    "id": "3",
                    "name": "In Progress",
                    "statusCategory": {"key": "indeterminate", "name": "In Progress", "colorName": "yellow"}
                },
                "hasScreen": false,
                "isAvailable": true,
                "fields": {}
            },
            {"id": "31", "name": "Done", "hasScreen": true}
        ]}),
    );

    let out = mcp
        .call("jira_get_transitions", json!({"issue_key": "SWS/1"}))
        .await;
    assert_eq!(
        out,
        json!({"key": "SWS/1", "transitions": [
            {
                "id": "21",
                "name": "Start progress",
                "to": {"name": "In Progress", "id": "3", "statusCategory": {"key": "indeterminate", "name": "In Progress"}}
            },
            {"id": "31", "name": "Done", "to": null}
        ]})
    );
    assert_eq!(mcp.fake.only_request().method, "GET");
}

// ---- jira_get_issue_history ----

fn history_entry(id: &str, items: Value) -> Value {
    json!({
        "id": id,
        "author": author_raw(),
        "created": "2026-11-05T12:00:00.000+0000",
        "items": items
    })
}

#[tokio::test]
async fn issue_history_field_filter_is_case_insensitive() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/rest/api/3/issue/SWS-1/changelog",
        200,
        json!({"startAt": 0, "maxResults": 100, "total": 2, "isLast": true, "values": [
            history_entry("2001", json!([
                {"field": "status", "fieldtype": "jira", "from": "10000", "fromString": "To Do", "to": "3", "toString": "In Progress"}
            ])),
            history_entry("2002", json!([
                {"field": "assignee", "fieldtype": "jira", "from": null, "fromString": null, "to": "acc-1", "toString": "Test User"}
            ]))
        ]}),
    );

    let out = mcp
        .call(
            "jira_get_issue_history",
            json!({"issue_key": "SWS-1", "field": " Status "}),
        )
        .await;
    assert_eq!(
        out,
        json!({
            "key": "SWS-1",
            "field": "status",
            "total_changes": 2,
            "returned": 1,
            "next_start_at": null,
            "history": [{
                "id": "2001",
                "author": {"displayName": "Test User", "emailAddress": "test.user@example.invalid"},
                "created": "2026-11-05T12:00:00.000+0000",
                "items": [{"field": "status", "from": "To Do", "to": "In Progress"}]
            }]
        })
    );
    let req = mcp.fake.only_request();
    assert_eq!(
        req.query_pairs(),
        q(&[("startAt", "0"), ("maxResults", "100")])
    );
}

#[tokio::test]
async fn issue_history_max_results_sets_resume_cursor() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/rest/api/3/issue/SWS-1/changelog",
        200,
        json!({"total": 2, "isLast": false, "values": [
            history_entry("2001", json!([{"field": "status", "fromString": "To Do", "toString": "Done"}])),
            history_entry("2002", json!([{"field": "status", "fromString": "Done", "toString": "To Do"}]))
        ]}),
    );

    let out = mcp
        .call(
            "jira_get_issue_history",
            json!({"issue_key": "SWS-1", "max_results": 1, "start_at": 5}),
        )
        .await;
    assert_eq!(out["returned"], 1);
    assert_eq!(out["history"][0]["id"], "2001");
    assert!(out["field"].is_null());
    // One raw entry was consumed, so the next page starts at 5 + 1.
    assert_eq!(out["next_start_at"], 6);
    assert_eq!(
        mcp.fake.only_request().query_pairs(),
        q(&[("startAt", "5"), ("maxResults", "100")])
    );
}

#[tokio::test]
async fn issue_history_resumes_from_consumed_entries_across_pages() {
    let (site, script) = scripted_jira(vec![
        reply(200, json!({"total": 3, "isLast": false, "values": [
            history_entry("2001", json!([{"field": "assignee", "fromString": null, "toString": "Test User"}])),
            history_entry("2002", json!([{"field": "status", "fromString": "To Do", "toString": "In Progress"}]))
        ]})),
        reply(200, json!({"total": 3, "isLast": true, "values": [
            history_entry("2003", json!([{"field": "status", "fromString": "In Progress", "toString": "Done"}]))
        ]})),
    ])
    .await;
    let mcp = Mcp::start().await;

    let resp = call_on_site(
        &mcp,
        &site,
        "jira_get_issue_history",
        json!({"issue_key": "SWS-1", "field": "status", "max_results": 5}),
    )
    .await;
    let out = result_json(&resp);
    assert_eq!(out["total_changes"], 3);
    assert_eq!(out["returned"], 2);
    assert_eq!(out["history"][0]["id"], "2002");
    assert_eq!(out["history"][1]["id"], "2003");
    assert!(out["next_start_at"].is_null());

    let seen = seen_requests(&script);
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[0].query_pairs(),
        q(&[("startAt", "0"), ("maxResults", "100")])
    );
    // Page two resumes after both raw entries of page one, the filtered-out one included.
    assert_eq!(
        seen[1].query_pairs(),
        q(&[("startAt", "2"), ("maxResults", "100")])
    );
}

// ---- jira_transition_issue ----

#[tokio::test]
async fn transition_issue_posts_trimmed_id_and_returns_ok() {
    let mcp = Mcp::start().await;
    mcp.fake
        .on_raw("POST", "/rest/api/3/issue/SWS-1/transitions", 204, "");

    let out = mcp
        .call(
            "jira_transition_issue",
            json!({"issue_key": "SWS-1", "transition_id": " 31 "}),
        )
        .await;
    assert_eq!(out, json!({"ok": true}));
    let req = mcp.fake.only_request();
    assert_eq!(req.method, "POST");
    assert_eq!(req.json(), json!({"transition": {"id": "31"}}));
}

#[tokio::test]
async fn transition_issue_blank_id_fails_before_request() {
    let mcp = Mcp::start().await;
    let err = mcp
        .call_err(
            "jira_transition_issue",
            json!({"issue_key": "SWS-1", "transition_id": "  "}),
        )
        .await;
    assert_eq!(err["code"], -32603);
    assert_eq!(err["message"], "transition_id must not be empty");
    assert!(mcp.fake.requests().is_empty());
}

// ---- jira_set_assignee ----

#[tokio::test]
async fn set_assignee_sends_account_id_or_null_to_unassign() {
    let mcp = Mcp::start().await;
    mcp.fake
        .on_raw("PUT", "/rest/api/3/issue/SWS-1/assignee", 204, "");

    let out = mcp
        .call(
            "jira_set_assignee",
            json!({"issue_key": "SWS-1", "account_id": "acc-1"}),
        )
        .await;
    assert_eq!(out, json!({"ok": true}));

    mcp.call(
        "jira_set_assignee",
        json!({"issue_key": "SWS-1", "account_id": "-1"}),
    )
    .await;
    mcp.call("jira_set_assignee", json!({"issue_key": "SWS-1"}))
        .await;
    mcp.call(
        "jira_set_assignee",
        json!({"issue_key": "SWS-1", "account_id": "  "}),
    )
    .await;

    let bodies: Vec<Value> = mcp.fake.requests().iter().map(Recorded::json).collect();
    assert_eq!(
        bodies,
        vec![
            json!({"accountId": "acc-1"}),
            json!({"accountId": "-1"}),
            json!({"accountId": null}),
            json!({"accountId": null}),
        ]
    );
}

// ---- jira_list_boards ----

#[tokio::test]
async fn list_boards_passes_filters_and_slims_values() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/rest/agile/1.0/board",
        200,
        json!({"maxResults": 5, "startAt": 10, "total": 12, "isLast": false, "values": [
            {
                "id": 7,
                "name": "SWS board",
                "type": "scrum",
                "self": "https://example.invalid/rest/agile/1.0/board/7",
                "location": {"projectKey": "SWS", "displayName": "SWS"}
            }
        ]}),
    );

    let out = mcp
        .call(
            "jira_list_boards",
            json!({"project_key": "SWS", "max_results": 5, "start_at": 10}),
        )
        .await;
    assert_eq!(
        out,
        json!({"boards": [{"id": 7, "name": "SWS board", "type": "scrum"}], "isLast": false, "total": 12})
    );
    assert_eq!(
        mcp.fake.only_request().query_pairs(),
        q(&[
            ("maxResults", "5"),
            ("startAt", "10"),
            ("projectKeyOrId", "SWS")
        ])
    );
}

#[tokio::test]
async fn list_boards_clamps_max_and_omits_blank_project() {
    let mcp = Mcp::start().await;
    mcp.fake
        .on("GET", "/rest/agile/1.0/board", 200, json!({"values": []}));

    let defaults = mcp
        .call("jira_list_boards", json!({"project_key": "   "}))
        .await;
    assert_eq!(
        defaults,
        json!({"boards": [], "isLast": null, "total": null})
    );
    mcp.call("jira_list_boards", json!({"max_results": 500}))
        .await;
    mcp.call("jira_list_boards", json!({"max_results": 0}))
        .await;

    let seen = mcp.fake.requests();
    assert_eq!(
        seen[0].query_pairs(),
        q(&[("maxResults", "50"), ("startAt", "0")])
    );
    assert_eq!(
        seen[1].query_pairs(),
        q(&[("maxResults", "50"), ("startAt", "0")])
    );
    assert_eq!(
        seen[2].query_pairs(),
        q(&[("maxResults", "1"), ("startAt", "0")])
    );
}

// ---- jira_board_issues ----

#[tokio::test]
async fn board_issues_board_scope_subtracts_backlog_and_applies_done_filter() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/rest/agile/1.0/board/7/issue",
        200,
        json!({"total": 2, "issues": [raw_issue("SWS-1", "On board"), raw_issue("SWS-2", "Also in backlog")]}),
    );
    mcp.fake.on(
        "GET",
        "/rest/agile/1.0/board/7/backlog",
        200,
        json!({"total": 1, "issues": [raw_issue("SWS-2", "Also in backlog")]}),
    );

    let out = mcp.call("jira_board_issues", json!({"board_id": 7})).await;
    assert_eq!(
        out,
        json!({
            "scope": "board",
            "board_id": 7,
            "total": 1,
            "issues": [slim_issue("SWS-1", "On board")],
            "truncated": false
        })
    );
    let board = requests_to(&mcp, "/rest/agile/1.0/board/7/issue");
    let backlog = requests_to(&mcp, "/rest/agile/1.0/board/7/backlog");
    assert_eq!(board.len(), 1);
    assert_eq!(backlog.len(), 1);
    assert_eq!(
        board[0].query_pairs(),
        q(&[
            ("maxResults", "100"),
            ("startAt", "0"),
            ("fields", "key,summary,status,issuetype,assignee,updated"),
            ("jql", "statusCategory != Done"),
        ])
    );
    assert_eq!(backlog[0].query_pairs(), board[0].query_pairs());
}

#[tokio::test]
async fn board_issues_all_scope_with_include_done_keeps_caller_jql() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/rest/agile/1.0/board/7/issue",
        200,
        json!({"total": 1, "issues": [raw_issue("SWS-1", "Done work")]}),
    );

    let out = mcp
        .call(
            "jira_board_issues",
            json!({"board_id": 7, "scope": "all", "include_done": true, "jql": "project = SWS ORDER BY updated DESC"}),
        )
        .await;
    assert_eq!(out["scope"], "all");
    assert_eq!(out["issues"], json!([slim_issue("SWS-1", "Done work")]));
    let req = mcp.fake.only_request();
    assert!(req.path.ends_with("/issue"));
    assert_eq!(
        req.query_pairs().last(),
        Some(&(
            "jql".to_string(),
            "project = SWS ORDER BY updated DESC".to_string()
        ))
    );
}

#[tokio::test]
async fn board_issues_backlog_scope_puts_done_filter_before_order_by() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/rest/agile/1.0/board/7/backlog",
        200,
        json!({"total": 0, "issues": []}),
    );

    let out = mcp
        .call(
            "jira_board_issues",
            json!({"board_id": 7, "scope": "backlog", "jql": "project = SWS ORDER BY updated DESC"}),
        )
        .await;
    assert_eq!(
        out,
        json!({"scope": "backlog", "board_id": 7, "total": 0, "issues": [], "truncated": false})
    );
    assert_eq!(
        mcp.fake.only_request().query_pairs().last(),
        Some(&(
            "jql".to_string(),
            "(project = SWS) AND statusCategory != Done ORDER BY updated DESC".to_string()
        ))
    );
}

#[tokio::test]
async fn board_issues_paging_applies_to_final_list_and_clamps_max() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/rest/agile/1.0/board/7/issue",
        200,
        json!({"total": 3, "issues": [
            raw_issue("SWS-1", "First"),
            raw_issue("SWS-2", "Second"),
            raw_issue("SWS-3", "Third")
        ]}),
    );

    let page = mcp
        .call(
            "jira_board_issues",
            json!({"board_id": 7, "scope": "all", "start_at": 1, "max_results": 1}),
        )
        .await;
    assert_eq!(page["total"], 3);
    assert_eq!(page["issues"], json!([slim_issue("SWS-2", "Second")]));

    let clamped = mcp
        .call(
            "jira_board_issues",
            json!({"board_id": 7, "scope": "all", "max_results": 0}),
        )
        .await;
    assert_eq!(clamped["issues"], json!([slim_issue("SWS-1", "First")]));
}

#[tokio::test]
async fn board_issues_invalid_scope_is_rejected_before_request() {
    let mcp = Mcp::start().await;
    let err = mcp
        .call_err(
            "jira_board_issues",
            json!({"board_id": 7, "scope": "sprint"}),
        )
        .await;
    assert_eq!(err["code"], -32602);
    assert_eq!(
        err["message"],
        "Invalid scope 'sprint'. Must be one of: \"board\", \"backlog\", \"all\"."
    );
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn board_issues_reports_truncation_after_fifty_pages() {
    let mcp = Mcp::start().await;
    // Every page returns one issue and claims 200 in total, so the 50-page cap cuts it off.
    mcp.fake.on(
        "GET",
        "/rest/agile/1.0/board/7/issue",
        200,
        json!({"total": 200, "issues": [raw_issue("SWS-9", "Looping")]}),
    );

    let out = mcp
        .call("jira_board_issues", json!({"board_id": 7, "scope": "all"}))
        .await;
    assert_eq!(out["truncated"], true);
    assert_eq!(out["total"], 50);
    assert_eq!(mcp.fake.requests().len(), 50);
}

#[tokio::test]
async fn board_issues_board_scope_truncation_is_an_error() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/rest/agile/1.0/board/7/issue",
        200,
        json!({"total": 200, "issues": [raw_issue("SWS-9", "Looping")]}),
    );
    mcp.fake.on(
        "GET",
        "/rest/agile/1.0/board/7/backlog",
        200,
        json!({"total": 0, "issues": []}),
    );

    let err = mcp
        .call_err("jira_board_issues", json!({"board_id": 7}))
        .await;
    assert_eq!(err["code"], -32603);
    assert_eq!(
        err["message"],
        "board 7 scope=\"board\" set difference is unreliable: board or backlog fetch was truncated by the MAX_PAGES cap"
    );
}

// ---- jira_move_to_board / jira_move_to_backlog ----

#[tokio::test]
async fn move_to_board_posts_keys_and_accepts_fifty() {
    let mcp = Mcp::start().await;
    mcp.fake
        .on_raw("POST", "/rest/agile/1.0/board/7/issue", 204, "");
    let keys: Vec<String> = (1..=50).map(|n| format!("SWS-{n}")).collect();

    let out = mcp
        .call(
            "jira_move_to_board",
            json!({"board_id": 7, "issue_keys": keys}),
        )
        .await;
    assert_eq!(
        out,
        json!({"moved_to": "board", "board_id": 7, "issue_keys": keys})
    );
    let req = mcp.fake.only_request();
    assert_eq!(req.method, "POST");
    assert_eq!(req.json(), json!({"issues": keys}));
}

#[tokio::test]
async fn move_to_board_rejects_empty_and_too_many_keys_before_request() {
    let mcp = Mcp::start().await;
    let empty = mcp
        .call_err(
            "jira_move_to_board",
            json!({"board_id": 7, "issue_keys": []}),
        )
        .await;
    assert_eq!(empty["code"], -32602);
    assert_eq!(empty["message"], "issue_keys must not be empty");

    let too_many: Vec<String> = (1..=51).map(|n| format!("SWS-{n}")).collect();
    let over = mcp
        .call_err(
            "jira_move_to_board",
            json!({"board_id": 7, "issue_keys": too_many}),
        )
        .await;
    assert_eq!(over["code"], -32602);
    assert_eq!(
        over["message"],
        "issue_keys has 51 entries; Jira caps this endpoint at 50 per call"
    );
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn move_to_backlog_posts_board_scoped_endpoint() {
    let mcp = Mcp::start().await;
    mcp.fake
        .on_raw("POST", "/rest/agile/1.0/backlog/7/issue", 204, "");

    let out = mcp
        .call(
            "jira_move_to_backlog",
            json!({"board_id": 7, "issue_keys": ["SWS-1", "SWS-2"]}),
        )
        .await;
    assert_eq!(
        out,
        json!({"moved_to": "backlog", "board_id": 7, "issue_keys": ["SWS-1", "SWS-2"]})
    );
    let req = mcp.fake.only_request();
    assert_eq!(req.path, "/rest/agile/1.0/backlog/7/issue");
    assert_eq!(req.json(), json!({"issues": ["SWS-1", "SWS-2"]}));
}

// ---- jira_search ----

#[tokio::test]
async fn search_pages_with_next_token_until_max_results() {
    let (site, script) = scripted_jira(vec![
        reply(
            200,
            json!({
                "issues": [raw_issue("SWS-1", "A"), raw_issue("SWS-2", "B")],
                "isLast": false,
                "nextPageToken": "t1"
            }),
        ),
        reply(
            200,
            json!({
                "issues": [raw_issue("SWS-3", "C")],
                "isLast": true,
                "total": 3
            }),
        ),
    ])
    .await;
    let mcp = Mcp::start().await;

    let resp = call_on_site(
        &mcp,
        &site,
        "jira_search",
        json!({"jql": "project = SWS", "max_results": 3}),
    )
    .await;
    assert_eq!(
        result_json(&resp),
        json!({
            "startAt": 0,
            "maxResults": 3,
            "total": 3,
            "issues": [slim_issue("SWS-1", "A"), slim_issue("SWS-2", "B"), slim_issue("SWS-3", "C")],
            "isLast": true,
            "nextPageToken": null
        })
    );
    let seen = seen_requests(&script);
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].path, SEARCH_JQL);
    assert_eq!(
        seen[0].json(),
        json!({
            "jql": "project = SWS",
            "maxResults": 100,
            "fields": ["key", "summary", "status", "issuetype", "assignee", "updated"]
        })
    );
    // Pins current behaviour: the upstream page size is always 100, whatever max_results says.
    assert_eq!(seen[1].json()["nextPageToken"], "t1");
}

#[tokio::test]
async fn search_start_at_skips_across_pages() {
    let (site, _script) = scripted_jira(vec![
        reply(
            200,
            json!({
                "issues": [raw_issue("SWS-1", "A"), raw_issue("SWS-2", "B")],
                "isLast": false,
                "nextPageToken": "t1"
            }),
        ),
        reply(
            200,
            json!({
                "issues": [raw_issue("SWS-3", "C")],
                "isLast": true
            }),
        ),
    ])
    .await;
    let mcp = Mcp::start().await;

    let resp = call_on_site(
        &mcp,
        &site,
        "jira_search",
        json!({"jql": "project = SWS", "start_at": 1, "max_results": 2}),
    )
    .await;
    let out = result_json(&resp);
    assert_eq!(out["startAt"], 1);
    assert_eq!(
        out["issues"],
        json!([slim_issue("SWS-2", "B"), slim_issue("SWS-3", "C")])
    );
    assert!(out["total"].is_null());
}

#[tokio::test]
async fn search_cursor_mode_passes_token_and_fields_untouched() {
    let mcp = Mcp::start().await;
    let raw = raw_issue("SWS-1", "A");
    mcp.fake.on(
        "POST",
        SEARCH_JQL,
        200,
        json!({"issues": [raw.clone()], "isLast": false, "nextPageToken": "t2", "total": 9}),
    );

    let out = mcp
        .call(
            "jira_search",
            json!({"jql": "project = SWS", "fields": ["summary", "labels"], "next_page_token": "t1"}),
        )
        .await;
    assert_eq!(out["issues"], json!([raw]));
    assert_eq!(out["nextPageToken"], "t2");
    assert_eq!(out["isLast"], false);
    assert_eq!(out["total"], 9);
    assert_eq!(out["startAt"], 0);
    assert_eq!(out["maxResults"], 25);
    assert_eq!(
        mcp.fake.only_request().json(),
        json!({
            "jql": "project = SWS",
            "maxResults": 25,
            "fields": ["summary", "labels"],
            "nextPageToken": "t1"
        })
    );
}

#[tokio::test]
async fn search_cursor_mode_with_token_only_slims_issues() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        SEARCH_JQL,
        200,
        json!({"issues": [raw_issue("SWS-1", "A")], "isLast": true}),
    );

    let out = mcp
        .call(
            "jira_search",
            json!({"jql": "project = SWS", "next_page_token": "t1"}),
        )
        .await;
    assert_eq!(out["issues"], json!([slim_issue("SWS-1", "A")]));
    assert!(out["nextPageToken"].is_null());
    let body = mcp.fake.only_request().json();
    assert_eq!(body["nextPageToken"], "t1");
    assert_eq!(body["maxResults"], 25);
}

#[tokio::test]
async fn search_blank_token_is_not_sent() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        SEARCH_JQL,
        200,
        json!({"issues": [], "isLast": true}),
    );

    mcp.call(
        "jira_search",
        json!({"jql": "project = SWS", "next_page_token": "  "}),
    )
    .await;
    let body = mcp.fake.only_request().json();
    assert!(body.get("nextPageToken").is_none());
}

#[tokio::test]
async fn search_cursor_mode_login_failure_is_an_error() {
    let mcp = Mcp::start().await;
    let failed = Reply {
        status: 200,
        headers: vec![("X-Seraph-LoginReason", "AUTHENTICATED_FAILED")],
        body: json!({"issues": [], "isLast": true}).to_string(),
    };
    let (site, _script) = scripted_jira(vec![failed.clone()]).await;

    let resp = call_on_site(
        &mcp,
        &site,
        "jira_search",
        json!({"jql": "project = SWS", "fields": ["summary"]}),
    )
    .await;
    assert_eq!(resp["error"]["code"], -32603);
    assert_eq!(
        resp["error"]["message"],
        "Jira search/jql failed: Jira returned 401 Unauthorized: X-Seraph-LoginReason AUTHENTICATED_FAILED (check email and API token)"
    );

    // Pins current behaviour: plain paging ignores the login header and returns the anonymous 200.
    let (site, _script) = scripted_jira(vec![failed]).await;
    let resp = call_on_site(&mcp, &site, "jira_search", json!({"jql": "project = SWS"})).await;
    assert_eq!(result_json(&resp)["issues"], json!([]));
}

#[tokio::test]
async fn search_validation_fails_before_request() {
    let mcp = Mcp::start().await;
    let blank = mcp.call_err("jira_search", json!({"jql": "  "})).await;
    assert_eq!(blank["code"], -32603);
    assert_eq!(blank["message"], "jql must not be empty");

    let start = mcp
        .call_err(
            "jira_search",
            json!({"jql": "project = SWS", "start_at": 1, "next_page_token": "t1"}),
        )
        .await;
    assert_eq!(start["code"], -32603);
    assert_eq!(
        start["message"],
        "start_at cannot be combined with fields or next_page_token; page with next_page_token"
    );

    let no_fields = mcp
        .call_err("jira_search", json!({"jql": "project = SWS", "fields": []}))
        .await;
    assert_eq!(no_fields["code"], -32603);
    assert_eq!(no_fields["message"], "fields must list at least one field");
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn search_clamps_reported_max_results() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        SEARCH_JQL,
        200,
        json!({"issues": [], "isLast": true}),
    );

    let low = mcp
        .call("jira_search", json!({"jql": "x", "max_results": 0}))
        .await;
    let high = mcp
        .call("jira_search", json!({"jql": "x", "max_results": 500}))
        .await;
    assert_eq!(low["maxResults"], 1);
    assert_eq!(high["maxResults"], 100);
}

// ---- jira_search_users ----

#[tokio::test]
async fn search_users_passes_query_and_slims_results() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/rest/api/3/user/search",
        200,
        json!([
            {"accountId": "acc-1", "displayName": "Test User", "emailAddress": "test.user@example.invalid", "active": true, "timeZone": "UTC"},
            {"accountId": "acc-2", "displayName": "Other User", "accountType": "atlassian"},
            {"displayName": "No Account Id"}
        ]),
    );

    let out = mcp
        .call("jira_search_users", json!({"query": "Test User"}))
        .await;
    assert_eq!(
        out,
        json!({
            "users": [
                {"accountId": "acc-1", "displayName": "Test User", "emailAddress": "test.user@example.invalid", "active": true},
                {"accountId": "acc-2", "displayName": "Other User", "emailAddress": null, "active": null}
            ],
            "total": 2
        })
    );
    assert_eq!(
        mcp.fake.only_request().query_pairs(),
        q(&[("query", "Test User"), ("maxResults", "10")])
    );
}

#[tokio::test]
async fn search_users_clamps_max_results_and_rejects_blank_query() {
    let mcp = Mcp::start().await;
    mcp.fake
        .on("GET", "/rest/api/3/user/search", 200, json!([]));

    mcp.call(
        "jira_search_users",
        json!({"query": "x", "max_results": 500}),
    )
    .await;
    mcp.call("jira_search_users", json!({"query": "x", "max_results": 0}))
        .await;
    let seen = mcp.fake.requests();
    assert_eq!(
        seen[0].query_pairs(),
        q(&[("query", "x"), ("maxResults", "50")])
    );
    assert_eq!(
        seen[1].query_pairs(),
        q(&[("query", "x"), ("maxResults", "1")])
    );

    let err = mcp
        .call_err("jira_search_users", json!({"query": "   "}))
        .await;
    assert_eq!(err["code"], -32603);
    assert_eq!(err["message"], "query must not be empty");
    assert_eq!(mcp.fake.requests().len(), 2);
}

#[tokio::test]
async fn search_users_upstream_error_keeps_status_and_body() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/rest/api/3/user/search",
        401,
        json!({"errorMessages": ["Unauthorized"]}),
    );
    let err = mcp
        .call_err("jira_search_users", json!({"query": "Test"}))
        .await;
    assert_eq!(err["code"], -32603);
    assert_eq!(
        err["message"],
        r#"Jira returned 401 Unauthorized: {"errorMessages":["Unauthorized"]}"#
    );
}

// ---- jira_list_attachments / jira_add_attachment / jira_delete_attachment ----

fn attachment_raw(id: &str, filename: &str) -> Value {
    json!({
        "id": id,
        "filename": filename,
        "mimeType": "image/png",
        "size": 2048,
        "content": format!("https://example.invalid/attachment/{id}"),
        "thumbnail": format!("https://example.invalid/thumb/{id}"),
        "created": "2026-11-02T08:00:00.000+0000",
        "author": author_raw(),
        "self": format!("https://example.invalid/rest/api/3/attachment/{id}")
    })
}

fn attachment_slim(id: &str, filename: &str) -> Value {
    json!({
        "id": id,
        "filename": filename,
        "mimeType": "image/png",
        "size": 2048,
        "content": format!("https://example.invalid/attachment/{id}"),
        "created": "2026-11-02T08:00:00.000+0000",
        "author": {"displayName": "Test User", "emailAddress": "test.user@example.invalid"}
    })
}

#[tokio::test]
async fn list_attachments_slims_and_counts() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        ISSUE,
        200,
        json!({"key": "SWS-1", "fields": {"attachment": [attachment_raw("10001", "screen.png")]}}),
    );

    let out = mcp
        .call("jira_list_attachments", json!({"issue_key": "SWS-1"}))
        .await;
    assert_eq!(
        out,
        json!({"key": "SWS-1", "attachments": [attachment_slim("10001", "screen.png")], "total": 1})
    );
    assert_eq!(
        mcp.fake.only_request().query_pairs(),
        q(&[("fields", "attachment")])
    );
}

#[tokio::test]
async fn list_attachments_without_field_is_empty() {
    let mcp = Mcp::start().await;
    mcp.fake
        .on("GET", ISSUE, 200, json!({"key": "SWS-1", "fields": {}}));

    let out = mcp
        .call("jira_list_attachments", json!({"issue_key": "SWS-1"}))
        .await;
    assert_eq!(out, json!({"key": "SWS-1", "attachments": [], "total": 0}));
}

#[tokio::test]
async fn add_attachment_posts_multipart_and_slims_response() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        "/rest/api/3/issue/SWS-1/attachments",
        200,
        json!([attachment_raw("10001", "report.pdf")]),
    );

    let out = mcp
        .call(
            "jira_add_attachment",
            json!({"issue_key": "SWS-1", "filename": "report.pdf", "file_base64": "aGVsbG8="}),
        )
        .await;
    assert_eq!(
        out,
        json!({"ok": true, "attachments": [attachment_slim("10001", "report.pdf")]})
    );

    let req = mcp.fake.only_request();
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/rest/api/3/issue/SWS-1/attachments");
    let token = req.headers.get("x-atlassian-token").expect("token header");
    assert_eq!(token.to_str().expect("ascii"), "no-check");
    let basic = base64::engine::general_purpose::STANDARD.encode("tester@example.com:test-token");
    let auth = req.headers.get("authorization").expect("auth header");
    assert_eq!(auth.to_str().expect("ascii"), format!("Basic {basic}"));
    assert!(contains(&req.body, br#"filename="report.pdf""#));
    assert!(contains(&req.body, b"hello"));
}

#[tokio::test]
async fn add_attachment_invalid_base64_is_invalid_request() {
    let mcp = Mcp::start().await;
    let err = mcp
        .call_err(
            "jira_add_attachment",
            json!({"issue_key": "SWS-1", "filename": "a.png", "file_base64": "not base64!"}),
        )
        .await;
    assert_eq!(err["code"], -32600);
    assert_eq!(
        err["message"],
        "Invalid base64 in file_base64: Invalid symbol 32, offset 3."
    );
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn add_attachment_blank_filename_fails_before_request() {
    let mcp = Mcp::start().await;
    let err = mcp
        .call_err(
            "jira_add_attachment",
            json!({"issue_key": "SWS-1", "filename": "  ", "file_base64": "aGVsbG8="}),
        )
        .await;
    // Pins current behaviour: a blank filename is an internal error (-32603), not invalid params.
    assert_eq!(err["code"], -32603);
    assert_eq!(err["message"], "filename must not be empty");
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn delete_attachment_returns_ok_on_204() {
    let mcp = Mcp::start().await;
    mcp.fake
        .on_raw("DELETE", "/rest/api/3/attachment/10001", 204, "");

    let out = mcp
        .call("jira_delete_attachment", json!({"attachment_id": "10001"}))
        .await;
    assert_eq!(out, json!({"ok": true}));
    assert_eq!(mcp.fake.only_request().method, "DELETE");
}

#[tokio::test]
async fn delete_attachment_blank_id_and_upstream_404() {
    let mcp = Mcp::start().await;
    let blank = mcp
        .call_err("jira_delete_attachment", json!({"attachment_id": "  "}))
        .await;
    assert_eq!(blank["code"], -32603);
    assert_eq!(blank["message"], "attachment_id must not be empty");
    assert!(mcp.fake.requests().is_empty());

    mcp.fake.on(
        "DELETE",
        "/rest/api/3/attachment/10001",
        404,
        json!({"errorMessages": ["Attachment does not exist"]}),
    );
    let missing = mcp
        .call_err("jira_delete_attachment", json!({"attachment_id": "10001"}))
        .await;
    assert_eq!(
        missing["message"],
        r#"Jira returned 404 Not Found: {"errorMessages":["Attachment does not exist"]}"#
    );
}

// ---- jira_get_attachment_upload_url ----

#[tokio::test]
async fn upload_url_ttl_defaults_and_clamps_to_one_hour() {
    let mcp = Mcp::start().await;
    let default = mcp
        .call(
            "jira_get_attachment_upload_url",
            json!({"issue_key": "SWS-1", "filename": "report.zip"}),
        )
        .await;
    assert_eq!(default["expires_in_seconds"], 600);
    assert_eq!(default["method"], "PUT");
    assert_eq!(default["single_use"], true);
    assert_eq!(default["issue_key"], "SWS-1");
    assert_eq!(default["filename"], "report.zip");
    let url = default["upload_url"].as_str().expect("upload_url");
    assert!(url.starts_with(&format!("{}/upload/", mcp.base_url)));
    assert_eq!(
        default["curl"],
        format!("curl -sS -X PUT --data-binary @'report.zip' '{url}'")
    );

    for (requested, expected) in [(0, 1), (120, 120), (99_999, 3600)] {
        let out = mcp
            .call(
                "jira_get_attachment_upload_url",
                json!({"issue_key": "SWS-1", "filename": "report.zip", "ttl_seconds": requested}),
            )
            .await;
        assert_eq!(out["expires_in_seconds"], expected, "ttl {requested}");
    }
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn upload_url_trims_key_and_rejects_blank_key() {
    let mcp = Mcp::start().await;
    let out = mcp
        .call(
            "jira_get_attachment_upload_url",
            json!({"issue_key": "  SWS-1  ", "filename": " report.zip "}),
        )
        .await;
    assert_eq!(out["issue_key"], "SWS-1");
    assert_eq!(out["filename"], "report.zip");

    let err = mcp
        .call_err(
            "jira_get_attachment_upload_url",
            json!({"issue_key": "   ", "filename": "report.zip"}),
        )
        .await;
    assert_eq!(err["code"], -32600);
    assert_eq!(err["message"], "issue_key and filename must not be empty");
}
