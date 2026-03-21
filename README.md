# atlassian-mcp-rs

[![CI](https://github.com/matpb/atlassian-mcp-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/matpb/atlassian-mcp-rs/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)

Rust [Model Context Protocol](https://modelcontextprotocol.io/) server for **Atlassian Cloud** — **Jira** (REST API v3), **Confluence** (REST API under `/wiki`), and **Bitbucket** ([REST API 2.0](https://developer.atlassian.com/cloud/bitbucket/rest/intro/)) — using **streamable HTTP** (`rmcp` + Axum). Jira and Confluence use **HTTP Basic** with `email` + [API token](https://id.atlassian.com/manage-profile/security/api-tokens). Bitbucket uses **HTTP Basic** with your Bitbucket **username** + [app password](https://support.atlassian.com/bitbucket-cloud/docs/app-passwords/) (not the Jira/Confluence API token).

## Authentication (required headers)

The server **does not** read Atlassian credentials from its environment. Every MCP HTTP request to `POST /mcp` must include:

| Header | Value |
|--------|--------|
| `X-Atlassian-Site-Url` | `https://your-company.atlassian.net` (no trailing slash required) |
| `X-Atlassian-Email` | Account email for the API token |
| `X-Atlassian-Api-Token` | API token |

If any required header is missing or empty, the server responds with a **JSON-RPC 2.0 error** using **code `-32600` (Invalid Request)** via `rmcp`’s `invalid_request`, plus a short `data` object that includes `"reason": "missing_atlassian_credential_headers"`.

### Confluence base URL (optional)

On **standard Atlassian Cloud**, Jira and Confluence share the same site: Confluence REST is at `{X-Atlassian-Site-Url}/wiki/rest/api` (see [Confluence REST](https://developer.atlassian.com/cloud/confluence/using-the-rest-api/)).

If Confluence is on a **different** Cloud site, set:

| Header | Value |
|--------|--------|
| `X-Atlassian-Confluence-Site-Url` | `https://other-site.atlassian.net` |

Confluence calls then use `{that}/wiki/rest/api`; Jira still uses `{X-Atlassian-Site-Url}/rest/api/3`.

### Bitbucket (required headers for Bitbucket tools)

Bitbucket credentials are also taken **only** from HTTP headers (not from the server environment). Any tool whose name starts with `bitbucket_` requires:

| Header | Value |
|--------|--------|
| `X-Bitbucket-Workspace` | Workspace slug (e.g. `acme`) |
| `X-Bitbucket-Username` | Bitbucket account username (used with app password) |
| `X-Bitbucket-App-Password` | [App password](https://support.atlassian.com/bitbucket-cloud/docs/app-passwords/) with appropriate scopes |

Optional:

| Header | Value |
|--------|--------|
| `X-Bitbucket-Base-Url` | API root including version path. Default: `https://api.bitbucket.org/2.0` (Bitbucket Cloud). For **Bitbucket Server / Data Center**, set this to your instance’s REST base (often ends in `/rest/api/1.0`). |

If Bitbucket headers are missing when a Bitbucket tool runs, the server returns JSON-RPC **`-32600` Invalid Request** with `reason`: **`missing_bitbucket_credential_headers`**.

You can send **both** Atlassian (Jira/Confluence) and Bitbucket headers on every request so all tools work from one client configuration.

### Server process environment (bind only)

| Variable | Default | Description |
|----------|---------|-------------|
| `MCP_HOST` | `0.0.0.0` | Bind address |
| `MCP_PORT` | `8432` | HTTP port |

Optional: `RUST_LOG` for tracing.

## Quick start (Docker)

From the repository root:

```bash
docker compose up -d --build
```

That builds the image, starts the container, and publishes **port 8432** by default.

**Optional `.env` (bind / logging only)** — Create a `.env` file next to `docker-compose.yml` if you want to override defaults. Compose loads it into the container when present (`required: false` if the file is missing). Use only server variables such as `MCP_PORT`, `MCP_HOST`, or `RUST_LOG`. **Do not** put Atlassian API tokens here; the container does not use them—the MCP client must still send the `X-Atlassian-*` headers on every request.

If you change `MCP_PORT`, set it in **`.env`** (not only in your shell) so `docker-compose.yml`’s port mapping and the server inside the container stay aligned.

```bash
# example .env (optional)
MCP_PORT=8432
```

**Check that it is up** (use your `MCP_PORT` if you overrode the default):

```bash
curl -sf http://127.0.0.1:8432/health
```

**Point your MCP client at** `http://127.0.0.1:8432/mcp` (adjust host and port if the service runs elsewhere or you set `MCP_PORT` in `.env`).

**Makefile shortcuts:** `make docker-build`, `make up`, `make health` (uses `MCP_PORT` from the environment, default `8432`).

**Without Compose:**

```bash
docker build -t atlassian-mcp:local .
docker run --rm -p 8432:8432 atlassian-mcp:local
```

Use `-e MCP_PORT=9000 -p 9000:9000` if you need a different port inside and outside the container.

## Run (from source)

```bash
cargo run --release
```

Use your MCP client to send the three `X-Atlassian-*` headers on **every** MCP request (initialize, tools, etc.).

- Health: `GET http://127.0.0.1:8432/health`
- MCP (streamable HTTP): `http://127.0.0.1:8432/mcp`

## Claude Code

[Claude Code](https://code.claude.com/docs/en/mcp) can attach headers in `.mcp.json` ([env expansion](https://code.claude.com/docs/en/mcp#environment-variable-expansion-in-mcpjson) recommended):

```json
{
  "mcpServers": {
    "atlassian": {
      "type": "http",
      "url": "http://127.0.0.1:8432/mcp",
      "headers": {
        "X-Atlassian-Site-Url": "https://your-company.atlassian.net",
        "X-Atlassian-Email": "${ATLASSIAN_EMAIL}",
        "X-Atlassian-Api-Token": "${ATLASSIAN_API_TOKEN}",
        "X-Bitbucket-Workspace": "${BITBUCKET_WORKSPACE}",
        "X-Bitbucket-Username": "${BITBUCKET_USERNAME}",
        "X-Bitbucket-App-Password": "${BITBUCKET_APP_PASSWORD}"
      }
    }
  }
}
```

**CLI** (then edit the generated entry to add the headers above):

```bash
claude mcp add --transport http atlassian http://127.0.0.1:8432/mcp
```

## Jira rich text (Atlassian Document Format)

Jira Cloud **REST API v3** stores issue descriptions and comment bodies as **[Atlassian Document Format (ADF)](https://developer.atlassian.com/cloud/jira/platform/apis/document/structure/)** — JSON with `type`, `version`, and nested `content` (headings, lists, links, code blocks, mentions, etc.).

This MCP server:

- **`content_format`: `plain` (default)** — You pass normal text; the server wraps it as simple ADF paragraphs (line breaks → paragraphs). No bold, lists, or links.
- **`content_format`: `adf`** — You pass a **single JSON string** whose value is a full ADF document root: `{"type":"doc","version":1,"content":[...]}`. Use this when an LLM (or you) need Jira-native rich structure. Invalid JSON or a missing/wrong `type`/`version` returns a tool error.

To **read** rich content without losing structure, call **`jira_get_issue`** with **`include_adf`: true** — the response adds `description_adf` and each comment’s `body_adf` (raw ADF from Jira) alongside the lossy plain-text `description` / `body` fields.

## Tools

### Jira & Confluence

| Tool | Description |
|------|-------------|
| `jira_get_issue` | Compact issue: `key`, `summary`, plain-text `description`, `status`, `attachments`, all comments. Optional **`include_adf`** adds `description_adf` and per-comment **`body_adf`** (raw ADF) for lossless editing. |
| `jira_add_comment` | Add a comment. **`content_format`**: `plain` (default) or **`adf`** (JSON string of full ADF document). |
| `jira_update_description` | Replace description; same **`content_format`** as `jira_add_comment`. |
| `jira_create_issue` | Create an issue: **`project_key`**, **`issue_type`** (name, e.g. `Task`), **`summary`**, optional **`description`** with **`description_content_format`** `plain` or `adf`. |
| `jira_search` | **JQL** via `POST /rest/api/3/search/jql` (not legacy `/search`). `max_results` (1–100, default 25), `start_at`; response includes `nextPageToken` / `isLast` for pagination. |
| `confluence_search` | **CQL** search; `limit` (1–100, default 25). |
| `confluence_get_page` | Page by content id: **trimmed** fields for LLMs — id, title, status, space (key/name), version, `lastUpdated` (when + author display/email), `body.storage` with `char_count_*` and **truncation** after 120k characters, plus `links.webui` / `links.tinyui` only (no full `_links` map). |

### Bitbucket (REST 2.0–compatible)

Most tools accept optional `workspace` (overrides `X-Bitbucket-Workspace`) and, where the API supports it, optional `pagelen` / `page` (1–100 for `pagelen`).

| Tool | Description |
|------|-------------|
| `bitbucket_list_repositories` | List repos in the workspace; optional `role` filter. |
| `bitbucket_get_repository` | Repository metadata by `repo_slug`. |
| `bitbucket_list_branches` | `refs/branches`; optional `name_filter` (substring, Bitbucket `q=name~"..."`). |
| `bitbucket_list_pull_requests` | List PRs; optional `state` (`OPEN`, `MERGED`, `DECLINED`, `SUPERSEDED`). |
| `bitbucket_get_pull_request` | Single PR by `pull_request_id`. |
| `bitbucket_create_pull_request` | Open a PR: `title`, `source_branch`, `destination_branch`, optional `description`, `close_source_branch`. |
| `bitbucket_get_pull_request_diff` | Full unified diff as JSON `{ "diff": "..." }`. |
| `bitbucket_get_pull_request_diffstat` | Per-file diff stats (`diffstat`). |
| `bitbucket_list_pull_request_comments` | PR comments (paginated). |
| `bitbucket_get_pull_request_comment` | One comment by `comment_id`. |
| `bitbucket_create_pull_request_comment` | New comment; optional `parent_comment_id` (reply) or `inline_path` + `inline_to` (inline). |
| `bitbucket_reply_pull_request_comment` | Reply via `parent_comment_id` + `content`. |
| `bitbucket_update_pull_request_comment` | Edit comment body (`content.raw`). |
| `bitbucket_delete_pull_request_comment` | Delete a comment. |
| `bitbucket_list_commits` | Commits for `revision` (branch/tag/commit; default `HEAD`). |
| `bitbucket_get_commit` | Single commit by hash. |
| `bitbucket_list_pull_request_activity` | PR activity stream. |
| `bitbucket_approve_pull_request` / `bitbucket_unapprove_pull_request` | Approve or withdraw approval. |
| `bitbucket_decline_pull_request` | Decline PR; optional `message`. |
| `bitbucket_merge_pull_request` | Merge; optional `merge_strategy` (`merge_commit`, `squash`, `fast_forward`), `close_source_branch`, `message`. |

## Other MCP clients (e.g. Cursor)

Configure HTTP MCP and supply the same three headers per your client’s docs.

## Jira / Confluence Server (Data Center)

This project targets **Atlassian Cloud** paths. On-prem may differ.

## Bitbucket Server (Data Center)

Set `X-Bitbucket-Base-Url` to your server’s REST API root. Paths and payloads can differ from Cloud; prefer the [server REST docs](https://developer.atlassian.com/server/bitbucket/rest/v906/intro/) for your version when troubleshooting.

## Security

- **Never commit** Atlassian API tokens or site URLs that identify private data. `.env` is listed in `.gitignore`; keep secrets in your MCP client configuration (for example env expansion in `.mcp.json` as above).
- The server logs **bind address and port** only (`ServerConfig`); it does not read Atlassian credentials from the process environment.

## License

[MIT](LICENSE)
