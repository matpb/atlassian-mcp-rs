use base64::Engine;
use serde_json::json;

use super::harness::Mcp;

const ISSUE_ATTACHMENTS: &str = "/rest/api/3/issue/SWS-1/attachments";

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

async fn upload_url(mcp: &Mcp, issue_key: &str, filename: &str) -> String {
    let out = mcp
        .call(
            "jira_get_attachment_upload_url",
            json!({"issue_key": issue_key, "filename": filename}),
        )
        .await;
    out["upload_url"]
        .as_str()
        .expect("upload_url is a string")
        .to_string()
}

async fn put(url: &str, body: &[u8]) -> (u16, String) {
    let resp = reqwest::Client::new()
        .put(url)
        .body(body.to_vec())
        .send()
        .await
        .expect("PUT upload");
    let status = resp.status().as_u16();
    (status, resp.text().await.expect("read upload response"))
}

#[tokio::test]
async fn upload_happy_path_posts_multipart_to_jira() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        ISSUE_ATTACHMENTS,
        200,
        json!([{"id": "10", "filename": "a b.png", "size": 5}]),
    );
    let url = upload_url(&mcp, "SWS-1", "a b.png").await;

    let (status, body) = put(&url, b"hello").await;
    assert_eq!(status, 200);
    let out: serde_json::Value = serde_json::from_str(&body).expect("upload response JSON");
    assert_eq!(out["ok"], true);
    assert_eq!(out["attachments"][0]["id"], "10");
    assert_eq!(out["attachments"][0]["size"], 5);

    let req = mcp.fake.only_request();
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, ISSUE_ATTACHMENTS);
    let token = req
        .headers
        .get("x-atlassian-token")
        .expect("X-Atlassian-Token header")
        .to_str()
        .expect("ascii header");
    assert_eq!(token, "no-check");
    let auth = req
        .headers
        .get("authorization")
        .expect("Authorization header")
        .to_str()
        .expect("ascii header");
    let basic = base64::engine::general_purpose::STANDARD.encode("tester@example.com:test-token");
    assert_eq!(auth, format!("Basic {basic}"));
    assert!(contains(&req.body, br#"filename="a b.png""#));
    assert!(contains(&req.body, b"hello"));
}

#[tokio::test]
async fn upload_token_is_single_use() {
    let mcp = Mcp::start().await;
    mcp.fake
        .on("POST", ISSUE_ATTACHMENTS, 200, json!([{"id": "10"}]));
    let url = upload_url(&mcp, "SWS-1", "a.png").await;

    let (first, _) = put(&url, b"hello").await;
    assert_eq!(first, 200);
    let (second, body) = put(&url, b"hello").await;
    assert_eq!(second, 404);
    assert_eq!(
        body,
        "Unknown, expired or already used upload token. Call jira_get_attachment_upload_url again."
    );
    assert_eq!(mcp.fake.requests().len(), 1);
}

#[tokio::test]
async fn upload_unknown_token_is_404() {
    let mcp = Mcp::start().await;
    let (status, body) = put(
        &format!("{}/upload/not-a-real-token", mcp.base_url),
        b"hello",
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(
        body,
        "Unknown, expired or already used upload token. Call jira_get_attachment_upload_url again."
    );
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn upload_empty_body_is_400() {
    let mcp = Mcp::start().await;
    let url = upload_url(&mcp, "SWS-1", "a.png").await;
    let (status, body) = put(&url, b"").await;
    assert_eq!(status, 400);
    assert_eq!(body, "Empty request body");
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn upload_over_limit_is_413_and_burns_ticket() {
    let mcp = Mcp::start_with_max_upload(4).await;
    let url = upload_url(&mcp, "SWS-1", "a.png").await;

    let (status, body) = put(&url, b"hello").await;
    assert_eq!(status, 413);
    assert_eq!(
        body,
        "Upload rejected (length limit exceeded); limit is 4 bytes"
    );

    // Pins current behaviour: the ticket is consumed before the size check.
    let (retry, _) = put(&url, b"hi").await;
    assert_eq!(retry, 404);
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn upload_jira_error_is_502() {
    let mcp = Mcp::start().await;
    mcp.fake.on_raw("POST", ISSUE_ATTACHMENTS, 500, "boom");
    let url = upload_url(&mcp, "SWS-1", "a.png").await;

    let (status, body) = put(&url, b"hello").await;
    assert_eq!(status, 502);
    assert_eq!(body, "Jira returned 500 Internal Server Error: boom");
    assert_eq!(mcp.fake.only_request().path, ISSUE_ATTACHMENTS);
}

#[tokio::test]
async fn upload_url_tool_rejects_blank_filename() {
    let mcp = Mcp::start().await;
    let err = mcp
        .call_err(
            "jira_get_attachment_upload_url",
            json!({"issue_key": "SWS-1", "filename": "   "}),
        )
        .await;
    assert_eq!(err["code"], -32600);
    assert_eq!(err["message"], "issue_key and filename must not be empty");
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn upload_url_curl_hint_is_shell_quoted() {
    let mcp = Mcp::start().await;
    let out = mcp
        .call(
            "jira_get_attachment_upload_url",
            json!({"issue_key": "SWS-1", "filename": "it's.png"}),
        )
        .await;
    let url = out["upload_url"].as_str().expect("upload_url is a string");
    assert_eq!(
        out["curl"],
        format!("curl -sS -X PUT --data-binary @'it'\\''s.png' '{url}'")
    );
}
