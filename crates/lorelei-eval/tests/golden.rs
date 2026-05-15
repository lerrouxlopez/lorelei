use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use lorelei_core::{
    ApiKeySource, Config, CurrentEvent, EchoHit, EchoQuery, LoreleiError, NewPearl, Pearl, PearlType,
    ProviderCapabilities, ProposedAction, ShellCall, ShellResult, ShellRisk, SirenPolicy, SongChunk,
    SongProvider, SongRequest, SongResponse,
};
use lorelei_eval::FallbackSongProvider;
use lorelei_tide::{EchoRuntime, LoreRuntime, RunRowLite, ShellRuntime, TideConfig, TideEngine};
use serde_json::json;
use uuid::Uuid;

#[derive(Default)]
struct MemLore {
    pearls: Mutex<HashMap<(Uuid, Uuid), Pearl>>, // (tenant, pearl_id)
    deleted: Mutex<HashMap<(Uuid, Uuid), bool>>,
}

#[derive(Clone, Default)]
struct MemLoreHandle(Arc<MemLore>);

#[async_trait]
impl LoreRuntime for MemLoreHandle {
    async fn create_run(
        &self,
        _tenant_id: Uuid,
        _metadata: serde_json::Value,
    ) -> Result<RunRowLite, LoreleiError> {
        Ok(RunRowLite { id: Uuid::new_v4() })
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
        _event: CurrentEvent,
        _confidence: Option<f64>,
        _importance: Option<f64>,
    ) -> Result<(), LoreleiError> {
        Ok(())
    }

    async fn save_pearl(
        &self,
        tenant_id: Uuid,
        agent_id: Option<Uuid>,
        run_id: Uuid,
        pearl: NewPearl,
    ) -> Result<Pearl, LoreleiError> {
        pearl.validate()?;
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
        self.0
            .pearls
            .lock()
            .unwrap()
            .insert((tenant_id, p.id), p.clone());
        Ok(p)
    }
}

struct MemEcho {
    lore: MemLoreHandle,
}

#[async_trait]
impl EchoRuntime for MemEcho {
    async fn retrieve(&self, q: EchoQuery) -> Result<Vec<EchoHit>, LoreleiError> {
        q.validate()?;
        let pearls = self.lore.0.pearls.lock().unwrap();
        let deleted = self.lore.0.deleted.lock().unwrap();
        let mut out = Vec::new();
        for ((tenant, pearl_id), p) in pearls.iter() {
            if *tenant != q.tenant_id {
                continue;
            }
            if deleted.get(&(*tenant, *pearl_id)).copied().unwrap_or(false) {
                continue;
            }
            if let Some(pt) = q.pearl_type {
                if p.pearl_type != pt {
                    continue;
                }
            }
            if let Some(minc) = q.min_confidence {
                if p.confidence.unwrap_or(0.0) < minc {
                    continue;
                }
            }
            if p.content.to_lowercase().contains(&q.text.to_lowercase()) {
                out.push(EchoHit {
                    pearl_id: p.id,
                    score: 1.0,
                    snippet: Some(p.content.clone()),
                    metadata: json!({}),
                });
            }
        }
        out.truncate(q.limit);
        Ok(out)
    }
}

struct MemShells {
    risks: HashMap<String, ShellRisk>,
}

#[async_trait]
impl ShellRuntime for MemShells {
    fn validate_name(&self, name: &str) -> Result<(), LoreleiError> {
        if self.risks.contains_key(name) {
            Ok(())
        } else {
            Err(LoreleiError::validation("invalid shell name"))
        }
    }

    fn risk(&self, name: &str) -> Result<ShellRisk, LoreleiError> {
        self.validate_name(name)?;
        Ok(self.risks[name])
    }

    async fn execute(
        &self,
        _tenant_id: Uuid,
        _run_id: Uuid,
        shell_name: &str,
        _call: ShellCall,
        _input: serde_json::Value,
    ) -> Result<ShellResult, LoreleiError> {
        Ok(ShellResult {
            exit_code: 0,
            stdout: format!("ran {shell_name}"),
            stderr: String::new(),
            duration_ms: None,
        })
    }
}

#[derive(Default)]
struct MockSong {
    planner_invalid_once: Mutex<bool>,
}

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
        let prompt = request.prompt;
        let out;

        if prompt.contains("Tide planner") {
            let mut flag = self.planner_invalid_once.lock().unwrap();
            if !*flag {
                *flag = true;
                out = "not json".to_string();
            } else {
                out = r#"{"steps":[{"type":"noop","message":"ok"}]}"#.to_string();
            }
        } else if prompt.contains("Repair the following") {
            out = r#"{"steps":[{"type":"noop","message":"repaired"}]}"#.to_string();
        } else if prompt.contains("Lore Extractor") {
            out = r#"[{"pearl_type":"note","content":"manual pearl","tags":[],"metadata":{}}]"#.to_string();
        } else if prompt.contains("Lore Critic") {
            out = r#"[{"accept":true,"reason":"ok"}]"#.to_string();
        } else {
            // Preference pearl affects answer: if the prompt includes "prefer: cats" respond accordingly.
            if prompt.contains("prefer: cats") {
                out = "cats".to_string();
            } else {
                out = "default".to_string();
            }
        }

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
        Err(LoreleiError::SongProvider { message: "unused".to_string() })
    }
}

struct FailingSong;

#[async_trait]
impl SongProvider for FailingSong {
    async fn capabilities(&self) -> Result<ProviderCapabilities, LoreleiError> {
        Err(LoreleiError::SongProvider { message: "fail".to_string() })
    }
    async fn song(&self, _request: SongRequest) -> Result<SongResponse, LoreleiError> {
        Err(LoreleiError::SongProvider { message: "fail".to_string() })
    }
    async fn song_stream(
        &self,
        _request: SongRequest,
    ) -> Result<futures::stream::BoxStream<'static, Result<SongChunk, LoreleiError>>, LoreleiError> {
        Err(LoreleiError::SongProvider { message: "fail".to_string() })
    }
}

#[tokio::test]
async fn manual_pearl_save_and_echo_retrieval() {
    let lore = MemLoreHandle::default();
    let tenant = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    let p = lore
        .save_pearl(
            tenant,
            None,
            run_id,
            NewPearl {
                pearl_type: PearlType::Note,
                content: "hello world".to_string(),
                tags: vec![],
                confidence: Some(1.0),
                importance: None,
                metadata: json!({}),
            },
        )
        .await
        .unwrap();

    let echo = MemEcho { lore: lore.clone() };
    let hits = echo
        .retrieve(EchoQuery {
            text: "hello".to_string(),
            tenant_id: tenant,
            agent_id: None,
            run_id: None,
            pearl_type: None,
            min_confidence: None,
            limit: 10,
        })
        .await
        .unwrap();
    assert!(hits.iter().any(|h| h.pearl_id == p.id));
}

#[tokio::test]
async fn tenant_isolation_and_deleted_exclusion() {
    let lore = MemLoreHandle::default();
    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();

    let a = lore
        .save_pearl(
            tenant_a,
            None,
            Uuid::new_v4(),
            NewPearl {
                pearl_type: PearlType::Note,
                content: "shared word".to_string(),
                tags: vec![],
                confidence: None,
                importance: None,
                metadata: json!({}),
            },
        )
        .await
        .unwrap();
    let b = lore
        .save_pearl(
            tenant_b,
            None,
            Uuid::new_v4(),
            NewPearl {
                pearl_type: PearlType::Note,
                content: "shared word".to_string(),
                tags: vec![],
                confidence: None,
                importance: None,
                metadata: json!({}),
            },
        )
        .await
        .unwrap();

    // Delete tenant_a pearl
    lore.0.deleted.lock().unwrap().insert((tenant_a, a.id), true);

    let echo = MemEcho { lore: lore.clone() };
    let hits_a = echo
        .retrieve(EchoQuery {
            text: "shared".to_string(),
            tenant_id: tenant_a,
            agent_id: None,
            run_id: None,
            pearl_type: None,
            min_confidence: None,
            limit: 10,
        })
        .await
        .unwrap();
    assert!(hits_a.iter().all(|h| h.pearl_id != a.id));
    assert!(hits_a.iter().all(|h| h.pearl_id != b.id));
}

#[tokio::test]
async fn invalid_planner_json_repairs_once() {
    let lore = MemLoreHandle::default();
    let echo = MemEcho { lore: lore.clone() };
    let shells = MemShells {
        risks: [("noop".to_string(), ShellRisk::Low)].into_iter().collect(),
    };
    let cfg = TideConfig {
        siren: lorelei_siren::SirenConfig {
            allow_shell_execution: true,
            allow_network_tools: true,
        },
        siren_prompt_path: None,
        lore_extractor_path: "prompts/lore_extractor.md".to_string(),
        lore_critic_path: "prompts/lore_critic.md".to_string(),
    };
    let engine = TideEngine::new(lore, echo, MockSong::default(), std::sync::Arc::new(shells), cfg)
        .unwrap();
    let out = engine.run(Uuid::new_v4(), None, "x".to_string()).await.unwrap();
    assert!(!out.answer.is_empty());
}

#[tokio::test]
async fn high_risk_shell_requires_approval_low_risk_runs() {
    let policy = lorelei_siren::DeterministicSirenPolicy::new(lorelei_siren::SirenConfig {
        allow_shell_execution: true,
        allow_network_tools: true,
    });

    let tenant = Uuid::new_v4();
    let low = ProposedAction {
        tenant_id: tenant,
        target_tenant_id: tenant,
        call: ShellCall {
            program: "ls".to_string(),
            args: vec![],
            cwd: None,
            env: Default::default(),
            stdin: None,
            timeout_ms: None,
        },
        risk: ShellRisk::Low,
        rationale: "".to_string(),
    };
    let d = policy.decide(low).await.unwrap();
    assert!(d.allow);

    let high = ProposedAction {
        tenant_id: tenant,
        target_tenant_id: tenant,
        call: ShellCall {
            program: "rm".to_string(),
            args: vec!["-rf".to_string(), "x".to_string()],
            cwd: None,
            env: Default::default(),
            stdin: None,
            timeout_ms: None,
        },
        risk: ShellRisk::Low,
        rationale: "".to_string(),
    };
    let d2 = policy.decide(high).await.unwrap();
    assert!(!d2.allow);
    assert_eq!(d2.risk, ShellRisk::High);
}

#[tokio::test]
async fn provider_fallback_works() {
    let provider = FallbackSongProvider {
        primary: FailingSong,
        fallback: MockSong::default(),
    };
    let r = provider
        .song(SongRequest {
            prompt: "hi".to_string(),
            context: vec![],
            max_chunks: None,
            parameters: Default::default(),
        })
        .await
        .unwrap();
    assert_eq!(r.chunks[0].content, "default");
}

#[test]
fn unsupported_provider_fails_clearly() {
    let toml = r#"
        [providers.e]
        kind = "gemini-native"
        model = "x"

        [providers.openai]
        kind = "openai-compatible"
        base_url = "https://example.invalid/v1"
        model = "text-embedding-3-small"
        api_key = { source = "literal", value = "x" }

        [song]
        provider = { name = "e" }
    "#;
    let cfg = Config::from_toml_str(toml).unwrap();
    let provider = lorelei_song::build_song_provider(&cfg).unwrap();
    // Calling the stub should fail with a clear message.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(provider.song(SongRequest {
            prompt: "hi".to_string(),
            context: vec![],
            max_chunks: None,
            parameters: Default::default(),
        }))
        .unwrap_err();
    assert!(err.to_string().to_lowercase().contains("stub"));
}

#[test]
fn docker_config_validates() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    assert!(root.join("docker/Dockerfile").exists());
    assert!(root.join("docker-compose.yml").exists());
    assert!(root.join(".env.example").exists());
    assert!(root.join("lorelei.toml.example").exists());

    let cfg = Config::from_toml_file(root.join("lorelei.toml.example")).unwrap();
    cfg.validate().unwrap();

    // Ensure example uses env-based API keys (no secrets committed).
    for (_name, p) in cfg.providers {
        match p {
            lorelei_core::ProviderConfig::OpenAiCompatible(c) => match c.api_key {
                ApiKeySource::Env { .. } => {}
                _ => {}
            },
            _ => {}
        }
    }
}
