use http::request::Parts;
use reqwest::Client;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::confluence::ConfluenceClient;
use crate::credentials;
use crate::jira::JiraClient;

#[derive(Clone)]
pub struct AtlassianMcp {
    http: Client,
    tool_router: rmcp::handler::server::tool::ToolRouter<Self>,
}

impl AtlassianMcp {
    pub fn new(http: Client) -> Self {
        let tool_router = Self::tool_router();
        Self { http, tool_router }
    }

    fn resolve(parts: &Parts) -> Result<crate::credentials::AtlassianCredentials, ErrorData> {
        credentials::resolve_credentials(parts).map_err(|msg| {
            ErrorData::invalid_request(
                msg,
                Some(serde_json::json!({
                    "code": -32600,
                    "reason": "missing_atlassian_credential_headers"
                })),
            )
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetIssueParams {
    /// Jira issue key, e.g. `PROJ-123`
    issue_key: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AddCommentParams {
    /// Jira issue key, e.g. `PROJ-123`
    issue_key: String,
    /// Plain-text comment body (sent to Jira as Atlassian Document Format)
    comment: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct UpdateDescriptionParams {
    /// Jira issue key, e.g. `PROJ-123`
    issue_key: String,
    /// New description as plain text (stored as Atlassian Document Format)
    description: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchJqlParams {
    /// Jira Query Language string
    jql: String,
    /// Maximum issues to return (1–100, default 25)
    #[serde(default = "default_max_results")]
    max_results: u32,
    /// Pagination offset (default 0)
    #[serde(default)]
    start_at: u32,
}

fn default_max_results() -> u32 {
    25
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ConfluenceSearchParams {
    /// Confluence Query Language string, e.g. `type=page and space=TEAM`
    cql: String,
    /// Max hits (1–100, default 25)
    #[serde(default = "default_max_results")]
    limit: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ConfluenceGetPageParams {
    /// Confluence content ID (numeric string)
    page_id: String,
}

#[tool_router]
impl AtlassianMcp {
    #[tool(
        name = "jira_get_issue",
        description = "Fetch a Jira issue as a compact summary for an LLM: key, summary (title), plain-text description, status, attachments, and all comments. Requires X-Atlassian-Site-Url, X-Atlassian-Email, and X-Atlassian-Api-Token on every MCP HTTP request."
    )]
    async fn jira_get_issue(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<GetIssueParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let value = jira
            .get_issue_for_ai(&p.issue_key)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "jira_add_comment",
        description = "Add a plain-text comment to a Jira issue (posted as Atlassian Document Format)."
    )]
    async fn jira_add_comment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<AddCommentParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let value = jira
            .add_comment_plain(&p.issue_key, &p.comment)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "jira_update_description",
        description = "Replace the Jira issue description with plain text (converted to Atlassian Document Format)."
    )]
    async fn jira_update_description(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<UpdateDescriptionParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let value = jira
            .update_description_plain(&p.issue_key, &p.description)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "jira_search",
        description = "Search Jira issues using JQL via POST /rest/api/3/search/jql. Returns trimmed issues plus isLast and nextPageToken for cursor pagination (offset start_at is applied by fetching pages until enough issues are skipped)."
    )]
    async fn jira_search(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<SearchJqlParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let value = jira
            .search_jql(&p.jql, p.max_results, p.start_at)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "confluence_search",
        description = "Search Confluence content using CQL (Confluence Query Language). Returns id, title, type, excerpt, and web UI path hints."
    )]
    async fn confluence_search(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<ConfluenceSearchParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let cf = ConfluenceClient::new(self.http.clone(), &creds);
        let value = cf
            .search_cql(&p.cql, p.limit)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "confluence_get_page",
        description = "Fetch a Confluence page by content ID. Returns a trimmed JSON view: id, title, status, space (key/name), version, lastUpdated, body.storage (truncated if very long), and webui/tinyui links only."
    )]
    async fn confluence_get_page(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<ConfluenceGetPageParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let cf = ConfluenceClient::new(self.http.clone(), &creds);
        let value = cf
            .get_page(&p.page_id)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }
}

#[tool_handler]
impl ServerHandler for AtlassianMcp {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.instructions = Some(
            "Atlassian Cloud MCP (Jira REST v3 + Confluence REST). Every MCP HTTP request MUST include headers: X-Atlassian-Site-Url (e.g. https://company.atlassian.net), X-Atlassian-Email, X-Atlassian-Api-Token. Optional: X-Atlassian-Confluence-Site-Url when Confluence is on a different Atlassian Cloud site than Jira (otherwise same host + /wiki/rest/api is used). Missing headers yield JSON-RPC Invalid Request (-32600). Tools: jira_get_issue, jira_add_comment, jira_update_description, jira_search (JQL), confluence_search (CQL), confluence_get_page."
                .to_string(),
        );
        info
    }
}
