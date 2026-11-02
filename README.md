# atlassian-mcp-rs

[![CI](https://github.com/matpb/atlassian-mcp-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/matpb/atlassian-mcp-rs/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)

Rust [Model Context Protocol](https://modelcontextprotocol.io/) server for **Atlassian Cloud**: **Jira** (REST API v3), **Confluence** (REST API under `/wiki`), and **Bitbucket** ([REST API 2.0](https://developer.atlassian.com/cloud/bitbucket/rest/intro/)), using **streamable HTTP** (`rmcp` + Axum). Jira and Confluence use **HTTP Basic** with `email` + [API token](https://id.atlassian.com/manage-profile/security/api-tokens). Bitbucket also uses **HTTP Basic**, with your Atlassian account **email** + an [API token](https://id.atlassian.com/manage-profile/security/api-tokens) scoped for Bitbucket. App passwords were retired by Atlassian in July 2026, so the credential header is `X-Bitbucket-Api-Token`; the older `X-Bitbucket-App-Password` name is still accepted for backwards compatibility.

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

Confluence calls then use `{that}/wiki/rest/api` (and `{that}/wiki/api/v2` for the v2 endpoints `confluence_create_page` uses); Jira still uses `{X-Atlassian-Site-Url}/rest/api/3`.

### Bitbucket (required headers for Bitbucket tools)

Bitbucket credentials are also taken **only** from HTTP headers (not from the server environment). Any tool whose name starts with `bitbucket_` requires:

| Header | Value |
|--------|--------|
| `X-Bitbucket-Workspace` | Workspace slug (e.g. `acme`) |
| `X-Bitbucket-Username` | Atlassian account **email** (not the Bitbucket username) |
| `X-Bitbucket-Api-Token` | [API token](https://id.atlassian.com/manage-profile/security/api-tokens) with Bitbucket read/write repository scopes. Add `read:pipeline:bitbucket` for the pipeline tools, `write:pipeline:bitbucket` to trigger/stop runs and `admin:pipeline:bitbucket` to write pipeline variables |
| `X-Bitbucket-App-Password` | **Legacy alias** for `X-Bitbucket-Api-Token`, still accepted so existing configs keep working. App passwords themselves were retired by Atlassian in July 2026, so the value must be an API token either way. If both headers are sent, `X-Bitbucket-Api-Token` wins |

> **Do not** put `x-bitbucket-api-token-auth` in `X-Bitbucket-Username`. That static username is correct for git-over-HTTPS but the REST API rejects it with `401 API token must be used with an atlassian registered email`. This server only speaks REST, so the username must be your Atlassian account email.

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
| `MCP_PUBLIC_URL` | derived from `X-Forwarded-Proto`/`X-Forwarded-Host`/`Host` | Origin agents reach the server at; used to build `jira_get_attachment_upload_url` links. Set it behind a proxy that does not forward those headers. |
| `MCP_MAX_UPLOAD_BYTES` | `104857600` | Body limit for `PUT /upload/{token}`. |

Optional: `RUST_LOG` for tracing.

## Quick start (Docker)

From the repository root:

```bash
docker compose up -d --build
```

That builds the image, starts the container, and publishes **port 8432** by default.

**Optional `.env` (bind / logging only)**: create a `.env` file next to `docker-compose.yml` if you want to override defaults. Compose loads it into the container when present (`required: false` if the file is missing). Use only server variables such as `MCP_PORT`, `MCP_HOST`, or `RUST_LOG`. **Do not** put Atlassian API tokens here; the container does not use them; the MCP client must still send the `X-Atlassian-*` headers on every request.

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
        "X-Bitbucket-Username": "${ATLASSIAN_EMAIL}",
        "X-Bitbucket-Api-Token": "${BITBUCKET_API_TOKEN}"
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

Jira Cloud **REST API v3** stores issue descriptions and comment bodies as **[Atlassian Document Format (ADF)](https://developer.atlassian.com/cloud/jira/platform/apis/document/structure/)**: JSON with `type`, `version`, and nested `content` (headings, lists, links, code blocks, mentions, etc.).

This MCP server:

- **`content_format`: `plain` (default)**: you pass normal text; the server wraps it as simple ADF paragraphs (line breaks → paragraphs). No bold, lists, or links.
- **`content_format`: `adf`**: you pass a **single JSON string** whose value is a full ADF document root: `{"type":"doc","version":1,"content":[...]}`. Use this when an LLM (or you) need Jira-native rich structure. Invalid JSON or a missing/wrong `type`/`version` returns a tool error.

To **read** rich content without losing structure, call **`jira_get_issue`** with **`include_adf`: true**; the response adds `description_adf` and each comment’s `body_adf` (raw ADF from Jira) alongside the lossy plain-text `description` / `body` fields.

## Tools

### Jira & Confluence

| Tool | Description |
|------|-------------|
| `jira_get_issue` | Compact issue: `key`, `summary`, plain-text `description`, `status`, `attachments`, all comments. Optional **`include_adf`** adds `description_adf` and per-comment **`body_adf`** (raw ADF) for lossless editing. |
| `jira_add_comment` | Add a comment. **`content_format`**: `plain` (default) or **`adf`** (JSON string of full ADF document). |
| `jira_update_description` | Replace description; same **`content_format`** as `jira_add_comment`. |
| `jira_update_summary` | Replace an issue's summary (title) via `PUT /rest/api/3/issue/{key}`. Plain text, trimmed. |
| `jira_create_issue` | Create an issue: **`project_key`**, **`issue_type`** (name, e.g. `Task`), **`summary`**, optional **`description`** with **`description_content_format`** `plain` or `adf`. |
| `jira_set_assignee` | Assign an issue via `PUT /rest/api/3/issue/{key}/assignee`. Pass **`account_id`** from `jira_search_users`; `"-1"` uses the project default; omit or pass `null` to unassign. |
| `jira_get_transitions` | List available workflow transitions for an issue; returns each transition's `id`, `name`, and target status. Feed the id into `jira_transition_issue`. |
| `jira_transition_issue` | Move an issue to a new status using a **`transition_id`** from `jira_get_transitions` (e.g. To Do → In Progress → Done). |
| `jira_list_boards` | List Jira Agile boards (`GET /rest/agile/1.0/board`), optional **`project_key`** filter. Returns each board's `id`, `name`, `type`; the `id` feeds `jira_board_issues`, `jira_move_to_board`, `jira_move_to_backlog`. |
| `jira_board_issues` | List a board's issues by **`scope`**: `board` (default, on the board), `backlog`, or `all` (both). On a team-managed board, backlog membership has **no JQL equivalent**: `/board/{id}/issue` includes backlog issues too, so `scope=board` is computed client-side as the set difference against `/board/{id}/backlog`. `include_done` (default `false`) filters out `statusCategory != Done`; optional extra `jql` is ANDed in. `start_at`/`max_results` (1 to 100, default 100) paginate the final list. |
| `jira_move_to_board` | Move issues from the backlog onto a board (`POST /rest/agile/1.0/board/{id}/issue`). Does **not** change issue status. Up to 50 `issue_keys` per call (Jira's cap). |
| `jira_move_to_backlog` | Move issues from a board into its backlog (`POST /rest/agile/1.0/backlog/{id}/issue`, board-scoped; the boardless `/backlog/issue` endpoint is scrum-only). Does **not** change issue status. Up to 50 `issue_keys` per call. |
| `jira_get_issue_history` | Issue changelog via `GET /rest/api/3/issue/{key}/changelog`: who changed which field, `from` → `to`, and when. Optional **`field`** narrows it to one field (`status` for the workflow timeline, `assignee` for reassignments); `max_results` (1 to 100, default 50), `start_at`. Timestamps come back verbatim from Jira **including its UTC offset**; do not assume they are local time. |
| `jira_search` | **JQL** via `POST /rest/api/3/search/jql` (not legacy `/search`). `max_results` (1 to 100, default 25), `start_at`; response includes `nextPageToken` / `isLast` for pagination. Optional **`fields`** (Jira field names) returns Jira's untouched issues; optional **`next_page_token`** continues a previous response. Either one switches to single-page cursor mode (no `start_at`). |
| `jira_search_users` | Search users by name or email. Returns `accountId` (needed for ADF `@`-mentions and `jira_set_assignee`), `displayName`, `emailAddress`, `active`. |
| `jira_list_attachments` | List attachments on an issue: id, filename, `mimeType`, size, download URL, created date, author. |
| `jira_add_attachment` | Upload a small file to an issue. **`filename`** + **`file_base64`** (base64-encoded content). Returns attachment metadata. |
| `jira_get_attachment_upload_url` | Upload a **local file without base64**: returns a one-shot, short-lived `upload_url` (+ ready `curl` line). `PUT` the raw bytes to it; the server forwards them to Jira with the credentials of the tool call and stores nothing. **`issue_key`**, **`filename`**, optional `ttl_seconds` (default 600, max 3600). |
| `jira_delete_attachment` | Delete an attachment by **`attachment_id`** (from `jira_list_attachments` or `jira_get_issue`). |
| `confluence_search` | **CQL** search; `limit` (1 to 100, default 25). |
| `confluence_create_page` | Create a page via the **v2 API** (`POST /wiki/api/v2/pages`): **`space_id`** (numeric id or space key, e.g. `DOCS`), **`title`**, **`body`**, optional **`body_format`** `storage` (Confluence XHTML, default) or `wiki`, **`parent_id`** (omit for top-level), **`status`** `current` (default) or `draft`. Returns id, title, status and the webui link. |
| `confluence_get_page` | Page by content id: **trimmed** fields for LLMs: id, title, status, space (key/name), version, `lastUpdated` (when + author display/email), `body.storage` with `char_count_*` and **truncation** after 120k characters, plus `links.webui` / `links.tinyui` only (no full `_links` map). |

### Bitbucket (REST 2.0 compatible)

Most tools accept optional `workspace` (overrides `X-Bitbucket-Workspace`) and, where the API supports it, optional `pagelen` / `page` (1 to 100 for `pagelen`).

| Tool | Description |
|------|-------------|
| `bitbucket_list_repositories` | List repos in the workspace; optional `role` filter. |
| `bitbucket_get_repository` | Repository metadata by `repo_slug`. |
| `bitbucket_list_branches` | `refs/branches`; optional `name_filter` (substring, Bitbucket `q=name~"..."`). |
| `bitbucket_list_tags` | Tags (`refs/tags`), Bitbucket JSON unchanged; optional `sort` (e.g. `-target.date`) and `q` filter. |
| `bitbucket_list_pull_requests` | List PRs; optional `state` (`OPEN`, `MERGED`, `DECLINED`, `SUPERSEDED`) and `sort` (e.g. `-updated_on`). |
| `bitbucket_get_pull_request` | Single PR by `pull_request_id`. |
| `bitbucket_create_pull_request` | Open a PR: `title`, `source_branch`, `destination_branch`, optional `description`, `close_source_branch`, and `reviewers` (list of account UUIDs from `bitbucket_search_users`). |
| `bitbucket_update_pull_request` | Edit a PR: `description` and/or `title`. Read-modify-write, not a patch: Bitbucket replaces what it receives and resets what it does not, so the tool GETs the PR and re-sends the current title, description, reviewers, destination branch and `close_source_branch` for omitted fields. A supplied `description` overwrites the whole body; empty string clears it. |
| `bitbucket_search_users` | Search workspace members by display name or nickname; returns `uuid`, `display_name`, `nickname` for use as PR reviewers. |
| `bitbucket_get_pull_request_diff` | Full unified diff as JSON `{ "diff": "..." }`. |
| `bitbucket_get_pull_request_diffstat` | Per-file diff stats (`diffstat`). |
| `bitbucket_list_pull_request_comments` | PR comments (paginated). |
| `bitbucket_get_pull_request_comment` | One comment by `comment_id`. |
| `bitbucket_create_pull_request_comment` | New comment; optional `parent_comment_id` (reply) or `inline_path` + `inline_to` (inline). |
| `bitbucket_reply_pull_request_comment` | Reply via `parent_comment_id` + `content`. |
| `bitbucket_update_pull_request_comment` | Edit comment body (`content.raw`). |
| `bitbucket_delete_pull_request_comment` | Delete a comment. |
| `bitbucket_list_commits` | Commits for `revision` (branch/tag/commit; default `HEAD`), or `include` / `exclude` (each a string or list) for commits reachable from include and not from exclude. For later pages pass `page`, and `ctx` if the `next` link has one. |
| `bitbucket_get_commit` | Single commit by hash. |
| `bitbucket_get_file` | Raw file content at a `commit` and `path` (`GET .../src/{commit}/{path}`), returned as plain text. |
| `bitbucket_file_history` | Commits that touched `path` at `commit` (`GET .../filehistory/{commit}/{path}`), Bitbucket JSON unchanged. Optional `pagelen` and `page` (opaque cursor from the `next` link). |
| `bitbucket_list_pull_request_activity` | PR activity stream. |
| `bitbucket_approve_pull_request` / `bitbucket_unapprove_pull_request` | Approve or withdraw approval. |
| `bitbucket_decline_pull_request` | Decline PR; optional `message`. |
| `bitbucket_merge_pull_request` | Merge; optional `merge_strategy` (`merge_commit`, `squash`, `fast_forward`), `close_source_branch`, `message`. |
| `bitbucket_list_pipelines` | Pipeline runs, newest first, as a compact summary (build_number, uuid, state, result, ref, commit, trigger, creator, timing, web URL). Optional `target_branch` filters the fetched page; `raw` returns Bitbucket's full objects. |
| `bitbucket_get_pipeline` | One run by build number or UUID. Finished when `state` is `COMPLETED`; a run with a manual step stays `IN_PROGRESS` / `PAUSED` after its automatic steps, so check steps or deployments when watching a deploy. |
| `bitbucket_list_pipeline_steps` | Steps of a run: name, state, result, timing, trigger (automatic/manual) and the step `uuid`. |
| `bitbucket_get_pipeline_step_log` | Console log of one step; returns the last `tail_chars` characters (default 20000) with `total_chars` and `truncated_head`. 404 until the step has started. |
| `bitbucket_trigger_pipeline` | Start a run on `ref_name` (branch, or tag with `ref_type`); optional `commit_hash`, `custom_pipeline` (a `custom:` pipeline from the YAML) and `variables`. Needs the **`write:pipeline:bitbucket`** token scope. |
| `bitbucket_stop_pipeline` | Stop a running pipeline. Needs `write:pipeline:bitbucket`. |
| `bitbucket_list_deployments` | Deployments, newest first: environment, state, status, build_number, commit, timing. Optional `environment` (UUID) and `sort` (default `-state.started_on`). `raw` for the full payload. |
| `bitbucket_list_deployment_environments` | Environments with uuid, name, slug, type and rank. |
| `bitbucket_list_pipeline_variables` | Repository variables (omit `environment`) or one deployment environment's variables (`environment` = name, slug or UUID). Secured values are never returned. |
| `bitbucket_create_pipeline_variable` | Create a variable in either scope: `key`, `value`, `secured`. Needs **`admin:pipeline:bitbucket`**. |
| `bitbucket_update_pipeline_variable` | Update by key name or UUID; omitted fields are carried over. `value` is required for a secured variable. Needs `admin:pipeline:bitbucket`. |
| `bitbucket_delete_pipeline_variable` | Delete by key name or UUID. Needs `admin:pipeline:bitbucket`. |

## Errors and paging

A failed tool call comes back as a JSON-RPC error, not as a result with `isError`: `{"error": {"code": -32603, "message": "..."}}`. When an upstream API refused the request, the message carries its HTTP status first, so scripts can match it:

| Source | Message shape |
|--------|---------------|
| Bitbucket | `Bitbucket returned 404 Not Found: <response body>` |
| Jira (`jira_search`) | `Jira search/jql failed: Jira returned 429 Too Many Requests: <response body>` |
| Jira (other tools) | `Jira returned 403 Forbidden: <response body>` |

Jira Cloud answers a bad email or API token with HTTP 200 and serves the request anonymously, so a search simply finds nothing. In cursor mode (`fields` or `next_page_token` set), `jira_search` detects this from the `X-Seraph-LoginReason` header and fails with `Jira returned 401 Unauthorized: ...` instead.

Paging: Bitbucket list tools return Bitbucket's `next` link. Pass its `page` value back as `page` (and `ctx` for `bitbucket_list_commits` when present). For `bitbucket_file_history` the page value is an opaque string such as `zxUz`, not a number. `jira_search` pages with `nextPageToken` in cursor mode.

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
