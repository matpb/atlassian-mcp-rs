use http::request::Parts;
use reqwest::Client;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::bitbucket::BitbucketClient;
use crate::confluence::ConfluenceClient;
use crate::credentials;
use crate::jira::{self, JiraClient};

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

    fn resolve_bitbucket(
        parts: &Parts,
    ) -> Result<crate::credentials::BitbucketCredentials, ErrorData> {
        credentials::resolve_bitbucket_credentials(parts).map_err(|msg| {
            ErrorData::invalid_request(
                msg,
                Some(serde_json::json!({
                    "code": -32600,
                    "reason": "missing_bitbucket_credential_headers"
                })),
            )
        })
    }
}

fn bb_workspace_override(w: &Option<String>) -> Option<&str> {
    w.as_ref()
        .map(|s| s.as_str())
        .filter(|s| !s.trim().is_empty())
}

fn default_jira_content_format() -> String {
    "plain".to_string()
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetIssueParams {
    /// Jira issue key, e.g. `PROJ-123`
    issue_key: String,
    /// When true, include `description_adf` and each comment's `body_adf` (raw Atlassian Document Format JSON) for lossless round-trips. Default false returns only plain-text `description` and `body`.
    #[serde(default)]
    include_adf: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AddCommentParams {
    /// Jira issue key, e.g. `PROJ-123`
    issue_key: String,
    /// Comment body: plain text if `content_format` is `plain`, or a **JSON string** of a full ADF document `{"type":"doc","version":1,"content":[...]}` if `content_format` is `adf`
    comment: String,
    /// `plain` (default): each line becomes a paragraph. `adf`: `comment` must be valid ADF document JSON (headings, lists, links, code blocks, etc.).
    #[serde(default = "default_jira_content_format")]
    content_format: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct UpdateDescriptionParams {
    /// Jira issue key, e.g. `PROJ-123`
    issue_key: String,
    /// New description: plain text if `content_format` is `plain`, or ADF document JSON string if `content_format` is `adf`
    description: String,
    /// `plain` (default) or `adf` (full Atlassian Document Format JSON string)
    #[serde(default = "default_jira_content_format")]
    content_format: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CreateIssueParams {
    /// Project key, e.g. `PROJ`
    project_key: String,
    /// Issue type **name** as shown in Jira (e.g. `Task`, `Bug`, `Story`)
    issue_type: String,
    /// Issue summary (title)
    summary: String,
    /// Optional description; interpreted per `description_content_format`
    #[serde(default)]
    description: Option<String>,
    /// When `description` is set: `plain` (default) or `adf` (JSON string of full ADF document)
    #[serde(default = "default_jira_content_format")]
    description_content_format: String,
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
struct SearchUsersParams {
    /// Search term matched against user display name and email address
    query: String,
    /// Maximum users to return (1–50, default 10)
    #[serde(default = "default_user_search_max")]
    max_results: u32,
}

fn default_user_search_max() -> u32 {
    10
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListAttachmentsParams {
    /// Jira issue key, e.g. `PROJ-123`
    issue_key: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AddAttachmentParams {
    /// Jira issue key, e.g. `PROJ-123`
    issue_key: String,
    /// File name including extension, e.g. `report.pdf`
    filename: String,
    /// File content as a **base64-encoded** string
    file_base64: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DeleteAttachmentParams {
    /// Jira attachment ID (numeric string), as returned by `jira_list_attachments` or `jira_get_issue`
    attachment_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetTransitionsParams {
    /// Jira issue key, e.g. `PROJ-123`
    issue_key: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TransitionIssueParams {
    /// Jira issue key, e.g. `PROJ-123`
    issue_key: String,
    /// Transition ID (get available IDs from `jira_get_transitions`)
    transition_id: String,
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

fn default_body_format() -> String {
    "storage".to_string()
}

fn default_status() -> String {
    "current".to_string()
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ConfluenceCreatePageParams {
    /// Confluence space ID (numeric string) or space key (e.g. "SIKU")
    space_id: String,
    /// Page title
    title: String,
    /// Page body content (XHTML for "storage" format, or wiki markup for "wiki" format)
    body: String,
    /// Body format: "storage" (Confluence XHTML, default) or "wiki" (wiki markup)
    #[serde(default = "default_body_format")]
    body_format: String,
    /// Parent page ID (numeric string); omit to create a top-level page in the space
    #[serde(default)]
    parent_id: Option<String>,
    /// Page status: "current" (default) or "draft"
    #[serde(default = "default_status")]
    status: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbListReposParams {
    /// When set, use this workspace instead of `X-Bitbucket-Workspace`
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    pagelen: Option<u32>,
    #[serde(default)]
    page: Option<u32>,
    /// Bitbucket role filter, e.g. `member`
    #[serde(default)]
    role: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbRepoParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbListPrsParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    /// OPEN, MERGED, DECLINED, SUPERSEDED — omit for all active states per API defaults
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    pagelen: Option<u32>,
    #[serde(default)]
    page: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbPrIdParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    pull_request_id: u32,
    #[serde(default)]
    pagelen: Option<u32>,
    #[serde(default)]
    page: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbCreatePrParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    source_branch: String,
    destination_branch: String,
    #[serde(default)]
    close_source_branch: bool,
    /// Optional list of Bitbucket account UUIDs to add as reviewers.
    #[serde(default)]
    reviewers: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbPrCommentIdParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    pull_request_id: u32,
    comment_id: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbCreatePrCommentParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    pull_request_id: u32,
    /// Comment body in markdown (stored as `content.raw`)
    content: String,
    /// When set, this comment is a reply to an existing top-level comment
    #[serde(default)]
    parent_comment_id: Option<u64>,
    /// File path for an inline comment (requires `inline_to`)
    #[serde(default)]
    inline_path: Option<String>,
    /// End line for inline comment (requires `inline_path`)
    #[serde(default)]
    inline_to: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbReplyPrCommentParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    pull_request_id: u32,
    parent_comment_id: u64,
    content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbUpdatePrCommentParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    pull_request_id: u32,
    comment_id: u64,
    content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbListCommitsParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    /// Branch name, tag, or commit; default `HEAD`
    #[serde(default)]
    revision: Option<String>,
    #[serde(default)]
    pagelen: Option<u32>,
    #[serde(default)]
    page: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbGetCommitParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    commit: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbListBranchesParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    #[serde(default)]
    pagelen: Option<u32>,
    #[serde(default)]
    page: Option<u32>,
    /// Substring matched with Bitbucket `q=name~\"...\"`
    #[serde(default)]
    name_filter: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbSearchUsersParams {
    #[serde(default)]
    workspace: Option<String>,
    /// Search term matched against display name and nickname.
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    pagelen: Option<u32>,
    #[serde(default)]
    page: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbDeclinePrParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    pull_request_id: u32,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbMergePrParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    pull_request_id: u32,
    /// merge_commit, squash, or fast_forward
    #[serde(default)]
    merge_strategy: Option<String>,
    #[serde(default)]
    close_source_branch: Option<bool>,
    #[serde(default)]
    message: Option<String>,
}

#[tool_router]
impl AtlassianMcp {
    #[tool(
        name = "jira_get_issue",
        description = "Fetch a Jira issue for an LLM: key, summary, description and comments. By default description/comments are **lossy plain text** extracted from Atlassian Document Format (ADF). Set include_adf=true to also get description_adf and per-comment body_adf (raw ADF JSON) so you can edit rich content without losing structure. Requires X-Atlassian-Site-Url, X-Atlassian-Email, and X-Atlassian-Api-Token on every MCP HTTP request."
    )]
    async fn jira_get_issue(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<GetIssueParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let value = jira
            .get_issue_for_ai(&p.issue_key, p.include_adf)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "jira_add_comment",
        description = "Add a comment on a Jira issue. Jira stores the body as Atlassian Document Format (ADF). Use content_format=plain (default) for simple text (line breaks become paragraphs). Use content_format=adf and pass a JSON **string** of a full ADF document {\"type\":\"doc\",\"version\":1,\"content\":[...]} for headings, bullet/ordered lists, links, code blocks, mentions, etc. Prefer ADF over plain text for better structure, formatting, and @mention support. See Atlassian Jira ADF structure docs."
    )]
    async fn jira_add_comment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<AddCommentParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let body = jira::document_from_content_format(&p.content_format, &p.comment)
            .map_err(|e| ErrorData::invalid_params(e, None))?;
        let value = jira
            .add_comment_with_body(&p.issue_key, body)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "jira_update_description",
        description = "Replace the Jira issue description. Same content_format semantics as jira_add_comment: plain (default) wraps text as ADF paragraphs; adf accepts a JSON string of a full ADF document for rich formatting compatible with Jira Cloud REST API v3. Prefer ADF over plain text for better structure and formatting."
    )]
    async fn jira_update_description(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<UpdateDescriptionParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let doc = jira::document_from_content_format(&p.content_format, &p.description)
            .map_err(|e| ErrorData::invalid_params(e, None))?;
        let value = jira
            .update_description_with_adf(&p.issue_key, doc)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "jira_create_issue",
        description = "Create a Jira issue (POST /rest/api/3/issue): project_key, issue_type name (e.g. Task), summary, and optional description. Optional description uses description_content_format plain (default) or adf (JSON string of full ADF document). Same ADF rules as jira_add_comment. Prefer ADF over plain text for better structure and formatting."
    )]
    async fn jira_create_issue(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<CreateIssueParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let desc = match &p.description {
            None => None,
            Some(s) if s.trim().is_empty() => None,
            Some(s) => Some(
                jira::document_from_content_format(&p.description_content_format, s)
                    .map_err(|e| ErrorData::invalid_params(e, None))?,
            ),
        };
        let value = jira
            .create_issue(&p.project_key, &p.issue_type, &p.summary, desc)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "jira_get_transitions",
        description = "List available transitions for a Jira issue. Returns transition IDs and target status names. Use the transition ID with `jira_transition_issue` to move an issue to a new status."
    )]
    async fn jira_get_transitions(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<GetTransitionsParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let value = jira
            .get_transitions(&p.issue_key)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "jira_transition_issue",
        description = "Transition a Jira issue to a new status. Requires a transition_id obtained from `jira_get_transitions`. This changes the issue's workflow status (e.g. To Do → In Progress → Done)."
    )]
    async fn jira_transition_issue(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<TransitionIssueParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let value = jira
            .transition_issue(&p.issue_key, &p.transition_id)
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
        name = "jira_search_users",
        description = "Search for Jira users by name or email. Returns accountId (needed for ADF @mentions), displayName, emailAddress, and active status. Use accountId in ADF mention nodes: {\"type\":\"mention\",\"attrs\":{\"id\":\"<accountId>\",\"text\":\"@DisplayName\"}}."
    )]
    async fn jira_search_users(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<SearchUsersParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let value = jira
            .search_users(&p.query, p.max_results)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "jira_list_attachments",
        description = "List all attachments on a Jira issue. Returns attachment id, filename, mimeType, size, download URL, created date, and author."
    )]
    async fn jira_list_attachments(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<ListAttachmentsParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let value = jira
            .list_attachments(&p.issue_key)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "jira_add_attachment",
        description = "Upload a file attachment to a Jira issue. The file content must be provided as a base64-encoded string. Returns the created attachment metadata."
    )]
    async fn jira_add_attachment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<AddAttachmentParams>,
    ) -> Result<String, ErrorData> {
        use base64::Engine;
        let file_bytes = base64::engine::general_purpose::STANDARD
            .decode(&p.file_base64)
            .map_err(|e| {
                ErrorData::invalid_request(format!("Invalid base64 in file_base64: {e}"), None)
            })?;

        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let value = jira
            .add_attachment(&p.issue_key, &p.filename, file_bytes)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "jira_delete_attachment",
        description = "Delete an attachment from Jira by its attachment ID. Use jira_list_attachments or jira_get_issue to find attachment IDs."
    )]
    async fn jira_delete_attachment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<DeleteAttachmentParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let value = jira
            .delete_attachment(&p.attachment_id)
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

    #[tool(
        name = "confluence_create_page",
        description = "Create a new Confluence page via the v2 API. Body can be Confluence storage format (XHTML) or wiki markup. Returns the created page's id, title, status, and webui link."
    )]
    async fn confluence_create_page(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<ConfluenceCreatePageParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let cf = ConfluenceClient::new(self.http.clone(), &creds);
        let value = cf
            .create_page(
                &p.space_id,
                &p.title,
                &p.body,
                &p.body_format,
                p.parent_id.as_deref(),
                &p.status,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_list_repositories",
        description = "List repositories in a Bitbucket workspace (Cloud REST 2.0). Requires X-Bitbucket-Workspace, X-Bitbucket-Username, X-Bitbucket-App-Password; optional X-Bitbucket-Base-Url (default https://api.bitbucket.org/2.0). Optional `workspace` overrides the header."
    )]
    async fn bitbucket_list_repositories(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbListReposParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .list_repositories(
                bb_workspace_override(&p.workspace),
                p.pagelen,
                p.page,
                p.role.as_deref(),
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_get_repository",
        description = "Get repository metadata (Bitbucket Cloud REST 2.0 GET .../repositories/{workspace}/{repo_slug})."
    )]
    async fn bitbucket_get_repository(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbRepoParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .get_repository(bb_workspace_override(&p.workspace), &p.repo_slug)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_list_branches",
        description = "List branches in a repository (`refs/branches`). Optional `name_filter` uses Bitbucket query `name~\"...\"`."
    )]
    async fn bitbucket_list_branches(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbListBranchesParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .list_branches(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pagelen,
                p.page,
                p.name_filter.as_deref(),
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_list_pull_requests",
        description = "List pull requests for a repository. Optional `state`: OPEN, MERGED, DECLINED, SUPERSEDED."
    )]
    async fn bitbucket_list_pull_requests(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbListPrsParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .list_pull_requests(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.state.as_deref(),
                p.pagelen,
                p.page,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_get_pull_request",
        description = "Get a single pull request by id."
    )]
    async fn bitbucket_get_pull_request(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbPrIdParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .get_pull_request(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pull_request_id,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_create_pull_request",
        description = "Open a pull request from `source_branch` into `destination_branch` in the same repository. Optionally add `reviewers` (list of Bitbucket account UUIDs)."
    )]
    async fn bitbucket_create_pull_request(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbCreatePrParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .create_pull_request(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                &p.title,
                p.description.as_deref(),
                &p.source_branch,
                &p.destination_branch,
                p.close_source_branch,
                p.reviewers.as_deref(),
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_get_pull_request_diff",
        description = "Raw unified diff for a pull request (text). Returned JSON: { \"diff\": \"...\" }."
    )]
    async fn bitbucket_get_pull_request_diff(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbPrIdParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let diff = bb
            .get_pull_request_diff(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pull_request_id,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        let value = serde_json::json!({ "diff": diff });
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_get_pull_request_diffstat",
        description = "Per-file diff statistics for a pull request (JSON from Bitbucket `diffstat` endpoint)."
    )]
    async fn bitbucket_get_pull_request_diffstat(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbPrIdParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .get_pull_request_diffstat(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pull_request_id,
                p.pagelen,
                p.page,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_list_pull_request_comments",
        description = "List comments on a pull request (paginated; use `next` in the response to fetch more)."
    )]
    async fn bitbucket_list_pull_request_comments(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbPrIdParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .list_pull_request_comments(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pull_request_id,
                p.pagelen,
                p.page,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_get_pull_request_comment",
        description = "Fetch one pull request comment by id."
    )]
    async fn bitbucket_get_pull_request_comment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbPrCommentIdParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .get_pull_request_comment(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pull_request_id,
                p.comment_id,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_create_pull_request_comment",
        description = "Add a PR comment. Use `parent_comment_id` to reply, or `inline_path` + `inline_to` for an inline comment."
    )]
    async fn bitbucket_create_pull_request_comment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbCreatePrCommentParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .create_pull_request_comment(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pull_request_id,
                &p.content,
                p.parent_comment_id,
                p.inline_path.as_deref(),
                p.inline_to,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_reply_pull_request_comment",
        description = "Reply to an existing pull request comment (sets `parent` to `parent_comment_id`)."
    )]
    async fn bitbucket_reply_pull_request_comment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbReplyPrCommentParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .create_pull_request_comment(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pull_request_id,
                &p.content,
                Some(p.parent_comment_id),
                None,
                None,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_update_pull_request_comment",
        description = "Edit a pull request comment body (`content.raw`). Use this to apply revised text or fixups."
    )]
    async fn bitbucket_update_pull_request_comment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbUpdatePrCommentParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .update_pull_request_comment(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pull_request_id,
                p.comment_id,
                &p.content,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_delete_pull_request_comment",
        description = "Delete a pull request comment (requires permission)."
    )]
    async fn bitbucket_delete_pull_request_comment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbPrCommentIdParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .delete_pull_request_comment(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pull_request_id,
                p.comment_id,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_list_commits",
        description = "List commits reachable from `revision` (branch name, tag, or commit; default HEAD)."
    )]
    async fn bitbucket_list_commits(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbListCommitsParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .list_commits(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.revision.as_deref(),
                p.pagelen,
                p.page,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_get_commit",
        description = "Get a single commit by hash (`GET .../commit/{commit}`)."
    )]
    async fn bitbucket_get_commit(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbGetCommitParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .get_commit(bb_workspace_override(&p.workspace), &p.repo_slug, &p.commit)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_list_pull_request_activity",
        description = "Activity stream for a pull request (approvals, comments, updates, etc.)."
    )]
    async fn bitbucket_list_pull_request_activity(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbPrIdParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .list_pull_request_activity(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pull_request_id,
                p.pagelen,
                p.page,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_approve_pull_request",
        description = "Approve a pull request as the authenticated user."
    )]
    async fn bitbucket_approve_pull_request(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbPrIdParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .approve_pull_request(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pull_request_id,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_unapprove_pull_request",
        description = "Withdraw your approval on a pull request."
    )]
    async fn bitbucket_unapprove_pull_request(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbPrIdParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .unapprove_pull_request(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pull_request_id,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_decline_pull_request",
        description = "Decline (reject) a pull request. Optional `message`."
    )]
    async fn bitbucket_decline_pull_request(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbDeclinePrParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .decline_pull_request(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pull_request_id,
                p.message.as_deref(),
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_merge_pull_request",
        description = "Merge an open pull request. Optional `merge_strategy`: merge_commit, squash, fast_forward; optional `close_source_branch`, `message`."
    )]
    async fn bitbucket_merge_pull_request(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbMergePrParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .merge_pull_request(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pull_request_id,
                p.merge_strategy.as_deref(),
                p.close_source_branch,
                p.message.as_deref(),
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_search_users",
        description = "Search workspace members by display name or nickname. Returns user UUIDs, display names, and nicknames. Use this to look up reviewer UUIDs before adding them to a pull request."
    )]
    async fn bitbucket_search_users(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbSearchUsersParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .search_workspace_members(
                bb_workspace_override(&p.workspace),
                p.query.as_deref(),
                p.pagelen,
                p.page,
            )
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
            "Atlassian Cloud MCP: Jira REST v3 + Confluence REST + Bitbucket REST 2.0. Jira/Confluence: every MCP HTTP request MUST include X-Atlassian-Site-Url, X-Atlassian-Email, X-Atlassian-Api-Token (optional X-Atlassian-Confluence-Site-Url). Missing those yields -32600 with reason missing_atlassian_credential_headers. Bitbucket tools: send X-Bitbucket-Workspace, X-Bitbucket-Username, X-Bitbucket-App-Password on each request; optional X-Bitbucket-Base-Url (default https://api.bitbucket.org/2.0 for Cloud; set for Server/Data Center API roots). Missing Bitbucket headers yields -32600 with reason missing_bitbucket_credential_headers. Optional `workspace` on Bitbucket tool arguments overrides X-Bitbucket-Workspace."
                .to_string(),
        );
        info
    }
}
