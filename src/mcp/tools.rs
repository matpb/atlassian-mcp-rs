use http::request::Parts;
use reqwest::Client;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::bitbucket::{BitbucketClient, PipelineVariable};
use crate::confluence::ConfluenceClient;
use crate::credentials;
use crate::jira::{self, JiraClient};
use crate::upload::{self, UploadTickets};

#[derive(Clone)]
pub struct AtlassianMcp {
    http: Client,
    tickets: UploadTickets,
    public_url: Option<String>,
    tool_router: rmcp::handler::server::tool::ToolRouter<Self>,
}

impl AtlassianMcp {
    pub fn new(http: Client, tickets: UploadTickets, public_url: Option<String>) -> Self {
        let tool_router = Self::tool_router();
        Self {
            http,
            tickets,
            public_url,
            tool_router,
        }
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
struct UpdateSummaryParams {
    /// Jira issue key, e.g. `PROJ-123`
    issue_key: String,
    /// New summary (title), plain text on one line
    summary: String,
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
struct GetAttachmentUploadUrlParams {
    /// Jira issue key, e.g. `PROJ-123`
    issue_key: String,
    /// File name including extension, e.g. `report.zip`
    filename: String,
    /// Seconds the upload URL stays valid. Default 600, max 3600.
    ttl_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DeleteAttachmentParams {
    /// Jira attachment ID (numeric string), as returned by `jira_list_attachments` or `jira_get_issue`
    attachment_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetChangelogParams {
    /// Jira issue key, e.g. `PROJ-123`
    issue_key: String,
    /// Only return changes to this field, e.g. `status`, `assignee`, `Sprint`. Omit for every field.
    #[serde(default)]
    field: Option<String>,
    /// Maximum change entries to return (1–100, default 50)
    #[serde(default = "default_changelog_max")]
    max_results: u32,
    /// Pagination offset into the changelog (default 0, oldest first)
    #[serde(default)]
    start_at: u32,
}

fn default_changelog_max() -> u32 {
    50
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
struct SetAssigneeParams {
    /// Jira issue key, e.g. `PROJ-123`
    issue_key: String,
    /// Atlassian accountId of the user to assign. Obtain from `jira_search_users`.
    /// Pass `"-1"` to use the project's default assignee.
    /// Omit or pass `null` to unassign the issue.
    #[serde(default)]
    account_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListBoardsParams {
    /// Restrict to boards for this project key, e.g. `PROJ`. Omit for all boards.
    #[serde(default)]
    project_key: Option<String>,
    /// Maximum boards to return (1-50, default 50)
    #[serde(default)]
    max_results: Option<u32>,
    /// Pagination offset (default 0)
    #[serde(default)]
    start_at: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BoardIssuesParams {
    /// Jira Agile board ID, from `jira_list_boards`
    board_id: u64,
    /// `"board"` (on the board, default), `"backlog"` (backlog only), or `"all"` (board + backlog combined)
    #[serde(default)]
    scope: Option<String>,
    /// Include issues whose status category is Done (default false)
    #[serde(default)]
    include_done: Option<bool>,
    /// Extra JQL filter, ANDed with the done-status filter
    #[serde(default)]
    jql: Option<String>,
    /// Maximum issues to return (1-100, default 100)
    #[serde(default)]
    max_results: Option<u32>,
    /// Pagination offset into the final result list (default 0)
    #[serde(default)]
    start_at: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct MoveBoardIssuesParams {
    /// Jira Agile board ID
    board_id: u64,
    /// Issue keys to move, e.g. `["PROJ-1","PROJ-2"]`. 1-50 keys per call (Jira's cap).
    issue_keys: Vec<String>,
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
struct BbUpdatePrParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    pull_request_id: u32,
    /// Full replacement PR description in markdown; it overwrites the existing body
    /// rather than appending to it. Empty string clears the body, omit to keep it as-is.
    #[serde(default)]
    description: Option<String>,
    /// Full replacement PR title. Omit to leave the title unchanged.
    #[serde(default)]
    title: Option<String>,
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

#[derive(Debug, Deserialize, JsonSchema)]
struct BbListPipelinesParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    /// Page size (1–100, default 10). Newest first.
    #[serde(default)]
    pagelen: Option<u32>,
    #[serde(default)]
    page: Option<u32>,
    /// Keep only pipelines whose target branch/tag equals this name. Filters the fetched page, so raise `pagelen` if the branch is quiet.
    #[serde(default)]
    target_branch: Option<String>,
    /// Return Bitbucket's full pipeline objects instead of the compact summary.
    #[serde(default)]
    raw: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbPipelineParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    /// Pipeline build number (e.g. `142`) or UUID including braces (e.g. `{a1b2c3d4-...}`).
    pipeline: String,
    /// Return Bitbucket's full object instead of the compact summary.
    #[serde(default)]
    raw: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbStopPipelineParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    /// Pipeline build number or UUID including braces.
    pipeline: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbPipelineStepLogParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    /// Pipeline build number or UUID including braces.
    pipeline: String,
    /// Step UUID including braces, from `bitbucket_list_pipeline_steps`.
    step_uuid: String,
    /// Keep only the last N characters of the log (default 20000). The response reports the total size and whether the head was cut.
    #[serde(default)]
    tail_chars: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbTriggerPipelineParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    /// Branch or tag name to run the pipeline on, e.g. `master`.
    ref_name: String,
    /// `branch` (default) or `tag`.
    #[serde(default)]
    ref_type: Option<String>,
    /// Pin the run to a specific commit hash on that ref instead of its current head.
    #[serde(default)]
    commit_hash: Option<String>,
    /// Name of a `custom:` pipeline from bitbucket-pipelines.yml. Omit to run the ref's default pipeline.
    #[serde(default)]
    custom_pipeline: Option<String>,
    /// Variables for the run: `[{"key": "X", "value": "y", "secured": false}]`. Only variables declared on a custom pipeline are honoured.
    #[serde(default)]
    variables: Option<Vec<PipelineVariable>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbListDeploymentsParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    #[serde(default)]
    pagelen: Option<u32>,
    #[serde(default)]
    page: Option<u32>,
    /// Return Bitbucket's full deployment objects instead of the compact summary.
    #[serde(default)]
    raw: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbListEnvironmentsParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    /// Return Bitbucket's full environment objects instead of the compact summary.
    #[serde(default)]
    raw: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbListVariablesParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    /// Deployment environment name, slug or UUID. Omit for repository-level variables.
    #[serde(default)]
    environment: Option<String>,
    #[serde(default)]
    pagelen: Option<u32>,
    #[serde(default)]
    page: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbCreateVariableParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    /// Deployment environment name, slug or UUID. Omit for a repository-level variable.
    #[serde(default)]
    environment: Option<String>,
    key: String,
    value: String,
    /// Secured variables are write-only: Bitbucket never returns the value again.
    #[serde(default)]
    secured: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbUpdateVariableParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    /// Deployment environment name, slug or UUID. Omit for a repository-level variable.
    #[serde(default)]
    environment: Option<String>,
    /// Existing variable, by key name or UUID including braces.
    variable: String,
    /// New key name. Omit to keep.
    #[serde(default)]
    key: Option<String>,
    /// New value. Required when the variable is secured, since its current value cannot be read back.
    #[serde(default)]
    value: Option<String>,
    /// Change the secured flag. Omit to keep.
    #[serde(default)]
    secured: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BbDeleteVariableParams {
    #[serde(default)]
    workspace: Option<String>,
    repo_slug: String,
    /// Deployment environment name, slug or UUID. Omit for a repository-level variable.
    #[serde(default)]
    environment: Option<String>,
    /// Existing variable, by key name or UUID including braces.
    variable: String,
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
        name = "jira_update_summary",
        description = "Replace a Jira issue's summary (title) via PUT /rest/api/3/issue/{key}. Plain text, one line; surrounding whitespace is trimmed."
    )]
    async fn jira_update_summary(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<UpdateSummaryParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let value = jira
            .update_summary(&p.issue_key, &p.summary)
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
        name = "jira_get_issue_history",
        description = "Fetch a Jira issue's change history (changelog): who changed what field, from what value to what, and when. Optional `field` narrows it to one field, e.g. `status` to get the full workflow timeline or `assignee` to see reassignments. Timestamps are returned verbatim from Jira, including its UTC offset. Use this rather than JQL when you need to prove who transitioned an issue and when."
    )]
    async fn jira_get_issue_history(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<GetChangelogParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let value = jira
            .get_changelog(&p.issue_key, p.field.as_deref(), p.max_results, p.start_at)
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
        name = "jira_set_assignee",
        description = "Set the assignee for a Jira issue via PUT /rest/api/3/issue/{key}/assignee. Pass the assignee's Atlassian accountId (look up with `jira_search_users`). Pass `\"-1\"` to use the project's default assignee. Omit `account_id` (or pass null) to unassign the issue."
    )]
    async fn jira_set_assignee(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<SetAssigneeParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let value = jira
            .set_assignee(&p.issue_key, p.account_id.as_deref())
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "jira_list_boards",
        description = "List Jira Agile boards (GET /rest/agile/1.0/board), optionally filtered to one project. Use the returned board `id` with jira_board_issues, jira_move_to_board and jira_move_to_backlog."
    )]
    async fn jira_list_boards(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<ListBoardsParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        let max = p.max_results.unwrap_or(50).clamp(1, 50);
        let raw = jira
            .list_boards(p.project_key.as_deref(), max, p.start_at.unwrap_or(0))
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        let boards: Vec<Value> = raw
            .get("values")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|b| {
                        serde_json::json!({
                            "id": b.get("id"),
                            "name": b.get("name"),
                            "type": b.get("type"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let out = serde_json::json!({
            "boards": boards,
            "isLast": raw.get("isLast"),
            "total": raw.get("total"),
        });
        serde_json::to_string_pretty(&out)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "jira_board_issues",
        description = "List a Jira Agile board's issues, split by scope: \"board\" (default, issues on the board's columns), \"backlog\" (issues in the board's backlog), or \"all\" (board + backlog combined). On a team-managed board, backlog membership has NO JQL equivalent -- `/board/{id}/issue` returns backlog issues too, so scope=\"board\" is computed as the set difference of that endpoint's keys minus `/board/{id}/backlog`'s keys. include_done=false (default) adds `statusCategory != Done` to the JQL. start_at/max_results paginate the FINAL list, after scope filtering."
    )]
    async fn jira_board_issues(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BoardIssuesParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);

        let scope = p.scope.as_deref().unwrap_or("board");
        if !matches!(scope, "board" | "backlog" | "all") {
            return Err(ErrorData::invalid_params(
                format!(
                    "Invalid scope '{scope}'. Must be one of: \"board\", \"backlog\", \"all\"."
                ),
                None,
            ));
        }

        let include_done = p.include_done.unwrap_or(false);
        let effective_jql = build_effective_jql(p.jql.as_deref(), include_done);

        let (raw_issues, truncated): (Vec<Value>, bool) = if scope == "backlog" {
            jira.paginate_board_endpoint(p.board_id, "backlog", effective_jql.as_deref())
                .await
                .map_err(|e| ErrorData::internal_error(e, None))?
        } else if scope == "all" {
            jira.paginate_board_endpoint(p.board_id, "issue", effective_jql.as_deref())
                .await
                .map_err(|e| ErrorData::internal_error(e, None))?
        } else {
            let (board_raw, board_truncated) = jira
                .paginate_board_endpoint(p.board_id, "issue", effective_jql.as_deref())
                .await
                .map_err(|e| ErrorData::internal_error(e, None))?;
            let (backlog_raw, backlog_truncated) = jira
                .paginate_board_endpoint(p.board_id, "backlog", effective_jql.as_deref())
                .await
                .map_err(|e| ErrorData::internal_error(e, None))?;
            if board_truncated || backlog_truncated {
                return Err(ErrorData::internal_error(
                    format!(
                        "board {} scope=\"board\" set difference is unreliable: board or backlog fetch was truncated by the MAX_PAGES cap",
                        p.board_id
                    ),
                    None,
                ));
            }
            let board_keys = extract_keys(&board_raw);
            let backlog_keys = extract_keys(&backlog_raw);
            let on_board = on_board_key_set(&board_keys, &backlog_keys);
            let filtered = board_raw
                .into_iter()
                .filter(|v| {
                    v.get("key")
                        .and_then(|k| k.as_str())
                        .is_some_and(|k| on_board.contains(k))
                })
                .collect();
            (filtered, false)
        };

        let total = raw_issues.len();
        let start_at = p.start_at.unwrap_or(0) as usize;
        let max = p.max_results.unwrap_or(100).clamp(1, 100) as usize;
        let page: Vec<Value> = raw_issues
            .into_iter()
            .skip(start_at)
            .take(max)
            .filter_map(|v| v.as_object().map(jira::slim_search_issue))
            .collect();

        let out = serde_json::json!({
            "scope": scope,
            "board_id": p.board_id,
            "total": total,
            "issues": page,
            "truncated": truncated,
        });
        serde_json::to_string_pretty(&out)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "jira_move_to_board",
        description = "Move issues from a board's backlog onto the board (POST /rest/agile/1.0/board/{id}/issue). Does NOT change issue status -- only backlog/board membership. Up to 50 issue keys per call (Jira's cap)."
    )]
    async fn jira_move_to_board(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<MoveBoardIssuesParams>,
    ) -> Result<String, ErrorData> {
        validate_issue_keys(&p.issue_keys)?;
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        jira.move_issues_to_board(p.board_id, &p.issue_keys)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        let out = serde_json::json!({
            "moved_to": "board",
            "board_id": p.board_id,
            "issue_keys": p.issue_keys,
        });
        serde_json::to_string_pretty(&out)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "jira_move_to_backlog",
        description = "Move issues from a board onto its backlog (POST /rest/agile/1.0/backlog/{id}/issue, the board-scoped form -- the boardless /backlog/issue endpoint is scrum-only). Does NOT change issue status -- only backlog/board membership. Up to 50 issue keys per call (Jira's cap)."
    )]
    async fn jira_move_to_backlog(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<MoveBoardIssuesParams>,
    ) -> Result<String, ErrorData> {
        validate_issue_keys(&p.issue_keys)?;
        let creds = Self::resolve(&parts)?;
        let jira = JiraClient::new(self.http.clone(), &creds);
        jira.move_issues_to_backlog(p.board_id, &p.issue_keys)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;

        let out = serde_json::json!({
            "moved_to": "backlog",
            "board_id": p.board_id,
            "issue_keys": p.issue_keys,
        });
        serde_json::to_string_pretty(&out)
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
        description = "Upload a small file attachment to a Jira issue, content as a base64-encoded string. For anything beyond a few hundred KB use jira_get_attachment_upload_url and send the raw bytes with curl instead. Returns the created attachment metadata."
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
        name = "jira_get_attachment_upload_url",
        description = "Get a one-shot, short-lived URL to upload a LOCAL file as a Jira attachment without base64. Then run the returned `curl` command (raw bytes, PUT, no auth headers needed): the server forwards the bytes straight to Jira using the credentials of this call and stores nothing. Each URL works exactly once. Returns `upload_url`, `curl`, `expires_in_seconds`."
    )]
    async fn jira_get_attachment_upload_url(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<GetAttachmentUploadUrlParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve(&parts)?;
        let issue_key = p.issue_key.trim().to_string();
        let filename = p.filename.trim().to_string();
        if issue_key.is_empty() || filename.is_empty() {
            return Err(ErrorData::invalid_request(
                "issue_key and filename must not be empty",
                None,
            ));
        }
        let origin =
            upload::public_origin(self.public_url.as_deref(), &parts.headers).ok_or_else(|| {
                ErrorData::internal_error(
                    "Cannot determine the public URL of this server; set MCP_PUBLIC_URL",
                    None,
                )
            })?;
        let ttl = std::time::Duration::from_secs(
            p.ttl_seconds
                .unwrap_or(upload::DEFAULT_TTL.as_secs())
                .clamp(1, upload::MAX_TTL.as_secs()),
        );
        let token = self
            .tickets
            .issue(creds, issue_key.clone(), filename.clone(), ttl);
        let upload_url = format!("{origin}{}", upload::upload_path(&token));

        let value = serde_json::json!({
            "issue_key": issue_key,
            "filename": filename,
            "upload_url": upload_url,
            "method": "PUT",
            "expires_in_seconds": ttl.as_secs(),
            "single_use": true,
            "curl": format!("curl -sS -X PUT --data-binary @'{filename}' '{upload_url}'"),
        });
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
        description = "List repositories in a Bitbucket workspace (Cloud REST 2.0). Requires X-Bitbucket-Workspace, X-Bitbucket-Username, X-Bitbucket-Api-Token (legacy alias: X-Bitbucket-App-Password); optional X-Bitbucket-Base-Url (default https://api.bitbucket.org/2.0). Optional `workspace` overrides the header."
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
        name = "bitbucket_update_pull_request",
        description = "Edit a pull request's `description` and/or `title`. NOT a patch: Bitbucket's update endpoint REPLACES the fields it receives and resets the ones it doesn't, so this tool GETs the PR first and re-sends its current title, description, reviewers, destination branch and close_source_branch for whatever you omit. Consequences: (1) `description` REPLACES the whole existing body, it is not appended to, so read the PR with `bitbucket_get_pull_request` first if you mean to keep any of the old text; (2) an empty-string `description` clears the body; (3) omitting a field leaves it as-is, but because the carried-over values are read moments before the write, a concurrent edit in the Bitbucket UI can be overwritten."
    )]
    async fn bitbucket_update_pull_request(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbUpdatePrParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .update_pull_request(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pull_request_id,
                p.title.as_deref(),
                p.description.as_deref(),
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

    // --- Pipelines & deployments ---

    #[tool(
        name = "bitbucket_list_pipelines",
        description = "List pipeline runs for a repository, newest first (GET .../pipelines?sort=-created_on). Compact per-run summary: build_number, uuid, state (PENDING/IN_PROGRESS/COMPLETED), result (SUCCESSFUL/FAILED/STOPPED/ERROR), target ref, commit, trigger, creator, timing and the web URL. Optional `target_branch` filters the fetched page. Set `raw` for the full Bitbucket payload. Poll `bitbucket_get_pipeline` to watch one run finish."
    )]
    async fn bitbucket_list_pipelines(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbListPipelinesParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .list_pipelines(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pagelen,
                p.page,
                p.target_branch.as_deref(),
                p.raw,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_get_pipeline",
        description = "Get one pipeline run by build number or UUID. A run is finished when `state` is COMPLETED; `result` then says SUCCESSFUL, FAILED, STOPPED or ERROR. While running, `stage` shows RUNNING/PAUSED/HALTED. A pipeline with a manual step (e.g. a production deploy) sits at IN_PROGRESS/PAUSED indefinitely after its automatic steps finish, so when monitoring a deploy check `bitbucket_list_pipeline_steps` or `bitbucket_list_deployments` rather than waiting for COMPLETED."
    )]
    async fn bitbucket_get_pipeline(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbPipelineParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .get_pipeline(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                &p.pipeline,
                p.raw,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_list_pipeline_steps",
        description = "List the steps of a pipeline run with per-step state, result, deployment environment, timing and the step UUID needed by `bitbucket_get_pipeline_step_log`."
    )]
    async fn bitbucket_list_pipeline_steps(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbPipelineParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .list_pipeline_steps(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                &p.pipeline,
                p.raw,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_get_pipeline_step_log",
        description = "Fetch the console log of one pipeline step (GET .../steps/{step_uuid}/log). Returns the LAST `tail_chars` characters (default 20000) plus `total_chars` and `truncated_head`, so a failed step's error is usually in the first call. Logs of steps still running may be partial or 404 until the step starts."
    )]
    async fn bitbucket_get_pipeline_step_log(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbPipelineStepLogParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .get_pipeline_step_log(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                &p.pipeline,
                &p.step_uuid,
                p.tail_chars,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_trigger_pipeline",
        description = "Start a pipeline (POST .../pipelines). `ref_name` is the branch (or tag, with `ref_type` = `tag`); optional `commit_hash` pins the run to a commit on that ref; optional `custom_pipeline` names a `custom:` pipeline from bitbucket-pipelines.yml; optional `variables` are passed to it. Returns the new run's summary (build_number, uuid) for polling with `bitbucket_get_pipeline`. This runs real CI/CD, including deployments, so confirm the ref and pipeline name first."
    )]
    async fn bitbucket_trigger_pipeline(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbTriggerPipelineParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .trigger_pipeline(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                &p.ref_name,
                p.ref_type.as_deref(),
                p.commit_hash.as_deref(),
                p.custom_pipeline.as_deref(),
                p.variables.as_deref(),
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_stop_pipeline",
        description = "Stop a running pipeline (POST .../pipelines/{pipeline}/stopPipeline). Bitbucket rejects the call once the run has completed."
    )]
    async fn bitbucket_stop_pipeline(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbStopPipelineParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .stop_pipeline(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                &p.pipeline,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_list_deployments",
        description = "List deployments for a repository, newest first: environment, state (COMPLETED/IN_PROGRESS/UNDEPLOYED), status (SUCCESSFUL/FAILED/STOPPED), the pipeline build_number and commit that produced it, and timing. Set `raw` for the full payload."
    )]
    async fn bitbucket_list_deployments(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbListDeploymentsParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .list_deployments(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.pagelen,
                p.page,
                p.raw,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_list_deployment_environments",
        description = "List a repository's deployment environments (uuid, name, slug, type Test/Staging/Production, rank). The `environment` argument of the pipeline-variable tools accepts any of name, slug or uuid."
    )]
    async fn bitbucket_list_deployment_environments(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbListEnvironmentsParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .list_environments(bb_workspace_override(&p.workspace), &p.repo_slug, p.raw)
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    // --- Pipeline variables ---

    #[tool(
        name = "bitbucket_list_pipeline_variables",
        description = "List pipeline variables. Omit `environment` for repository-level variables (GET .../pipelines_config/variables); pass an environment name, slug or UUID for that deployment's variables (GET .../deployments_config/environments/{uuid}/variables). Deployment-level values override repository-level ones with the same key. Secured variables come back without a value."
    )]
    async fn bitbucket_list_pipeline_variables(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbListVariablesParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .list_variables(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.environment.as_deref(),
                p.pagelen,
                p.page,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_create_pipeline_variable",
        description = "Create a pipeline variable at repository level (omit `environment`) or for one deployment environment (name, slug or UUID). `secured` makes it write-only. Needs an API token with the admin:pipeline:bitbucket scope; a read-only token gets 403 `required privilege scopes`."
    )]
    async fn bitbucket_create_pipeline_variable(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbCreateVariableParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .create_variable(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.environment.as_deref(),
                &p.key,
                &p.value,
                p.secured,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_update_pipeline_variable",
        description = "Update an existing pipeline variable, addressed by key name or UUID, at repository level (omit `environment`) or in one deployment environment. Read-modify-write: fields you omit are carried over from the current variable. A secured variable's value cannot be read back, so `value` is required to update one. Needs the admin:pipeline:bitbucket scope."
    )]
    async fn bitbucket_update_pipeline_variable(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbUpdateVariableParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .update_variable(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.environment.as_deref(),
                &p.variable,
                p.key.as_deref(),
                p.value.as_deref(),
                p.secured,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e, None))?;
        serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "bitbucket_delete_pipeline_variable",
        description = "Delete a pipeline variable, addressed by key name or UUID, at repository level (omit `environment`) or in one deployment environment. Irreversible; a secured value cannot be recovered afterwards. Needs the admin:pipeline:bitbucket scope."
    )]
    async fn bitbucket_delete_pipeline_variable(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(p): Parameters<BbDeleteVariableParams>,
    ) -> Result<String, ErrorData> {
        let creds = Self::resolve_bitbucket(&parts)?;
        let bb = BitbucketClient::new(self.http.clone(), &creds);
        let value = bb
            .delete_variable(
                bb_workspace_override(&p.workspace),
                &p.repo_slug,
                p.environment.as_deref(),
                &p.variable,
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
            "Atlassian Cloud MCP: Jira REST v3 + Confluence REST + Bitbucket REST 2.0. Jira/Confluence: every MCP HTTP request MUST include X-Atlassian-Site-Url, X-Atlassian-Email, X-Atlassian-Api-Token (optional X-Atlassian-Confluence-Site-Url). Missing those yields -32600 with reason missing_atlassian_credential_headers. Bitbucket tools: send X-Bitbucket-Workspace, X-Bitbucket-Username (your Atlassian account email -- NOT the Bitbucket username and NOT x-bitbucket-api-token-auth, which the REST API rejects), X-Bitbucket-Api-Token (an Atlassian API token with Bitbucket repository scopes; the legacy name X-Bitbucket-App-Password is still accepted) on each request; optional X-Bitbucket-Base-Url (default https://api.bitbucket.org/2.0 for Cloud; set for Server/Data Center API roots). Missing Bitbucket headers yields -32600 with reason missing_bitbucket_credential_headers. Optional `workspace` on Bitbucket tool arguments overrides X-Bitbucket-Workspace. Pipelines: bitbucket_list_pipelines / get_pipeline / list_pipeline_steps / get_pipeline_step_log / list_deployments read runs; trigger_pipeline and stop_pipeline need the write:pipeline:bitbucket token scope; the *_pipeline_variable tools (repository or per-deployment-environment) need admin:pipeline:bitbucket to write."
                .to_string(),
        );
        info
    }
}

/// Splits trailing `ORDER BY ...` (case-insensitive, last occurrence) off a JQL string.
fn split_order_by(jql: &str) -> (&str, Option<&str>) {
    let lower = jql.to_ascii_lowercase();
    match lower.rfind("order by") {
        Some(idx) => (jql[..idx].trim_end(), Some(jql[idx..].trim())),
        None => (jql.trim_end(), None),
    }
}

/// Builds the effective JQL for jira_board_issues: ANDs the done-status filter onto the
/// caller's where-clause, preserving any trailing ORDER BY tail at the end.
fn build_effective_jql(caller_jql: Option<&str>, include_done: bool) -> Option<String> {
    let Some(j) = caller_jql else {
        return if include_done {
            None
        } else {
            Some("statusCategory != Done".to_string())
        };
    };
    if include_done {
        return Some(j.to_string());
    }
    let (where_clause, order_tail) = split_order_by(j);
    let base = if where_clause.is_empty() {
        "statusCategory != Done".to_string()
    } else {
        format!("({where_clause}) AND statusCategory != Done")
    };
    match order_tail {
        Some(tail) => Some(format!("{base} {tail}")),
        None => Some(base),
    }
}

fn extract_keys(issues: &[Value]) -> Vec<String> {
    issues
        .iter()
        .filter_map(|v| v.get("key").and_then(|k| k.as_str()).map(str::to_string))
        .collect()
}

/// Pure set difference: board keys not present in the backlog set.
fn on_board_key_set(
    board_keys: &[String],
    backlog_keys: &[String],
) -> std::collections::HashSet<String> {
    let backlog: std::collections::HashSet<&str> =
        backlog_keys.iter().map(String::as_str).collect();
    board_keys
        .iter()
        .filter(|k| !backlog.contains(k.as_str()))
        .cloned()
        .collect()
}

fn validate_issue_keys(keys: &[String]) -> Result<(), ErrorData> {
    if keys.is_empty() {
        return Err(ErrorData::invalid_params(
            "issue_keys must not be empty".to_string(),
            None,
        ));
    }
    if keys.len() > 50 {
        return Err(ErrorData::invalid_params(
            format!(
                "issue_keys has {} entries; Jira caps this endpoint at 50 per call",
                keys.len()
            ),
            None,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_board_key_set_excludes_backlog_members() {
        let board = vec!["A-1".to_string(), "A-2".to_string(), "A-3".to_string()];
        let backlog = vec!["A-2".to_string(), "A-3".to_string()];
        let on_board = on_board_key_set(&board, &backlog);
        assert_eq!(
            on_board,
            std::collections::HashSet::from(["A-1".to_string()])
        );
    }

    #[test]
    fn on_board_key_set_with_empty_backlog_keeps_everything() {
        let board = vec!["A-1".to_string(), "A-2".to_string()];
        let backlog: Vec<String> = vec![];
        let on_board = on_board_key_set(&board, &backlog);
        assert_eq!(
            on_board,
            std::collections::HashSet::from(["A-1".to_string(), "A-2".to_string()])
        );
    }

    #[test]
    fn effective_jql_no_order_by() {
        let out = build_effective_jql(Some("project = SWS"), false);
        assert_eq!(
            out.as_deref(),
            Some("(project = SWS) AND statusCategory != Done")
        );
    }

    #[test]
    fn effective_jql_lowercase_order_by() {
        let out = build_effective_jql(
            Some("assignee = currentUser() order by updated DESC"),
            false,
        );
        assert_eq!(
            out.as_deref(),
            Some("(assignee = currentUser()) AND statusCategory != Done order by updated DESC")
        );
    }

    #[test]
    fn effective_jql_trailing_order_by() {
        let out = build_effective_jql(
            Some("assignee = currentUser() ORDER BY updated DESC"),
            false,
        );
        assert_eq!(
            out.as_deref(),
            Some("(assignee = currentUser()) AND statusCategory != Done ORDER BY updated DESC")
        );
    }

    #[test]
    fn effective_jql_include_done_leaves_caller_jql_untouched() {
        let out = build_effective_jql(Some("assignee = currentUser() ORDER BY updated DESC"), true);
        assert_eq!(
            out.as_deref(),
            Some("assignee = currentUser() ORDER BY updated DESC")
        );
    }

    #[test]
    fn effective_jql_done_filter_lands_before_order_by_tail() {
        let out = build_effective_jql(Some("project = SWS ORDER BY updated DESC"), false).unwrap();
        let done_idx = out
            .find("statusCategory != Done")
            .expect("done filter present");
        let order_idx = out.find("ORDER BY").expect("order by present");
        assert!(done_idx < order_idx);
    }
}
