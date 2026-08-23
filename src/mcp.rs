use std::sync::Arc;

use axum::{
    Router,
    extract::State,
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use rmcp::ServiceExt;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tracing::info;

use crate::client::{RedatomClient, ReqwestAdapter};
use crate::tools::ToolServer;
use crate::ua::Ua;

/// Serve over stdio (default transport).
pub async fn run_stdio() -> anyhow::Result<()> {
    let client = Arc::new(RedatomClient::new(
        Arc::new(ReqwestAdapter::new(Ua::from_env())),
        min_interval_ms(),
        cache_cap(),
    ));
    let server = ToolServer::new(client);
    info!("stdio transport started");
    let (stdin, stdout) = rmcp::transport::stdio();
    server.serve((stdin, stdout)).await?.waiting().await?;
    Ok(())
}

fn build_router(client: Arc<RedatomClient>, key: &Option<String>, require_auth: bool) -> Router {
    let server_factory = move || Ok(ToolServer::new(client.clone()));

    let mcp_service = StreamableHttpService::new(
        server_factory,
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    // /health is liveness-only: it is mounted WITHOUT the auth layer so that
    // Docker HEALTHCHECK works without REDDIT_RSS_MCP_KEY.
    let auth = BearerAuth {
        key: key.clone(),
        require: require_auth,
    };
    let health_router = Router::new().route("/health", get(health));
    let mcp_router = Router::new()
        .nest_service("/mcp", mcp_service)
        .route_layer(middleware::from_fn_with_state(auth, auth_middleware));
    Router::new().merge(health_router).merge(mcp_router)
}

/// Serve over Streamable HTTP at `/mcp`, plus `GET /health`.
pub async fn run_http(bind: &str, key: &Option<String>) -> anyhow::Result<()> {
    let addr: std::net::SocketAddr = bind
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid --bind address '{bind}': {e}"))?;

    // Non-loopback binds require a bearer key; loopback binds are keyless.
    let require_auth = !addr.ip().is_loopback();
    if require_auth && (key.is_none() || key.as_deref().unwrap_or("").is_empty()) {
        anyhow::bail!("binding {bind} (non-loopback) requires REDDIT_RSS_MCP_KEY to be set");
    }

    let client = Arc::new(RedatomClient::new(
        Arc::new(ReqwestAdapter::new(Ua::from_env())),
        min_interval_ms(),
        cache_cap(),
    ));
    let app = build_router(client, key, require_auth);

    info!(%addr, require_auth, "http transport starting");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, serde_json::json!({"ok": true}).to_string())
}

#[derive(Clone)]
struct BearerAuth {
    key: Option<String>,
    require: bool,
}

async fn auth_middleware(
    State(auth): State<BearerAuth>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    if !auth.require {
        return next.run(req).await;
    }
    let expected = match &auth.key {
        Some(k) if !k.is_empty() => k,
        _ => return next.run(req).await,
    };
    let provided = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if provided == Some(expected.as_str()) {
        next.run(req).await
    } else {
        (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response()
    }
}

fn min_interval_ms() -> u64 {
    std::env::var("REDDIT_RSS_MIN_INTERVAL_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000)
}

fn cache_cap() -> usize {
    std::env::var("REDDIT_RSS_CACHE_CAP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(256)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{NoopHttp, RedatomClient};
    use http::Request as HttpRequest;
    use tower::ServiceExt;

    fn router_with_auth() -> Router {
        let client = Arc::new(RedatomClient::new(Arc::new(NoopHttp), 0, 8));
        build_router(client, &Some("secret".to_string()), true)
    }

    #[tokio::test]
    async fn health_returns_200_without_bearer() {
        let app = router_with_auth();
        let req = HttpRequest::builder()
            .uri("/health")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, r#"{"ok":true}"#);
    }

    #[tokio::test]
    async fn mcp_without_bearer_is_401() {
        let app = router_with_auth();
        let req = HttpRequest::builder()
            .uri("/mcp")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn mcp_with_correct_bearer_is_not_401() {
        let app = router_with_auth();
        let req = HttpRequest::builder()
            .uri("/mcp")
            .header("Authorization", "Bearer secret")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
