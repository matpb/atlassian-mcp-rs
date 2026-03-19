# atlassian-mcp-rs

[![CI](https://github.com/matpb/atlassian-mcp-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/matpb/atlassian-mcp-rs/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)

Rust [Model Context Protocol](https://modelcontextprotocol.io/) server for **Atlassian Cloud** — **Jira** (REST API v3) and **Confluence** (REST API under `/wiki`) — using **streamable HTTP** (`rmcp` + Axum) and **HTTP Basic** auth (`email` + [API token](https://id.atlassian.com/manage-profile/security/api-tokens)).

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
        "X-Atlassian-Api-Token": "${ATLASSIAN_API_TOKEN}"
      }
    }
  }
}
```

**CLI** (then edit the generated entry to add the headers above):

```bash
claude mcp add --transport http atlassian http://127.0.0.1:8432/mcp
```

## Tools

| Tool | Description |
|------|-------------|
| `jira_get_issue` | Compact issue: `key`, `summary`, plain-text `description`, `status`, `attachments`, all comments. |
| `jira_add_comment` | Plain-text comment → Atlassian Document Format. |
| `jira_update_description` | Replace description from plain text (ADF). |
| `jira_search` | **JQL** via `POST /rest/api/3/search/jql` (not legacy `/search`). `max_results` (1–100, default 25), `start_at`; response includes `nextPageToken` / `isLast` for pagination. |
| `confluence_search` | **CQL** search; `limit` (1–100, default 25). |
| `confluence_get_page` | Page by content id: **trimmed** fields for LLMs — id, title, status, space (key/name), version, `lastUpdated` (when + author display/email), `body.storage` with `char_count_*` and **truncation** after 120k characters, plus `links.webui` / `links.tinyui` only (no full `_links` map). |

## Other MCP clients (e.g. Cursor)

Configure HTTP MCP and supply the same three headers per your client’s docs.

## Jira / Confluence Server (Data Center)

This project targets **Atlassian Cloud** paths. On-prem may differ.

## Security

- **Never commit** Atlassian API tokens or site URLs that identify private data. `.env` is listed in `.gitignore`; keep secrets in your MCP client configuration (for example env expansion in `.mcp.json` as above).
- The server logs **bind address and port** only (`ServerConfig`); it does not read Atlassian credentials from the process environment.

## License

[MIT](LICENSE)
