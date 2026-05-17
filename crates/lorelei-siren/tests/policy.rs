use lorelei_core::config::{
    AgentConfig, EchoConfig, HarborConfig, LoreConfig, LoreleiConfig, ProviderConfig, ProviderKind,
    SirenConfig,
};
use lorelei_core::traits::{ShellRegistry, SirenPolicy};
use lorelei_core::types::{
    AgentId, NormalizedToolCall, RunId, ShellCall, ShellResult, ShellRisk, SirenDecision,
    SongRequest, SongResponse, TenantId,
};
use lorelei_siren::policy::DeterministicSirenPolicy;
use lorelei_tide::flow::TideShellExecutor;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

fn base_config() -> LoreleiConfig {
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

fn request() -> SongRequest {
    SongRequest {
        tenant_id: TenantId(Uuid::from_u128(1)),
        agent_id: AgentId(Uuid::from_u128(2)),
        run_id: RunId(Uuid::new_v4()),
        input: "please do it".to_string(),
        context: Vec::new(),
        reasoning_summary: None,
    }
}

fn response() -> SongResponse {
    SongResponse {
        output: "ok".to_string(),
        reasoning_summary: None,
        tool_calls: Vec::new(),
    }
}

#[tokio::test]
async fn low_risk_echo_shell_allowed_when_shells_enabled() {
    let mut cfg = base_config();
    cfg.siren.allow_shell_execution = true;
    let policy = DeterministicSirenPolicy::new(cfg);

    let tool_calls = vec![NormalizedToolCall {
        call_id: "1".to_string(),
        name: "echo".to_string(),
        arguments: json!({"message":"hi"}),
    }];

    let decision = policy
        .decide(
            TenantId(Uuid::from_u128(1)),
            AgentId(Uuid::from_u128(2)),
            RunId(Uuid::new_v4()),
            None,
            &request(),
            &response(),
            &tool_calls,
            &[],
        )
        .await
        .unwrap();

    assert!(matches!(decision, SirenDecision::Allow { .. }));
}

#[tokio::test]
async fn forget_pearl_requires_approval() {
    let mut cfg = base_config();
    cfg.siren.allow_shell_execution = true;
    let policy = DeterministicSirenPolicy::new(cfg);

    let tool_calls = vec![NormalizedToolCall {
        call_id: "1".to_string(),
        name: "forget_pearl".to_string(),
        arguments: json!({"pearl_id": Uuid::new_v4()}),
    }];

    let decision = policy
        .decide(
            TenantId(Uuid::from_u128(1)),
            AgentId(Uuid::from_u128(2)),
            RunId(Uuid::new_v4()),
            None,
            &request(),
            &response(),
            &tool_calls,
            &[],
        )
        .await
        .unwrap();

    assert!(matches!(decision, SirenDecision::RequireApproval { .. }));
}

#[tokio::test]
async fn http_get_denied_when_network_disabled() {
    let mut cfg = base_config();
    cfg.siren.allow_shell_execution = true;
    cfg.siren.allow_network_tools = false;
    let policy = DeterministicSirenPolicy::new(cfg);

    let tool_calls = vec![NormalizedToolCall {
        call_id: "1".to_string(),
        name: "http_get".to_string(),
        arguments: json!({"url": "http://example.com"}),
    }];

    let decision = policy
        .decide(
            TenantId(Uuid::from_u128(1)),
            AgentId(Uuid::from_u128(2)),
            RunId(Uuid::new_v4()),
            None,
            &request(),
            &response(),
            &tool_calls,
            &[],
        )
        .await
        .unwrap();

    assert!(matches!(decision, SirenDecision::Deny { .. }));
}

#[tokio::test]
async fn shell_execution_denied_by_default() {
    let cfg = base_config();
    let policy = DeterministicSirenPolicy::new(cfg);

    let tool_calls = vec![NormalizedToolCall {
        call_id: "1".to_string(),
        name: "echo".to_string(),
        arguments: json!({"message":"hi"}),
    }];

    let decision = policy
        .decide(
            TenantId(Uuid::from_u128(1)),
            AgentId(Uuid::from_u128(2)),
            RunId(Uuid::new_v4()),
            None,
            &request(),
            &response(),
            &tool_calls,
            &[],
        )
        .await
        .unwrap();

    assert!(matches!(decision, SirenDecision::Deny { .. }));
}

#[tokio::test]
async fn cross_tenant_access_denied() {
    let mut cfg = base_config();
    cfg.siren.allow_shell_execution = true;
    let policy = DeterministicSirenPolicy::new(cfg);

    let mut req = request();
    req.tenant_id = TenantId(Uuid::from_u128(999));

    let tool_calls = vec![NormalizedToolCall {
        call_id: "1".to_string(),
        name: "echo".to_string(),
        arguments: json!({"message":"hi"}),
    }];

    let decision = policy
        .decide(
            TenantId(Uuid::from_u128(1)),
            AgentId(Uuid::from_u128(2)),
            RunId(Uuid::new_v4()),
            None,
            &req,
            &response(),
            &tool_calls,
            &[],
        )
        .await
        .unwrap();

    assert!(matches!(decision, SirenDecision::Deny { .. }));
}

#[derive(Default)]
struct CountingShells {
    calls: Mutex<u32>,
}

#[async_trait::async_trait]
impl ShellRegistry for CountingShells {
    async fn list_shells(&self) -> Result<Vec<String>, lorelei_core::error::LoreleiError> {
        Ok(vec!["forget_pearl".to_string()])
    }

    async fn call(
        &self,
        call: ShellCall,
    ) -> Result<ShellResult, lorelei_core::error::LoreleiError> {
        *self.calls.lock().unwrap() += 1;
        Ok(ShellResult {
            call_id: call.call_id,
            ok: true,
            output: json!({}),
            error: None,
            started_at: chrono::Utc::now(),
            finished_at: chrono::Utc::now(),
        })
    }
}

#[tokio::test]
async fn tide_cannot_execute_high_risk_without_approval() {
    let mut cfg = base_config();
    cfg.siren.allow_shell_execution = true;
    let policy = Arc::new(DeterministicSirenPolicy::new(cfg));
    let shells = Arc::new(CountingShells::default());
    let tide = TideShellExecutor::new(policy, shells.clone());

    let call = ShellCall {
        call_id: Uuid::new_v4(),
        tenant_id: TenantId(Uuid::from_u128(1)),
        agent_id: AgentId(Uuid::from_u128(2)),
        run_id: RunId(Uuid::new_v4()),
        shell: "builtin".to_string(),
        tool: "forget_pearl".to_string(),
        input: json!({"pearl_id": Uuid::new_v4()}),
        risk: ShellRisk::High,
        requested_at: chrono::Utc::now(),
    };

    let err = tide.execute_shell_call(call, false).await.unwrap_err();
    assert!(err.to_string().contains("requires approval"));
    assert_eq!(*shells.calls.lock().unwrap(), 0);
}
