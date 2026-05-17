use lorelei_core::config::{
    AgentConfig, EchoConfig, HarborConfig, LoreConfig, LoreleiConfig, ProviderConfig, ProviderKind,
    SirenConfig,
};
use lorelei_core::traits::DocumentStore;
use lorelei_core::traits::{EchoRetriever, LoreStore};
use lorelei_core::types::{
    AgentId, EchoHit, EchoQuery, NewPearl, Pearl, PearlId, PearlListQuery, TenantId,
};
use lorelei_harbor::http::server::AppState;
use lorelei_harbor::runtime::autonomy::PgAutonomy;
use lorelei_harbor::runtime::pg::PgCurrentStore;
use lorelei_harbor::worker::{run_with_state, WorkerConfig};
use lorelei_shells::registry::BuiltinShellRegistry;
use lorelei_shells::repo::NullShellCallRepository;
use lorelei_siren::policy::DeterministicSirenPolicy;
use lorelei_song::providers::mock::MockSongProvider;
use lorelei_tide::runtime::SingleAgentTideRuntime;
use qdrant_client::Qdrant;
use sqlx::postgres::PgPoolOptions;
use sqlx::Row;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone, Default)]
struct NullDocs;

#[async_trait::async_trait]
impl DocumentStore for NullDocs {
    async fn ingest_document_path(
        &self,
        _tenant_id: lorelei_core::types::TenantId,
        _agent_id: lorelei_core::types::AgentId,
        _path: &std::path::Path,
    ) -> Result<Uuid, lorelei_core::error::LoreleiError> {
        Err(lorelei_core::error::LoreleiError::Unsupported(
            "docs not available".to_string(),
        ))
    }

    async fn get_document_chunk_for_echo(
        &self,
        _tenant_id: lorelei_core::types::TenantId,
        _chunk_id: Uuid,
    ) -> Result<
        Option<(
            String,
            lorelei_core::types::EchoCitation,
            chrono::DateTime<chrono::Utc>,
        )>,
        lorelei_core::error::LoreleiError,
    > {
        Ok(None)
    }

    async fn soft_delete_document(
        &self,
        _tenant_id: lorelei_core::types::TenantId,
        _document_id: Uuid,
    ) -> Result<(), lorelei_core::error::LoreleiError> {
        Ok(())
    }
}

struct ScriptedSong {
    responses: Mutex<Vec<String>>,
}

impl ScriptedSong {
    fn new(responses: Vec<String>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }
}

#[async_trait::async_trait]
impl lorelei_core::traits::SongProvider for ScriptedSong {
    fn capabilities(&self) -> lorelei_core::types::ProviderCapabilities {
        Default::default()
    }

    async fn complete(
        &self,
        _request: lorelei_core::types::SongRequest,
    ) -> Result<lorelei_core::types::SongResponse, lorelei_core::error::LoreleiError> {
        let mut r = self.responses.lock().unwrap();
        let out = r.remove(0);
        Ok(lorelei_core::types::SongResponse {
            output: out,
            reasoning_summary: None,
            tool_calls: vec![],
        })
    }

    async fn stream(
        &self,
        _request: lorelei_core::types::SongRequest,
    ) -> Result<
        futures::stream::BoxStream<'static, lorelei_core::types::SongChunk>,
        lorelei_core::error::LoreleiError,
    > {
        Err(lorelei_core::error::LoreleiError::Unsupported(
            "stream not implemented".to_string(),
        ))
    }

    async fn embed(
        &self,
        _request: lorelei_core::types::EmbeddingRequest,
    ) -> Result<lorelei_core::types::EmbeddingResponse, lorelei_core::error::LoreleiError> {
        Err(lorelei_core::error::LoreleiError::Unsupported(
            "embed not implemented".to_string(),
        ))
    }
}

#[derive(Default)]
struct MemLoreStore {
    pearls: Mutex<Vec<Pearl>>,
}

#[async_trait::async_trait]
impl LoreStore for MemLoreStore {
    async fn save_pearl(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        pearl: NewPearl,
    ) -> Result<Pearl, lorelei_core::error::LoreleiError> {
        let p = Pearl {
            pearl_id: PearlId(Uuid::new_v4()),
            tenant_id,
            agent_id,
            pearl_type: pearl.pearl_type,
            content: pearl.content,
            importance: pearl.importance,
            confidence: pearl.confidence,
            created_at: chrono::Utc::now(),
            metadata: pearl.metadata,
        };
        self.pearls.lock().unwrap().push(p.clone());
        Ok(p)
    }

    async fn get_pearl(
        &self,
        tenant_id: TenantId,
        pearl_id: PearlId,
        _include_deleted: bool,
    ) -> Result<Option<Pearl>, lorelei_core::error::LoreleiError> {
        Ok(self
            .pearls
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.tenant_id == tenant_id && p.pearl_id == pearl_id)
            .cloned())
    }

    async fn list_pearls(
        &self,
        tenant_id: TenantId,
        _query: PearlListQuery,
    ) -> Result<Vec<Pearl>, lorelei_core::error::LoreleiError> {
        Ok(self
            .pearls
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.tenant_id == tenant_id)
            .cloned()
            .collect())
    }

    async fn forget_pearl(
        &self,
        _tenant_id: TenantId,
        _pearl_id: PearlId,
    ) -> Result<(), lorelei_core::error::LoreleiError> {
        Ok(())
    }

    async fn update_last_echoed_at(
        &self,
        _tenant_id: TenantId,
        _pearl_id: PearlId,
        _last_echoed_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), lorelei_core::error::LoreleiError> {
        Ok(())
    }
}

struct EmptyEcho;

#[async_trait::async_trait]
impl EchoRetriever for EmptyEcho {
    async fn query(
        &self,
        _tenant_id: TenantId,
        _agent_id: AgentId,
        _query: EchoQuery,
    ) -> Result<Vec<EchoHit>, lorelei_core::error::LoreleiError> {
        Ok(Vec::new())
    }
}

async fn maybe_state() -> Option<AppState> {
    let pg = std::env::var("DATABASE_URL").ok()?;

    let tenant_id = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());

    let mut providers = BTreeMap::new();
    providers.insert(
        "mock".to_string(),
        ProviderConfig {
            kind: ProviderKind::Mock,
            base_url: Some("http://127.0.0.1:0".to_string()),
            api_key_env: "IGNORED".to_string(),
            chat_model: "mock-chat".to_string(),
            embedding_model: Some("mock-embed".to_string()),
        },
    );

    let cfg = LoreleiConfig {
        agent: AgentConfig {
            tenant_id,
            agent_id,
            default_provider: "mock".to_string(),
            default_embedding_provider: "mock".to_string(),
        },
        harbor: HarborConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
        },
        lore: LoreConfig {
            postgres_url_env: "DATABASE_URL".to_string(),
            qdrant_url_env: "QDRANT_URL".to_string(),
            collection: "lorelei".to_string(),
        },
        echo: EchoConfig {
            top_k: 5,
            rerank_top_k: 5,
            min_confidence: None,
        },
        siren: SirenConfig {
            require_approval_for_high_risk: true,
            allow_shell_execution: true,
            allow_network_tools: false,
        },
        docs: Default::default(),
        providers,
    };

    let pg_pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&pg)
        .await
        .ok()?;

    let qdrant = Qdrant::from_url("http://127.0.0.1:1").build().ok()?;
    let qdrant_index = lorelei_lore::qdrant::QdrantPearlIndex::new(qdrant.clone(), "lorelei");

    let currents = Arc::new(PgCurrentStore::new(pg_pool.clone()));
    currents.migrate().await.ok()?;

    let autonomy = Arc::new(PgAutonomy::new(pg_pool.clone()));

    let song: Arc<dyn lorelei_core::traits::SongProvider> =
        Arc::new(MockSongProvider::deterministic());
    let lore_store: Arc<dyn LoreStore> = Arc::new(MemLoreStore::default());
    let echo: Arc<dyn EchoRetriever> = Arc::new(EmptyEcho);
    let shells = Arc::new(BuiltinShellRegistry::new(
        cfg.clone(),
        lore_store.clone(),
        echo.clone(),
        Arc::new(NullDocs),
        Arc::new(NullShellCallRepository),
    ));

    let siren =
        Arc::new(DeterministicSirenPolicy::new(cfg.clone()).with_approval_store(autonomy.clone()));

    let tide: Arc<SingleAgentTideRuntime> = Arc::new(SingleAgentTideRuntime::new(
        cfg.clone(),
        currents.clone(),
        currents.clone(),
        echo.clone(),
        lore_store.clone(),
        song,
        shells.clone(),
        siren.clone(),
    ));

    Some(AppState {
        config: cfg,
        pg_pool,
        qdrant,
        qdrant_index,
        lore_store,
        echo,
        providers: Arc::new(lorelei_song::registry::ProviderRegistry::from_providers(
            BTreeMap::new(),
        )),
        shells,
        currents,
        siren,
        tide,
        autonomy,
        documents: Arc::new(NullDocs),
    })
}

async fn maybe_state_with_song(
    song: Arc<dyn lorelei_core::traits::SongProvider>,
) -> Option<AppState> {
    let pg = std::env::var("DATABASE_URL").ok()?;

    let tenant_id = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());

    let mut providers = BTreeMap::new();
    providers.insert(
        "mock".to_string(),
        ProviderConfig {
            kind: ProviderKind::Mock,
            base_url: Some("http://127.0.0.1:0".to_string()),
            api_key_env: "IGNORED".to_string(),
            chat_model: "mock-chat".to_string(),
            embedding_model: Some("mock-embed".to_string()),
        },
    );

    let cfg = LoreleiConfig {
        agent: AgentConfig {
            tenant_id,
            agent_id,
            default_provider: "mock".to_string(),
            default_embedding_provider: "mock".to_string(),
        },
        harbor: HarborConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
        },
        lore: LoreConfig {
            postgres_url_env: "DATABASE_URL".to_string(),
            qdrant_url_env: "QDRANT_URL".to_string(),
            collection: "lorelei".to_string(),
        },
        echo: EchoConfig {
            top_k: 5,
            rerank_top_k: 5,
            min_confidence: None,
        },
        siren: SirenConfig {
            require_approval_for_high_risk: true,
            allow_shell_execution: true,
            allow_network_tools: false,
        },
        docs: Default::default(),
        providers,
    };

    let pg_pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&pg)
        .await
        .ok()?;

    let qdrant = Qdrant::from_url("http://127.0.0.1:1").build().ok()?;
    let qdrant_index = lorelei_lore::qdrant::QdrantPearlIndex::new(qdrant.clone(), "lorelei");

    let currents = Arc::new(PgCurrentStore::new(pg_pool.clone()));
    currents.migrate().await.ok()?;

    let autonomy = Arc::new(PgAutonomy::new(pg_pool.clone()));

    let lore_store: Arc<dyn LoreStore> = Arc::new(MemLoreStore::default());
    let echo: Arc<dyn EchoRetriever> = Arc::new(EmptyEcho);
    let shells = Arc::new(BuiltinShellRegistry::new(
        cfg.clone(),
        lore_store.clone(),
        echo.clone(),
        Arc::new(NullDocs),
        Arc::new(NullShellCallRepository),
    ));
    let siren =
        Arc::new(DeterministicSirenPolicy::new(cfg.clone()).with_approval_store(autonomy.clone()));

    let tide: Arc<SingleAgentTideRuntime> = Arc::new(SingleAgentTideRuntime::new(
        cfg.clone(),
        currents.clone(),
        currents.clone(),
        echo.clone(),
        lore_store.clone(),
        song,
        shells.clone(),
        siren.clone(),
    ));

    Some(AppState {
        config: cfg,
        pg_pool,
        qdrant,
        qdrant_index,
        lore_store,
        echo,
        providers: Arc::new(lorelei_song::registry::ProviderRegistry::from_providers(
            BTreeMap::new(),
        )),
        shells,
        currents,
        siren,
        tide,
        autonomy,
        documents: Arc::new(NullDocs),
    })
}

#[tokio::test]
async fn due_task_creates_a_run() {
    let Some(state) = maybe_state().await else {
        return;
    };
    let state = Arc::new(state);

    let task = state
        .autonomy
        .add_daily_task(
            state.config.agent.tenant_id,
            state.config.agent.agent_id,
            "hello",
            "00:00",
        )
        .await
        .expect("add task");

    // Force due now.
    sqlx::query(
        "update autonomous_tasks set next_run_at = now() - interval '1 minute' where id = $1",
    )
    .bind(task.task_id.0)
    .execute(&state.pg_pool)
    .await
    .expect("mark due");

    run_with_state(
        state.clone(),
        WorkerConfig {
            once: true,
            poll_every: Duration::from_millis(1),
            ..Default::default()
        },
    )
    .await
    .expect("worker");

    let c: i64 = sqlx::query_scalar("select count(*) from task_run_links where task_id = $1")
        .bind(task.task_id.0)
        .fetch_one(&state.pg_pool)
        .await
        .expect("count links");
    assert!(c >= 1);
}

#[tokio::test]
async fn paused_task_does_not_run() {
    let Some(state) = maybe_state().await else {
        return;
    };
    let state = Arc::new(state);
    let task = state
        .autonomy
        .add_daily_task(
            state.config.agent.tenant_id,
            state.config.agent.agent_id,
            "hello",
            "00:00",
        )
        .await
        .expect("add task");

    sqlx::query("update autonomous_tasks set next_run_at = now() - interval '1 minute', status = 'paused' where id = $1")
        .bind(task.task_id.0)
        .execute(&state.pg_pool)
        .await
        .expect("pause");

    run_with_state(
        state.clone(),
        WorkerConfig {
            once: true,
            poll_every: Duration::from_millis(1),
            ..Default::default()
        },
    )
    .await
    .expect("worker");

    let c: i64 = sqlx::query_scalar("select count(*) from task_run_links where task_id = $1")
        .bind(task.task_id.0)
        .fetch_one(&state.pg_pool)
        .await
        .expect("count links");
    assert_eq!(c, 0);
}

#[tokio::test]
async fn worker_is_idempotent_when_task_is_leased() {
    let Some(state) = maybe_state().await else {
        return;
    };
    let state = Arc::new(state);
    let task = state
        .autonomy
        .add_daily_task(
            state.config.agent.tenant_id,
            state.config.agent.agent_id,
            "hello",
            "00:00",
        )
        .await
        .expect("add task");

    sqlx::query(
        "update autonomous_tasks set next_run_at = now() - interval '1 minute' where id = $1",
    )
    .bind(task.task_id.0)
    .execute(&state.pg_pool)
    .await
    .expect("mark due");

    // Manually claim it with a lease so the worker should not run it.
    let claimed = state
        .autonomy
        .claim_due_tasks(Uuid::new_v4(), 1, chrono::Duration::seconds(60))
        .await
        .expect("claim");
    assert_eq!(claimed.len(), 1);

    run_with_state(
        state.clone(),
        WorkerConfig {
            once: true,
            poll_every: Duration::from_millis(1),
            ..Default::default()
        },
    )
    .await
    .expect("worker");

    let c: i64 = sqlx::query_scalar("select count(*) from task_run_links where task_id = $1")
        .bind(task.task_id.0)
        .fetch_one(&state.pg_pool)
        .await
        .expect("count links");
    assert_eq!(c, 0);
}

#[tokio::test]
async fn high_risk_task_creates_approval_and_stops() {
    let song: Arc<dyn lorelei_core::traits::SongProvider> = Arc::new(ScriptedSong::new(vec![
        r#"{"action":"call_shell","tool":"forget_pearl","input":{"pearl_id":"00000000-0000-0000-0000-000000000000"}}"#.to_string(),
    ]));
    let Some(state) = maybe_state_with_song(song).await else {
        return;
    };
    let state = Arc::new(state);

    let task = state
        .autonomy
        .add_daily_task(
            state.config.agent.tenant_id,
            state.config.agent.agent_id,
            "forget it",
            "00:00",
        )
        .await
        .expect("add task");

    sqlx::query(
        "update autonomous_tasks set next_run_at = now() - interval '1 minute' where id = $1",
    )
    .bind(task.task_id.0)
    .execute(&state.pg_pool)
    .await
    .expect("mark due");

    run_with_state(
        state.clone(),
        WorkerConfig {
            once: false,
            poll_every: Duration::from_millis(1),
            ..Default::default()
        },
    )
    .await
    .expect("worker stops on approval");

    let c: i64 = sqlx::query_scalar(
        "select count(*) from approvals where task_id = $1 and state = 'pending'",
    )
    .bind(task.task_id.0)
    .fetch_one(&state.pg_pool)
    .await
    .expect("count approvals");
    assert_eq!(c, 1);
}

#[tokio::test]
async fn failed_task_records_error_and_backoff() {
    // Return invalid JSON for planner to force failure.
    let song: Arc<dyn lorelei_core::traits::SongProvider> =
        Arc::new(ScriptedSong::new(vec!["not-json".to_string()]));
    let Some(state) = maybe_state_with_song(song).await else {
        return;
    };
    let state = Arc::new(state);

    let task = state
        .autonomy
        .add_daily_task(
            state.config.agent.tenant_id,
            state.config.agent.agent_id,
            "bad",
            "00:00",
        )
        .await
        .expect("add task");

    sqlx::query(
        "update autonomous_tasks set next_run_at = now() - interval '1 minute' where id = $1",
    )
    .bind(task.task_id.0)
    .execute(&state.pg_pool)
    .await
    .expect("mark due");

    run_with_state(
        state.clone(),
        WorkerConfig {
            once: true,
            poll_every: Duration::from_millis(1),
            ..Default::default()
        },
    )
    .await
    .expect("worker");

    let row = sqlx::query(
        "select consecutive_failures, last_error, next_run_at from autonomous_tasks where id = $1",
    )
    .bind(task.task_id.0)
    .fetch_one(&state.pg_pool)
    .await
    .expect("task row");
    let failures: i32 = row.get("consecutive_failures");
    let last_error: Option<String> = row.get("last_error");
    let next_run_at: chrono::DateTime<chrono::Utc> = row.get("next_run_at");
    assert!(failures >= 1);
    assert!(!last_error.unwrap_or_default().is_empty());
    assert!(next_run_at > chrono::Utc::now());
}
