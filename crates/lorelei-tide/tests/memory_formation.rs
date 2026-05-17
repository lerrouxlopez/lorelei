use async_trait::async_trait;
use lorelei_core::config::{
    AgentConfig, EchoConfig, HarborConfig, LoreConfig, LoreleiConfig, ProviderConfig, ProviderKind,
    SirenConfig,
};
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::{
    CurrentStore, EchoRetriever, LoreStore, ShellRegistry, SirenPolicy, SongProvider,
};
use lorelei_core::types::{
    AgentId, CurrentEvent, CurrentEventType, EchoHit, EchoQuery, EchoSources, NewPearl, Pearl,
    PearlId, PearlListQuery, PearlType, Run, RunId, RunStatus, ShellCall, ShellResult, SongChunk,
    SongRequest, SongResponse, TenantId, UnitInterval,
};
use lorelei_siren::policy::DeterministicSirenPolicy;
use lorelei_tide::runtime::{RunRepository, SingleAgentTideRuntime};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Default)]
struct MemRuns {
    runs: Mutex<Vec<Run>>,
}

#[async_trait]
impl RunRepository for MemRuns {
    async fn create_run(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        _goal: &str,
    ) -> Result<Run, LoreleiError> {
        let run = Run {
            run_id: RunId(Uuid::new_v4()),
            tenant_id,
            agent_id,
            status: RunStatus::Running,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        self.runs.lock().unwrap().push(run.clone());
        Ok(run)
    }

    async fn complete_run(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        run_id: RunId,
        status: RunStatus,
    ) -> Result<(), LoreleiError> {
        let mut runs = self.runs.lock().unwrap();
        let Some(r) = runs
            .iter_mut()
            .find(|r| r.run_id == run_id && r.tenant_id == tenant_id && r.agent_id == agent_id)
        else {
            return Err(LoreleiError::NotFound("run not found".to_string()));
        };
        r.status = status;
        r.updated_at = chrono::Utc::now();
        Ok(())
    }

    async fn get_run(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        run_id: RunId,
    ) -> Result<Option<Run>, LoreleiError> {
        Ok(self
            .runs
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.run_id == run_id && r.tenant_id == tenant_id && r.agent_id == agent_id)
            .cloned())
    }
}

#[derive(Default)]
struct MemCurrents {
    events: Mutex<Vec<CurrentEvent>>,
}

#[async_trait]
impl CurrentStore for MemCurrents {
    async fn append_current_event(
        &self,
        _tenant_id: TenantId,
        _agent_id: AgentId,
        _run_id: RunId,
        event: CurrentEvent,
    ) -> Result<(), LoreleiError> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }

    async fn list_current_events(
        &self,
        _tenant_id: TenantId,
        _agent_id: AgentId,
        _run_id: RunId,
        _limit: usize,
    ) -> Result<Vec<CurrentEvent>, LoreleiError> {
        Ok(self.events.lock().unwrap().clone())
    }
}

struct FixedEcho;

#[async_trait]
impl EchoRetriever for FixedEcho {
    async fn query(
        &self,
        _tenant_id: TenantId,
        _agent_id: AgentId,
        _query: EchoQuery,
    ) -> Result<Vec<EchoHit>, LoreleiError> {
        Ok(vec![])
    }
}

#[derive(Default)]
struct FakeShells;

#[async_trait]
impl ShellRegistry for FakeShells {
    async fn list_shells(&self) -> Result<Vec<String>, LoreleiError> {
        Ok(vec![])
    }

    async fn call(&self, call: ShellCall) -> Result<ShellResult, LoreleiError> {
        let _ = call;
        Err(LoreleiError::Unsupported("no shells".to_string()))
    }
}

#[derive(Default)]
struct MemLoreStore {
    pearls: Mutex<HashMap<PearlId, Pearl>>,
}

impl MemLoreStore {
    fn count(&self) -> usize {
        self.pearls.lock().unwrap().len()
    }

    fn all(&self) -> Vec<Pearl> {
        self.pearls.lock().unwrap().values().cloned().collect()
    }
}

#[async_trait]
impl LoreStore for MemLoreStore {
    async fn save_pearl(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        pearl: NewPearl,
    ) -> Result<Pearl, LoreleiError> {
        let saved = Pearl {
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
        self.pearls
            .lock()
            .unwrap()
            .insert(saved.pearl_id, saved.clone());
        Ok(saved)
    }

    async fn get_pearl(
        &self,
        tenant_id: TenantId,
        pearl_id: PearlId,
        _include_deleted: bool,
    ) -> Result<Option<Pearl>, LoreleiError> {
        Ok(self
            .pearls
            .lock()
            .unwrap()
            .get(&pearl_id)
            .filter(|p| p.tenant_id == tenant_id)
            .cloned())
    }

    async fn list_pearls(
        &self,
        tenant_id: TenantId,
        query: PearlListQuery,
    ) -> Result<Vec<Pearl>, LoreleiError> {
        let pearls = self.pearls.lock().unwrap();
        let mut out: Vec<Pearl> = pearls
            .values()
            .filter(|p| p.tenant_id == tenant_id)
            .filter(|p| query.agent_id.map(|a| p.agent_id == a).unwrap_or(true))
            .filter(|p| query.pearl_type.map(|t| p.pearl_type == t).unwrap_or(true))
            .cloned()
            .collect();
        if let Some(limit) = query.limit {
            out.truncate(limit);
        }
        Ok(out)
    }

    async fn forget_pearl(
        &self,
        _tenant_id: TenantId,
        _pearl_id: PearlId,
    ) -> Result<(), LoreleiError> {
        Ok(())
    }

    async fn update_last_echoed_at(
        &self,
        _tenant_id: TenantId,
        _pearl_id: PearlId,
        _last_echoed_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), LoreleiError> {
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

#[async_trait]
impl SongProvider for ScriptedSong {
    fn capabilities(&self) -> lorelei_core::types::ProviderCapabilities {
        Default::default()
    }

    async fn complete(&self, _request: SongRequest) -> Result<SongResponse, LoreleiError> {
        let mut r = self.responses.lock().unwrap();
        let out = r.remove(0);
        Ok(SongResponse {
            output: out,
            reasoning_summary: None,
            tool_calls: vec![],
        })
    }

    async fn stream(
        &self,
        _request: SongRequest,
    ) -> Result<futures::stream::BoxStream<'static, SongChunk>, LoreleiError> {
        Err(LoreleiError::Unsupported(
            "stream not implemented".to_string(),
        ))
    }

    async fn embed(
        &self,
        _request: lorelei_core::types::EmbeddingRequest,
    ) -> Result<lorelei_core::types::EmbeddingResponse, LoreleiError> {
        Err(LoreleiError::Unsupported(
            "embed not implemented".to_string(),
        ))
    }
}

fn cfg() -> LoreleiConfig {
    let tenant_id = TenantId(Uuid::from_u128(1));
    let agent_id = AgentId(Uuid::from_u128(2));
    let mut providers = BTreeMap::new();
    providers.insert(
        "mock".to_string(),
        ProviderConfig {
            kind: ProviderKind::Mock,
            base_url: Some("http://127.0.0.1:0".to_string()),
            api_key_env: "IGNORED".to_string(),
            chat_model: "mock".to_string(),
            embedding_model: Some("mock".to_string()),
        },
    );
    LoreleiConfig {
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
            postgres_url_env: "PG".to_string(),
            qdrant_url_env: "QD".to_string(),
            collection: "lorelei".to_string(),
        },
        echo: EchoConfig {
            top_k: 10,
            rerank_top_k: 5,
            min_confidence: None,
        },
        siren: SirenConfig {
            require_approval_for_high_risk: true,
            allow_shell_execution: false,
            allow_network_tools: false,
        },
        docs: Default::default(),
        providers,
    }
}

fn tide_with(
    song: Arc<dyn SongProvider>,
    lore: Arc<dyn LoreStore>,
    currents: Arc<MemCurrents>,
) -> SingleAgentTideRuntime {
    std::env::set_var("LORELEI_DETERMINISTIC_EXTRACT", "0");

    let cfg = cfg();
    let runs = Arc::new(MemRuns::default());
    let echo: Arc<dyn EchoRetriever> = Arc::new(FixedEcho);
    let shells: Arc<dyn ShellRegistry> = Arc::new(FakeShells);
    let siren: Arc<dyn SirenPolicy> = Arc::new(DeterministicSirenPolicy::new(cfg.clone()));

    SingleAgentTideRuntime::new(cfg, runs, currents, echo, lore, song, shells, siren)
        .with_templates(
            r#"{"action":"answer","answer":"ok"}"#,
            r#"LORELEI_MODE=answer {{USER_INPUT}}"#,
        )
}

#[tokio::test]
async fn stable_preference_is_stored() {
    let lore = Arc::new(MemLoreStore::default());
    let currents = Arc::new(MemCurrents::default());
    let song: Arc<dyn SongProvider> = Arc::new(ScriptedSong::new(vec![
        r#"{"action":"answer","answer":"ok"}"#.to_string(),
        "ok".to_string(),
        r#"[{"pearl_type":"Preference","content":"User prefers concise output.","confidence":0.9,"importance":0.7,"tags":["preference"]}]"#.to_string(),
    ]));

    let tide = tide_with(song, lore.clone(), currents.clone());
    let res = tide
        .run_once(
            TenantId(Uuid::from_u128(1)),
            AgentId(Uuid::from_u128(2)),
            "hi".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(res.status, RunStatus::Succeeded);
    assert_eq!(lore.count(), 1);
    assert!(lore.all()[0].content.contains("concise output"));

    let events = currents.events.lock().unwrap();
    assert!(events
        .iter()
        .any(|e| e.event_type == CurrentEventType::System && e.summary == "memory formation"));
}

#[tokio::test]
async fn temporary_task_is_rejected() {
    let lore = Arc::new(MemLoreStore::default());
    let currents = Arc::new(MemCurrents::default());
    let song: Arc<dyn SongProvider> = Arc::new(ScriptedSong::new(vec![
        r#"{"action":"answer","answer":"ok"}"#.to_string(),
        "ok".to_string(),
        r#"[{"pearl_type":"Plan","content":"Remind me tomorrow to submit the report.","confidence":0.9,"importance":0.7}]"#.to_string(),
    ]));

    let tide = tide_with(song, lore.clone(), currents.clone());
    let _ = tide
        .run_once(
            TenantId(Uuid::from_u128(1)),
            AgentId(Uuid::from_u128(2)),
            "hi".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(lore.count(), 0);
}

#[tokio::test]
async fn duplicate_memory_is_rejected_by_exact_match() {
    let lore = Arc::new(MemLoreStore::default());
    let currents = Arc::new(MemCurrents::default());

    let existing = NewPearl::new(
        PearlType::Fact,
        "The reef uses Postgres.",
        UnitInterval::new(0.5).unwrap(),
        UnitInterval::new(0.9).unwrap(),
        Default::default(),
    )
    .unwrap();
    let _ = lore
        .save_pearl(
            TenantId(Uuid::from_u128(1)),
            AgentId(Uuid::from_u128(2)),
            existing,
        )
        .await
        .unwrap();

    let song: Arc<dyn SongProvider> = Arc::new(ScriptedSong::new(vec![
        r#"{"action":"answer","answer":"ok"}"#.to_string(),
        "ok".to_string(),
        r#"[{"pearl_type":"Fact","content":"  the   reef  uses  postgres. ","confidence":0.9,"importance":0.7}]"#.to_string(),
    ]));

    let tide = tide_with(song, lore.clone(), currents);
    let _ = tide
        .run_once(
            TenantId(Uuid::from_u128(1)),
            AgentId(Uuid::from_u128(2)),
            "hi".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(lore.count(), 1);
}

#[tokio::test]
async fn explicit_remember_request_is_stored() {
    let lore = Arc::new(MemLoreStore::default());
    let currents = Arc::new(MemCurrents::default());
    let song: Arc<dyn SongProvider> = Arc::new(ScriptedSong::new(vec![
        r#"{"action":"answer","answer":"ok"}"#.to_string(),
        "ok".to_string(),
        r#"[{"pearl_type":"Preference","content":"User prefers tea.","confidence":0.9,"importance":0.7}]"#.to_string(),
    ]));

    let tide = tide_with(song, lore.clone(), currents);
    let _ = tide
        .run_once(
            TenantId(Uuid::from_u128(1)),
            AgentId(Uuid::from_u128(2)),
            "Remember that I prefer tea.".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(lore.count(), 1);
    assert!(lore.all()[0].content.contains("prefers tea"));
}

#[tokio::test]
async fn sensitive_unsupported_memory_is_rejected() {
    let lore = Arc::new(MemLoreStore::default());
    let currents = Arc::new(MemCurrents::default());
    let song: Arc<dyn SongProvider> = Arc::new(ScriptedSong::new(vec![
        r#"{"action":"answer","answer":"ok"}"#.to_string(),
        "ok".to_string(),
        r#"[{"pearl_type":"Fact","content":"My password is hunter2.","confidence":0.9,"importance":0.7}]"#.to_string(),
    ]));

    let tide = tide_with(song, lore.clone(), currents);
    let _ = tide
        .run_once(
            TenantId(Uuid::from_u128(1)),
            AgentId(Uuid::from_u128(2)),
            "hi".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(lore.count(), 0);
}

async fn maybe_pg() -> Option<sqlx::PgPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return None,
    };
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .ok()
}

fn maybe_qdrant() -> Option<qdrant_client::Qdrant> {
    let url = match std::env::var("QDRANT_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return None,
    };
    qdrant_client::Qdrant::from_url(&url).build().ok()
}

#[tokio::test]
async fn accepted_pearl_is_searchable_by_echo_integration() {
    let Some(pool) = maybe_pg().await else { return };
    let Some(client) = maybe_qdrant() else { return };

    let tenant_id = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());
    let collection = format!("lorelei_mem_test_{}", Uuid::new_v4());

    let index = lorelei_lore::qdrant::QdrantPearlIndex::new(client, collection);
    let embedder: Arc<dyn lorelei_lore::embedding::EmbeddingProvider> =
        Arc::new(lorelei_lore::embedding::DeterministicMockEmbeddingProvider::new(64));

    let pool_tide = pool.clone();
    let pool_echo = pool.clone();
    let pool_verify = pool;

    // Separate store instances backed by same Postgres/Qdrant for Tide vs Echo verification.
    let tide_store = lorelei_lore::pg::PgLoreStore::new_indexed(
        pool_tide,
        index.clone(),
        embedder.clone(),
        "mock",
    );
    tide_store.migrate().await.expect("migrate");

    let echo_store = lorelei_lore::pg::PgLoreStore::new_indexed(
        pool_echo,
        index.clone(),
        embedder.clone(),
        "mock",
    );

    let currents = Arc::new(MemCurrents::default());
    let cfg = cfg();
    let runs = Arc::new(MemRuns::default());
    let shells: Arc<dyn ShellRegistry> = Arc::new(FakeShells);
    let siren: Arc<dyn SirenPolicy> = Arc::new(DeterministicSirenPolicy::new(cfg.clone()));

    let song: Arc<dyn SongProvider> = Arc::new(ScriptedSong::new(vec![
        r#"{"action":"answer","answer":"ok"}"#.to_string(),
        "ok".to_string(),
        r#"[{"pearl_type":"Fact","content":"The Lore starts in Postgres.","confidence":0.9,"importance":0.7}]"#.to_string(),
    ]));

    let tide = SingleAgentTideRuntime::new(
        cfg,
        runs,
        currents,
        Arc::new(lorelei_echo::retriever::EchoEngine::new(
            echo_store,
            index.clone(),
            embedder.clone(),
            "mock",
            lorelei_echo::retriever::EchoRetrievalConfig {
                rerank_top_k: 10,
                enable_query_rewrite: false,
            },
        )),
        Arc::new(tide_store),
        song,
        shells,
        siren,
    )
    .with_templates(
        r#"{"action":"answer","answer":"ok"}"#,
        r#"LORELEI_MODE=answer {{USER_INPUT}}"#,
    );

    let _ = tide
        .run_once(tenant_id, agent_id, "hi".to_string())
        .await
        .expect("run");

    let verifier_store = lorelei_lore::pg::PgLoreStore::new_indexed(
        pool_verify,
        index.clone(),
        embedder.clone(),
        "mock",
    );
    let verifier = lorelei_echo::retriever::EchoEngine::new(
        verifier_store,
        index,
        embedder,
        "mock",
        lorelei_echo::retriever::EchoRetrievalConfig {
            rerank_top_k: 10,
            enable_query_rewrite: false,
        },
    );

    let hits = verifier
        .query(
            tenant_id,
            agent_id,
            EchoQuery {
                query: "starts in postgres".to_string(),
                top_k: 10,
                min_confidence: Some(UnitInterval::new(0.0).unwrap()),
                pearl_type: None,
                sources: EchoSources::Pearls,
            },
        )
        .await
        .expect("echo");

    assert!(!hits.is_empty());
    assert!(hits.iter().any(|h| h.content.contains("Postgres")));
}
