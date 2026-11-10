# atlassian-mcp-rs: Rust MCP server (streamable HTTP) giving AI agents Jira, Confluence and Bitbucket Cloud tools

This codebase is yours: what it's for and how it's built are your call, and you'll keep building on it.

## Ground rules
- The server stores no credentials. Every Atlassian and Bitbucket credential arrives as HTTP headers on each `POST /mcp` request; never add env vars, files or caches that hold tokens.
- Run `cargo fmt --all` before committing: CI fails on `cargo fmt --all -- --check`, and that has broken master before.
- Every new tool lands with its README tool-table row and a unit test for its request or payload builder.
- Test against real Jira or Bitbucket only on a throwaway issue or PR you created yourself, then clean it up.
- Public repo: no site URLs, emails, tokens, hostnames or workspace names in code, tests, README or commits. Tests use stub literals that do not look like credentials.

## Commands
- Build: `cargo build --release`
- Test, full suite (about 66 tests, under a second): `cargo test`
- Test, one name filter: `cargo test summary_payload`
- Lint: `cargo clippy --all-targets -- -D warnings`
- Format check (what CI runs): `cargo fmt --all -- --check`
- Run locally: `MCP_PORT=18432 ./target/release/atlassian-mcp`, then `MCP_PORT=18432 make health` prints `ok`
- Docker image: `docker compose build`
- Smoke test without credentials (stateless, no initialize needed):
  `curl -s -X POST http://127.0.0.1:18432/mcp -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'`

## Layout worth knowing
- `src/mcp/tools.rs` is the whole MCP surface: param structs, the `#[tool_router]` impl, and every `#[tool]`. Tool bodies stay thin: resolve creds, call a client, serialize.
- `src/jira.rs`, `src/confluence.rs`, `src/bitbucket.rs` are the HTTP clients; payload builders live there as plain functions so they can be unit tested without a network.
- `src/credentials.rs` reads the request headers (`X-Atlassian-*`, `X-Bitbucket-*`) from the axum `Parts` extension.
- `src/upload.rs` serves `/upload/{token}`, the signed one-shot URL behind `jira_get_attachment_upload_url`, outside the MCP protocol.

## Conventions
- New tool: copy the closest existing one (for example `jira_update_description` or `jira_update_summary`) for naming, description wording and output shape.
- Outputs are slimmed JSON for an LLM, not raw API responses: pick the fields an agent needs.
- Tool descriptions name the headers the tool requires; agents read them.
- Missing credential headers return JSON-RPC `-32600` with `reason` `missing_atlassian_credential_headers` or `missing_bitbucket_credential_headers`. Upstream failures return `-32603` with the HTTP status first in the message (`Jira returned 403 Forbidden: ...`); the README "Errors and paging" section documents those shapes, keep them stable.
- Jira rich text is ADF: `content_format` `plain` (default) wraps text in paragraphs, `adf` takes a full ADF document string.
- Comments are one or two factual lines.

## Landmines
- Do not switch back to stateful sessions: clients hit "session not found" after idle periods. `NeverSessionManager` plus `stateful_mode: false` in `src/main.rs` makes each request independent, which works because credentials are per request.
- Do not take the upload body as a `Bytes` extractor in `handle_upload`: it buffers the whole body before the token check, so anyone can make the server allocate up to the upload cap with a bogus token. Validate the token first, then read the body with a limit.
- Upload tickets live in process memory: they vanish on restart and do not work across replicas. Behind a proxy, set `MCP_PUBLIC_URL` (or forward `X-Forwarded-Proto`/`Host`) or the returned URL falls back to the raw `Host` header over plain `http` (`upload::public_origin`).
- `X-Bitbucket-Username` must be the Atlassian account email. The static `x-bitbucket-api-token-auth` username works for git over HTTPS but the REST API rejects it with 401.
- Jira Cloud answers a bad email or token with HTTP 200 as an anonymous user, so searches just come back empty. Only cursor-mode `jira_search` detects this via `X-Seraph-LoginReason`; do not assume a 200 means you are authenticated.
- `X-Bitbucket-App-Password` is a legacy alias of `X-Bitbucket-Api-Token`; keep accepting it, existing clients still send it.
- Do not put Atlassian tokens in `.env`: it only configures bind and logging (`MCP_HOST`, `MCP_PORT`, `MCP_PUBLIC_URL`, `MCP_MAX_UPLOAD_BYTES`, `RUST_LOG`).

## Deeper context
- Headers, tool list, ADF, errors and paging, Docker quick start -> `README.md`
- Server env vars -> `env.example`, `src/config.rs`
- CI checks -> `.github/workflows/ci.yml`
