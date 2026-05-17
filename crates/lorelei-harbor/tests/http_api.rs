use axum::body::Body;
use axum::http::{Request, StatusCode};
use lorelei_core::config::{
    AgentConfig, EchoConfig, HarborConfig, LoreConfig, LoreleiConfig, ProviderConfig, ProviderKind,
    SirenConfig,
};
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::DocumentStore;
use lorelei_core::traits::{EchoRetriever, LoreStore};
use lorelei_core::types::{
    AgentId, EchoHit, EchoQuery, NewPearl, Pearl, PearlId, PearlListQuery, TenantId, UnitInterval,
};
use lorelei_echo::retriever::{EchoEngine, EchoRetrievalConfig};
use lorelei_harbor::http::server::{router, AppState};
use lorelei_harbor::runtime::autonomy::PgAutonomy;
use lorelei_harbor::runtime::pg::PgCurrentStore;
use lorelei_lore::pg::PgLoreStore;
use lorelei_shells::registry::BuiltinShellRegistry;
use lorelei_shells::repo::NullShellCallRepository;
use lorelei_siren::policy::DeterministicSirenPolicy;
use lorelei_song::providers::mock::MockSongProvider;
use lorelei_tide::runtime::SingleAgentTideRuntime;
use qdrant_client::Qdrant;
use sqlx::postgres::PgPoolOptions;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

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
    ) -> Result<Pearl, LoreleiError> {
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
    ) -> Result<Option<Pearl>, LoreleiError> {
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
    ) -> Result<Vec<Pearl>, LoreleiError> {
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

struct EmptyEcho;

#[async_trait::async_trait]
impl EchoRetriever for EmptyEcho {
    async fn query(
        &self,
        _tenant_id: TenantId,
        _agent_id: AgentId,
        _query: EchoQuery,
    ) -> Result<Vec<EchoHit>, LoreleiError> {
        Ok(Vec::new())
    }
}

fn minimal_state() -> AppState {
    let tenant_id = TenantId(Uuid::from_u128(1));
    let agent_id = AgentId(Uuid::from_u128(2));

    let mut providers = BTreeMap::new();
    providers.insert(
        "mock".to_string(),
        ProviderConfig {
            kind: ProviderKind::Mock,
            base_url: None,
            api_key_env: "IGNORED".to_string(),
            chat_model: "mock".to_string(),
            embedding_model: Some("mock".to_string()),
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
    };

    let pg_pool = PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(200))
        .connect_lazy("postgres://lorelei:lorelei@127.0.0.1:1/lorelei")
        .unwrap();
    let qdrant = Qdrant::from_url("http://127.0.0.1:1").build().unwrap();

    let currents = Arc::new(PgCurrentStore::new(pg_pool.clone()));
    let siren = Arc::new(DeterministicSirenPolicy::new(cfg.clone()));
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

    let autonomy = Arc::new(PgAutonomy::new(pg_pool.clone()));

    AppState {
        config: cfg,
        pg_pool,
        qdrant: qdrant.clone(),
        qdrant_index: lorelei_lore::qdrant::QdrantPearlIndex::new(qdrant, "lorelei"),
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
    }
}

async fn env_state() -> Option<AppState> {
    let pg = std::env::var("DATABASE_URL").ok()?;
    let qd = std::env::var("QDRANT_URL").ok()?;

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
            collection: format!("lorelei_http_test_{}", Uuid::new_v4()),
        },
        echo: EchoConfig {
            top_k: 10,
            rerank_top_k: 10,
            min_confidence: Some(UnitInterval::new(0.0).unwrap()),
        },
        siren: SirenConfig {
            require_approval_for_high_risk: true,
            allow_shell_execution: false,
            allow_network_tools: false,
        },
        docs: Default::default(),
        providers,
    };

    let pg_pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_lazy(&pg)
        .ok()?;
    let qdrant = Qdrant::from_url(&qd).build().ok()?;
    let qdrant_index =
        lorelei_lore::qdrant::QdrantPearlIndex::new(qdrant.clone(), cfg.lore.collection.clone());

    // Use the deterministic embedder directly.
    let embedder: Arc<dyn lorelei_lore::embedding::EmbeddingProvider> =
        Arc::new(lorelei_lore::embedding::DeterministicMockEmbeddingProvider::new(64));

    let lore_store = PgLoreStore::new_indexed(
        pg_pool.clone(),
        qdrant_index.clone(),
        embedder.clone(),
        "mock",
    );

    // Ensure schema exists for tests.
    lore_store.migrate().await.ok()?;
    qdrant_index.ensure_collection(64).await.ok()?;

    let echo_engine = Arc::new(EchoEngine::new(
        PgLoreStore::new(pg_pool.clone()),
        qdrant_index.clone(),
        embedder,
        "mock",
        EchoRetrievalConfig {
            rerank_top_k: cfg.echo.rerank_top_k,
            enable_query_rewrite: false,
        },
    ));

    let providers_reg = Arc::new(lorelei_song::registry::ProviderRegistry::from_config(&cfg).ok()?);

    let currents = Arc::new(PgCurrentStore::new(pg_pool.clone()));
    let siren = Arc::new(DeterministicSirenPolicy::new(cfg.clone()));
    let song: Arc<dyn lorelei_core::traits::SongProvider> =
        providers_reg.get(&cfg.agent.default_provider).ok()?;

    let lore_store_arc: Arc<dyn LoreStore> = Arc::new(lore_store);
    let shells = Arc::new(BuiltinShellRegistry::new(
        cfg.clone(),
        lore_store_arc.clone(),
        echo_engine.clone(),
        Arc::new(NullDocs),
        Arc::new(NullShellCallRepository),
    ));
    let tide: Arc<SingleAgentTideRuntime> = Arc::new(SingleAgentTideRuntime::new(
        cfg.clone(),
        currents.clone(),
        currents.clone(),
        echo_engine.clone(),
        lore_store_arc.clone(),
        song,
        shells.clone(),
        siren.clone(),
    ));

    let autonomy = Arc::new(PgAutonomy::new(pg_pool.clone()));

    Some(AppState {
        config: cfg,
        pg_pool,
        qdrant,
        qdrant_index,
        lore_store: lore_store_arc,
        echo: echo_engine,
        providers: providers_reg,
        shells,
        currents,
        siren,
        tide,
        autonomy,
        documents: Arc::new(NullDocs),
    })
}

#[tokio::test]
async fn healthz_returns_200() {
    let app = router(minimal_state());
    let res = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn readyz_fails_when_dependencies_unavailable() {
    let app = router(minimal_state());
    let res = app
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn create_run_and_list_currents() {
    let Some(state) = env_state().await else {
        eprintln!("skipping (set DATABASE_URL and QDRANT_URL to run integration test)");
        return;
    };

    let tenant_id = state.config.agent.tenant_id.0;
    let agent_id = state.config.agent.agent_id.0;

    let app = router(state.clone());
    let body = serde_json::json!({
        "tenant_id": tenant_id,
        "agent_id": agent_id,
        "input": "Say hello from The Song."
    });
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/runs")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let run_id = created.get("run_id").unwrap().as_str().unwrap();

    let res2 = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/runs/{run_id}?tenant_id={tenant_id}&agent_id={agent_id}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res2.status(), StatusCode::OK);

    let res3 = router(state)
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/runs/{run_id}/currents?tenant_id={tenant_id}&agent_id={agent_id}&limit=500"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res3.status(), StatusCode::OK);
    let bytes3 = axum::body::to_bytes(res3.into_body(), usize::MAX)
        .await
        .unwrap();
    let currents: serde_json::Value = serde_json::from_slice(&bytes3).unwrap();
    assert!(currents.as_array().unwrap().len() >= 2);
}

#[tokio::test]
async fn create_list_delete_pearl_through_http_and_echo() {
    let Some(state) = env_state().await else {
        return;
    };
    let app = router(state.clone());

    let body = serde_json::json!({
        "tenant_id": state.config.agent.tenant_id.0,
        "agent_id": state.config.agent.agent_id.0,
        "pearl_type": "Other",
        "content": "deep memory",
        "confidence": 0.9,
        "importance": 0.6
    });

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/pearls")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let pearl_id = created
        .get("pearl_id")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    let list_uri = format!("/v1/pearls?tenant_id={}", state.config.agent.tenant_id.0);
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(list_uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Echo should return hits
    let echo_body = serde_json::json!({
        "tenant_id": state.config.agent.tenant_id.0,
        "agent_id": state.config.agent.agent_id.0,
        "query": "deep memory",
        "top_k": 5
    });
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/echo")
                .header("content-type", "application/json")
                .body(Body::from(echo_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let hits: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(hits
        .as_array()
        .unwrap()
        .iter()
        .any(|h| h["content"] == "deep memory"));

    // Delete pearl
    let del_uri = format!(
        "/v1/pearls/{pearl_id}?tenant_id={}",
        state.config.agent.tenant_id.0
    );
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(del_uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn provider_endpoint_redacts_secrets() {
    let Some(state) = env_state().await else {
        return;
    };
    let app = router(state);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/v1/providers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let s = v.to_string();
    assert!(!s.contains("api_key_env"));
}
#[derive(Clone, Default)]
struct NullDocs;
#[async_trait::async_trait]
impl DocumentStore for NullDocs {
    async fn ingest_document_path(
        &self,
        _tenant_id: lorelei_core::types::TenantId,
        _agent_id: lorelei_core::types::AgentId,
        _path: &std::path::Path,
    ) -> Result<Uuid, LoreleiError> {
        Err(LoreleiError::Unsupported("docs not available".to_string()))
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
        LoreleiError,
    > {
        Ok(None)
    }

    async fn soft_delete_document(
        &self,
        _tenant_id: lorelei_core::types::TenantId,
        _document_id: Uuid,
    ) -> Result<(), LoreleiError> {
        Ok(())
    }
}
