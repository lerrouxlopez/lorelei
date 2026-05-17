use futures::StreamExt;
use lorelei_core::config::{
    AgentConfig, EchoConfig, HarborConfig, LoreConfig, LoreleiConfig, ProviderConfig, ProviderKind,
    SirenConfig,
};
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::SongProvider;
use lorelei_core::types::{AgentId, EmbeddingRequest, ProviderCapabilities, SongRequest, TenantId};
use lorelei_song::providers::mock::MockSongProvider;
use lorelei_song::registry::ProviderRegistry;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

fn ids() -> (TenantId, AgentId) {
    (TenantId(Uuid::from_u128(1)), AgentId(Uuid::from_u128(2)))
}

#[tokio::test]
async fn mock_provider_returns_deterministic_response() {
    let (tenant_id, agent_id) = ids();
    let provider = MockSongProvider::deterministic();
    let resp = provider
        .complete(SongRequest {
            tenant_id,
            agent_id,
            run_id: lorelei_core::types::RunId(Uuid::from_u128(3)),
            input: "hello".to_string(),
            context: vec![],
            reasoning_summary: None,
        })
        .await
        .unwrap();
    assert_eq!(resp.output, "mock: hello");
}

#[test]
fn missing_api_key_produces_clear_error() {
    let (tenant_id, agent_id) = ids();

    let mut providers = BTreeMap::new();
    providers.insert(
        "p1".to_string(),
        ProviderConfig {
            kind: ProviderKind::OpenaiCompatible,
            base_url: Some("http://127.0.0.1:0".to_string()),
            api_key_env: "LORELEI_MISSING_KEY".to_string(),
            chat_model: "gpt".to_string(),
            embedding_model: None,
        },
    );

    let cfg = LoreleiConfig {
        agent: AgentConfig {
            tenant_id,
            agent_id,
            default_provider: "p1".to_string(),
            default_embedding_provider: "p1".to_string(),
        },
        harbor: HarborConfig {
            host: "127.0.0.1".to_string(),
            port: 7331,
        },
        lore: LoreConfig {
            postgres_url_env: "PG".to_string(),
            qdrant_url_env: "QD".to_string(),
            collection: "c".to_string(),
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

    let res = ProviderRegistry::from_config(&cfg);
    match res {
        Ok(_) => panic!("expected error"),
        Err(err) => assert!(matches!(err, LoreleiError::Validation { .. })),
    }
}

#[tokio::test]
async fn provider_without_embeddings_fails_capability_check() {
    let (tenant_id, _agent_id) = ids();

    let p = Arc::new(MockSongProvider {
        capabilities: ProviderCapabilities {
            supports_streaming: false,
            supports_tools: false,
            supports_json_mode: false,
            supports_embeddings: false,
            context_window: None,
            metadata: Default::default(),
        },
    });

    let mut map = BTreeMap::new();
    map.insert("noembed".to_string(), p as Arc<dyn SongProvider>);
    let reg = ProviderRegistry::from_providers(map);

    let req = EmbeddingRequest {
        tenant_id,
        provider: "noembed".to_string(),
        inputs: vec!["x".to_string()],
    };

    let err = reg
        .embed_with_fallback(&["noembed".to_string()], req)
        .await
        .unwrap_err();
    assert!(matches!(err, LoreleiError::Unsupported(_)));
}

struct RetryableFailProvider;

#[async_trait::async_trait]
impl SongProvider for RetryableFailProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_streaming: false,
            supports_tools: false,
            supports_json_mode: false,
            supports_embeddings: false,
            context_window: None,
            metadata: Default::default(),
        }
    }

    async fn complete(
        &self,
        _request: SongRequest,
    ) -> Result<lorelei_core::types::SongResponse, LoreleiError> {
        Err(LoreleiError::Provider("provider `p1` http 429".to_string()))
    }

    async fn stream(
        &self,
        _request: SongRequest,
    ) -> Result<futures::stream::BoxStream<'static, lorelei_core::types::SongChunk>, LoreleiError>
    {
        Err(LoreleiError::Provider("provider `p1` http 429".to_string()))
    }

    async fn embed(
        &self,
        _request: EmbeddingRequest,
    ) -> Result<lorelei_core::types::EmbeddingResponse, LoreleiError> {
        Err(LoreleiError::Provider("provider `p1` http 429".to_string()))
    }
}

#[tokio::test]
async fn fallback_uses_second_provider_on_retryable_error() {
    let (tenant_id, agent_id) = ids();

    let mut map = BTreeMap::new();
    map.insert(
        "p1".to_string(),
        Arc::new(RetryableFailProvider) as Arc<dyn SongProvider>,
    );
    map.insert(
        "p2".to_string(),
        Arc::new(MockSongProvider::deterministic()) as Arc<dyn SongProvider>,
    );
    let reg = ProviderRegistry::from_providers(map);

    let resp = reg
        .complete_with_fallback(
            &["p1".to_string(), "p2".to_string()],
            SongRequest {
                tenant_id,
                agent_id,
                run_id: lorelei_core::types::RunId(Uuid::from_u128(3)),
                input: "hello".to_string(),
                context: vec![],
                reasoning_summary: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(resp.output, "mock: hello");
}

#[tokio::test]
async fn streaming_chunks_can_be_collected() {
    let (tenant_id, agent_id) = ids();
    let provider = Arc::new(MockSongProvider::deterministic()) as Arc<dyn SongProvider>;
    let mut map = BTreeMap::new();
    map.insert("mock".to_string(), provider);
    let reg = ProviderRegistry::from_providers(map);

    let mut stream = reg
        .stream_with_fallback(
            &["mock".to_string()],
            SongRequest {
                tenant_id,
                agent_id,
                run_id: lorelei_core::types::RunId(Uuid::from_u128(3)),
                input: "stream me".to_string(),
                context: vec![],
                reasoning_summary: None,
            },
        )
        .await
        .unwrap();

    let mut out = String::new();
    while let Some(chunk) = stream.next().await {
        out.push_str(&chunk.delta);
        if chunk.done {
            break;
        }
    }
    assert_eq!(out, "mock: stream me");
}
