#![forbid(unsafe_code)]

use crate::providers::mock::MockSongProvider;
use crate::providers::openai_compatible::OpenAiCompatibleProvider;
use crate::providers::stubs::UnsupportedProvider;
use lorelei_core::config::{LoreleiConfig, ProviderKind};
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::SongProvider;
use lorelei_core::types::{EmbeddingRequest, EmbeddingResponse, ProviderCapabilities, SongChunk, SongRequest, SongResponse};
use futures::stream::BoxStream;
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct ProviderRegistry {
    providers: BTreeMap<String, Arc<dyn SongProvider>>,
}

impl ProviderRegistry {
    pub fn from_config(cfg: &LoreleiConfig) -> Result<Self, LoreleiError> {
        let mut providers: BTreeMap<String, Arc<dyn SongProvider>> = BTreeMap::new();

        for (name, p) in &cfg.providers {
            let provider: Arc<dyn SongProvider> = match p.kind {
                ProviderKind::Mock => Arc::new(MockSongProvider::deterministic()),
                ProviderKind::OpenaiCompatible | ProviderKind::Local => {
                    let base_url = p
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "https://api.openai.com".to_string());
                    let api_key = std::env::var(&p.api_key_env).map_err(|_| {
                        LoreleiError::validation(
                            "providers.<name>.api_key_env",
                            format!("missing required env var (value not shown): {}", p.api_key_env),
                        )
                    })?;

                    let caps = ProviderCapabilities {
                        supports_streaming: true,
                        supports_tools: true,
                        supports_json_mode: true,
                        supports_embeddings: p.embedding_model.is_some(),
                        context_window: None,
                        metadata: Default::default(),
                    };

                    Arc::new(OpenAiCompatibleProvider::new(
                        name.clone(),
                        base_url,
                        api_key,
                        p.chat_model.clone(),
                        p.embedding_model.clone(),
                        caps,
                    )?)
                }
                ProviderKind::Anthropic => Arc::new(UnsupportedProvider {
                    name: name.clone(),
                    kind: "anthropic".to_string(),
                    capabilities: ProviderCapabilities {
                        supports_streaming: true,
                        supports_tools: true,
                        supports_json_mode: true,
                        supports_embeddings: p.embedding_model.is_some(),
                        context_window: None,
                        metadata: Default::default(),
                    },
                }),
                ProviderKind::GeminiNative => Arc::new(UnsupportedProvider {
                    name: name.clone(),
                    kind: "gemini-native".to_string(),
                    capabilities: ProviderCapabilities {
                        supports_streaming: true,
                        supports_tools: true,
                        supports_json_mode: true,
                        supports_embeddings: p.embedding_model.is_some(),
                        context_window: None,
                        metadata: Default::default(),
                    },
                }),
                ProviderKind::Bedrock => Arc::new(UnsupportedProvider {
                    name: name.clone(),
                    kind: "bedrock".to_string(),
                    capabilities: ProviderCapabilities {
                        supports_streaming: false,
                        supports_tools: false,
                        supports_json_mode: false,
                        supports_embeddings: false,
                        context_window: None,
                        metadata: Default::default(),
                    },
                }),
            };

            providers.insert(name.clone(), provider);
        }

        Ok(Self { providers })
    }

    pub fn get(&self, name: &str) -> Result<Arc<dyn SongProvider>, LoreleiError> {
        self.providers
            .get(name)
            .cloned()
            .ok_or_else(|| LoreleiError::NotFound(format!("provider `{name}` not found")))
    }

    pub fn list_names(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    pub async fn complete_with_fallback(
        &self,
        provider_names: &[String],
        request: SongRequest,
    ) -> Result<SongResponse, LoreleiError> {
        let mut last_err: Option<LoreleiError> = None;
        for name in provider_names {
            let p = self.get(name)?;
            match p.complete(request.clone()).await {
                Ok(r) => return Ok(r),
                Err(e) => {
                    if is_retryable_error(&e) {
                        last_err = Some(e);
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            LoreleiError::validation("agent.default_provider", "no providers configured")
        }))
    }

    pub async fn embed_with_fallback(
        &self,
        provider_names: &[String],
        request: EmbeddingRequest,
    ) -> Result<EmbeddingResponse, LoreleiError> {
        let mut last_err: Option<LoreleiError> = None;
        for name in provider_names {
            let p = self.get(name)?;
            if !p.capabilities().supports_embeddings {
                last_err = Some(LoreleiError::Unsupported(format!(
                    "provider `{name}` does not support embeddings"
                )));
                continue;
            }
            match p.embed(request.clone()).await {
                Ok(r) => return Ok(r),
                Err(e) => {
                    if is_retryable_error(&e) {
                        last_err = Some(e);
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| LoreleiError::Unsupported("no embedding providers available".to_string())))
    }

    pub async fn stream_with_fallback(
        &self,
        provider_names: &[String],
        request: SongRequest,
    ) -> Result<BoxStream<'static, SongChunk>, LoreleiError> {
        let mut last_err: Option<LoreleiError> = None;
        for name in provider_names {
            let p = self.get(name)?;
            if !p.capabilities().supports_streaming {
                last_err = Some(LoreleiError::Unsupported(format!(
                    "provider `{name}` does not support streaming"
                )));
                continue;
            }
            match p.stream(request.clone()).await {
                Ok(s) => return Ok(s),
                Err(e) => {
                    if is_retryable_error(&e) {
                        last_err = Some(e);
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| LoreleiError::Unsupported("no streaming providers available".to_string())))
    }
}

fn is_retryable_error(err: &LoreleiError) -> bool {
    match err {
        LoreleiError::Provider(msg) => msg.contains("http 429") || msg.contains("http 5") || msg.contains("request failed"),
        LoreleiError::Internal(msg) => msg.contains("timeout"),
        _ => false,
    }
}

