#![forbid(unsafe_code)]

use crate::runtime::autonomy::PgAutonomy;
use crate::runtime::pg::PgCurrentStore;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use lorelei_core::config::LoreleiConfig;
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::{CurrentStore, EchoRetriever, LoreStore};
use lorelei_core::types::{
    ApprovalState, AutonomousTaskId, EchoQuery, NewPearl, PearlListQuery, PearlType, RunStatus,
    ShellRisk, UnitInterval,
};
use lorelei_echo::retriever::{EchoEngine, EchoRetrievalConfig};
use lorelei_lore::embedding::{DynSongProviderEmbeddingAdapter, EmbeddingProvider};
use lorelei_lore::pg::PgLoreStore;
use lorelei_lore::qdrant::QdrantPearlIndex;
use lorelei_shells::registry::BuiltinShellRegistry;
use lorelei_shells::repo::PgShellCallRepository;
use lorelei_siren::policy::DeterministicSirenPolicy;
use lorelei_song::registry::ProviderRegistry;
use lorelei_tide::runtime::SingleAgentTideRuntime;
use qdrant_client::Qdrant;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub config: LoreleiConfig,
    pub pg_pool: PgPool,
    pub qdrant: Qdrant,
    pub qdrant_index: QdrantPearlIndex,
    pub lore_store: Arc<dyn LoreStore>,
    pub echo: Arc<dyn EchoRetriever>,
    pub providers: Arc<ProviderRegistry>,
    pub shells: Arc<BuiltinShellRegistry>,
    pub currents: Arc<PgCurrentStore>,
    pub siren: Arc<DeterministicSirenPolicy>,
    pub tide: Arc<SingleAgentTideRuntime>,
    pub autonomy: Arc<PgAutonomy>,
}

#[derive(Debug, Serialize)]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
    pub request_id: String,
}

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub request_id: Uuid,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-request-id",
            HeaderValue::from_str(&self.request_id.to_string()).unwrap(),
        );
        (
            self.status,
            headers,
            Json(ApiErrorBody {
                code: self.code.to_string(),
                message: self.message,
                request_id: self.request_id.to_string(),
            }),
        )
            .into_response()
    }
}

#[derive(Clone, Copy)]
pub struct RequestId(pub Uuid);

async fn request_id_middleware(mut req: Request, next: Next) -> Response {
    let id = Uuid::new_v4();
    req.extensions_mut().insert(RequestId(id));
    let mut res = next.run(req).await;
    res.headers_mut().insert(
        "x-request-id",
        HeaderValue::from_str(&id.to_string()).unwrap(),
    );
    res
}

fn request_id_from_headers_or_ext(headers: &HeaderMap, ext: Option<&RequestId>) -> Uuid {
    if let Some(v) = headers.get("x-request-id") {
        if let Ok(s) = v.to_str() {
            if let Ok(u) = Uuid::parse_str(s) {
                return u;
            }
        }
    }
    ext.map(|r| r.0).unwrap_or_else(Uuid::new_v4)
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v1/runs", post(create_run))
        .route("/v1/runs/:run_id", get(get_run))
        .route("/v1/pearls", post(create_pearl).get(list_pearls))
        .route("/v1/pearls/:pearl_id", get(get_pearl).delete(delete_pearl))
        .route("/v1/echo", post(echo))
        .route("/v1/providers", get(list_providers))
        .route("/v1/shells", get(list_shells))
        .route("/v1/runs/:run_id/currents", get(list_currents))
        .route("/v1/tasks", post(create_task).get(list_tasks))
        .route("/v1/tasks/:task_id/pause", post(pause_task))
        .route("/v1/tasks/:task_id/resume", post(resume_task))
        .route("/v1/approvals", get(list_approvals))
        .route("/v1/approvals/:approval_id/approve", post(approve_approval))
        .layer(middleware::from_fn(request_id_middleware))
        .with_state(state)
}

pub async fn build_state() -> Result<AppState, LoreleiError> {
    let _ = dotenvy::dotenv();
    let config = LoreleiConfig::load_from_toml_path("lorelei.toml")?;

    let pg_url = std::env::var(&config.lore.postgres_url_env).map_err(|_| {
        LoreleiError::validation(
            "lore.postgres_url_env",
            format!(
                "missing required env var (value not shown): {}",
                config.lore.postgres_url_env
            ),
        )
    })?;
    let qdrant_url = std::env::var(&config.lore.qdrant_url_env).map_err(|_| {
        LoreleiError::validation(
            "lore.qdrant_url_env",
            format!(
                "missing required env var (value not shown): {}",
                config.lore.qdrant_url_env
            ),
        )
    })?;

    let pg_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&pg_url)
        .await
        .map_err(|e| LoreleiError::Internal(format!("postgres connect failed: {e}")))?;

    let qdrant = Qdrant::from_url(&qdrant_url)
        .build()
        .map_err(|e| LoreleiError::Internal(format!("qdrant client init failed: {e}")))?;
    let qdrant_index = QdrantPearlIndex::new(qdrant.clone(), config.lore.collection.clone());
    let currents = Arc::new(PgCurrentStore::new(pg_pool.clone()));
    currents.migrate().await?;

    let providers = Arc::new(ProviderRegistry::from_config(&config)?);
    let embed_provider = providers.get(&config.agent.default_embedding_provider)?;
    let embedder: Arc<dyn EmbeddingProvider> =
        Arc::new(DynSongProviderEmbeddingAdapter::new(embed_provider));

    let lore_store_indexed = PgLoreStore::new_indexed(
        pg_pool.clone(),
        qdrant_index.clone(),
        embedder.clone(),
        config.agent.default_embedding_provider.clone(),
    );
    lore_store_indexed.migrate().await?;

    let echo_engine = EchoEngine::new(
        PgLoreStore::new(pg_pool.clone()),
        qdrant_index.clone(),
        embedder,
        config.agent.default_embedding_provider.clone(),
        EchoRetrievalConfig {
            rerank_top_k: config.echo.rerank_top_k,
            enable_query_rewrite: false,
        },
    );

    let lore_store: Arc<dyn LoreStore> = Arc::new(lore_store_indexed);
    let echo: Arc<dyn EchoRetriever> = Arc::new(echo_engine);

    let shells_repo = Arc::new(PgShellCallRepository::new(pg_pool.clone()));
    let shells: Arc<BuiltinShellRegistry> = Arc::new(
        BuiltinShellRegistry::new(
            config.clone(),
            lore_store.clone(),
            echo.clone(),
            shells_repo,
        )
        .with_current_id_provider(|call| Some(call.call_id)),
    );

    let autonomy = Arc::new(PgAutonomy::new(pg_pool.clone()));
    let siren = Arc::new(
        DeterministicSirenPolicy::new(config.clone()).with_approval_store(autonomy.clone()),
    );
    let song = providers.get(&config.agent.default_provider)?;
    let runs: Arc<dyn lorelei_tide::runtime::RunRepository> = currents.clone();
    let currents_trait: Arc<dyn lorelei_core::traits::CurrentStore> = currents.clone();

    let tide: Arc<SingleAgentTideRuntime> = Arc::new(SingleAgentTideRuntime::new(
        config.clone(),
        runs,
        currents_trait,
        echo.clone(),
        lore_store.clone(),
        song,
        shells.clone(),
        siren.clone(),
    ));

    Ok(AppState {
        config,
        pg_pool,
        qdrant,
        qdrant_index,
        lore_store,
        echo,
        providers,
        shells,
        currents,
        siren,
        tide,
        autonomy,
    })
}

pub async fn serve() -> Result<(), LoreleiError> {
    let state = build_state().await?;
    let addr = format!("{}:{}", state.config.harbor.host, state.config.harbor.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| LoreleiError::Internal(format!("bind failed: {e}")))?;

    lorelei_core::observability::init_tracing("harbor");

    info!("harbor listening on {}", addr);
    axum::serve(listener, router(state))
        .await
        .map_err(|e| LoreleiError::Internal(format!("server error: {e}")))?;
    Ok(())
}

async fn healthz() -> impl IntoResponse {
    StatusCode::OK
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let pg_ok = sqlx::query("select 1")
        .execute(&state.pg_pool)
        .await
        .is_ok();
    let q_ok = state.qdrant.health_check().await.is_ok();
    if pg_ok && q_ok {
        StatusCode::OK.into_response()
    } else {
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    }
}

#[derive(Debug, Deserialize)]
pub struct CreatePearlRequest {
    pub tenant_id: Uuid,
    pub agent_id: Uuid,
    pub pearl_type: Option<PearlType>,
    pub content: String,
    pub confidence: Option<f64>,
    pub importance: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct PearlResponse {
    pub pearl_id: Uuid,
    pub tenant_id: Uuid,
    pub agent_id: Uuid,
    pub pearl_type: PearlType,
    pub content: String,
    pub confidence: f64,
    pub importance: f64,
    pub created_at: String,
}

async fn create_pearl(
    State(state): State<AppState>,
    axum::extract::Extension(rid): axum::extract::Extension<RequestId>,
    headers: HeaderMap,
    Json(body): Json<CreatePearlRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = request_id_from_headers_or_ext(&headers, Some(&rid));

    let confidence = body.confidence.unwrap_or(0.8);
    let importance = body.importance.unwrap_or(0.5);
    let new = NewPearl::new(
        body.pearl_type.unwrap_or(PearlType::Other),
        body.content,
        UnitInterval::new(importance).map_err(|e| map_err(e, request_id))?,
        UnitInterval::new(confidence).map_err(|e| map_err(e, request_id))?,
        Default::default(),
    )
    .map_err(|e| map_err(e, request_id))?;

    let pearl = state
        .lore_store
        .save_pearl(
            lorelei_core::types::TenantId(body.tenant_id),
            lorelei_core::types::AgentId(body.agent_id),
            new,
        )
        .await
        .map_err(|e| map_err(e, request_id))?;

    Ok((
        StatusCode::CREATED,
        Json(PearlResponse {
            pearl_id: pearl.pearl_id.0,
            tenant_id: pearl.tenant_id.0,
            agent_id: pearl.agent_id.0,
            pearl_type: pearl.pearl_type,
            content: pearl.content,
            confidence: pearl.confidence.get(),
            importance: pearl.importance.get(),
            created_at: pearl.created_at.to_rfc3339(),
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct ListPearlsQuery {
    pub tenant_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub pearl_type: Option<PearlType>,
    pub limit: Option<usize>,
}

async fn list_pearls(
    State(state): State<AppState>,
    axum::extract::Extension(rid): axum::extract::Extension<RequestId>,
    headers: HeaderMap,
    Query(q): Query<ListPearlsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = request_id_from_headers_or_ext(&headers, Some(&rid));
    let query = PearlListQuery {
        agent_id: q.agent_id.map(lorelei_core::types::AgentId),
        pearl_type: q.pearl_type,
        limit: q.limit,
        include_deleted: false,
        ..Default::default()
    };
    let pearls = state
        .lore_store
        .list_pearls(lorelei_core::types::TenantId(q.tenant_id), query)
        .await
        .map_err(|e| map_err(e, request_id))?;

    let out: Vec<PearlResponse> = pearls
        .into_iter()
        .map(|p| PearlResponse {
            pearl_id: p.pearl_id.0,
            tenant_id: p.tenant_id.0,
            agent_id: p.agent_id.0,
            pearl_type: p.pearl_type,
            content: p.content,
            confidence: p.confidence.get(),
            importance: p.importance.get(),
            created_at: p.created_at.to_rfc3339(),
        })
        .collect();
    Ok(Json(out))
}

async fn get_pearl(
    State(state): State<AppState>,
    axum::extract::Extension(rid): axum::extract::Extension<RequestId>,
    headers: HeaderMap,
    Path(pearl_id): Path<Uuid>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = request_id_from_headers_or_ext(&headers, Some(&rid));
    let tenant_id = q
        .get("tenant_id")
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: "tenant_id query param required".to_string(),
            request_id,
        })?;

    let got = state
        .lore_store
        .get_pearl(
            lorelei_core::types::TenantId(tenant_id),
            lorelei_core::types::PearlId(pearl_id),
            false,
        )
        .await
        .map_err(|e| map_err(e, request_id))?;

    let Some(p) = got else {
        return Err(ApiError {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: "pearl not found".to_string(),
            request_id,
        });
    };

    Ok(Json(PearlResponse {
        pearl_id: p.pearl_id.0,
        tenant_id: p.tenant_id.0,
        agent_id: p.agent_id.0,
        pearl_type: p.pearl_type,
        content: p.content,
        confidence: p.confidence.get(),
        importance: p.importance.get(),
        created_at: p.created_at.to_rfc3339(),
    }))
}

async fn delete_pearl(
    State(state): State<AppState>,
    axum::extract::Extension(rid): axum::extract::Extension<RequestId>,
    headers: HeaderMap,
    Path(pearl_id): Path<Uuid>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = request_id_from_headers_or_ext(&headers, Some(&rid));
    let tenant_id = q
        .get("tenant_id")
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: "tenant_id query param required".to_string(),
            request_id,
        })?;

    state
        .lore_store
        .forget_pearl(
            lorelei_core::types::TenantId(tenant_id),
            lorelei_core::types::PearlId(pearl_id),
        )
        .await
        .map_err(|e| map_err(e, request_id))?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct EchoRequest {
    pub tenant_id: Uuid,
    pub agent_id: Uuid,
    pub query: String,
    pub top_k: Option<usize>,
    pub min_confidence: Option<f64>,
    pub pearl_type: Option<PearlType>,
}

async fn echo(
    State(state): State<AppState>,
    axum::extract::Extension(rid): axum::extract::Extension<RequestId>,
    headers: HeaderMap,
    Json(body): Json<EchoRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = request_id_from_headers_or_ext(&headers, Some(&rid));
    let min_conf = match body.min_confidence {
        Some(v) => Some(UnitInterval::new(v).map_err(|e| map_err(e, request_id))?),
        None => state.config.echo.min_confidence,
    };

    let hits = state
        .echo
        .query(
            lorelei_core::types::TenantId(body.tenant_id),
            lorelei_core::types::AgentId(body.agent_id),
            EchoQuery {
                query: body.query,
                top_k: body.top_k.unwrap_or(state.config.echo.top_k),
                min_confidence: min_conf,
                pearl_type: body.pearl_type,
            },
        )
        .await
        .map_err(|e| map_err(e, request_id))?;

    Ok(Json(hits))
}

#[derive(Debug, Serialize)]
struct ProviderInfo {
    name: String,
    kind: String,
    base_url: Option<String>,
    chat_model: String,
    embedding_model: Option<String>,
    capabilities: lorelei_core::types::ProviderCapabilities,
}

async fn list_providers(State(state): State<AppState>) -> impl IntoResponse {
    let mut out = Vec::new();
    for (name, p) in &state.config.providers {
        let caps = state
            .providers
            .get(name)
            .map(|sp| sp.capabilities())
            .unwrap_or_default();
        out.push(ProviderInfo {
            name: name.clone(),
            kind: provider_kind_str(&p.kind).to_string(),
            base_url: p.base_url.clone(),
            chat_model: p.chat_model.clone(),
            embedding_model: p.embedding_model.clone(),
            capabilities: caps,
        });
    }
    Json(out)
}

async fn list_shells(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.shells.specs())
}

#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub tenant_id: Uuid,
    pub agent_id: Uuid,
    pub prompt: String,
    #[serde(default)]
    pub daily: bool,
    pub at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TaskResponse {
    pub task_id: Uuid,
    pub tenant_id: Uuid,
    pub agent_id: Uuid,
    pub prompt: String,
    pub status: lorelei_core::types::TaskStatus,
    pub schedule: lorelei_core::types::TaskSchedule,
    pub next_run_at: String,
    pub last_run_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

async fn create_task(
    State(state): State<AppState>,
    axum::extract::Extension(rid): axum::extract::Extension<RequestId>,
    headers: HeaderMap,
    Json(body): Json<CreateTaskRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = request_id_from_headers_or_ext(&headers, Some(&rid));

    if !body.daily {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: "only --daily schedules are supported in v1".to_string(),
            request_id,
        });
    }
    let Some(at) = body.at.as_deref() else {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: "at is required for daily schedule (HH:MM)".to_string(),
            request_id,
        });
    };

    let task = state
        .autonomy
        .add_daily_task(
            lorelei_core::types::TenantId(body.tenant_id),
            lorelei_core::types::AgentId(body.agent_id),
            &body.prompt,
            at,
        )
        .await
        .map_err(|e| map_err(e, request_id))?;

    Ok((StatusCode::CREATED, Json(to_task_response(task))))
}

#[derive(Debug, Deserialize)]
pub struct ListTasksQuery {
    pub tenant_id: Uuid,
    pub agent_id: Option<Uuid>,
}

async fn list_tasks(
    State(state): State<AppState>,
    axum::extract::Extension(rid): axum::extract::Extension<RequestId>,
    headers: HeaderMap,
    Query(q): Query<ListTasksQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = request_id_from_headers_or_ext(&headers, Some(&rid));
    let tasks = state
        .autonomy
        .list_tasks(
            lorelei_core::types::TenantId(q.tenant_id),
            q.agent_id.map(lorelei_core::types::AgentId),
        )
        .await
        .map_err(|e| map_err(e, request_id))?;

    let out: Vec<TaskResponse> = tasks.into_iter().map(to_task_response).collect();
    Ok(Json(out))
}

async fn pause_task(
    State(state): State<AppState>,
    axum::extract::Extension(rid): axum::extract::Extension<RequestId>,
    headers: HeaderMap,
    Path(task_id): Path<Uuid>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = request_id_from_headers_or_ext(&headers, Some(&rid));
    let tenant_id = q
        .get("tenant_id")
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: "tenant_id query param required".to_string(),
            request_id,
        })?;

    state
        .autonomy
        .pause_task(
            lorelei_core::types::TenantId(tenant_id),
            AutonomousTaskId(task_id),
        )
        .await
        .map_err(|e| map_err(e, request_id))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn resume_task(
    State(state): State<AppState>,
    axum::extract::Extension(rid): axum::extract::Extension<RequestId>,
    headers: HeaderMap,
    Path(task_id): Path<Uuid>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = request_id_from_headers_or_ext(&headers, Some(&rid));
    let tenant_id = q
        .get("tenant_id")
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: "tenant_id query param required".to_string(),
            request_id,
        })?;

    state
        .autonomy
        .resume_task(
            lorelei_core::types::TenantId(tenant_id),
            AutonomousTaskId(task_id),
        )
        .await
        .map_err(|e| map_err(e, request_id))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct ListApprovalsQuery {
    pub tenant_id: Uuid,
    pub state: Option<ApprovalState>,
}

#[derive(Debug, Serialize)]
pub struct ApprovalResponse {
    pub approval_id: Uuid,
    pub tenant_id: Uuid,
    pub agent_id: Uuid,
    pub task_id: Option<Uuid>,
    pub run_id: Uuid,
    pub tool: String,
    pub input: Value,
    pub risk: ShellRisk,
    pub state: ApprovalState,
    pub approval_prompt: String,
    pub created_at: String,
    pub decided_at: Option<String>,
}

async fn list_approvals(
    State(state): State<AppState>,
    axum::extract::Extension(rid): axum::extract::Extension<RequestId>,
    headers: HeaderMap,
    Query(q): Query<ListApprovalsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = request_id_from_headers_or_ext(&headers, Some(&rid));
    let approvals = state
        .autonomy
        .list_approvals(lorelei_core::types::TenantId(q.tenant_id), q.state)
        .await
        .map_err(|e| map_err(e, request_id))?;

    let out: Vec<ApprovalResponse> = approvals
        .into_iter()
        .map(|a| ApprovalResponse {
            approval_id: a.approval_id.0,
            tenant_id: a.tenant_id.0,
            agent_id: a.agent_id.0,
            task_id: a.task_id.map(|t| t.0),
            run_id: a.run_id.0,
            tool: a.tool,
            input: a.input,
            risk: a.risk,
            state: a.state,
            approval_prompt: a.approval_prompt,
            created_at: a.created_at.to_rfc3339(),
            decided_at: a.decided_at.map(|d| d.to_rfc3339()),
        })
        .collect();

    Ok(Json(out))
}

async fn approve_approval(
    State(state): State<AppState>,
    axum::extract::Extension(rid): axum::extract::Extension<RequestId>,
    headers: HeaderMap,
    Path(approval_id): Path<Uuid>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = request_id_from_headers_or_ext(&headers, Some(&rid));
    let tenant_id = q
        .get("tenant_id")
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: "tenant_id query param required".to_string(),
            request_id,
        })?;

    state
        .autonomy
        .approve(
            lorelei_core::types::TenantId(tenant_id),
            lorelei_core::types::ApprovalId(approval_id),
        )
        .await
        .map_err(|e| map_err(e, request_id))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn to_task_response(task: lorelei_core::types::AutonomousTask) -> TaskResponse {
    TaskResponse {
        task_id: task.task_id.0,
        tenant_id: task.tenant_id.0,
        agent_id: task.agent_id.0,
        prompt: task.prompt,
        status: task.status,
        schedule: task.schedule,
        next_run_at: task.next_run_at.to_rfc3339(),
        last_run_at: task.last_run_at.map(|d| d.to_rfc3339()),
        created_at: task.created_at.to_rfc3339(),
        updated_at: task.updated_at.to_rfc3339(),
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateRunRequest {
    pub tenant_id: Uuid,
    pub agent_id: Uuid,
    pub input: String,
    #[serde(default)]
    pub no_memory: bool,
}

#[derive(Debug, Serialize)]
pub struct RunResponse {
    pub run_id: Uuid,
    pub tenant_id: Uuid,
    pub agent_id: Uuid,
    pub status: RunStatus,
    pub output: Option<String>,
}

async fn create_run(
    State(state): State<AppState>,
    axum::extract::Extension(rid): axum::extract::Extension<RequestId>,
    headers: HeaderMap,
    Json(body): Json<CreateRunRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = request_id_from_headers_or_ext(&headers, Some(&rid));
    let res = state
        .tide
        .run_once_with_options(
            lorelei_core::types::TenantId(body.tenant_id),
            lorelei_core::types::AgentId(body.agent_id),
            body.input,
            !body.no_memory,
        )
        .await
        .map_err(|e| map_err(e, request_id))?;

    Ok((
        StatusCode::CREATED,
        Json(RunResponse {
            run_id: res.run_id.0,
            tenant_id: body.tenant_id,
            agent_id: body.agent_id,
            status: res.status,
            output: Some(res.output),
        }),
    ))
}

async fn get_run(
    State(state): State<AppState>,
    axum::extract::Extension(rid): axum::extract::Extension<RequestId>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = request_id_from_headers_or_ext(&headers, Some(&rid));
    let tenant_id = q
        .get("tenant_id")
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: "tenant_id query param required".to_string(),
            request_id,
        })?;
    let agent_id = q
        .get("agent_id")
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: "agent_id query param required".to_string(),
            request_id,
        })?;

    let got = state
        .currents
        .get_run(
            lorelei_core::types::TenantId(tenant_id),
            lorelei_core::types::AgentId(agent_id),
            lorelei_core::types::RunId(run_id),
        )
        .await
        .map_err(|e| map_err(e, request_id))?;

    let Some(run) = got else {
        return Err(ApiError {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: "run not found".to_string(),
            request_id,
        });
    };

    Ok(Json(RunResponse {
        run_id: run.run_id.0,
        tenant_id: run.tenant_id.0,
        agent_id: run.agent_id.0,
        status: run.status,
        output: None,
    }))
}

#[derive(Debug, Deserialize)]
pub struct CurrentsQuery {
    pub tenant_id: Uuid,
    pub agent_id: Uuid,
    #[serde(default = "default_currents_limit")]
    pub limit: usize,
}

fn default_currents_limit() -> usize {
    500
}

async fn list_currents(
    State(state): State<AppState>,
    axum::extract::Extension(rid): axum::extract::Extension<RequestId>,
    headers: HeaderMap,
    Path(run_id): Path<Uuid>,
    Query(q): Query<CurrentsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = request_id_from_headers_or_ext(&headers, Some(&rid));
    let currents = state
        .currents
        .list_current_events(
            lorelei_core::types::TenantId(q.tenant_id),
            lorelei_core::types::AgentId(q.agent_id),
            lorelei_core::types::RunId(run_id),
            q.limit,
        )
        .await
        .map_err(|e| map_err(e, request_id))?;
    Ok(Json(currents))
}

fn map_err(err: LoreleiError, request_id: Uuid) -> ApiError {
    match err {
        LoreleiError::Validation { message, .. } => ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "validation_error",
            message,
            request_id,
        },
        LoreleiError::NotFound(m) => ApiError {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: m,
            request_id,
        },
        LoreleiError::Unsupported(m) => ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "unsupported",
            message: m,
            request_id,
        },
        LoreleiError::Provider(m) => ApiError {
            status: StatusCode::BAD_GATEWAY,
            code: "provider_error",
            message: m,
            request_id,
        },
        LoreleiError::Shell(m) => ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "shell_error",
            message: m,
            request_id,
        },
        LoreleiError::Internal(m) => ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: m,
            request_id,
        },
    }
}

fn provider_kind_str(kind: &lorelei_core::config::ProviderKind) -> &'static str {
    use lorelei_core::config::ProviderKind as K;
    match kind {
        K::OpenaiCompatible => "openai-compatible",
        K::Anthropic => "anthropic",
        K::GeminiNative => "gemini-native",
        K::Bedrock => "bedrock",
        K::Local => "local",
        K::Mock => "mock",
    }
}
