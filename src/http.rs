//! Streamable HTTP transport for hosted deployments.
//!
//! One long-lived process serves many remote clients. The MCP endpoint is
//! stateless (no per-client session, no server→client SSE push): every request
//! is an independent POST over the immutable, `Arc`-backed store, so any replica
//! can serve any request. A plain `GET /health` backs container/k8s probes.

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{Json, Router, routing::get};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use serde_json::json;
use tokio::net::TcpListener;

use crate::store::SdeStore;
use crate::tools::SdeMcpServer;

/// Serve MCP over Streamable HTTP until a shutdown signal (SIGINT/SIGTERM).
pub(crate) async fn serve(
    bind: &str,
    path: &str,
    store: Arc<SdeStore>,
    language: Option<String>,
) -> Result<()> {
    let app = router(path, store, language);

    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind HTTP listener on {bind}"))?;
    eprintln!("HTTP MCP server listening on {bind} (MCP at {path}, health at /health)");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server error")?;
    Ok(())
}

/// Build the axum app: the stateless MCP service mounted at `path`, plus a
/// `GET /health` probe. Split out from [`serve`] so tests can drive it on an
/// ephemeral port without the bind/signal machinery.
fn router(path: &str, store: Arc<SdeStore>, language: Option<String>) -> Router {
    // The build/release_date reported by `/health` come straight from the store
    // (same source as the `sde_status` tool). Read them out before the store is
    // moved into the service factory. Because the listener only binds after
    // download + scan complete, a 200 from `/health` honestly means "ready".
    let build = store.build;
    let release_date = store.release_date.clone();

    // Stateless factory: the heavy state (`SdeStore`) is shared behind an `Arc`;
    // a fresh, cheap handler is produced per request by cloning the `Arc` + the
    // language `Option<String>`.
    let factory = move || Ok(SdeMcpServer::new(store.clone(), language.clone()));

    let config = StreamableHttpServerConfig::default()
        .with_stateful_mode(false)
        // Return `application/json` directly instead of SSE framing — the tools
        // are simple request/response, so there is nothing to stream.
        .with_json_response(true)
        // rmcp defaults to accepting only loopback `Host` headers (DNS-rebinding
        // guard for locally-run servers). A hosted server is reached via its
        // ingress hostname, so that guard would reject all real traffic.
        // Host/Origin validation is the reverse proxy's responsibility here
        // (see docs/adr/0002-hosted-http-transport.md: no built-in auth).
        .disable_allowed_hosts();

    let mcp = StreamableHttpService::new(factory, Arc::new(NeverSessionManager::default()), config);

    Router::new()
        .route(
            "/health",
            get(move || {
                let release_date = release_date.clone();
                async move {
                    Json(json!({
                        "status": "ok",
                        "build": build,
                        "release_date": release_date,
                    }))
                }
            }),
        )
        .nest_service(path, mcp)
}

/// Resolve when the process receives Ctrl-C or (on Unix) SIGTERM, so
/// `docker stop` / pod termination shut the server down cleanly.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::warn!("failed to install SIGTERM handler: {e}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    eprintln!("shutdown signal received, stopping HTTP server");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Boot the HTTP router over the checked-in fixture SDE on an ephemeral
    /// port; returns the base URL. No network — the store is built from
    /// `tests/fixtures/sde`.
    async fn spawn_server() -> String {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sde");
        // scan_sde stores build/release_date on the SdeStore, which is what
        // `/health` reports — so the assertions below check these values.
        let store = crate::scan::scan_sde(&fixture, 42, "2026-01-01T00:00:00Z").unwrap();
        let app = router("/mcp", store, Some("en".to_string()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    async fn body_json(resp: reqwest::Response) -> serde_json::Value {
        let text = resp.text().await.unwrap();
        serde_json::from_str(&text).unwrap()
    }

    #[tokio::test]
    async fn health_endpoint_reports_ready() {
        let base = spawn_server().await;
        let resp = reqwest::get(format!("{base}/health")).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = body_json(resp).await;
        assert_eq!(body["status"], "ok");
        assert_eq!(body["build"], 42);
        assert_eq!(body["release_date"], "2026-01-01T00:00:00Z");
    }

    #[tokio::test]
    async fn mcp_initialize_and_tool_call_succeed() {
        let base = spawn_server().await;
        let client = reqwest::Client::new();
        let mcp = format!("{base}/mcp");
        let headers = |req: reqwest::RequestBuilder| {
            req.header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
        };

        // initialize handshake
        let resp = headers(client.post(&mcp))
            .body(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
            )
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = body_json(resp).await;
        assert_eq!(body["result"]["serverInfo"]["name"], "eve-sde-mcp");

        // a real read-only tool call over the fixture store (stateless: no session)
        let resp = headers(client.post(&mcp))
            .body(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"sde_status","arguments":{}}}"#,
            )
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = body_json(resp).await;
        assert_eq!(body["result"]["isError"], false);
    }
}
