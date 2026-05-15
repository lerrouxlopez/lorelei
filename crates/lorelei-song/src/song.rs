
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use lorelei_core::{
    ApiKeySource, AnthropicConfig, Config, LoreleiError, OpenAiCompatibleConfig, ProviderCapabilities,
    ProviderConfig, SongChunk, SongProvider, SongRequest, SongResponse,
};
use reqwest::{header, StatusCode};
use serde_json::{json, Value as JsonValue};
use tokio::time::sleep;
use tracing::{debug, info, warn};
use uuid::Uuid;

#[derive(Clone)]
pub enum Provider {
    OpenAiCompatible(OpenAiCompatibleProvider),
    Anthropic(AnthropicProvider),
    Bedrock(BedrockProviderStub),
    Gemini(GeminiProviderStub),
    Mock(MockProvider),
}

pub fn build_song_provider(cfg: &Config) -> Result<Provider, LoreleiError> {
    let song = cfg.song.as_ref().ok_or_else(|| {
        LoreleiError::validation("missing [song] section in config (song.provider.name required)")
    })?;
    let name = song.provider.name.clone();
    let provider_cfg = cfg.providers.get(&name).ok_or_else(|| {
        LoreleiError::validation(format!("song.provider.name `{name}` not found in [providers]"))
    })?;

    match provider_cfg {
        ProviderConfig::OpenAiCompatible(p) => {
            Ok(Provider::OpenAiCompatible(OpenAiCompatibleProvider::new(
                name.clone(),
                p.clone(),
            )?))
        }
        ProviderConfig::Local(p) => {
            let endpoint = p.endpoint.clone().ok_or_else(|| LoreleiError::validation("local provider requires `endpoint`"))?;
            let model = p.model.clone().ok_or_else(|| LoreleiError::validation("local provider requires `model`"))?;
            Ok(Provider::OpenAiCompatible(OpenAiCompatibleProvider::new(
                name.clone(),
                OpenAiCompatibleConfig {
                    base_url: endpoint,
                    model,
                    api_key: ApiKeySource::Env { var: "LORELEI_LOCAL_API_KEY".to_string() },
                    headers: Default::default(),
                },
            )?))
        }
        ProviderConfig::Anthropic(p) => Ok(Provider::Anthropic(AnthropicProvider::new(name.clone(), p.clone())?)),
        ProviderConfig::Bedrock(p) => Ok(Provider::Bedrock(BedrockProviderStub::new(name.clone(), p.clone())?)),
        ProviderConfig::GeminiNative(p) => Ok(Provider::Gemini(GeminiProviderStub::new(name, p.clone())?)),
    }
}

#[async_trait]
impl SongProvider for Provider {
    async fn capabilities(&self) -> Result<ProviderCapabilities, LoreleiError> {
        match self {
            Provider::OpenAiCompatible(p) => p.capabilities().await,
            Provider::Anthropic(p) => p.capabilities().await,
            Provider::Bedrock(p) => p.capabilities().await,
            Provider::Gemini(p) => p.capabilities().await,
            Provider::Mock(p) => p.capabilities().await,
        }
    }

    async fn song(&self, request: SongRequest) -> Result<SongResponse, LoreleiError> {
        match self {
            Provider::OpenAiCompatible(p) => p.song(request).await,
            Provider::Anthropic(p) => p.song(request).await,
            Provider::Bedrock(p) => p.song(request).await,
            Provider::Gemini(p) => p.song(request).await,
            Provider::Mock(p) => p.song(request).await,
        }
    }

    async fn song_stream(
        &self,
        request: SongRequest,
    ) -> Result<BoxStream<'static, Result<SongChunk, LoreleiError>>, LoreleiError> {
        match self {
            Provider::OpenAiCompatible(p) => p.song_stream(request).await,
            Provider::Anthropic(p) => p.song_stream(request).await,
            Provider::Bedrock(p) => p.song_stream(request).await,
            Provider::Gemini(p) => p.song_stream(request).await,
            Provider::Mock(p) => p.song_stream(request).await,
        }
    }
}

#[derive(Clone)]
pub struct OpenAiCompatibleProvider {
    name: String,
    cfg: OpenAiCompatibleConfig,
    client: reqwest::Client,
}

impl OpenAiCompatibleProvider {
    pub fn new(name: String, cfg: OpenAiCompatibleConfig) -> Result<Self, LoreleiError> {
        cfg.validate()?;
        Ok(Self { name, cfg, client: reqwest::Client::new() })
    }

    fn base_url(&self, path: &str) -> String {
        format!("{}/{}", self.cfg.base_url.trim_end_matches('/'), path.trim_start_matches('/'))
    }

    fn auth_header(&self) -> Result<header::HeaderValue, LoreleiError> {
        let key = self.cfg.api_key.resolve()?;
        header::HeaderValue::from_str(&format!("Bearer {}", key.expose())).map_err(|e| {
            LoreleiError::SongProvider { message: format!("bad auth header: {e}") }
        })
    }

    async fn send_retry(&self, rb: impl Fn() -> reqwest::RequestBuilder) -> Result<reqwest::Response, LoreleiError> {
        let mut delay = Duration::from_millis(200);
        for attempt in 0..=5 {
            let resp = rb().send().await.map_err(|e| LoreleiError::SongProvider { message: e.to_string() })?;
            if resp.status() == StatusCode::TOO_MANY_REQUESTS || resp.status().is_server_error() {
                if attempt == 5 {
                    return Ok(resp);
                }
                sleep(delay).await;
                delay = std::cmp::min(delay * 2, Duration::from_secs(5));
                continue;
            }
            return Ok(resp);
        }
        unreachable!()
    }
}

#[async_trait]
impl SongProvider for OpenAiCompatibleProvider {
    async fn capabilities(&self) -> Result<ProviderCapabilities, LoreleiError> {
        Ok(ProviderCapabilities {
            streaming: true,
            max_context_tokens: None,
            json_mode: true,
            tools: true,
            embeddings: true,
        })
    }

    async fn song(&self, request: SongRequest) -> Result<SongResponse, LoreleiError> {
        request.validate()?;
        let start = Instant::now();
        let url = self.base_url("/chat/completions");
        let auth = self.auth_header()?;
        let body = json!({
          "model": self.cfg.model,
          "messages": [{"role":"user","content": request.prompt}],
          "stream": false
        });

        if lorelei_core::log_prompts_enabled() {
            debug!(
                provider_name = %self.name,
                model = %self.cfg.model,
                prompt = %request.prompt,
                "song.prompt"
            );
        } else {
            debug!(
                provider_name = %self.name,
                model = %self.cfg.model,
                prompt_len = request.prompt.len(),
                "song.prompt"
            );
        }

        let resp = self.send_retry(|| {
            self.client.post(url.clone()).header(header::AUTHORIZATION, auth.clone()).json(&body)
        }).await?;

        if !resp.status().is_success() {
            warn!(
                provider_name = %self.name,
                model = %self.cfg.model,
                status = %resp.status(),
                latency_ms = start.elapsed().as_millis() as u64,
                "song.http_error"
            );
            return Err(LoreleiError::SongProvider { message: format!("http {}", resp.status()) });
        }
        let v: JsonValue = resp.json().await.map_err(|e| LoreleiError::SongProvider { message: e.to_string() })?;
        let usage = v.get("usage");
        let prompt_tokens = usage.and_then(|u| u.get("prompt_tokens")).and_then(|n| n.as_u64());
        let completion_tokens = usage.and_then(|u| u.get("completion_tokens")).and_then(|n| n.as_u64());
        let total_tokens = usage.and_then(|u| u.get("total_tokens")).and_then(|n| n.as_u64());
        info!(
            provider_name = %self.name,
            model = %self.cfg.model,
            latency_ms = start.elapsed().as_millis() as u64,
            prompt_tokens = ?prompt_tokens,
            completion_tokens = ?completion_tokens,
            total_tokens = ?total_tokens,
            "song.request"
        );
        let content = v.get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        Ok(SongResponse {
            id: Uuid::new_v4(),
            created_at: chrono::Utc::now(),
            chunks: vec![SongChunk { index: 0, content, is_final: true, tool_calls: vec![] }],
            metadata: v,
        })
    }

    async fn song_stream(&self, request: SongRequest) -> Result<BoxStream<'static, Result<SongChunk, LoreleiError>>, LoreleiError> {
        // Streaming not implemented in this minimal provider; return single chunk.
        let r = self.song(request).await?;
        Ok(Box::pin(stream::iter(r.chunks.into_iter().map(Ok).collect::<Vec<_>>())))
    }
}

#[derive(Clone)]
pub struct AnthropicProvider {
    name: String,
    cfg: AnthropicConfig,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(name: String, cfg: AnthropicConfig) -> Result<Self, LoreleiError> {
        cfg.validate()?;
        Ok(Self { name, cfg, client: reqwest::Client::new() })
    }
}

#[async_trait]
impl SongProvider for AnthropicProvider {
    async fn capabilities(&self) -> Result<ProviderCapabilities, LoreleiError> {
        Ok(ProviderCapabilities {
            streaming: true,
            max_context_tokens: None,
            json_mode: false,
            tools: true,
            embeddings: false,
        })
    }

    async fn song(&self, request: SongRequest) -> Result<SongResponse, LoreleiError> {
        request.validate()?;
        let start = Instant::now();
        if lorelei_core::log_prompts_enabled() {
            debug!(
                provider_name = %self.name,
                model = %self.cfg.model,
                prompt = %request.prompt,
                "song.prompt"
            );
        } else {
            debug!(
                provider_name = %self.name,
                model = %self.cfg.model,
                prompt_len = request.prompt.len(),
                "song.prompt"
            );
        }
        let key = self.cfg.api_key.resolve()?;
        let body = json!({
          "model": self.cfg.model,
          "max_tokens": 1024,
          "messages": [{"role":"user","content": request.prompt}],
        });
        let resp = self.client.post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", key.expose())
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|e| LoreleiError::SongProvider { message: e.to_string() })?;
        if !resp.status().is_success() {
            warn!(
                provider_name = %self.name,
                model = %self.cfg.model,
                status = %resp.status(),
                latency_ms = start.elapsed().as_millis() as u64,
                "song.http_error"
            );
            return Err(LoreleiError::SongProvider { message: format!("http {}", resp.status()) });
        }
        let v: JsonValue = resp.json().await.map_err(|e| LoreleiError::SongProvider { message: e.to_string() })?;
        let usage = v.get("usage");
        let input_tokens = usage.and_then(|u| u.get("input_tokens")).and_then(|n| n.as_u64());
        let output_tokens = usage.and_then(|u| u.get("output_tokens")).and_then(|n| n.as_u64());
        info!(
            provider_name = %self.name,
            model = %self.cfg.model,
            latency_ms = start.elapsed().as_millis() as u64,
            input_tokens = ?input_tokens,
            output_tokens = ?output_tokens,
            "song.request"
        );
        let content = v.get("content")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|o| o.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        Ok(SongResponse {
            id: Uuid::new_v4(),
            created_at: chrono::Utc::now(),
            chunks: vec![SongChunk { index: 0, content, is_final: true, tool_calls: vec![] }],
            metadata: v,
        })
    }

    async fn song_stream(&self, request: SongRequest) -> Result<BoxStream<'static, Result<SongChunk, LoreleiError>>, LoreleiError> {
        let r = self.song(request).await?;
        Ok(Box::pin(stream::iter(r.chunks.into_iter().map(Ok).collect::<Vec<_>>())))
    }
}

#[derive(Clone)]
pub struct BedrockProviderStub {
    name: String,
    _cfg: lorelei_core::BedrockConfig,
}

impl BedrockProviderStub {
    pub fn new(name: String, cfg: lorelei_core::BedrockConfig) -> Result<Self, LoreleiError> {
        cfg.validate()?;
        Ok(Self { name, _cfg: cfg })
    }
}

#[async_trait]
impl SongProvider for BedrockProviderStub {
    async fn capabilities(&self) -> Result<ProviderCapabilities, LoreleiError> {
        Ok(ProviderCapabilities {
            streaming: false,
            max_context_tokens: None,
            json_mode: false,
            tools: false,
            embeddings: false,
        })
    }
    async fn song(&self, _request: SongRequest) -> Result<SongResponse, LoreleiError> {
        warn!(provider_name=%self.name, "song.stub_provider");
        Err(LoreleiError::SongProvider { message: "bedrock provider stub: TODO implement SigV4 + InvokeModel".to_string() })
    }
    async fn song_stream(&self, _request: SongRequest) -> Result<BoxStream<'static, Result<SongChunk, LoreleiError>>, LoreleiError> {
        Err(LoreleiError::SongProvider { message: "bedrock provider stub: TODO implement streaming".to_string() })
    }
}

#[derive(Clone)]
pub struct GeminiProviderStub {
    name: String,
    _cfg: lorelei_core::GeminiNativeConfig,
}

impl GeminiProviderStub {
    pub fn new(name: String, cfg: lorelei_core::GeminiNativeConfig) -> Result<Self, LoreleiError> {
        cfg.validate()?;
        Ok(Self { name, _cfg: cfg })
    }
}

#[async_trait]
impl SongProvider for GeminiProviderStub {
    async fn capabilities(&self) -> Result<ProviderCapabilities, LoreleiError> {
        Ok(ProviderCapabilities {
            streaming: false,
            max_context_tokens: None,
            json_mode: true,
            tools: true,
            embeddings: false,
        })
    }
    async fn song(&self, _request: SongRequest) -> Result<SongResponse, LoreleiError> {
        warn!(provider_name=%self.name, "song.stub_provider");
        Err(LoreleiError::SongProvider { message: "gemini native provider stub: TODO implement auth + generateContent".to_string() })
    }
    async fn song_stream(&self, _request: SongRequest) -> Result<BoxStream<'static, Result<SongChunk, LoreleiError>>, LoreleiError> {
        Err(LoreleiError::SongProvider { message: "gemini native provider stub: TODO implement streaming".to_string() })
    }
}

#[derive(Clone, Default)]
pub struct MockProvider {
    pub response: String,
}

#[async_trait]
impl SongProvider for MockProvider {
    async fn capabilities(&self) -> Result<ProviderCapabilities, LoreleiError> {
        Ok(ProviderCapabilities {
            streaming: true,
            max_context_tokens: Some(8192),
            json_mode: true,
            tools: true,
            embeddings: true,
        })
    }
    async fn song(&self, _request: SongRequest) -> Result<SongResponse, LoreleiError> {
        Ok(SongResponse {
            id: Uuid::new_v4(),
            created_at: chrono::Utc::now(),
            chunks: vec![SongChunk { index: 0, content: self.response.clone(), is_final: true, tool_calls: vec![] }],
            metadata: json!({"mock": true}),
        })
    }
    async fn song_stream(&self, _request: SongRequest) -> Result<BoxStream<'static, Result<SongChunk, LoreleiError>>, LoreleiError> {
        Ok(Box::pin(stream::iter(vec![Ok(SongChunk { index: 0, content: self.response.clone(), is_final: true, tool_calls: vec![] })])))
    }
}
