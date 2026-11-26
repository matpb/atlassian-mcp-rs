use std::fs;

use serde_json::json;

use super::harness::Mcp;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/contract_tests/fixtures/tools_list.json"
);

#[tokio::test]
async fn tools_list_matches_snapshot() {
    let mcp = Mcp::start().await;
    let resp = mcp.rpc("tools/list", json!({})).await;
    let mut tools = resp["result"]["tools"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("tools/list has no tools array: {resp:#}"));
    assert_eq!(tools.len(), 61, "expected 61 tools");
    tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));

    let mut actual = serde_json::to_string_pretty(&tools).expect("serialize tools");
    actual.push('\n');
    if std::env::var("UPDATE_TOOLS_SNAPSHOT").as_deref() == Ok("1") {
        fs::write(FIXTURE, &actual).expect("write tools snapshot");
        return;
    }
    let expected = fs::read_to_string(FIXTURE).expect("read tools snapshot");
    assert!(
        actual == expected,
        "tools/list differs from {FIXTURE}; rerun with UPDATE_TOOLS_SNAPSHOT=1 cargo test tools_list_matches_snapshot and review the diff"
    );
}
