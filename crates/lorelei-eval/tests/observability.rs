use lorelei_core::config::LoreleiConfig;
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::{DocumentStore, EchoRetriever, LoreStore, SongProvider};
use lorelei_core::types::{AgentId, EchoQuery, PearlId, RunId, RunStatus, TenantId};
use lorelei_shells::registry::BuiltinShellRegistry;
use lorelei_shells::repo::NullShellCallRepository;
use lorelei_siren::policy::DeterministicSirenPolicy;
use lorelei_tide::runtime::{RunRepository, SingleAgentTideRuntime};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;

static TEST_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

#[derive(Clone, Default)]
struct BufWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for BufWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Default)]
struct MakeBufWriter(Arc<Mutex<Vec<u8>>>);

impl MakeWriter<'_> for MakeBufWriter {
    type Writer = BufWriter;

    fn make_writer(&self) -> Self::Writer {
        BufWriter(self.0.clone())
    }
}

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
        request: lorelei_core::types::SongRequest,
    ) -> Result<lorelei_core::types::SongResponse, LoreleiError> {
        let started = Instant::now();
        let mut r = self.responses.lock().unwrap();
        let out = if r.is_empty() {
            // If a test under-scripts the provider, fall back to a deterministic no-op response
            // so we can still assert on logging behavior.
            "{}".to_string()
        } else {
            r.remove(0)
        };
        let latency = started.elapsed();
        tracing::info!(
            run_id = %request.run_id.0,
            provider = "scripted",
            model = "scripted",
            latency_ms = latency.as_millis() as u64,
            retry_count = 0u32,
            prompt_tokens = Option::<u32>::None,
            completion_tokens = Option::<u32>::None,
            total_tokens = Option::<u32>::None,
            "song.complete"
        );
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

#[derive(Default)]
struct NoopRuns;

#[async_trait::async_trait]
impl RunRepository for NoopRuns {
    async fn create_run(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        _goal: &str,
    ) -> Result<lorelei_core::types::Run, LoreleiError> {
        Ok(lorelei_core::types::Run {
            run_id: RunId(Uuid::new_v4()),
            tenant_id,
            agent_id,
            status: RunStatus::Running,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        })
    }

    async fn complete_run(
        &self,
        _tenant_id: TenantId,
        _agent_id: AgentId,
        _run_id: RunId,
        _status: RunStatus,
    ) -> Result<(), LoreleiError> {
        Ok(())
    }

    async fn get_run(
        &self,
        _tenant_id: TenantId,
        _agent_id: AgentId,
        _run_id: RunId,
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
        _run_id: RunId,
        _event: lorelei_core::types::CurrentEvent,
    ) -> Result<(), LoreleiError> {
        Ok(())
    }

    async fn list_current_events(
        &self,
        _tenant_id: TenantId,
        _agent_id: AgentId,
        _run_id: RunId,
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
struct LoggingEcho;

#[async_trait::async_trait]
impl EchoRetriever for LoggingEcho {
    async fn query(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        query: EchoQuery,
    ) -> Result<Vec<lorelei_core::types::EchoHit>, LoreleiError> {
        let started = Instant::now();
        tracing::info!(
            tenant_id = %tenant_id.0,
            agent_id = %agent_id.0,
            query_variants = 1usize,
            candidates = 0usize,
            hits = 0usize,
            latency_ms = started.elapsed().as_millis() as u64,
            "echo.query"
        );
        let _ = query;
        Ok(vec![])
    }
}

#[tokio::test(flavor = "current_thread")]
async fn logs_include_run_id_and_redact_prompts_by_default() {
    let _lock = TEST_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    std::env::remove_var("LORELEI_LOG_PROMPTS");

    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let make = MakeBufWriter(buf.clone());

    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .with_writer(make)
        .with_target(false)
        .finish();

    let _guard = tracing::subscriber::set_default(subscriber);

    let cfg = LoreleiConfig::load_from_toml_path(repo_root().join("lorelei.toml.example")).unwrap();
    let mut cfg = cfg;
    // Keep tests deterministic: use the mock kind so Tide uses deterministic
    // memory extraction/critique and doesn't require scripting extra LLM calls.
    cfg.agent.default_provider = "mock".to_string();
    cfg.agent.default_embedding_provider = "mock".to_string();
    cfg.siren.allow_shell_execution = true;
    let lore: Arc<dyn LoreStore> = Arc::new(NullLore);
    let echo: Arc<dyn EchoRetriever> = Arc::new(LoggingEcho);
    let shells = Arc::new(BuiltinShellRegistry::new(
        cfg.clone(),
        lore.clone(),
        echo.clone(),
        Arc::new(NullDocs),
        Arc::new(NullShellCallRepository),
    ));
    let siren = Arc::new(DeterministicSirenPolicy::new(cfg.clone()).with_llm_policy_enabled(false));

    let song: Arc<dyn SongProvider> = Arc::new(ScriptedSong::new(vec![
        r#"{"action":"call_shell","tool":"echo","input":{"message":"hi"}}"#.to_string(),
        "done".to_string(),
    ]));

    let tide = SingleAgentTideRuntime::new(
        cfg.clone(),
        Arc::new(NoopRuns),
        Arc::new(NoopCurrents),
        echo.clone(),
        lore.clone(),
        song,
        shells,
        siren,
    )
    .with_templates("{}", "LORELEI_MODE=answer {{USER_INPUT}}");

    let secret_prompt = "PROMPT_SHOULD_NOT_APPEAR secret=SUPERSECRET".to_string();
    let res = tide
        .run_once_with_options(
            TenantId(Uuid::new_v4()),
            AgentId(Uuid::new_v4()),
            secret_prompt.clone(),
            true,
        )
        .await
        .unwrap();

    assert_eq!(res.status, RunStatus::Succeeded);

    let out = String::from_utf8_lossy(&buf.lock().unwrap().clone()).to_string();
    assert!(out.contains("song.complete"));
    assert!(out.contains(&res.run_id.0.to_string()));
    assert!(out.contains("siren.decision"));
    assert!(out.contains("shell.call"));
    assert!(out.contains("echo.query"));

    // By default we never log full prompts or secrets.
    assert!(!out.contains("PROMPT_SHOULD_NOT_APPEAR"));
    assert!(!out.contains("SUPERSECRET"));
}

#[tokio::test(flavor = "current_thread")]
async fn full_prompt_logging_only_when_enabled() {
    let _lock = TEST_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    std::env::set_var("LORELEI_LOG_PROMPTS", "true");

    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let make = MakeBufWriter(buf.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("debug"))
        .with_writer(make)
        .with_target(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let cfg = LoreleiConfig::load_from_toml_path(repo_root().join("lorelei.toml.example")).unwrap();
    let mut cfg = cfg;
    cfg.agent.default_provider = "mock".to_string();
    cfg.agent.default_embedding_provider = "mock".to_string();
    cfg.siren.allow_shell_execution = true;
    let lore: Arc<dyn LoreStore> = Arc::new(NullLore);
    let echo: Arc<dyn EchoRetriever> = Arc::new(LoggingEcho);
    let shells = Arc::new(BuiltinShellRegistry::new(
        cfg.clone(),
        lore.clone(),
        echo.clone(),
        Arc::new(NullDocs),
        Arc::new(NullShellCallRepository),
    ));
    let siren = Arc::new(DeterministicSirenPolicy::new(cfg.clone()));

    // Use the mock provider so it emits `song.prompt`/`song.prompt_redacted` debug logs.
    let song: Arc<dyn SongProvider> =
        Arc::new(lorelei_song::providers::mock::MockSongProvider::deterministic());

    let tide = SingleAgentTideRuntime::new(
        cfg.clone(),
        Arc::new(NoopRuns),
        Arc::new(NoopCurrents),
        echo,
        lore,
        song,
        shells,
        siren,
    );

    let prompt = "PROMPT_SHOULD_APPEAR_WHEN_ENABLED".to_string();
    let _ = tide
        .run_once_with_options(
            TenantId(Uuid::new_v4()),
            AgentId(Uuid::new_v4()),
            prompt.clone(),
            false,
        )
        .await
        .unwrap();

    let out = String::from_utf8_lossy(&buf.lock().unwrap().clone()).to_string();
    assert!(out.contains("song.prompt"));
    assert!(out.contains("PROMPT_SHOULD_APPEAR_WHEN_ENABLED"));
}
