mod config;
mod confluence;
mod credentials;
mod jira;
mod mcp;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::local::LocalSessionManager,
};
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use config::ServerConfig;
use jira::JiraClient;
use mcp::tools::AtlassianMcp;

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
    let http = Arc::new(http);

    let mcp_service = StreamableHttpService::new(
        move || Ok(AtlassianMcp::new((*http).clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let app = axum::Router::new()
        .route("/health", axum::routing::get(health))
        .route("/mcp", axum::routing::any_service(mcp_service))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

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

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}
