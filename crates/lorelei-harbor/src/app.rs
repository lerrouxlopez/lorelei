use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::header::HeaderName;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use lorelei_core::{Config as CoreConfig, EchoQuery, NewPearl, PearlType, ShellCall};
use lorelei_echo::{EchoConfig, EchoService};
use lorelei_lore::{LoreConfig, LoreStores};
use lorelei_shells::ShellRegistryPg;
use qdrant_client::Qdrant;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use thiserror::Error;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;
use tracing::{info, Level};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

pub async fn run() -> anyhow::Result<()> {
    init_tracing();

    let cfg = HarborConfig::load_from_env().map_err(anyhow::Error::msg)?;
    let state = AppState::new(cfg).await?;
    let app = build_router(state);

    let addr: SocketAddr = "0.0.0.0:8080".parse().unwrap();
    info!(%addr, "lorelei-harbor listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let json = lorelei_core::log_json_enabled();

    let fmt = tracing_subscriber::fmt().with_env_filter(filter);
    if json {
        fmt.json()
            .with_current_span(true)
            .with_span_list(true)
            .init();
    } else {
        fmt.init();
    }
}

fn build_router(state: AppState) -> Router {
    let x_request_id = HeaderName::from_static("x-request-id");
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v1/runs", post(create_run))
        .route("/v1/runs/:run_id", get(get_run))
        .route("/v1/runs/:run_id/currents", get(list_currents))
        .route("/v1/echo", post(echo))
        .route("/v1/pearls", post(create_pearl).get(list_pearls))
        .route("/v1/pearls/:pearl_id", get(get_pearl).delete(delete_pearl))
        .route("/v1/shells", get(list_shells))
        .route("/v1/shells/:shell_name/call", post(call_shell))
        .route("/v1/providers", get(list_providers))
        .layer(SetRequestIdLayer::new(x_request_id.clone(), MakeRequestUuid))
        .layer(PropagateRequestIdLayer::new(x_request_id))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request<_>| {
                    tracing::span!(
                        Level::INFO,
                        "http.request",
                        method = %request.method(),
                        uri = %request.uri(),
                        status = tracing::field::Empty,
                    )
                })
                .on_request(())
                .on_response(|response: &Response, latency: std::time::Duration, span: &tracing::Span| {
                    span.record("status", response.status().as_u16());
                    tracing::info!(
                        parent: span,
                        latency_ms = latency.as_millis() as u64,
                        "http.response"
                    );
                }),
        )
        .with_state(Arc::new(state))
}

struct AppState {
    cfg: HarborConfig,
    pool: PgPool,
    qdrant: Qdrant,
    lore: LoreStores,
    echo: EchoService,
    shells: ShellRegistryPg,
}

impl AppState {
    async fn new(cfg: HarborConfig) -> anyhow::Result<Self> {
        let pool = PgPool::connect(&cfg.database_url).await?;
        sqlx::migrate!("../../migrations").run(&pool).await?;

        let qdrant = Qdrant::from_url(&cfg.qdrant_url).build()?;

        let lore = LoreStores::new(
            pool.clone(),
            qdrant.clone(),
            LoreConfig {
                qdrant_collection: cfg.lore.qdrant_collection.clone(),
            },
        )?;

        let echo = EchoService::from_config(
            &cfg.core,
            lore.clone(),
            EchoConfig {
                qdrant_collection: cfg.lore.qdrant_collection.clone(),
                embedding_provider: cfg.echo.embedding_provider.clone(),
                embedding_model: cfg.echo.embedding_model.clone(),
                llm_rerank: cfg.echo.llm_rerank,
            },
        )?;

        let shells = ShellRegistryPg::new(pool.clone());

        Ok(Self {
            cfg,
            pool,
            qdrant,
            lore,
            echo,
            shells,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
struct HarborConfig {
    #[serde(flatten)]
    core: CoreConfig,
    #[serde(default)]
    database_url: String,
    #[serde(default)]
    qdrant_url: String,
    #[serde(default)]
    lore: LoreSection,
    #[serde(default)]
    echo: EchoSection,
}

#[derive(Debug, Clone, Deserialize)]
struct LoreSection {
    #[serde(default = "LoreSection::default_collection")]
    qdrant_collection: String,
}

impl LoreSection {
    fn default_collection() -> String {
        "lorelei".to_string()
    }
}

impl Default for LoreSection {
    fn default() -> Self {
        Self {
            qdrant_collection: Self::default_collection(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct EchoSection {
    #[serde(default = "EchoSection::default_embedding_provider")]
    embedding_provider: String,
    #[serde(default)]
    embedding_model: Option<String>,
    #[serde(default)]
    llm_rerank: bool,
}

impl EchoSection {
    fn default_embedding_provider() -> String {
        "openai".to_string()
    }
}

impl Default for EchoSection {
    fn default() -> Self {
        Self {
            embedding_provider: Self::default_embedding_provider(),
            embedding_model: None,
            llm_rerank: false,
        }
    }
}

impl HarborConfig {
    fn load_from_env() -> Result<Self, ApiError> {
        let path = std::env::var("LORELEI_CONFIG")
            .map_err(|_| ApiError::bad_request("missing LORELEI_CONFIG env var"))?;
        let text = std::fs::read_to_string(&path)
            .map_err(|e| ApiError::bad_request(format!("failed to read LORELEI_CONFIG `{path}`: {e}")))?;

        let mut cfg: Self = toml::from_str(&text)
            .map_err(|e| ApiError::bad_request(format!("config parse error: {e}")))?;

        cfg.core
            .validate()
            .map_err(|e| ApiError::bad_request(e.to_string()))?;

        cfg.database_url = std::env::var("DATABASE_URL")
            .map_err(|_| ApiError::bad_request("missing DATABASE_URL env var"))?;
        cfg.qdrant_url = std::env::var("QDRANT_URL")
            .map_err(|_| ApiError::bad_request("missing QDRANT_URL env var"))?;

        Ok(cfg)
    }
}

#[derive(Debug, Error)]
enum ApiError {
    #[error("{message}")]
    Error { status: StatusCode, code: &'static str, message: String },
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self::Error { status: StatusCode::BAD_REQUEST, code: "bad_request", message: message.into() }
    }
    fn not_found(message: impl Into<String>) -> Self {
        Self::Error { status: StatusCode::NOT_FOUND, code: "not_found", message: message.into() }
    }
    fn internal(message: impl Into<String>) -> Self {
        Self::Error { status: StatusCode::INTERNAL_SERVER_ERROR, code: "internal", message: message.into() }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            ApiError::Error { status, code, message } => (status, code, message),
        };
        let body = Json(json!({ "error": { "code": code, "message": message } }));
        (status, body).into_response()
    }
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "ok": true })))
}

async fn readyz(State(s): State<Arc<AppState>>) -> Result<impl IntoResponse, ApiError> {
    // DB check.
    sqlx::query("SELECT 1")
        .execute(&s.pool)
        .await
        .map_err(|e| ApiError::internal(format!("db not ready: {e}")))?;
    // Qdrant check.
    s.qdrant
        .health_check()
        .await
        .map_err(|e| ApiError::internal(format!("qdrant not ready: {e}")))?;
    Ok((StatusCode::OK, Json(json!({ "ready": true }))))
}

#[derive(Debug, Deserialize)]
struct CreateRunRequest {
    tenant_id: Uuid,
    #[serde(default)]
    metadata: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct RunResponse {
    id: Uuid,
    tenant_id: Uuid,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    metadata: serde_json::Value,
}

async fn create_run(
    State(s): State<Arc<AppState>>,
    Json(req): Json<CreateRunRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let run = s
        .lore
        .create_run(req.tenant_id, req.metadata)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok((StatusCode::CREATED, Json(RunResponse {
        id: run.id,
        tenant_id: run.tenant_id,
        started_at: run.started_at,
        ended_at: run.ended_at,
        metadata: run.metadata,
    })))
}

async fn get_run(
    State(s): State<Arc<AppState>>,
    Path(run_id): Path<Uuid>,
    Query(q): Query<TenantQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let run = s
        .lore
        .get_run(q.tenant_id, run_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let Some(run) = run else {
        return Err(ApiError::not_found("run not found"));
    };
    Ok(Json(RunResponse {
        id: run.id,
        tenant_id: run.tenant_id,
        started_at: run.started_at,
        ended_at: run.ended_at,
        metadata: run.metadata,
    }))
}

#[derive(Debug, Deserialize)]
struct TenantQuery {
    tenant_id: Uuid,
}

async fn list_currents(
    State(s): State<Arc<AppState>>,
    Path(run_id): Path<Uuid>,
    Query(q): Query<TenantQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let events = s
        .lore
        .list_currents(q.tenant_id, run_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(json!({ "items": events })))
}

#[derive(Debug, Deserialize)]
struct EchoRequest {
    text: String,
    tenant_id: Uuid,
    #[serde(default)]
    agent_id: Option<Uuid>,
    #[serde(default)]
    run_id: Option<Uuid>,
    #[serde(default)]
    pearl_type: Option<PearlType>,
    #[serde(default)]
    min_confidence: Option<f64>,
    #[serde(default = "default_top_k")]
    top_k: usize,
}

fn default_top_k() -> usize {
    10
}

async fn echo(State(s): State<Arc<AppState>>, Json(req): Json<EchoRequest>) -> Result<impl IntoResponse, ApiError> {
    let hits = s
        .echo
        .retrieve(EchoQuery {
            text: req.text,
            tenant_id: req.tenant_id,
            agent_id: req.agent_id,
            run_id: req.run_id,
            pearl_type: req.pearl_type,
            min_confidence: req.min_confidence,
            limit: req.top_k,
        })
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(json!({ "items": hits })))
}

#[derive(Debug, Deserialize)]
struct CreatePearlRequest {
    tenant_id: Uuid,
    #[serde(default)]
    agent_id: Option<Uuid>,
    run_id: Uuid,
    pearl: NewPearl,
}

async fn create_pearl(
    State(s): State<Arc<AppState>>,
    Json(req): Json<CreatePearlRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let saved = s
        .lore
        .save_pearl(req.tenant_id, req.agent_id, req.run_id, req.pearl)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok((StatusCode::CREATED, Json(saved)))
}

#[derive(Debug, Deserialize)]
struct ListPearlsQuery {
    tenant_id: Uuid,
    #[serde(default)]
    include_deleted: bool,
    #[serde(default)]
    pearl_type: Option<PearlType>,
    #[serde(default = "default_top_k")]
    top_k: usize,
}

async fn list_pearls(
    State(s): State<Arc<AppState>>,
    Query(q): Query<ListPearlsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let items = s
        .lore
        .list_pearls(q.tenant_id, q.pearl_type, q.include_deleted, q.top_k as i64)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(json!({ "items": items })))
}

async fn get_pearl(
    State(s): State<Arc<AppState>>,
    Path(pearl_id): Path<Uuid>,
    Query(q): Query<TenantQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let p = s
        .lore
        .get_pearl(q.tenant_id, pearl_id, true)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let Some(p) = p else { return Err(ApiError::not_found("pearl not found")); };
    Ok(Json(p))
}

async fn delete_pearl(
    State(s): State<Arc<AppState>>,
    Path(pearl_id): Path<Uuid>,
    Query(q): Query<TenantQuery>,
) -> Result<impl IntoResponse, ApiError> {
    s.lore
        .forget_pearl(q.tenant_id, pearl_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_shells(State(s): State<Arc<AppState>>) -> Result<impl IntoResponse, ApiError> {
    // For now, return built-ins with schema+risks via registry.
    let names = [
        "save_pearl",
        "echo_lore",
        "list_pearls",
        "forget_pearl",
        "http_get",
        "document_ingest",
        "echo",
        "noop",
    ];
    let items = names
        .iter()
        .filter_map(|n| {
            let schema = s.shells.schema(n).ok()?;
            let risk = s.shells.risk(n).ok()?;
            Some(json!({ "name": n, "risk": format!("{:?}", risk).to_lowercase(), "schema": schema }))
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

#[derive(Debug, Deserialize)]
struct ShellCallRequest {
    tenant_id: Uuid,
    run_id: Uuid,
    call: ShellCall,
    #[serde(default)]
    input: serde_json::Value,
}

async fn call_shell(
    State(s): State<Arc<AppState>>,
    Path(shell_name): Path<String>,
    Json(req): Json<ShellCallRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let res = s
        .shells
        .execute(req.tenant_id, req.run_id, &shell_name, req.call, req.input)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(res))
}

async fn list_providers(State(s): State<Arc<AppState>>) -> Result<impl IntoResponse, ApiError> {
    let items = s
        .cfg
        .core
        .providers
        .iter()
        .map(|(name, cfg)| json!({"name": name, "kind": format!("{:?}", cfg.kind()).to_lowercase()}))
        .collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}
