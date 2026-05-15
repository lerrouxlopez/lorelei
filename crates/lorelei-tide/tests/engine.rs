use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use lorelei_core::{
    EchoHit, EchoQuery, LoreleiError, NewPearl, Pearl, ProviderCapabilities, ShellCall, ShellResult,
    ShellRisk, SongChunk, SongProvider, SongRequest, SongResponse,
};
use lorelei_tide::{EchoRuntime, LoreRuntime, RunRowLite, ShellRuntime, TideConfig, TideEngine};
use serde_json::json;
use uuid::Uuid;

#[derive(Default)]
struct MockLore {
    runs: Mutex<Vec<Uuid>>,
    currents: Mutex<Vec<(Uuid, String)>>,
    pearls: Mutex<Vec<Pearl>>,
}

#[async_trait]
impl LoreRuntime for MockLore {
    async fn create_run(
        &self,
        _tenant_id: Uuid,
        _metadata: serde_json::Value,
    ) -> Result<RunRowLite, LoreleiError> {
        let id = Uuid::new_v4();
        self.runs.lock().unwrap().push(id);
        Ok(RunRowLite { id })
    }

    async fn complete_run(
        &self,
        _tenant_id: Uuid,
        _run_id: Uuid,
        _ended_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), LoreleiError> {
        Ok(())
    }

    async fn write_current(
        &self,
        _tenant_id: Uuid,
        event: lorelei_core::CurrentEvent,
        _confidence: Option<f64>,
        _importance: Option<f64>,
    ) -> Result<(), LoreleiError> {
        self.currents
            .lock()
            .unwrap()
            .push((event.run_id, event.kind));
        Ok(())
    }

    async fn save_pearl(
        &self,
        tenant_id: Uuid,
        agent_id: Option<Uuid>,
        run_id: Uuid,
        pearl: NewPearl,
    ) -> Result<Pearl, LoreleiError> {
        let p = Pearl {
            id: Uuid::new_v4(),
            tenant_id,
            agent_id,
            run_id,
            pearl_type: pearl.pearl_type,
            created_at: chrono::Utc::now(),
            deleted_at: None,
            last_echoed_at: None,
            content: pearl.content,
            tags: pearl.tags,
            confidence: pearl.confidence,
            importance: pearl.importance,
            metadata: pearl.metadata,
        };
        self.pearls.lock().unwrap().push(p.clone());
        Ok(p)
    }
}

struct MockEcho;

#[async_trait]
impl EchoRuntime for MockEcho {
    async fn retrieve(&self, q: EchoQuery) -> Result<Vec<EchoHit>, LoreleiError> {
        // Ensure tenant is always present (prevents cross-tenant hidden retrieval paths).
        let _ = q.tenant_id;
        Ok(vec![])
    }
}

struct MockShells;

#[async_trait]
impl ShellRuntime for MockShells {
    fn validate_name(&self, name: &str) -> Result<(), LoreleiError> {
        if name == "noop" || name == "echo" {
            Ok(())
        } else {
            Err(LoreleiError::validation("unknown shell"))
        }
    }

    fn risk(&self, name: &str) -> Result<ShellRisk, LoreleiError> {
        Ok(match name {
            "noop" => ShellRisk::Low,
            "echo" => ShellRisk::Low,
            _ => ShellRisk::High,
        })
    }

    async fn execute(
        &self,
        _tenant_id: Uuid,
        _run_id: Uuid,
        shell_name: &str,
        _call: ShellCall,
        input: serde_json::Value,
    ) -> Result<ShellResult, LoreleiError> {
        Ok(ShellResult {
            exit_code: 0,
            stdout: format!("{shell_name}:{input}"),
            stderr: String::new(),
            duration_ms: None,
        })
    }
}

#[derive(Default)]
struct MockSong;

#[async_trait]
impl SongProvider for MockSong {
    async fn capabilities(&self) -> Result<ProviderCapabilities, LoreleiError> {
        Ok(ProviderCapabilities {
            streaming: false,
            max_context_tokens: None,
            json_mode: true,
            tools: false,
            embeddings: false,
        })
    }

    async fn song(&self, request: SongRequest) -> Result<SongResponse, LoreleiError> {
        let p = request.prompt;
        let out = if p.contains("Tide planner") {
            // Plan includes a noop shell call.
            r#"{"steps":[{"type":"shell","name":"noop","input":{},"rationale":"test"}]}"#.to_string()
        } else if p.contains("Lore Extractor") {
            r#"[{"pearl_type":"note","content":"remember this","tags":[],"metadata":{}}]"#.to_string()
        } else if p.contains("Lore Critic") {
            r#"[{"accept":true,"reason":"ok"}]"#.to_string()
        } else if p.contains("Repair the following") {
            // In tests, repairs should not be needed; return empty JSON.
            r#"{"steps":[{"type":"noop","message":"repaired"}]}"#.to_string()
        } else {
            "final answer".to_string()
        };

        Ok(SongResponse {
            id: Uuid::new_v4(),
            created_at: chrono::Utc::now(),
            chunks: vec![SongChunk {
                index: 0,
                content: out,
                is_final: true,
                tool_calls: vec![],
            }],
            metadata: json!({"mock": true}),
        })
    }

    async fn song_stream(
        &self,
        _request: SongRequest,
    ) -> Result<futures::stream::BoxStream<'static, Result<SongChunk, LoreleiError>>, LoreleiError> {
        Err(LoreleiError::SongProvider {
            message: "mock streaming not used".to_string(),
        })
    }
}

#[tokio::test]
async fn tide_runs_with_mocks_and_saves_pearls() {
    let lore = MockLore::default();
    let echo = MockEcho;
    let song = MockSong::default();
    let shells: Arc<dyn ShellRuntime> = Arc::new(MockShells);

    let cfg = TideConfig {
        siren: lorelei_siren::SirenConfig {
            allow_shell_execution: true,
            allow_network_tools: false,
        },
        siren_prompt_path: None,
        lore_extractor_path: "prompts/lore_extractor.md".to_string(),
        lore_critic_path: "prompts/lore_critic.md".to_string(),
    };

    let engine = TideEngine::new(lore, echo, song, shells, cfg).unwrap();

    let tenant_id = Uuid::new_v4();
    let out = engine
        .run(tenant_id, None, "hello".to_string())
        .await
        .unwrap();

    assert!(!out.answer.is_empty());
}
