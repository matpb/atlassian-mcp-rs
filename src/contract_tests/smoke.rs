use base64::Engine;
use serde_json::json;

use super::harness::Mcp;

#[tokio::test]
async fn initialize_reports_server_name_and_tools_capability() {
    let mcp = Mcp::start().await;
    let resp = mcp
        .rpc(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "contract-test", "version": "0.0.0"}
            }),
        )
        .await;
    let result = &resp["result"];
    assert_eq!(result["serverInfo"]["name"], "atlassian-mcp");
    assert!(!result["capabilities"]["tools"].is_null());
    assert!(result["capabilities"].get("resources").is_none());
}

#[tokio::test]
async fn missing_credentials_give_32600_with_reason() {
    let mcp = Mcp::start().await;
    let jira = mcp
        .rpc_without_creds(
            "tools/call",
            json!({"name": "jira_get_issue", "arguments": {"issue_key": "SWS-1"}}),
        )
        .await;
    assert_eq!(jira["error"]["code"], -32600);
    assert_eq!(
        jira["error"]["data"]["reason"],
        "missing_atlassian_credential_headers"
    );

    let bitbucket = mcp
        .rpc_without_creds(
            "tools/call",
            json!({"name": "bitbucket_get_repository", "arguments": {"repo_slug": "r"}}),
        )
        .await;
    assert_eq!(bitbucket["error"]["code"], -32600);
    assert_eq!(
        bitbucket["error"]["data"]["reason"],
        "missing_bitbucket_credential_headers"
    );
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn jira_get_issue_hits_rest_v3_with_basic_auth() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/rest/api/3/issue/SWS-1",
        200,
        json!({
            "key": "SWS-1",
            "fields": {"summary": "Fix login", "status": {"name": "In Progress"}}
        }),
    );
    mcp.fake.on(
        "GET",
        "/rest/api/3/issue/SWS-1/comment",
        200,
        json!({"total": 0, "comments": []}),
    );

    let out = mcp
        .call("jira_get_issue", json!({"issue_key": "SWS-1"}))
        .await;
    assert_eq!(out["key"], "SWS-1");
    assert_eq!(out["summary"], "Fix login");

    let issue = mcp
        .fake
        .requests()
        .into_iter()
        .find(|r| r.path == "/rest/api/3/issue/SWS-1")
        .expect("issue request recorded");
    let basic = base64::engine::general_purpose::STANDARD.encode("tester@example.com:test-token");
    let auth = issue
        .headers
        .get("authorization")
        .expect("Authorization header")
        .to_str()
        .expect("ascii header");
    assert_eq!(auth, format!("Basic {basic}"));
}

#[tokio::test]
async fn jira_error_status_surfaces_in_error_message() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/rest/api/3/issue/SWS-1",
        404,
        json!({"errorMessages": ["Issue does not exist"]}),
    );
    let err = mcp
        .call_err("jira_get_issue", json!({"issue_key": "SWS-1"}))
        .await;
    assert_eq!(err["code"], -32603);
    assert_eq!(
        err["message"],
        r#"Jira returned 404 Not Found: {"errorMessages":["Issue does not exist"]}"#
    );
}

#[tokio::test]
async fn bitbucket_empty_204_becomes_ok_true() {
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
async fn bitbucket_uses_base_url_header_and_workspace() {
    let mcp = Mcp::start().await;
    mcp.fake
        .on("GET", "/2.0/repositories/ws/r", 200, json!({"slug": "r"}));

    let out = mcp
        .call("bitbucket_get_repository", json!({"repo_slug": "r"}))
        .await;
    assert_eq!(out["slug"], "r");

    let req = mcp.fake.only_request();
    assert_eq!(req.path, "/2.0/repositories/ws/r");
    let basic = base64::engine::general_purpose::STANDARD.encode("tester@example.com:bb-token");
    let auth = req
        .headers
        .get("authorization")
        .expect("Authorization header")
        .to_str()
        .expect("ascii header");
    assert_eq!(auth, format!("Basic {basic}"));
}

#[tokio::test]
async fn public_host_header_is_accepted() {
    let mcp = Mcp::start().await;
    // A reverse proxy forwards the public Host; rmcp rejects non-loopback hosts unless allowed_hosts is disabled.
    let response = reqwest::Client::new()
        .post(format!("{}/mcp", mcp.base_url))
        .header("Host", "mcp.example.com")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}))
        .send()
        .await
        .expect("send tools/list");
    assert_eq!(response.status().as_u16(), 200);
    let resp: serde_json::Value = response.json().await.expect("tools/list JSON");
    assert_eq!(resp["result"]["tools"].as_array().map(Vec::len), Some(61));
}

#[tokio::test]
async fn resources_and_prompts_are_not_advertised() {
    let mcp = Mcp::start().await;
    for method in ["resources/list", "prompts/list"] {
        let resp = mcp.rpc(method, json!({})).await;
        assert_eq!(resp["error"]["code"], -32601, "{method}: {resp:#}");
    }
}

#[tokio::test]
async fn input_errors_are_32602_and_upstream_errors_are_32603_across_products() {
    let mcp = Mcp::start().await;

    let jira = mcp
        .call_err("jira_get_issue", json!({"issue_key": "   "}))
        .await;
    assert_eq!(jira["code"], -32602);
    assert_eq!(jira["message"], "issue_key must not be empty");

    let confluence = mcp
        .call_err("confluence_search", json!({"cql": "   "}))
        .await;
    assert_eq!(confluence["code"], -32602);
    assert_eq!(confluence["message"], "cql must not be empty");

    let bitbucket = mcp
        .call_err("bitbucket_get_repository", json!({"repo_slug": "  "}))
        .await;
    assert_eq!(bitbucket["code"], -32602);
    assert_eq!(bitbucket["message"], "repo_slug must not be empty");
    assert!(mcp.fake.requests().is_empty());

    mcp.fake.on(
        "GET",
        "/rest/api/3/issue/SWS-1",
        500,
        json!({"errorMessages": ["boom"]}),
    );
    mcp.fake.on(
        "GET",
        "/wiki/rest/api/content/123",
        500,
        json!({"message": "boom"}),
    );
    mcp.fake.on(
        "GET",
        "/2.0/repositories/ws/r",
        500,
        json!({"error": {"message": "boom"}}),
    );

    let jira = mcp
        .call_err("jira_get_issue", json!({"issue_key": "SWS-1"}))
        .await;
    assert_eq!(jira["code"], -32603);
    assert_eq!(
        jira["message"],
        r#"Jira returned 500 Internal Server Error: {"errorMessages":["boom"]}"#
    );

    let confluence = mcp
        .call_err("confluence_get_page", json!({"page_id": "123"}))
        .await;
    assert_eq!(confluence["code"], -32603);
    assert_eq!(
        confluence["message"],
        r#"Confluence returned 500 Internal Server Error: {"message":"boom"}"#
    );

    let bitbucket = mcp
        .call_err("bitbucket_get_repository", json!({"repo_slug": "r"}))
        .await;
    assert_eq!(bitbucket["code"], -32603);
    assert_eq!(
        bitbucket["message"],
        r#"Bitbucket returned 500 Internal Server Error: {"error":{"message":"boom"}}"#
    );
}
