use async_trait::async_trait;
use lorelei_core::config::{
    AgentConfig, EchoConfig, HarborConfig, LoreConfig, LoreleiConfig, ProviderConfig, ProviderKind,
    SirenConfig,
};
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::{DocumentStore, EchoRetriever, LoreStore, ShellRegistry};
use lorelei_core::types::{
    AgentId, EchoHit, EchoQuery, NewPearl, Pearl, PearlId, PearlListQuery, RunId, ShellCall,
    ShellRisk, TenantId, UnitInterval,
};
use lorelei_shells::registry::BuiltinShellRegistry;
use lorelei_shells::repo::ShellCallRepository;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Default)]
struct MemLoreStore {
    pearls: Mutex<Vec<Pearl>>,
}

#[async_trait]
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
        tenant_id: TenantId,
        pearl_id: PearlId,
    ) -> Result<(), LoreleiError> {
        self.pearls
            .lock()
            .unwrap()
            .retain(|p| !(p.tenant_id == tenant_id && p.pearl_id == pearl_id));
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

struct MemEcho {
    lore: Arc<MemLoreStore>,
}

#[async_trait]
impl EchoRetriever for MemEcho {
    async fn query(
        &self,
        tenant_id: TenantId,
        _agent_id: AgentId,
        query: EchoQuery,
    ) -> Result<Vec<EchoHit>, LoreleiError> {
        let pearls = self
            .lore
            .list_pearls(tenant_id, PearlListQuery::default())
            .await?;
        let q = query.query.to_ascii_lowercase();
        let mut hits = Vec::new();
        for p in pearls {
            if p.content.to_ascii_lowercase().contains(&q) {
                hits.push(EchoHit {
                    score: UnitInterval::new(1.0).unwrap(),
                    pearl_id: p.pearl_id,
                    content: p.content,
                    pearl_type: p.pearl_type,
                    reason: "keyword match".to_string(),
                    created_at: p.created_at,
                    citation: None,
                });
            }
        }
        Ok(hits)
    }
}

#[derive(Clone, Default)]
struct NullDocs;

#[async_trait]
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
struct MemShellCalls {
    starts: Mutex<Vec<Uuid>>,
    finishes: Mutex<Vec<Uuid>>,
}

#[async_trait]
impl ShellCallRepository for MemShellCalls {
    async fn record_start(
        &self,
        _current_id: Option<Uuid>,
        call: &ShellCall,
    ) -> Result<(), LoreleiError> {
        self.starts.lock().unwrap().push(call.call_id);
        Ok(())
    }

    async fn record_finish(
        &self,
        result: &lorelei_core::types::ShellResult,
    ) -> Result<(), LoreleiError> {
        self.finishes.lock().unwrap().push(result.call_id);
        Ok(())
    }
}

fn test_config(allow_network_tools: bool) -> LoreleiConfig {
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
            allow_network_tools,
        },
        docs: Default::default(),
        providers,
    }
}

fn shell_call(tool: &str, input: serde_json::Value) -> ShellCall {
    ShellCall {
        call_id: Uuid::new_v4(),
        tenant_id: TenantId(Uuid::from_u128(1)),
        agent_id: AgentId(Uuid::from_u128(2)),
        run_id: RunId(Uuid::new_v4()),
        shell: "builtin".to_string(),
        tool: tool.to_string(),
        input,
        risk: ShellRisk::Low,
        requested_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn unknown_shell_returns_error() {
    let cfg = test_config(false);
    let lore = Arc::new(MemLoreStore::default());
    let echo = Arc::new(MemEcho { lore: lore.clone() });
    let calls = Arc::new(MemShellCalls::default());
    let reg = BuiltinShellRegistry::new(cfg, lore, echo, Arc::new(NullDocs), calls);

    let err = reg
        .call(shell_call("does_not_exist", json!({})))
        .await
        .unwrap_err();
    assert!(matches!(err, LoreleiError::NotFound(_)));
}

#[tokio::test]
async fn invalid_json_input_returns_validation_error() {
    let cfg = test_config(false);
    let lore = Arc::new(MemLoreStore::default());
    let echo = Arc::new(MemEcho { lore: lore.clone() });
    let calls = Arc::new(MemShellCalls::default());
    let reg = BuiltinShellRegistry::new(cfg, lore, echo, Arc::new(NullDocs), calls);

    let err = reg.call(shell_call("echo", json!({}))).await.unwrap_err();
    assert!(matches!(err, LoreleiError::Validation { .. }));
}

#[tokio::test]
async fn save_pearl_creates_a_pearl() {
    let cfg = test_config(false);
    let lore = Arc::new(MemLoreStore::default());
    let echo = Arc::new(MemEcho { lore: lore.clone() });
    let calls = Arc::new(MemShellCalls::default());
    let reg = BuiltinShellRegistry::new(cfg, lore.clone(), echo, Arc::new(NullDocs), calls);

    let _ = reg
        .call(shell_call(
            "save_pearl",
            json!({ "pearl_type": "Other", "content": "The Lore starts in Postgres." }),
        ))
        .await
        .unwrap();

    let pearls = lore
        .list_pearls(TenantId(Uuid::from_u128(1)), PearlListQuery::default())
        .await
        .unwrap();
    assert_eq!(pearls.len(), 1);
}

#[tokio::test]
async fn echo_lore_retrieves_pearls() {
    let cfg = test_config(false);
    let lore = Arc::new(MemLoreStore::default());
    let echo = Arc::new(MemEcho { lore: lore.clone() });
    let calls = Arc::new(MemShellCalls::default());
    let reg = BuiltinShellRegistry::new(cfg, lore.clone(), echo, Arc::new(NullDocs), calls);

    let _ = reg
        .call(shell_call(
            "save_pearl",
            json!({ "pearl_type": "Other", "content": "The Lore starts in Postgres." }),
        ))
        .await
        .unwrap();

    let res = reg
        .call(shell_call("echo_lore", json!({ "query": "postgres" })))
        .await
        .unwrap();
    assert!(res.ok);
    assert!(res.output.is_array());
    assert_eq!(res.output.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn forget_pearl_is_marked_high_risk() {
    let cfg = test_config(false);
    let lore = Arc::new(MemLoreStore::default());
    let echo = Arc::new(MemEcho { lore: lore.clone() });
    let calls = Arc::new(MemShellCalls::default());
    let reg = BuiltinShellRegistry::new(cfg, lore, echo, Arc::new(NullDocs), calls);

    let specs = reg.specs();
    let fp = specs.iter().find(|s| s.name == "forget_pearl").unwrap();
    assert_eq!(fp.risk, ShellRisk::High);
}

#[tokio::test]
async fn http_get_disabled_when_network_tools_disabled() {
    let cfg = test_config(false);
    let lore = Arc::new(MemLoreStore::default());
    let echo = Arc::new(MemEcho { lore: lore.clone() });
    let calls = Arc::new(MemShellCalls::default());
    let reg = BuiltinShellRegistry::new(cfg, lore, echo, Arc::new(NullDocs), calls);

    let err = reg
        .call(shell_call(
            "http_get",
            json!({ "url": "http://example.com" }),
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, LoreleiError::Unsupported(_)));
}
