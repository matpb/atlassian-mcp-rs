use serde_json::{Value, json};

use super::harness::{FakeAtlassian, Mcp};

const PAGE_PATH: &str = "/wiki/rest/api/content/123";
const SPACES_PATH: &str = "/wiki/api/v2/spaces";
const PAGES_PATH: &str = "/wiki/api/v2/pages";

fn runbook_page() -> Value {
    json!({
        "id": "123",
        "type": "page",
        "status": "current",
        "title": "Deploy runbook",
        "ari": "ari:cloud:confluence:noise",
        "space": {"id": 1, "key": "OPS", "name": "Operations", "ari": "x", "_links": {}},
        "version": {
            "number": 4,
            "when": "2026-03-01T10:00:00.000Z",
            "message": "",
            "by": {"displayName": "Pat"},
            "minorEdit": false
        },
        "history": {
            "lastUpdated": {
                "when": "2026-03-01T10:00:00.000Z",
                "by": {"displayName": "Pat", "email": "pat@example.com", "accountId": "acc-1"}
            }
        },
        "body": {"storage": {"representation": "storage", "value": "<p>Restart the queue.</p>"}},
        "_links": {
            "webui": "/spaces/OPS/pages/123/Deploy+runbook",
            "tinyui": "/x/AbC",
            "self": "https://example.com/wiki/rest/api/content/123",
            "editui": "/e"
        },
        "_expandable": {"children": "/c"}
    })
}

#[tokio::test]
async fn confluence_search_sends_cql_and_default_limit_and_slims_hits() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/wiki/rest/api/search",
        200,
        json!({
            "results": [{
                "content": {
                    "id": "111",
                    "type": "page",
                    "status": "current",
                    "title": "Runbook",
                    "version": {"number": 3},
                    "space": {"key": "OPS"},
                    "_links": {"webui": "/spaces/OPS/pages/111/Runbook", "self": "https://example.com/x"}
                },
                "title": "Runbook",
                "excerpt": "Restart the <span>queue</span>",
                "url": "/spaces/OPS/pages/111/Runbook",
                "entityType": "content",
                "lastModified": "2026-01-02T03:04:05.000Z",
                "score": 1.0
            }],
            "start": 0,
            "limit": 10,
            "size": 1,
            "totalSize": 1,
            "_links": {"base": "https://example.com/wiki"}
        }),
    );

    let cql = r#"type = page AND title ~ "deploy" & space = "OPS""#;
    let out = mcp.call("confluence_search", json!({"cql": cql})).await;
    assert_eq!(
        out,
        json!({
            "size": 1,
            "limit": 25,
            "results": [{
                "id": "111",
                "type": "page",
                "status": "current",
                "title": "Runbook",
                "excerpt": "Restart the <span>queue</span>",
                "webui": "/spaces/OPS/pages/111/Runbook"
            }]
        })
    );

    let req = mcp.fake.only_request();
    assert_eq!(req.path, "/wiki/rest/api/search");
    assert_eq!(
        req.query_pairs(),
        vec![
            ("cql".to_string(), cql.to_string()),
            ("limit".to_string(), "25".to_string()),
        ]
    );
}

#[tokio::test]
async fn confluence_search_numeric_id_and_blank_body() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/wiki/rest/api/search",
        200,
        json!({"results": [{"content": {"id": 444, "type": "page"}, "title": "Numeric"}]}),
    );
    let out = mcp
        .call("confluence_search", json!({"cql": "type = page"}))
        .await;
    assert_eq!(
        out["results"],
        json!([{
            "id": "444",
            "type": "page",
            "status": null,
            "title": "Numeric",
            "excerpt": null,
            "webui": null
        }])
    );

    mcp.fake.on_raw("GET", "/wiki/rest/api/search", 200, " \n");
    let out = mcp
        .call("confluence_search", json!({"cql": "type = page"}))
        .await;
    assert_eq!(out, json!({"size": null, "limit": 25, "results": []}));
}

#[tokio::test]
async fn confluence_search_limit_bounds_pass_and_out_of_range_is_rejected() {
    let mcp = Mcp::start().await;
    mcp.fake
        .on("GET", "/wiki/rest/api/search", 200, json!({"results": []}));

    let high = mcp
        .call(
            "confluence_search",
            json!({"cql": "type = page", "limit": 100}),
        )
        .await;
    let low = mcp
        .call(
            "confluence_search",
            json!({"cql": "type = page", "limit": 1}),
        )
        .await;
    assert_eq!(high, json!({"size": null, "limit": 100, "results": []}));
    assert_eq!(low, json!({"size": null, "limit": 1, "results": []}));

    for bad in [0, 101] {
        let err = mcp
            .call_err(
                "confluence_search",
                json!({"cql": "type = page", "limit": bad}),
            )
            .await;
        assert_eq!(err["code"], -32602);
        assert_eq!(
            err["message"],
            format!("limit must be between 1 and 100, got {bad}")
        );
    }
    assert_eq!(mcp.fake.requests().len(), 2);

    let sent: Vec<Vec<(String, String)>> = mcp
        .fake
        .requests()
        .iter()
        .map(|r| r.query_pairs())
        .collect();
    let limit_of = |pairs: &Vec<(String, String)>| {
        pairs
            .iter()
            .find(|(k, _)| k == "limit")
            .map(|(_, v)| v.clone())
    };
    assert_eq!(limit_of(&sent[0]).as_deref(), Some("100"));
    assert_eq!(limit_of(&sent[1]).as_deref(), Some("1"));
}

#[tokio::test]
async fn confluence_search_empty_cql_is_rejected_before_any_request() {
    let mcp = Mcp::start().await;
    let err = mcp
        .call_err("confluence_search", json!({"cql": "   "}))
        .await;
    assert_eq!(err["code"], -32602);
    assert_eq!(err["message"], "cql must not be empty");
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn confluence_get_page_requests_expand_and_returns_slim_view() {
    let mcp = Mcp::start().await;
    mcp.fake.on("GET", PAGE_PATH, 200, runbook_page());

    let out = mcp
        .call("confluence_get_page", json!({"page_id": "123"}))
        .await;
    assert_eq!(
        out,
        json!({
            "id": "123",
            "type": "page",
            "title": "Deploy runbook",
            "status": "current",
            "space": {"key": "OPS", "name": "Operations"},
            "version": {
                "number": 4,
                "when": "2026-03-01T10:00:00.000Z",
                "message": ""
            },
            "lastUpdated": {
                "when": "2026-03-01T10:00:00.000Z",
                "by": {"displayName": "Pat", "email": "pat@example.com"}
            },
            "body": {
                "representation": "storage",
                "value": "<p>Restart the queue.</p>",
                "char_count_total": 25,
                "char_count_included": 25,
                "truncated": false
            },
            "links": {
                "webui": "/spaces/OPS/pages/123/Deploy+runbook",
                "tinyui": "/x/AbC"
            }
        })
    );

    let req = mcp.fake.only_request();
    assert_eq!(req.method, "GET");
    assert_eq!(req.path, PAGE_PATH);
    assert_eq!(
        req.query_pairs(),
        vec![(
            "expand".to_string(),
            "body.storage,version,space,history.lastUpdated".to_string()
        )]
    );
}

#[tokio::test]
async fn confluence_get_page_truncates_long_body_by_characters() {
    let mcp = Mcp::start().await;
    // 120_001 two-byte characters: the cut must land on a character, not a byte.
    let long = "é".repeat(120_001);
    let mut page = runbook_page();
    page["body"]["storage"]["value"] = json!(long);
    mcp.fake.on("GET", PAGE_PATH, 200, page);

    let out = mcp
        .call("confluence_get_page", json!({"page_id": "123"}))
        .await;
    let expected_value = format!(
        "{}\n\n[… truncated for MCP: included 120000 of 120001 characters]",
        "é".repeat(120_000)
    );
    assert_eq!(out["body"]["value"], expected_value);
    assert_eq!(out["body"]["char_count_total"], 120_001);
    assert_eq!(out["body"]["char_count_included"], 120_000);
    assert_eq!(out["body"]["truncated"], true);
}

#[tokio::test]
async fn confluence_get_page_percent_encodes_page_id() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        "/wiki/rest/api/content/42%20%2Fx%3Fy%25",
        200,
        json!({"id": "42", "title": "Odd id"}),
    );

    let out = mcp
        .call("confluence_get_page", json!({"page_id": "42 /x?y%"}))
        .await;
    assert_eq!(out["title"], "Odd id");
    assert_eq!(
        mcp.fake.only_request().path,
        "/wiki/rest/api/content/42%20%2Fx%3Fy%25"
    );
}

#[tokio::test]
async fn confluence_get_page_error_status_reports_code_and_body() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        PAGE_PATH,
        404,
        json!({"message": "No content found"}),
    );

    let err = mcp
        .call_err("confluence_get_page", json!({"page_id": "123"}))
        .await;
    assert_eq!(err["code"], -32603);
    assert_eq!(
        err["message"],
        r#"Confluence returned 404 Not Found: {"message":"No content found"}"#
    );
}

#[tokio::test]
async fn confluence_get_page_empty_204_is_slimmed_from_ok() {
    // An empty body becomes {"ok": true}; get_page then slims it, so every page field is null.
    let mcp = Mcp::start().await;
    mcp.fake.on_raw("GET", PAGE_PATH, 204, "");

    let out = mcp
        .call("confluence_get_page", json!({"page_id": "123"}))
        .await;
    assert_eq!(
        out,
        json!({
            "id": null,
            "type": null,
            "title": null,
            "status": null,
            "space": null,
            "version": null,
            "lastUpdated": null,
            "body": {
                "representation": null,
                "value": "",
                "char_count_total": 0,
                "char_count_included": 0,
                "truncated": false
            },
            "links": {"webui": null, "tinyui": null}
        })
    );
}

#[tokio::test]
async fn confluence_create_page_resolves_space_key_then_posts_payload() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "GET",
        SPACES_PATH,
        200,
        json!({"results": [{"id": "98765", "key": "DOCS", "name": "Docs"}]}),
    );
    mcp.fake.on(
        "POST",
        PAGES_PATH,
        200,
        json!({
            "id": "1234",
            "title": "Runbook",
            "status": "current",
            "spaceId": "98765",
            "_links": {"webui": "/spaces/DOCS/pages/1234/Runbook", "base": "https://example.com/wiki"}
        }),
    );

    let out = mcp
        .call(
            "confluence_create_page",
            json!({
                "space_id": "DOCS",
                "title": "Runbook",
                "body": "<p>Hi</p>",
                "parent_id": "555"
            }),
        )
        .await;
    assert_eq!(
        out,
        json!({
            "id": "1234",
            "title": "Runbook",
            "status": "current",
            "webui": "/spaces/DOCS/pages/1234/Runbook"
        })
    );

    let requests = mcp.fake.requests();
    let seen: Vec<(String, String)> = requests
        .iter()
        .map(|r| (r.method.clone(), r.path.clone()))
        .collect();
    assert_eq!(
        seen,
        vec![
            ("GET".to_string(), SPACES_PATH.to_string()),
            ("POST".to_string(), PAGES_PATH.to_string()),
        ]
    );
    assert_eq!(
        requests[0].query_pairs(),
        vec![("keys".to_string(), "DOCS".to_string())]
    );
    assert_eq!(
        requests[1].json(),
        json!({
            "spaceId": "98765",
            "status": "current",
            "title": "Runbook",
            "body": {"representation": "storage", "value": "<p>Hi</p>"},
            "parentId": "555"
        })
    );
}

#[tokio::test]
async fn confluence_create_page_numeric_space_draft_wiki_skips_lookup() {
    let mcp = Mcp::start().await;
    mcp.fake.on(
        "POST",
        PAGES_PATH,
        200,
        json!({"id": "7", "title": "Notes", "status": "draft"}),
    );

    let out = mcp
        .call(
            "confluence_create_page",
            json!({
                "space_id": "98765",
                "title": "Notes",
                "body": "h1. Heading",
                "body_format": "wiki",
                "status": "draft"
            }),
        )
        .await;
    assert_eq!(
        out,
        json!({"id": "7", "title": "Notes", "status": "draft", "webui": null})
    );

    let req = mcp.fake.only_request();
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, PAGES_PATH);
    assert_eq!(
        req.json(),
        json!({
            "spaceId": "98765",
            "status": "draft",
            "title": "Notes",
            "body": {"representation": "wiki", "value": "h1. Heading"}
        })
    );
}

#[tokio::test]
async fn confluence_create_page_unknown_body_format_is_rejected() {
    let mcp = Mcp::start().await;
    let err = mcp
        .call_err(
            "confluence_create_page",
            json!({"space_id": "98765", "title": "Notes", "body": "x", "body_format": "markdown"}),
        )
        .await;
    assert_eq!(err["code"], -32602);
    assert_eq!(
        err["message"],
        r#"body_format must be "storage" or "wiki", got "markdown""#
    );
    assert!(mcp.fake.requests().is_empty());
}

#[tokio::test]
async fn confluence_create_page_unknown_space_key_is_reported() {
    let mcp = Mcp::start().await;
    mcp.fake.on("GET", SPACES_PATH, 200, json!({"results": []}));

    let err = mcp
        .call_err(
            "confluence_create_page",
            json!({"space_id": "NOPE", "title": "Notes", "body": "x"}),
        )
        .await;
    assert_eq!(err["code"], -32603);
    assert_eq!(err["message"], r#"Space "NOPE" not found"#);
    assert_eq!(mcp.fake.only_request().path, SPACES_PATH);
}

#[tokio::test]
async fn confluence_create_page_encodes_space_key_in_lookup() {
    let mcp = Mcp::start().await;
    mcp.fake.on("GET", SPACES_PATH, 200, json!({"results": []}));

    let _ = mcp
        .call_err(
            "confluence_create_page",
            json!({"space_id": "D O?S", "title": "Notes", "body": "x"}),
        )
        .await;
    let req = mcp.fake.only_request();
    assert_eq!(
        req.query_pairs(),
        vec![("keys".to_string(), "D O?S".to_string())]
    );
    assert_eq!(req.query.as_deref(), Some("keys=D%20O%3FS"));
}

#[tokio::test]
async fn confluence_site_override_header_sends_wiki_calls_to_second_host() {
    let mcp = Mcp::start().await;
    let wiki = FakeAtlassian::start().await;
    wiki.on("GET", PAGE_PATH, 200, runbook_page());

    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "confluence_get_page", "arguments": {"page_id": "123"}}
    });
    let resp: Value = reqwest::Client::new()
        .post(format!("{}/mcp", mcp.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("X-Atlassian-Site-Url", mcp.fake.base_url.as_str())
        .header("X-Atlassian-Email", "tester@example.com")
        .header("X-Atlassian-Api-Token", "test-token")
        .header("X-Atlassian-Confluence-Site-Url", wiki.base_url.as_str())
        .json(&body)
        .send()
        .await
        .expect("send tools/call")
        .json()
        .await
        .expect("tools/call JSON");
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no tool text: {resp:#}"));
    let out: Value = serde_json::from_str(text).expect("tool output JSON");
    assert_eq!(out["title"], "Deploy runbook");

    assert!(mcp.fake.requests().is_empty());
    assert_eq!(wiki.only_request().path, PAGE_PATH);
}

#[tokio::test]
async fn confluence_create_page_validates_before_space_lookup() {
    let mcp = Mcp::start().await;
    for args in [
        json!({"space_id": "DOCS", "title": "  ", "body": "x"}),
        json!({"space_id": "DOCS", "title": "Notes", "body": "x", "body_format": "markdown"}),
    ] {
        let err = mcp.call_err("confluence_create_page", args).await;
        assert_eq!(err["code"], -32602);
    }
    assert!(mcp.fake.requests().is_empty());
}
