mod bitbucket;
mod config;
mod confluence;
mod credentials;
mod jira;
mod mcp;
mod upload;

#[cfg(test)]
mod contract_tests;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use config::ServerConfig;
use jira::JiraClient;
use mcp::tools::AtlassianMcp;
use upload::{UploadState, UploadTickets};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("atlassian_mcp=info,rmcp=warn")),
        )
        .init();

    let cfg = Arc::new(ServerConfig::from_env());
    tracing::info!(?cfg, "Starting Atlassian MCP (streamable HTTP)");

    let http = JiraClient::build_http_client().expect("build HTTP client");
    let app = build_app(
        http,
        UploadTickets::default(),
        cfg.public_url.clone(),
        cfg.max_upload_bytes,
    );

    let addr: SocketAddr = format!("{}:{}", cfg.host, cfg.port).parse().unwrap();
    let listener = TcpListener::bind(addr).await.unwrap();
    tracing::info!("Atlassian MCP listening on http://{addr}/mcp");

    let shutdown = async {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("Shutting down...");
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .unwrap();
}

/// Router with the MCP endpoint, the upload endpoint and the health check.
pub(crate) fn build_app(
    http: reqwest::Client,
    tickets: UploadTickets,
    public_url: Option<String>,
    max_upload_bytes: usize,
) -> axum::Router {
    let upload_state = UploadState {
        http: http.clone(),
        tickets: tickets.clone(),
        max_bytes: max_upload_bytes,
    };

    let mcp_service = StreamableHttpService::new(
        move || {
            Ok(AtlassianMcp::new(
                http.clone(),
                tickets.clone(),
                public_url.clone(),
            ))
        },
        Arc::new(NeverSessionManager::default()),
        mcp_http_config(),
    );

    axum::Router::new()
        .route("/health", axum::routing::get(health))
        .route("/mcp", axum::routing::any_service(mcp_service))
        .route(
            "/upload/{token}",
            axum::routing::put(upload::handle_upload)
                .post(upload::handle_upload)
                .layer(DefaultBodyLimit::disable())
                .with_state(upload_state),
        )
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

// Host validation is off: the server holds no credentials and runs behind APISIX, which forwards the public Host.
fn mcp_http_config() -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .disable_allowed_hosts()
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_http_config_is_stateless_json_and_accepts_any_host() {
        let c = mcp_http_config();
        assert!(!c.legacy_session_mode);
        assert!(c.json_response);
        assert!(c.allowed_hosts.is_empty());
    }
}
