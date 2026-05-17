use axum::body::Body;
use axum::http::Request;
use lorelei_core::config::LoreleiConfig;
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::{EchoRetriever, LoreStore, SongProvider};
use lorelei_core::types::{
    AgentId, EchoQuery, EchoSources, NewPearl, PearlId, PearlType, RunStatus, TenantId,
    UnitInterval,
};
use lorelei_echo::retriever::{EchoEngine, EchoRetrievalConfig};
use lorelei_harbor::http::server::{router, AppState};
use lorelei_harbor::runtime::autonomy::PgAutonomy;
use lorelei_harbor::runtime::pg::PgCurrentStore;
use lorelei_lore::embedding::{DeterministicMockEmbeddingProvider, EmbeddingProvider};
use lorelei_lore::pg::PgLoreStore;
use lorelei_lore::qdrant::QdrantPearlIndex;
use lorelei_siren::policy::DeterministicSirenPolicy;
use lorelei_song::providers::stubs::UnsupportedProvider;
use lorelei_song::registry::ProviderRegistry;
use lorelei_tide::runtime::{RunRepository, SingleAgentTideRuntime};
use qdrant_client::Qdrant;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

fn repo_root() -> std::path::PathBuf {
    // crates/lorelei-eval -> repo root is ../..
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

#[derive(Default)]
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
impl SongProvider for ScriptedSong {
    fn capabilities(&self) -> lorelei_core::types::ProviderCapabilities {
        Default::default()
    }

    async fn complete(
        &self,
        _request: lorelei_core::types::SongRequest,
    ) -> Result<lorelei_core::types::SongResponse, LoreleiError> {
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
    ) -> Result<futures::stream::BoxStream<'static, lorelei_core::types::SongChunk>, LoreleiError>
    {
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

fn minimal_state_for_readyz_failure() -> AppState {
    use lorelei_core::config::{
        AgentConfig, EchoConfig, HarborConfig, LoreConfig, LoreleiConfig, ProviderConfig,
        ProviderKind, SirenConfig,
    };
    use lorelei_core::traits::DocumentStore;
    use lorelei_core::traits::{EchoRetriever, LoreStore};
    use lorelei_shells::registry::BuiltinShellRegistry;
    use lorelei_shells::repo::NullShellCallRepository;
    #[derive(Clone, Default)]
    struct NullDocs;
    #[async_trait::async_trait]
    impl DocumentStore for NullDocs {
        async fn ingest_document_path(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _path: &std::path::Path,
        ) -> Result<Uuid, LoreleiError> {
            Err(LoreleiError::Unsupported("docs not available".to_string()))
        }

        async fn get_document_chunk_for_echo(
            &self,
            _tenant_id: TenantId,
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
            _tenant_id: TenantId,
            _document_id: Uuid,
        ) -> Result<(), LoreleiError> {
            Ok(())
        }
    }
    #[derive(Default)]
    struct MemLoreStore {
        pearls: Mutex<Vec<lorelei_core::types::Pearl>>,
    }
    #[async_trait::async_trait]
    impl LoreStore for MemLoreStore {
        async fn save_pearl(
            &self,
            tenant_id: TenantId,
            agent_id: AgentId,
            pearl: NewPearl,
        ) -> Result<lorelei_core::types::Pearl, LoreleiError> {
            let p = lorelei_core::types::Pearl {
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
            _tenant_id: TenantId,
            _pearl_id: PearlId,
            _include_deleted: bool,
        ) -> Result<Option<lorelei_core::types::Pearl>, LoreleiError> {
            Ok(None)
        }
        async fn list_pearls(
            &self,
            _tenant_id: TenantId,
            _query: lorelei_core::types::PearlListQuery,
        ) -> Result<Vec<lorelei_core::types::Pearl>, LoreleiError> {
            Ok(vec![])
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
        ) -> Result<Vec<lorelei_core::types::EchoHit>, LoreleiError> {
            Ok(vec![])
        }
    }

    let tenant_id = TenantId(Uuid::from_u128(1));
    let agent_id = AgentId(Uuid::from_u128(2));
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
    let autonomy = Arc::new(PgAutonomy::new(pg_pool.clone()));
    let siren =
        Arc::new(DeterministicSirenPolicy::new(cfg.clone()).with_approval_store(autonomy.clone()));
    let song: Arc<dyn SongProvider> =
        Arc::new(lorelei_song::providers::mock::MockSongProvider::deterministic());

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

    AppState {
        config: cfg,
        pg_pool,
        qdrant: qdrant.clone(),
        qdrant_index: lorelei_lore::qdrant::QdrantPearlIndex::new(qdrant, "lorelei"),
        lore_store,
        echo,
        providers: Arc::new(ProviderRegistry::from_providers(BTreeMap::new())),
        shells,
        currents,
        siren,
        tide,
        autonomy,
        documents: Arc::new(NullDocs),
    }
}

async fn maybe_pg() -> Option<sqlx::PgPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return None,
    };
    PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .ok()
}

fn maybe_qdrant() -> Option<Qdrant> {
    let url = match std::env::var("QDRANT_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return None,
    };
    Qdrant::from_url(&url).build().ok()
}

fn new_pearl(ty: PearlType, content: &str) -> NewPearl {
    NewPearl::new(
        ty,
        content,
        UnitInterval::new(0.5).unwrap(),
        UnitInterval::new(0.9).unwrap(),
        Default::default(),
    )
    .unwrap()
}

#[tokio::test]
async fn docker_config_validates() {
    let root = repo_root();
    let cfg = LoreleiConfig::load_from_toml_path(root.join("lorelei.toml.example"))
        .expect("config loads");
    cfg.validate().expect("config validates");

    let compose = std::fs::read_to_string(root.join("docker-compose.yml")).expect("compose exists");
    let y: serde_yaml::Value = serde_yaml::from_str(&compose).expect("compose is valid YAML");
    assert!(y.get("services").is_some());
}

#[tokio::test]
async fn provider_fallback_works() {
    #[derive(Default)]
    struct RetryableFail;
    #[async_trait::async_trait]
    impl SongProvider for RetryableFail {
        fn capabilities(&self) -> lorelei_core::types::ProviderCapabilities {
            Default::default()
        }
        async fn complete(
            &self,
            _request: lorelei_core::types::SongRequest,
        ) -> Result<lorelei_core::types::SongResponse, LoreleiError> {
            Err(LoreleiError::Provider("provider `x` http 429".to_string()))
        }
        async fn stream(
            &self,
            _request: lorelei_core::types::SongRequest,
        ) -> Result<futures::stream::BoxStream<'static, lorelei_core::types::SongChunk>, LoreleiError>
        {
            Err(LoreleiError::Provider("provider `x` http 429".to_string()))
        }
        async fn embed(
            &self,
            _request: lorelei_core::types::EmbeddingRequest,
        ) -> Result<lorelei_core::types::EmbeddingResponse, LoreleiError> {
            Err(LoreleiError::Provider("provider `x` http 429".to_string()))
        }
    }

    #[derive(Default)]
    struct OkProvider;
    #[async_trait::async_trait]
    impl SongProvider for OkProvider {
        fn capabilities(&self) -> lorelei_core::types::ProviderCapabilities {
            Default::default()
        }
        async fn complete(
            &self,
            _request: lorelei_core::types::SongRequest,
        ) -> Result<lorelei_core::types::SongResponse, LoreleiError> {
            Ok(lorelei_core::types::SongResponse {
                output: "ok".to_string(),
                reasoning_summary: None,
                tool_calls: vec![],
            })
        }
        async fn stream(
            &self,
            _request: lorelei_core::types::SongRequest,
        ) -> Result<futures::stream::BoxStream<'static, lorelei_core::types::SongChunk>, LoreleiError>
        {
            Err(LoreleiError::Unsupported("no".to_string()))
        }
        async fn embed(
            &self,
            _request: lorelei_core::types::EmbeddingRequest,
        ) -> Result<lorelei_core::types::EmbeddingResponse, LoreleiError> {
            Err(LoreleiError::Unsupported("no".to_string()))
        }
    }

    let mut ps: BTreeMap<String, Arc<dyn SongProvider>> = BTreeMap::new();
    ps.insert("a".to_string(), Arc::new(RetryableFail));
    ps.insert("b".to_string(), Arc::new(OkProvider));
    let reg = ProviderRegistry::from_providers(ps);

    let resp = reg
        .complete_with_fallback(
            &["a".to_string(), "b".to_string()],
            lorelei_core::types::SongRequest {
                tenant_id: TenantId(Uuid::new_v4()),
                agent_id: AgentId(Uuid::new_v4()),
                run_id: lorelei_core::types::RunId(Uuid::new_v4()),
                input: "hi".to_string(),
                context: vec![],
                reasoning_summary: None,
            },
        )
        .await
        .expect("fallback ok");
    assert_eq!(resp.output, "ok");
}

#[tokio::test]
async fn unsupported_provider_fails_clearly() {
    let p = UnsupportedProvider {
        name: "anthropic".to_string(),
        kind: "anthropic".to_string(),
        capabilities: Default::default(),
    };
    let err = p
        .complete(lorelei_core::types::SongRequest {
            tenant_id: TenantId(Uuid::new_v4()),
            agent_id: AgentId(Uuid::new_v4()),
            run_id: lorelei_core::types::RunId(Uuid::new_v4()),
            input: "hi".to_string(),
            context: vec![],
            reasoning_summary: None,
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not implemented yet"));
}

#[tokio::test]
async fn no_full_prompts_or_secrets_in_errors_by_default() {
    std::env::remove_var("LORELEI_LOG_PROMPTS");

    let api_key = "SUPERSECRET_API_KEY_SHOULD_NOT_APPEAR".to_string();
    let prompt = "PROMPT_SHOULD_NOT_APPEAR".to_string();

    let provider = lorelei_song::providers::openai_compatible::OpenAiCompatibleProvider::new(
        "openai-like".to_string(),
        // Use an unroutable localhost port to force a transport error.
        "http://127.0.0.1:0".to_string(),
        api_key.clone(),
        "gpt-does-not-matter".to_string(),
        None,
        Default::default(),
    )
    .unwrap();

    let err = provider
        .complete(lorelei_core::types::SongRequest {
            tenant_id: TenantId(Uuid::new_v4()),
            agent_id: AgentId(Uuid::new_v4()),
            run_id: lorelei_core::types::RunId(Uuid::new_v4()),
            input: prompt.clone(),
            context: vec![],
            reasoning_summary: None,
        })
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(!msg.contains(&api_key));
    assert!(!msg.contains(&prompt));
}

#[tokio::test]
async fn invalid_planner_json_is_repaired_once() {
    // Reuse the Tide runtime behavior and the mock provider marker.
    let mut cfg =
        LoreleiConfig::load_from_toml_path(repo_root().join("lorelei.toml.example")).unwrap();
    cfg.providers.insert(
        "scripted".to_string(),
        lorelei_core::config::ProviderConfig {
            kind: lorelei_core::config::ProviderKind::Local,
            base_url: Some("http://127.0.0.1:0".to_string()),
            api_key_env: "IGNORED".to_string(),
            chat_model: "mock".to_string(),
            embedding_model: None,
        },
    );
    cfg.agent.default_provider = "scripted".to_string();

    #[derive(Default)]
    struct MemRuns {
        runs: Mutex<Vec<lorelei_core::types::Run>>,
    }
    #[async_trait::async_trait]
    impl RunRepository for MemRuns {
        async fn create_run(
            &self,
            tenant_id: TenantId,
            agent_id: AgentId,
            goal: &str,
        ) -> Result<lorelei_core::types::Run, LoreleiError> {
            let run = lorelei_core::types::Run {
                run_id: lorelei_core::types::RunId(Uuid::new_v4()),
                tenant_id,
                agent_id,
                status: lorelei_core::types::RunStatus::Running,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            };
            self.runs.lock().unwrap().push(run.clone());
            let _ = goal;
            Ok(run)
        }
        async fn complete_run(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
            _status: lorelei_core::types::RunStatus,
        ) -> Result<(), LoreleiError> {
            Ok(())
        }
        async fn get_run(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
        ) -> Result<Option<lorelei_core::types::Run>, LoreleiError> {
            Ok(None)
        }
    }

    #[derive(Default)]
    struct NoopCurrents;
    #[async_trait::async_trait]
    impl lorelei_core::traits::CurrentStore for NoopCurrents {
        async fn append_current_event(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
            _event: lorelei_core::types::CurrentEvent,
        ) -> Result<(), LoreleiError> {
            Ok(())
        }
        async fn list_current_events(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
            _limit: usize,
        ) -> Result<Vec<lorelei_core::types::CurrentEvent>, LoreleiError> {
            Ok(vec![])
        }
    }

    #[derive(Default)]
    struct FixedEcho;
    #[async_trait::async_trait]
    impl EchoRetriever for FixedEcho {
        async fn query(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _query: EchoQuery,
        ) -> Result<Vec<lorelei_core::types::EchoHit>, LoreleiError> {
            Ok(vec![])
        }
    }

    #[derive(Default)]
    struct NullLore;
    #[async_trait::async_trait]
    impl LoreStore for NullLore {
        async fn save_pearl(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _pearl: lorelei_core::types::NewPearl,
        ) -> Result<lorelei_core::types::Pearl, LoreleiError> {
            Err(LoreleiError::Unsupported("no".to_string()))
        }
        async fn get_pearl(
            &self,
            _tenant_id: TenantId,
            _pearl_id: PearlId,
            _include_deleted: bool,
        ) -> Result<Option<lorelei_core::types::Pearl>, LoreleiError> {
            Ok(None)
        }
        async fn list_pearls(
            &self,
            _tenant_id: TenantId,
            _query: lorelei_core::types::PearlListQuery,
        ) -> Result<Vec<lorelei_core::types::Pearl>, LoreleiError> {
            Ok(vec![])
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

    #[derive(Default)]
    struct NoopShells;
    #[async_trait::async_trait]
    impl lorelei_core::traits::ShellRegistry for NoopShells {
        async fn list_shells(&self) -> Result<Vec<String>, LoreleiError> {
            Ok(vec![])
        }
        async fn call(
            &self,
            _call: lorelei_core::types::ShellCall,
        ) -> Result<lorelei_core::types::ShellResult, LoreleiError> {
            Err(LoreleiError::Unsupported("no".to_string()))
        }
    }

    cfg.agent.default_provider = "mock".to_string();
    cfg.agent.default_embedding_provider = "mock".to_string();

    let tide = SingleAgentTideRuntime::new(
        cfg.clone(),
        Arc::new(MemRuns::default()),
        Arc::new(NoopCurrents),
        Arc::new(FixedEcho),
        Arc::new(NullLore),
        Arc::new(lorelei_song::providers::mock::MockSongProvider::deterministic()),
        Arc::new(NoopShells),
        Arc::new(DeterministicSirenPolicy::new(cfg)),
    )
    .with_templates(
        "LORELEI_MODE=planner_json_invalid_once",
        "LORELEI_MODE=answer {{USER_INPUT}}",
    );

    let res = tide
        .run_once(
            TenantId(Uuid::new_v4()),
            AgentId(Uuid::new_v4()),
            "hi".to_string(),
        )
        .await
        .expect("run");
    assert_eq!(res.status, RunStatus::Succeeded);
}

#[tokio::test]
async fn harbor_health_and_ready_behave_without_deps() {
    // Minimal state with lazy pg + dead qdrant should make readyz fail but healthz ok.
    let app = router(minimal_state_for_readyz_failure());
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn preference_pearl_affects_final_answer_context() {
    // If Echo returns a preference Pearl, Tide should pass it in context to SongProvider.
    struct PrefEcho;
    #[async_trait::async_trait]
    impl EchoRetriever for PrefEcho {
        async fn query(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _query: EchoQuery,
        ) -> Result<Vec<lorelei_core::types::EchoHit>, LoreleiError> {
            Ok(vec![lorelei_core::types::EchoHit {
                score: UnitInterval::new(1.0).unwrap(),
                pearl_id: PearlId(Uuid::new_v4()),
                content: "User prefers tea.".to_string(),
                pearl_type: PearlType::Preference,
                reason: "golden".to_string(),
                created_at: chrono::Utc::now(),
                citation: None,
            }])
        }
    }

    #[derive(Default)]
    struct CapturingSong {
        last: Mutex<Option<lorelei_core::types::SongRequest>>,
    }
    #[async_trait::async_trait]
    impl SongProvider for CapturingSong {
        fn capabilities(&self) -> lorelei_core::types::ProviderCapabilities {
            Default::default()
        }
        async fn complete(
            &self,
            request: lorelei_core::types::SongRequest,
        ) -> Result<lorelei_core::types::SongResponse, LoreleiError> {
            *self.last.lock().unwrap() = Some(request.clone());
            let out = if request.reasoning_summary.as_deref() == Some("planner") {
                r#"{"action":"answer","reasoning_summary":"ok","answer":"ok"}"#.to_string()
            } else if request.reasoning_summary.as_deref() == Some("lore_extractor") {
                "[]".to_string()
            } else if request.reasoning_summary.as_deref() == Some("lore_critic") {
                r#"{"accept_indices":[],"reject":[]}"#.to_string()
            } else if request.context.iter().any(|c| c.contains("prefers tea")) {
                "Tea acknowledged.".to_string()
            } else {
                "No preference.".to_string()
            };
            Ok(lorelei_core::types::SongResponse {
                output: out,
                reasoning_summary: None,
                tool_calls: vec![],
            })
        }
        async fn stream(
            &self,
            _request: lorelei_core::types::SongRequest,
        ) -> Result<futures::stream::BoxStream<'static, lorelei_core::types::SongChunk>, LoreleiError>
        {
            Err(LoreleiError::Unsupported("stream".to_string()))
        }
        async fn embed(
            &self,
            _request: lorelei_core::types::EmbeddingRequest,
        ) -> Result<lorelei_core::types::EmbeddingResponse, LoreleiError> {
            Err(LoreleiError::Unsupported("embed".to_string()))
        }
    }

    #[derive(Default)]
    struct MemRuns;
    #[async_trait::async_trait]
    impl RunRepository for MemRuns {
        async fn create_run(
            &self,
            tenant_id: TenantId,
            agent_id: AgentId,
            _goal: &str,
        ) -> Result<lorelei_core::types::Run, LoreleiError> {
            Ok(lorelei_core::types::Run {
                run_id: lorelei_core::types::RunId(Uuid::new_v4()),
                tenant_id,
                agent_id,
                status: lorelei_core::types::RunStatus::Running,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            })
        }
        async fn complete_run(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
            _status: lorelei_core::types::RunStatus,
        ) -> Result<(), LoreleiError> {
            Ok(())
        }
        async fn get_run(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
        ) -> Result<Option<lorelei_core::types::Run>, LoreleiError> {
            Ok(None)
        }
    }

    #[derive(Default)]
    struct NoopCurrents;
    #[async_trait::async_trait]
    impl lorelei_core::traits::CurrentStore for NoopCurrents {
        async fn append_current_event(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
            _event: lorelei_core::types::CurrentEvent,
        ) -> Result<(), LoreleiError> {
            Ok(())
        }
        async fn list_current_events(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
            _limit: usize,
        ) -> Result<Vec<lorelei_core::types::CurrentEvent>, LoreleiError> {
            Ok(vec![])
        }
    }

    #[derive(Default)]
    struct NullLore;
    #[async_trait::async_trait]
    impl LoreStore for NullLore {
        async fn save_pearl(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _pearl: lorelei_core::types::NewPearl,
        ) -> Result<lorelei_core::types::Pearl, LoreleiError> {
            Err(LoreleiError::Unsupported("no".to_string()))
        }
        async fn get_pearl(
            &self,
            _tenant_id: TenantId,
            _pearl_id: PearlId,
            _include_deleted: bool,
        ) -> Result<Option<lorelei_core::types::Pearl>, LoreleiError> {
            Ok(None)
        }
        async fn list_pearls(
            &self,
            _tenant_id: TenantId,
            _query: lorelei_core::types::PearlListQuery,
        ) -> Result<Vec<lorelei_core::types::Pearl>, LoreleiError> {
            Ok(vec![])
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

    #[derive(Default)]
    struct NoopShells;
    #[async_trait::async_trait]
    impl lorelei_core::traits::ShellRegistry for NoopShells {
        async fn list_shells(&self) -> Result<Vec<String>, LoreleiError> {
            Ok(vec![])
        }
        async fn call(
            &self,
            _call: lorelei_core::types::ShellCall,
        ) -> Result<lorelei_core::types::ShellResult, LoreleiError> {
            Err(LoreleiError::Unsupported("no".to_string()))
        }
    }

    let mut cfg =
        LoreleiConfig::load_from_toml_path(repo_root().join("lorelei.toml.example")).unwrap();
    cfg.providers.insert(
        "scripted".to_string(),
        lorelei_core::config::ProviderConfig {
            kind: lorelei_core::config::ProviderKind::Local,
            base_url: Some("http://127.0.0.1:0".to_string()),
            api_key_env: "IGNORED".to_string(),
            chat_model: "mock".to_string(),
            embedding_model: None,
        },
    );
    cfg.agent.default_provider = "scripted".to_string();
    let tide = SingleAgentTideRuntime::new(
        cfg.clone(),
        Arc::new(MemRuns),
        Arc::new(NoopCurrents),
        Arc::new(PrefEcho),
        Arc::new(NullLore),
        Arc::new(CapturingSong::default()),
        Arc::new(NoopShells),
        Arc::new(DeterministicSirenPolicy::new(cfg)),
    )
    .with_templates(
        r#"{"action":"answer","answer":"ok"}"#,
        "LORELEI_MODE=answer {{USER_INPUT}}",
    );

    let res = tide
        .run_once_with_options(
            TenantId(Uuid::new_v4()),
            AgentId(Uuid::new_v4()),
            "hi".to_string(),
            true,
        )
        .await
        .unwrap();
    assert!(res.output.contains("Tea acknowledged"));
}

#[tokio::test]
async fn high_risk_shell_requires_approval_and_worker_would_stop() {
    #[derive(Default)]
    struct MemRuns;
    #[async_trait::async_trait]
    impl RunRepository for MemRuns {
        async fn create_run(
            &self,
            tenant_id: TenantId,
            agent_id: AgentId,
            _goal: &str,
        ) -> Result<lorelei_core::types::Run, LoreleiError> {
            Ok(lorelei_core::types::Run {
                run_id: lorelei_core::types::RunId(Uuid::new_v4()),
                tenant_id,
                agent_id,
                status: lorelei_core::types::RunStatus::Running,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            })
        }
        async fn complete_run(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
            _status: lorelei_core::types::RunStatus,
        ) -> Result<(), LoreleiError> {
            Ok(())
        }
        async fn get_run(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
        ) -> Result<Option<lorelei_core::types::Run>, LoreleiError> {
            Ok(None)
        }
    }

    #[derive(Default)]
    struct MemCurrents {
        events: Mutex<Vec<lorelei_core::types::CurrentEvent>>,
    }
    #[async_trait::async_trait]
    impl lorelei_core::traits::CurrentStore for MemCurrents {
        async fn append_current_event(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
            event: lorelei_core::types::CurrentEvent,
        ) -> Result<(), LoreleiError> {
            self.events.lock().unwrap().push(event);
            Ok(())
        }
        async fn list_current_events(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
            _limit: usize,
        ) -> Result<Vec<lorelei_core::types::CurrentEvent>, LoreleiError> {
            Ok(self.events.lock().unwrap().clone())
        }
    }

    #[derive(Default)]
    struct EmptyEcho;
    #[async_trait::async_trait]
    impl EchoRetriever for EmptyEcho {
        async fn query(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _query: EchoQuery,
        ) -> Result<Vec<lorelei_core::types::EchoHit>, LoreleiError> {
            Ok(vec![])
        }
    }

    #[derive(Default)]
    struct NullLore;
    #[async_trait::async_trait]
    impl LoreStore for NullLore {
        async fn save_pearl(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _pearl: lorelei_core::types::NewPearl,
        ) -> Result<lorelei_core::types::Pearl, LoreleiError> {
            Err(LoreleiError::Unsupported("no".to_string()))
        }
        async fn get_pearl(
            &self,
            _tenant_id: TenantId,
            _pearl_id: PearlId,
            _include_deleted: bool,
        ) -> Result<Option<lorelei_core::types::Pearl>, LoreleiError> {
            Ok(None)
        }
        async fn list_pearls(
            &self,
            _tenant_id: TenantId,
            _query: lorelei_core::types::PearlListQuery,
        ) -> Result<Vec<lorelei_core::types::Pearl>, LoreleiError> {
            Ok(vec![])
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

    #[derive(Default)]
    struct CountingShells {
        calls: Mutex<u32>,
    }
    #[async_trait::async_trait]
    impl lorelei_core::traits::ShellRegistry for CountingShells {
        async fn list_shells(&self) -> Result<Vec<String>, LoreleiError> {
            Ok(vec!["forget_pearl".to_string()])
        }
        async fn call(
            &self,
            _call: lorelei_core::types::ShellCall,
        ) -> Result<lorelei_core::types::ShellResult, LoreleiError> {
            *self.calls.lock().unwrap() += 1;
            Err(LoreleiError::Unsupported("blocked".to_string()))
        }
    }

    // Config: allow shell execution but require approval for high-risk.
    let mut cfg =
        LoreleiConfig::load_from_toml_path(repo_root().join("lorelei.toml.example")).unwrap();
    cfg.siren.allow_shell_execution = true;

    let song: Arc<dyn SongProvider> = Arc::new(ScriptedSong::new(vec![
        r#"{"action":"call_shell","tool":"forget_pearl","input":{"pearl_id":"00000000-0000-0000-0000-000000000000"}}"#.to_string(),
    ]));

    let currents = Arc::new(MemCurrents::default());
    let tide = SingleAgentTideRuntime::new(
        cfg.clone(),
        Arc::new(MemRuns),
        currents.clone(),
        Arc::new(EmptyEcho),
        Arc::new(NullLore),
        song,
        Arc::new(CountingShells::default()),
        Arc::new(DeterministicSirenPolicy::new(cfg)),
    )
    .with_templates("{}", "LORELEI_MODE=answer {{USER_INPUT}}");

    let res = tide
        .run_once_with_options(
            TenantId(Uuid::new_v4()),
            AgentId(Uuid::new_v4()),
            "forget".to_string(),
            false,
        )
        .await
        .unwrap();
    assert_eq!(res.status, RunStatus::Canceled);

    let events = currents.events.lock().unwrap();
    assert!(events.iter().any(|e| e.summary == "approval required"));
}

#[tokio::test]
async fn low_risk_shell_runs_automatically() {
    #[derive(Default)]
    struct MemRuns;
    #[async_trait::async_trait]
    impl RunRepository for MemRuns {
        async fn create_run(
            &self,
            tenant_id: TenantId,
            agent_id: AgentId,
            _goal: &str,
        ) -> Result<lorelei_core::types::Run, LoreleiError> {
            Ok(lorelei_core::types::Run {
                run_id: lorelei_core::types::RunId(Uuid::new_v4()),
                tenant_id,
                agent_id,
                status: lorelei_core::types::RunStatus::Running,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            })
        }
        async fn complete_run(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
            _status: lorelei_core::types::RunStatus,
        ) -> Result<(), LoreleiError> {
            Ok(())
        }
        async fn get_run(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
        ) -> Result<Option<lorelei_core::types::Run>, LoreleiError> {
            Ok(None)
        }
    }

    #[derive(Default)]
    struct NoopCurrents;
    #[async_trait::async_trait]
    impl lorelei_core::traits::CurrentStore for NoopCurrents {
        async fn append_current_event(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
            _event: lorelei_core::types::CurrentEvent,
        ) -> Result<(), LoreleiError> {
            Ok(())
        }
        async fn list_current_events(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
            _limit: usize,
        ) -> Result<Vec<lorelei_core::types::CurrentEvent>, LoreleiError> {
            Ok(vec![])
        }
    }

    #[derive(Default)]
    struct EmptyEcho;
    #[async_trait::async_trait]
    impl EchoRetriever for EmptyEcho {
        async fn query(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _query: EchoQuery,
        ) -> Result<Vec<lorelei_core::types::EchoHit>, LoreleiError> {
            Ok(vec![])
        }
    }

    #[derive(Default)]
    struct NullLore;
    #[async_trait::async_trait]
    impl LoreStore for NullLore {
        async fn save_pearl(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _pearl: lorelei_core::types::NewPearl,
        ) -> Result<lorelei_core::types::Pearl, LoreleiError> {
            Err(LoreleiError::Unsupported("no".to_string()))
        }
        async fn get_pearl(
            &self,
            _tenant_id: TenantId,
            _pearl_id: PearlId,
            _include_deleted: bool,
        ) -> Result<Option<lorelei_core::types::Pearl>, LoreleiError> {
            Ok(None)
        }
        async fn list_pearls(
            &self,
            _tenant_id: TenantId,
            _query: lorelei_core::types::PearlListQuery,
        ) -> Result<Vec<lorelei_core::types::Pearl>, LoreleiError> {
            Ok(vec![])
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

    #[derive(Default)]
    struct CountingShells {
        calls: Mutex<u32>,
    }
    #[async_trait::async_trait]
    impl lorelei_core::traits::ShellRegistry for CountingShells {
        async fn list_shells(&self) -> Result<Vec<String>, LoreleiError> {
            Ok(vec!["echo".to_string()])
        }
        async fn call(
            &self,
            call: lorelei_core::types::ShellCall,
        ) -> Result<lorelei_core::types::ShellResult, LoreleiError> {
            *self.calls.lock().unwrap() += 1;
            Ok(lorelei_core::types::ShellResult {
                call_id: call.call_id,
                ok: true,
                output: json!({"ok": true}),
                error: None,
                started_at: chrono::Utc::now(),
                finished_at: chrono::Utc::now(),
            })
        }
    }

    let mut cfg =
        LoreleiConfig::load_from_toml_path(repo_root().join("lorelei.toml.example")).unwrap();
    cfg.siren.allow_shell_execution = true;

    let song: Arc<dyn SongProvider> = Arc::new(ScriptedSong::new(vec![
        r#"{"action":"call_shell","tool":"echo","input":{"message":"hi"}}"#.to_string(),
        "done".to_string(),
    ]));

    let shells = Arc::new(CountingShells::default());
    let tide = SingleAgentTideRuntime::new(
        cfg.clone(),
        Arc::new(MemRuns),
        Arc::new(NoopCurrents),
        Arc::new(EmptyEcho),
        Arc::new(NullLore),
        song,
        shells.clone(),
        Arc::new(DeterministicSirenPolicy::new(cfg)),
    )
    .with_templates("{}", "LORELEI_MODE=answer {{USER_INPUT}}");

    let res = tide
        .run_once_with_options(
            TenantId(Uuid::new_v4()),
            AgentId(Uuid::new_v4()),
            "hi".to_string(),
            false,
        )
        .await
        .unwrap();
    assert_eq!(res.status, RunStatus::Succeeded);
    assert_eq!(*shells.calls.lock().unwrap(), 1);
}

#[tokio::test]
async fn reflection_stores_durable_memory_and_rejects_temporary_facts() {
    #[derive(Default)]
    struct MemRuns;
    #[async_trait::async_trait]
    impl RunRepository for MemRuns {
        async fn create_run(
            &self,
            tenant_id: TenantId,
            agent_id: AgentId,
            _goal: &str,
        ) -> Result<lorelei_core::types::Run, LoreleiError> {
            Ok(lorelei_core::types::Run {
                run_id: lorelei_core::types::RunId(Uuid::new_v4()),
                tenant_id,
                agent_id,
                status: lorelei_core::types::RunStatus::Running,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            })
        }
        async fn complete_run(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
            _status: lorelei_core::types::RunStatus,
        ) -> Result<(), LoreleiError> {
            Ok(())
        }
        async fn get_run(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
        ) -> Result<Option<lorelei_core::types::Run>, LoreleiError> {
            Ok(None)
        }
    }

    #[derive(Default)]
    struct NoopCurrents;
    #[async_trait::async_trait]
    impl lorelei_core::traits::CurrentStore for NoopCurrents {
        async fn append_current_event(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
            _event: lorelei_core::types::CurrentEvent,
        ) -> Result<(), LoreleiError> {
            Ok(())
        }
        async fn list_current_events(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _run_id: lorelei_core::types::RunId,
            _limit: usize,
        ) -> Result<Vec<lorelei_core::types::CurrentEvent>, LoreleiError> {
            Ok(vec![])
        }
    }

    #[derive(Default)]
    struct EmptyEcho;
    #[async_trait::async_trait]
    impl EchoRetriever for EmptyEcho {
        async fn query(
            &self,
            _tenant_id: TenantId,
            _agent_id: AgentId,
            _query: EchoQuery,
        ) -> Result<Vec<lorelei_core::types::EchoHit>, LoreleiError> {
            Ok(vec![])
        }
    }

    #[derive(Default)]
    struct MemLore {
        saved: Mutex<Vec<String>>,
    }
    #[async_trait::async_trait]
    impl LoreStore for MemLore {
        async fn save_pearl(
            &self,
            tenant_id: TenantId,
            agent_id: AgentId,
            pearl: lorelei_core::types::NewPearl,
        ) -> Result<lorelei_core::types::Pearl, LoreleiError> {
            self.saved.lock().unwrap().push(pearl.content.clone());
            Ok(lorelei_core::types::Pearl {
                pearl_id: PearlId(Uuid::new_v4()),
                tenant_id,
                agent_id,
                pearl_type: pearl.pearl_type,
                content: pearl.content,
                importance: pearl.importance,
                confidence: pearl.confidence,
                created_at: chrono::Utc::now(),
                metadata: pearl.metadata,
            })
        }
        async fn get_pearl(
            &self,
            _tenant_id: TenantId,
            _pearl_id: PearlId,
            _include_deleted: bool,
        ) -> Result<Option<lorelei_core::types::Pearl>, LoreleiError> {
            Ok(None)
        }
        async fn list_pearls(
            &self,
            _tenant_id: TenantId,
            _query: lorelei_core::types::PearlListQuery,
        ) -> Result<Vec<lorelei_core::types::Pearl>, LoreleiError> {
            Ok(vec![])
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

    #[derive(Default)]
    struct NoopShells;
    #[async_trait::async_trait]
    impl lorelei_core::traits::ShellRegistry for NoopShells {
        async fn list_shells(&self) -> Result<Vec<String>, LoreleiError> {
            Ok(vec![])
        }
        async fn call(
            &self,
            _call: lorelei_core::types::ShellCall,
        ) -> Result<lorelei_core::types::ShellResult, LoreleiError> {
            Err(LoreleiError::Unsupported("no".to_string()))
        }
    }

    let cfg = LoreleiConfig::load_from_toml_path(repo_root().join("lorelei.toml.example")).unwrap();
    let mut cfg = cfg;
    cfg.providers.insert(
        "scripted".to_string(),
        lorelei_core::config::ProviderConfig {
            kind: lorelei_core::config::ProviderKind::Local,
            base_url: Some("http://127.0.0.1:0".to_string()),
            api_key_env: "IGNORED".to_string(),
            chat_model: "mock".to_string(),
            embedding_model: None,
        },
    );
    cfg.agent.default_provider = "scripted".to_string();

    let lore = Arc::new(MemLore::default());
    let song: Arc<dyn SongProvider> = Arc::new(ScriptedSong::new(vec![
        r#"{"action":"answer","reasoning_summary":"ok","answer":"ok"}"#.to_string(),
        "ok".to_string(),
        r#"[{"pearl_type":"Preference","content":"User prefers tea.","confidence":0.9,"importance":0.7},{"pearl_type":"Plan","content":"Remind me tomorrow to do X.","confidence":0.9,"importance":0.7}]"#.to_string(),
        r#"{"accept_indices":[0,1],"reject":[]}"#.to_string(),
    ]));

    let tide = SingleAgentTideRuntime::new(
        cfg.clone(),
        Arc::new(MemRuns),
        Arc::new(NoopCurrents),
        Arc::new(EmptyEcho),
        lore.clone(),
        song,
        Arc::new(NoopShells),
        Arc::new(DeterministicSirenPolicy::new(cfg)),
    )
    .with_templates(
        r#"{"action":"answer","answer":"ok"}"#,
        "LORELEI_MODE=answer {{USER_INPUT}}",
    );

    let _ = tide
        .run_once(
            TenantId(Uuid::new_v4()),
            AgentId(Uuid::new_v4()),
            "hi".to_string(),
        )
        .await
        .unwrap();

    let saved = lore.saved.lock().unwrap().clone();
    assert!(saved.iter().any(|s| s.contains("prefers tea")));
    assert!(!saved
        .iter()
        .any(|s| s.to_ascii_lowercase().contains("tomorrow")));
}

#[tokio::test]
async fn golden_storage_and_echo_integration() {
    let Some(pool) = maybe_pg().await else { return };
    let Some(client) = maybe_qdrant() else { return };

    let tenant_a = TenantId(Uuid::new_v4());
    let tenant_b = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());

    let collection = format!("lorelei_eval_{}", Uuid::new_v4());
    let index = QdrantPearlIndex::new(client, collection);
    let embedder: Arc<dyn EmbeddingProvider> =
        Arc::new(DeterministicMockEmbeddingProvider::new(64));

    let store_writer =
        PgLoreStore::new_indexed(pool.clone(), index.clone(), embedder.clone(), "mock");
    store_writer.migrate().await.expect("migrate");
    let store_reader = PgLoreStore::new_indexed(pool, index.clone(), embedder.clone(), "mock");

    // 1. Manual Pearl save/retrieve.
    let saved = store_writer
        .save_pearl(
            tenant_a,
            agent_id,
            new_pearl(PearlType::Fact, "deep memory"),
        )
        .await
        .expect("save");
    let got = store_writer
        .get_pearl(tenant_a, saved.pearl_id, false)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(got.content, "deep memory");

    // 3. No cross-tenant.
    let other = store_writer
        .get_pearl(tenant_b, saved.pearl_id, false)
        .await
        .expect("get b");
    assert!(other.is_none());

    // 2 + 4. Echo retrieves relevant + excludes deleted.
    let echo = EchoEngine::new(
        store_reader,
        index.clone(),
        embedder.clone(),
        "mock",
        EchoRetrievalConfig {
            rerank_top_k: 10,
            enable_query_rewrite: false,
        },
    );

    let hits = echo
        .query(
            tenant_a,
            agent_id,
            EchoQuery {
                query: "deep memory".to_string(),
                top_k: 5,
                min_confidence: Some(UnitInterval::new(0.0).unwrap()),
                pearl_type: None,
                sources: EchoSources::Pearls,
            },
        )
        .await
        .expect("echo");
    assert!(!hits.is_empty());

    store_writer
        .forget_pearl(tenant_a, saved.pearl_id)
        .await
        .expect("forget");

    let hits = echo
        .query(
            tenant_a,
            agent_id,
            EchoQuery {
                query: "deep memory".to_string(),
                top_k: 5,
                min_confidence: Some(UnitInterval::new(0.0).unwrap()),
                pearl_type: None,
                sources: EchoSources::Pearls,
            },
        )
        .await
        .expect("echo");
    assert!(hits.is_empty());
}
