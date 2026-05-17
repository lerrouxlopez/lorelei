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
    AgentId, CurrentEvent, EchoHit, EchoQuery, Pearl, PearlId, PearlListQuery, PearlType, Run,
    RunId, RunStatus, ShellCall, ShellResult, SongChunk, SongRequest, SongResponse, TenantId,
    UnitInterval,
};
use lorelei_siren::policy::DeterministicSirenPolicy;
use lorelei_song::providers::mock::MockSongProvider;
use lorelei_tide::runtime::{RunRepository, SingleAgentTideRuntime};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

const PLANNER_OK: &str = r#"
LORELEI_MODE=planner_json
{ "dummy": "{{USER_INPUT}}" }
"#;

const PLANNER_INVALID_ONCE: &str = r#"
LORELEI_MODE=planner_json_invalid_once
"#;

const ANSWER_TEMPLATE: &str = r#"
LORELEI_MODE=answer
User: {{USER_INPUT}}
Echo: {{ECHO_HITS}}
"#;

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
        goal: &str,
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
        let _ = goal;
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

struct FixedEcho {
    hits: Vec<EchoHit>,
}

#[async_trait]
impl EchoRetriever for FixedEcho {
    async fn query(
        &self,
        _tenant_id: TenantId,
        _agent_id: AgentId,
        _query: EchoQuery,
    ) -> Result<Vec<EchoHit>, LoreleiError> {
        Ok(self.hits.clone())
    }
}

#[derive(Default)]
struct FakeShells {
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl ShellRegistry for FakeShells {
    async fn list_shells(&self) -> Result<Vec<String>, LoreleiError> {
        Ok(vec!["echo".to_string(), "forget_pearl".to_string()])
    }

    async fn call(&self, call: ShellCall) -> Result<ShellResult, LoreleiError> {
        self.calls.lock().unwrap().push(call.tool.clone());
        Ok(ShellResult {
            call_id: call.call_id,
            ok: true,
            output: json!({"ok": true}),
            error: None,
            started_at: chrono::Utc::now(),
            finished_at: chrono::Utc::now(),
        })
    }
}

#[derive(Default)]
struct CapturingSong {
    inner: MockSongProvider,
    requests: Mutex<Vec<SongRequest>>,
}

#[derive(Default)]
struct MemLoreStore {
    pearls: Mutex<HashMap<PearlId, Pearl>>,
}

#[async_trait]
impl LoreStore for MemLoreStore {
    async fn save_pearl(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        pearl: lorelei_core::types::NewPearl,
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

#[async_trait]
impl SongProvider for CapturingSong {
    fn capabilities(&self) -> lorelei_core::types::ProviderCapabilities {
        self.inner.capabilities()
    }

    async fn complete(&self, request: SongRequest) -> Result<SongResponse, LoreleiError> {
        self.requests.lock().unwrap().push(request.clone());
        self.inner.complete(request).await
    }

    async fn stream(
        &self,
        request: SongRequest,
    ) -> Result<futures::stream::BoxStream<'static, SongChunk>, LoreleiError> {
        self.inner.stream(request).await
    }

    async fn embed(
        &self,
        request: lorelei_core::types::EmbeddingRequest,
    ) -> Result<lorelei_core::types::EmbeddingResponse, LoreleiError> {
        self.inner.embed(request).await
    }
}

fn cfg(allow_shell_execution: bool, allow_network: bool) -> LoreleiConfig {
    std::env::set_var("LORELEI_DETERMINISTIC_EXTRACT", "0");

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
            allow_shell_execution,
            allow_network_tools: allow_network,
        },
        docs: Default::default(),
        providers,
    }
}

#[tokio::test]
async fn direct_answer_path() {
    let cfg = cfg(false, false);
    let runs = Arc::new(MemRuns::default());
    let currents = Arc::new(MemCurrents::default());
    let echo = Arc::new(FixedEcho {
        hits: vec![EchoHit {
            score: UnitInterval::new(1.0).unwrap(),
            pearl_id: PearlId(Uuid::new_v4()),
            content: "remember this".to_string(),
            pearl_type: PearlType::Other,
            reason: "test".to_string(),
            created_at: chrono::Utc::now(),
            citation: None,
        }],
    });
    let song = Arc::new(CapturingSong {
        inner: MockSongProvider::deterministic(),
        requests: Mutex::new(Vec::new()),
    });
    let lore: Arc<dyn LoreStore> = Arc::new(MemLoreStore::default());
    let shells = Arc::new(FakeShells::default());
    let siren: Arc<dyn SirenPolicy> = Arc::new(DeterministicSirenPolicy::new(cfg.clone()));

    let tide = SingleAgentTideRuntime::new(
        cfg,
        runs,
        currents.clone(),
        echo,
        lore,
        song.clone(),
        shells,
        siren,
    )
    .with_templates(PLANNER_OK, ANSWER_TEMPLATE);

    let res = tide
        .run_once(
            TenantId(Uuid::from_u128(1)),
            AgentId(Uuid::from_u128(2)),
            "hi".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(res.status, RunStatus::Succeeded);
    assert!(res.output.contains("run_id="));

    let reqs = song.requests.lock().unwrap().clone();
    let answer_req = reqs
        .iter()
        .find(|r| r.reasoning_summary.as_deref() == Some("answer"))
        .expect("answer request");
    assert!(answer_req
        .context
        .iter()
        .any(|c| c.contains("remember this")));

    let events = currents.events.lock().unwrap();
    assert!(events
        .iter()
        .any(|e| e.event_type == lorelei_core::types::CurrentEventType::User));
    assert!(events
        .iter()
        .any(|e| e.event_type == lorelei_core::types::CurrentEventType::Assistant));
}

#[tokio::test]
async fn invalid_planner_json_repair_path() {
    let cfg = cfg(false, false);
    let runs = Arc::new(MemRuns::default());
    let currents = Arc::new(MemCurrents::default());
    let echo = Arc::new(FixedEcho { hits: vec![] });
    let lore: Arc<dyn LoreStore> = Arc::new(MemLoreStore::default());
    let song: Arc<dyn SongProvider> = Arc::new(MockSongProvider::deterministic());
    let shells = Arc::new(FakeShells::default());
    let siren: Arc<dyn SirenPolicy> = Arc::new(DeterministicSirenPolicy::new(cfg.clone()));

    let tide = SingleAgentTideRuntime::new(cfg, runs, currents, echo, lore, song, shells, siren)
        .with_templates(PLANNER_INVALID_ONCE, ANSWER_TEMPLATE);
    let res = tide
        .run_once(
            TenantId(Uuid::from_u128(1)),
            AgentId(Uuid::from_u128(2)),
            "hi".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(res.status, RunStatus::Succeeded);
}

#[tokio::test]
async fn siren_requires_approval_path() {
    let cfg = cfg(true, false);
    let runs = Arc::new(MemRuns::default());
    let currents = Arc::new(MemCurrents::default());
    let echo = Arc::new(FixedEcho { hits: vec![] });
    let lore: Arc<dyn LoreStore> = Arc::new(MemLoreStore::default());
    let song: Arc<dyn SongProvider> = Arc::new(ScriptedSong::new(vec![
        r#"{"action":"call_shell","tool":"forget_pearl","input":{"pearl_id":"00000000-0000-0000-0000-000000000000"}}"#.to_string(),
        "[]".to_string(),
    ]));
    let shells = Arc::new(FakeShells::default());
    let siren: Arc<dyn SirenPolicy> = Arc::new(DeterministicSirenPolicy::new(cfg.clone()));

    let tide = SingleAgentTideRuntime::new(cfg, runs, currents, echo, lore, song, shells, siren)
        .with_templates("{}", ANSWER_TEMPLATE);
    let res = tide
        .run_once(
            TenantId(Uuid::from_u128(1)),
            AgentId(Uuid::from_u128(2)),
            "forget it".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(res.status, RunStatus::Canceled);
    assert!(res.output.contains("Approval required"));
}

#[tokio::test]
async fn shell_call_path() {
    let cfg = cfg(true, false);
    let runs = Arc::new(MemRuns::default());
    let currents = Arc::new(MemCurrents::default());
    let echo = Arc::new(FixedEcho { hits: vec![] });
    let lore: Arc<dyn LoreStore> = Arc::new(MemLoreStore::default());
    let song: Arc<dyn SongProvider> = Arc::new(ScriptedSong::new(vec![
        r#"{"action":"call_shell","tool":"echo","input":{"message":"hi"}}"#.to_string(),
        "done".to_string(),
        "[]".to_string(),
    ]));
    let shells = Arc::new(FakeShells::default());
    let siren: Arc<dyn SirenPolicy> = Arc::new(DeterministicSirenPolicy::new(cfg.clone()));

    let tide = SingleAgentTideRuntime::new(
        cfg,
        runs,
        currents.clone(),
        echo,
        lore,
        song,
        shells.clone(),
        siren,
    )
    .with_templates("{}", ANSWER_TEMPLATE);
    let res = tide
        .run_once(
            TenantId(Uuid::from_u128(1)),
            AgentId(Uuid::from_u128(2)),
            "say hi".to_string(),
        )
        .await
        .unwrap();

    assert_eq!(res.status, RunStatus::Succeeded);
    assert_eq!(
        shells.calls.lock().unwrap().as_slice(),
        &["echo".to_string()]
    );
    let events = currents.events.lock().unwrap();
    assert!(events
        .iter()
        .any(|e| e.event_type == lorelei_core::types::CurrentEventType::ToolCall));
    assert!(events
        .iter()
        .any(|e| e.event_type == lorelei_core::types::CurrentEventType::ToolResult));
}
