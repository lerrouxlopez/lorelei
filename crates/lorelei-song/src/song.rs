use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::{self, BoxStream, StreamExt};
use futures::TryStreamExt;
use lorelei_core::{
    config::AnthropicConfig, ApiKeySource, Config, LoreleiError, OpenAiCompatibleConfig,
    ProviderCapabilities, ProviderConfig, SongChunk, SongProvider, SongRequest, SongResponse,
    ToolCall,
};
use reqwest::{header, StatusCode};
use serde_json::{json, Value as JsonValue};
use thiserror::Error;
use tokio::time::sleep;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum LoreleiSongError {
    #[error("config error: {0}")]
    Config(String),
    #[error("provider error: {0}")]
    Provider(String),
}

impl From<LoreleiSongError> for LoreleiError {
    fn from(value: LoreleiSongError) -> Self {
        LoreleiError::SongProvider {
            message: value.to_string(),
        }
    }
}

#[derive(Clone)]
pub enum Provider {
    OpenAiCompatible(OpenAiCompatibleProvider),
    Anthropic(AnthropicProvider),
    Bedrock(BedrockProviderStub),
    Gemini(GeminiProviderStub),
}

#[async_trait]
impl SongProvider for Provider {
    async fn capabilities(&self) -> Result<ProviderCapabilities, LoreleiError> {
        match self {
            Provider::OpenAiCompatible(p) => p.capabilities().await,
            Provider::Anthropic(p) => p.capabilities().await,
            Provider::Bedrock(p) => p.capabilities().await,
            Provider::Gemini(p) => p.capabilities().await,
        }
    }

    async fn song(&self, request: SongRequest) -> Result<SongResponse, LoreleiError> {
        match self {
            Provider::OpenAiCompatible(p) => p.song(request).await,
            Provider::Anthropic(p) => p.song(request).await,
            Provider::Bedrock(p) => p.song(request).await,
            Provider::Gemini(p) => p.song(request).await,
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
        }
    }
}

pub fn build_song_provider(cfg: &Config) -> Result<Provider, LoreleiError> {
    let song = cfg.song.as_ref().ok_or_else(|| {
        LoreleiError::validation("missing [song] section in config (song.provider.name required)")
    })?;
    let provider_name = &song.provider.name;
    let provider_cfg = cfg.providers.get(provider_name).ok_or_else(|| {
        LoreleiError::validation(format!(
            "song.provider.name `{provider_name}` not found in [providers]"
        ))
    })?;

    match provider_cfg {
        ProviderConfig::OpenAiCompatible(p) => Ok(Provider::OpenAiCompatible(
            OpenAiCompatibleProvider::new(p.clone())?,
        )),
        ProviderConfig::Anthropic(p) => Ok(Provider::Anthropic(AnthropicProvider::new(p.clone())?)),
        ProviderConfig::Bedrock(p) => Ok(Provider::Bedrock(BedrockProviderStub::new(p.clone())?)),
        ProviderConfig::GeminiNative(p) => Ok(Provider::Gemini(GeminiProviderStub::new(p.clone())?)),
        ProviderConfig::Local(p) => {
            // Local is treated as an OpenAI-compatible endpoint when endpoint is provided.
            let endpoint = p.endpoint.clone().ok_or_else(|| {
                LoreleiError::validation("local provider requires `endpoint`")
            })?;
            let model = p.model.clone().ok_or_else(|| LoreleiError::validation("local provider requires `model`"))?;
            let openai = OpenAiCompatibleConfig {
                base_url: endpoint,
                model,
                api_key: ApiKeySource::Env {
                    var: "LORELEI_LOCAL_API_KEY".to_string(),
                },
                headers: Default::default(),
            };
            Ok(Provider::OpenAiCompatible(OpenAiCompatibleProvider::new(openai)?))
        }
    }
}

#[derive(Clone)]
pub struct OpenAiCompatibleProvider {
    cfg: OpenAiCompatibleConfig,
    client: reqwest::Client,
}

impl OpenAiCompatibleProvider {
    pub fn new(cfg: OpenAiCompatibleConfig) -> Result<Self, LoreleiError> {
        cfg.validate()?;
        let client = reqwest::Client::builder()
            .user_agent("lorelei/0.1")
            .build()
            .map_err(|e| LoreleiError::SongProvider {
                message: format!("reqwest client init failed: {e}"),
            })?;
        Ok(Self { cfg, client })
    }

    fn auth_header(&self) -> Result<header::HeaderValue, LoreleiError> {
        let key = self.cfg.api_key.resolve()?;
        let v = format!("Bearer {}", key.expose());
        header::HeaderValue::from_str(&v).map_err(|e| LoreleiError::SongProvider {
            message: format!("invalid auth header: {e}"),
        })
    }

    fn base_url(&self, path: &str) -> String {
        format!("{}/{}", self.cfg.base_url.trim_end_matches('/'), path.trim_start_matches('/'))
    }

    async fn request_with_retry(
        &self,
        req: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, LoreleiError> {
        let mut attempt = 0u32;
        let mut delay = Duration::from_millis(200);
        loop {
            let resp = req()
                .send()
                .await
                .map_err(|e| LoreleiError::SongProvider {
                    message: format!("request failed: {e}"),
                })?;

            if resp.status() == StatusCode::TOO_MANY_REQUESTS
                || resp.status().is_server_error()
                || resp.status() == StatusCode::REQUEST_TIMEOUT
            {
                if attempt >= 5 {
                    return Err(LoreleiError::SongProvider {
                        message: format!("request failed after retries: HTTP {}", resp.status()),
                    });
                }
                attempt += 1;
                sleep(delay).await;
                delay = std::cmp::min(delay * 2, Duration::from_secs(5));
                continue;
            }

            return Ok(resp);
        }
    }
}

#[async_trait]
impl SongProvider for OpenAiCompatibleProvider {
    async fn capabilities(&self) -> Result<ProviderCapabilities, LoreleiError> {
        // Conservative defaults; can be overridden via config later.
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
        let url = self.base_url("/chat/completions");
        let auth = self.auth_header()?;

        let body = openai_chat_body(&self.cfg.model, &request, false)?;
        let resp = self
            .request_with_retry(|| {
                let mut b = self.client.post(url.clone()).header(header::AUTHORIZATION, auth.clone());
                for (k, v) in &self.cfg.headers {
                    b = b.header(k, v);
                }
                b.json(&body)
            })
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(LoreleiError::SongProvider {
                message: normalize_http_error(status, &text),
            });
        }

        let v: JsonValue = resp.json().await.map_err(|e| LoreleiError::SongProvider {
            message: format!("invalid JSON: {e}"),
        })?;

        let (content, tool_calls) = openai_extract_message(&v)?;
        let chunk = SongChunk {
            index: 0,
            content,
            is_final: true,
            tool_calls,
        };

        Ok(SongResponse {
            id: Uuid::new_v4(),
            created_at: chrono::Utc::now(),
            chunks: vec![chunk],
            metadata: v,
        })
    }

    async fn song_stream(
        &self,
        request: SongRequest,
    ) -> Result<BoxStream<'static, Result<SongChunk, LoreleiError>>, LoreleiError> {
        request.validate()?;
        let url = self.base_url("/chat/completions");
        let auth = self.auth_header()?;
        let body = openai_chat_body(&self.cfg.model, &request, true)?;

        let resp = self
            .request_with_retry(|| {
                let mut b = self
                    .client
                    .post(url.clone())
                    .header(header::AUTHORIZATION, auth.clone())
                    .header(header::ACCEPT, "text/event-stream");
                for (k, v) in &self.cfg.headers {
                    b = b.header(k, v);
                }
                b.json(&body)
            })
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(LoreleiError::SongProvider {
                message: normalize_http_error(status, &text),
            });
        }

        let stream: BoxStream<'static, Result<String, LoreleiError>> = Box::pin(
            resp.bytes_stream()
                .map_err(|e| LoreleiError::SongProvider {
                    message: format!("stream read error: {e}"),
                })
                .flat_map(sse_lines_from_bytes),
        );

        let mapped = stream::unfold(
            (stream, 0u32, String::new(), Vec::<ToolCall>::new()),
            |(mut s, idx, mut acc, mut tools): (
                BoxStream<'static, Result<String, LoreleiError>>,
                u32,
                String,
                Vec<ToolCall>,
            )| async move {
                while let Some(line) = s.next().await {
                    let line = match line {
                        Ok(l) => l,
                        Err(e) => return Some((Err(e), (s, idx, acc, tools))),
                    };
                    if !line.starts_with("data:") {
                        continue;
                    }
                    let data = line.trim_start_matches("data:").trim();
                    if data == "[DONE]" {
                        let final_chunk = SongChunk {
                            index: idx,
                            content: acc,
                            is_final: true,
                            tool_calls: tools,
                        };
                        return Some((Ok(final_chunk), (s, idx + 1, String::new(), Vec::new())));
                    }

                    let v: JsonValue = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    if let Some(delta) = v
                        .get("choices")
                        .and_then(|c| c.get(0))
                        .and_then(|c| c.get("delta"))
                    {
                        if let Some(part) = delta.get("content").and_then(|c| c.as_str()) {
                            acc.push_str(part);
                        }
                        if let Some(tc) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                            tools.extend(openai_delta_tool_calls(tc));
                        }
                    }

                    if !acc.is_empty() || !tools.is_empty() {
                        let out = SongChunk {
                            index: idx,
                            content: std::mem::take(&mut acc),
                            is_final: false,
                            tool_calls: std::mem::take(&mut tools),
                        };
                        return Some((Ok(out), (s, idx + 1, String::new(), Vec::new())));
                    }
                }
                None
            },
        );

        Ok(Box::pin(mapped))
    }
}

fn openai_chat_body(model: &str, req: &SongRequest, stream: bool) -> Result<JsonValue, LoreleiError> {
    let mut messages = Vec::new();
    for p in &req.context {
        messages.push(json!({"role":"system","content": p.content }));
    }
    messages.push(json!({"role":"user","content": req.prompt }));

    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": stream,
    });
    if let Some(max) = req.max_chunks {
        body["max_tokens"] = json!(max);
    }
    if !req.parameters.is_empty() {
        body["extra"] = json!(req.parameters);
    }
    Ok(body)
}

fn openai_extract_message(v: &JsonValue) -> Result<(String, Vec<ToolCall>), LoreleiError> {
    let msg = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .ok_or_else(|| LoreleiError::SongProvider {
            message: "missing choices[0].message".to_string(),
        })?;

    let content = msg
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .to_string();

        let tool_calls = msg
        .get("tool_calls")
        .and_then(|t| t.as_array())
        .map(|a| openai_message_tool_calls(a))
        .unwrap_or_default();

    Ok((content, tool_calls))
}

fn openai_message_tool_calls(arr: &[JsonValue]) -> Vec<ToolCall> {
    arr.iter()
        .filter_map(|tc| {
            let id = tc.get("id")?.as_str()?.to_string();
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let args_str = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str())
                .unwrap_or("{}");
            let args = serde_json::from_str(args_str).unwrap_or(JsonValue::String(args_str.to_string()));
            Some(ToolCall {
                id,
                name,
                arguments: args,
            })
        })
        .collect()
}

fn openai_delta_tool_calls(arr: &[JsonValue]) -> Vec<ToolCall> {
    arr.iter()
        .filter_map(|tc| {
            let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("delta").to_string();
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let args_str = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str())
                .unwrap_or("{}");
            let args = serde_json::from_str(args_str).unwrap_or(JsonValue::String(args_str.to_string()));
            if name.is_empty() && args == JsonValue::String("{}".to_string()) {
                None
            } else {
                Some(ToolCall { id, name, arguments: args })
            }
        })
        .collect()
}

fn sse_lines_from_bytes(chunk: Result<Bytes, LoreleiError>) -> stream::Iter<std::vec::IntoIter<Result<String, LoreleiError>>> {
    match chunk {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let lines = text
                .split('\n')
                .map(|l| Ok(l.trim_end_matches('\r').to_string()))
                .collect::<Vec<_>>();
            stream::iter(lines)
        }
        Err(e) => stream::iter(vec![Err(e)]),
    }
}

fn normalize_http_error(status: StatusCode, body: &str) -> String {
    let b = body.trim();
    if b.is_empty() {
        return format!("HTTP {status}");
    }
    // Avoid printing secrets; body could include them, but providers should not echo keys.
    format!("HTTP {status}: {}", truncate(b, 4000))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut out = s[..max].to_string();
    out.push_str("…");
    out
}

#[derive(Clone)]
pub struct AnthropicProvider {
    model: String,
    api_key: ApiKeySource,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(cfg: AnthropicConfig) -> Result<Self, LoreleiError> {
        cfg.validate()?;
        let client = reqwest::Client::builder()
            .user_agent("lorelei/0.1")
            .build()
            .map_err(|e| LoreleiError::SongProvider {
                message: format!("reqwest client init failed: {e}"),
            })?;
        Ok(Self {
            model: cfg.model,
            api_key: cfg.api_key,
            client,
        })
    }

    fn key_header(&self) -> Result<header::HeaderValue, LoreleiError> {
        let key = self.api_key.resolve()?;
        header::HeaderValue::from_str(key.expose()).map_err(|e| LoreleiError::SongProvider {
            message: format!("invalid api key header: {e}"),
        })
    }

    async fn request_with_retry(
        &self,
        req: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, LoreleiError> {
        let mut attempt = 0u32;
        let mut delay = Duration::from_millis(200);
        loop {
            let resp = req()
                .send()
                .await
                .map_err(|e| LoreleiError::SongProvider {
                    message: format!("request failed: {e}"),
                })?;
            if resp.status() == StatusCode::TOO_MANY_REQUESTS
                || resp.status().is_server_error()
                || resp.status() == StatusCode::REQUEST_TIMEOUT
            {
                if attempt >= 5 {
                    return Err(LoreleiError::SongProvider {
                        message: format!("request failed after retries: HTTP {}", resp.status()),
                    });
                }
                attempt += 1;
                sleep(delay).await;
                delay = std::cmp::min(delay * 2, Duration::from_secs(5));
                continue;
            }
            return Ok(resp);
        }
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
        let key = self.key_header()?;
        let url = "https://api.anthropic.com/v1/messages";

        let body = anthropic_body(&self.model, &request, false);
        let resp = self
            .request_with_retry(|| {
                self.client
                    .post(url)
                    .header("x-api-key", key.clone())
                    .header("anthropic-version", "2023-06-01")
                    .json(&body)
            })
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(LoreleiError::SongProvider {
                message: normalize_http_error(status, &text),
            });
        }

        let v: JsonValue = resp.json().await.map_err(|e| LoreleiError::SongProvider {
            message: format!("invalid JSON: {e}"),
        })?;
        let content = anthropic_extract_text(&v);
        Ok(SongResponse {
            id: Uuid::new_v4(),
            created_at: chrono::Utc::now(),
            chunks: vec![SongChunk {
                index: 0,
                content,
                is_final: true,
                tool_calls: Vec::new(),
            }],
            metadata: v,
        })
    }

    async fn song_stream(
        &self,
        request: SongRequest,
    ) -> Result<BoxStream<'static, Result<SongChunk, LoreleiError>>, LoreleiError> {
        request.validate()?;
        let key = self.key_header()?;
        let url = "https://api.anthropic.com/v1/messages";
        let body = anthropic_body(&self.model, &request, true);

        let resp = self
            .request_with_retry(|| {
                self.client
                    .post(url)
                    .header("x-api-key", key.clone())
                    .header("anthropic-version", "2023-06-01")
                    .header(header::ACCEPT, "text/event-stream")
                    .json(&body)
            })
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(LoreleiError::SongProvider {
                message: normalize_http_error(status, &text),
            });
        }

        let stream: BoxStream<'static, Result<String, LoreleiError>> = Box::pin(
            resp.bytes_stream()
                .map_err(|e| LoreleiError::SongProvider {
                    message: format!("stream read error: {e}"),
                })
                .flat_map(sse_lines_from_bytes),
        );

        let mapped = stream::unfold(
            (stream, 0u32, String::new()),
            |(mut s, idx, mut acc): (BoxStream<'static, Result<String, LoreleiError>>, u32, String)| async move {
            while let Some(line) = s.next().await {
                let line = match line {
                    Ok(l) => l,
                    Err(e) => return Some((Err(e), (s, idx, acc))),
                };
                if !line.starts_with("data:") {
                    continue;
                }
                let data = line.trim_start_matches("data:").trim();
                let v: JsonValue = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(t) = v.get("type").and_then(|t| t.as_str()) {
                    if t == "message_stop" {
                        let out = SongChunk {
                            index: idx,
                            content: acc,
                            is_final: true,
                            tool_calls: Vec::new(),
                        };
                        return Some((Ok(out), (s, idx + 1, String::new())));
                    }
                    if t == "content_block_delta" {
                        if let Some(text) = v
                            .get("delta")
                            .and_then(|d| d.get("text"))
                            .and_then(|t| t.as_str())
                        {
                            acc.push_str(text);
                            let out = SongChunk {
                                index: idx,
                                content: std::mem::take(&mut acc),
                                is_final: false,
                                tool_calls: Vec::new(),
                            };
                            return Some((Ok(out), (s, idx + 1, String::new())));
                        }
                    }
                }
            }
            None
        });

        Ok(Box::pin(mapped))
    }
}

fn anthropic_body(model: &str, req: &SongRequest, stream: bool) -> JsonValue {
    let mut messages = Vec::new();
    messages.push(json!({"role":"user","content": req.prompt}));
    json!({
        "model": model,
        "max_tokens": 1024,
        "messages": messages,
        "stream": stream,
    })
}

fn anthropic_extract_text(v: &JsonValue) -> String {
    v.get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.iter().find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text")))
        .and_then(|b| b.get("text").and_then(|t| t.as_str()))
        .unwrap_or_default()
        .to_string()
}

#[derive(Clone)]
pub struct BedrockProviderStub {
    _cfg: lorelei_core::BedrockConfig,
}

impl BedrockProviderStub {
    pub fn new(cfg: lorelei_core::BedrockConfig) -> Result<Self, LoreleiError> {
        cfg.validate()?;
        Ok(Self { _cfg: cfg })
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
        Err(LoreleiError::SongProvider {
            message: "Bedrock provider stub: TODO implement AWS SigV4 signing + InvokeModel".to_string(),
        })
    }

    async fn song_stream(
        &self,
        _request: SongRequest,
    ) -> Result<BoxStream<'static, Result<SongChunk, LoreleiError>>, LoreleiError> {
        Err(LoreleiError::SongProvider {
            message: "Bedrock provider stub: TODO implement streaming (InvokeModelWithResponseStream)".to_string(),
        })
    }
}

#[derive(Clone)]
pub struct GeminiProviderStub {
    _cfg: lorelei_core::GeminiNativeConfig,
}

impl GeminiProviderStub {
    pub fn new(cfg: lorelei_core::GeminiNativeConfig) -> Result<Self, LoreleiError> {
        cfg.validate()?;
        Ok(Self { _cfg: cfg })
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
        Err(LoreleiError::SongProvider {
            message: "Gemini native provider stub: TODO implement Google auth + generateContent".to_string(),
        })
    }

    async fn song_stream(
        &self,
        _request: SongRequest,
    ) -> Result<BoxStream<'static, Result<SongChunk, LoreleiError>>, LoreleiError> {
        Err(LoreleiError::SongProvider {
            message: "Gemini native provider stub: TODO implement streaming generateContent".to_string(),
        })
    }
}

#[derive(Clone, Default)]
pub struct MockProvider {
    pub response: Arc<String>,
}

#[async_trait]
impl SongProvider for MockProvider {
    async fn capabilities(&self) -> Result<ProviderCapabilities, LoreleiError> {
        Ok(ProviderCapabilities {
            streaming: true,
            max_context_tokens: Some(8_192),
            json_mode: true,
            tools: true,
            embeddings: true,
        })
    }

    async fn song(&self, _request: SongRequest) -> Result<SongResponse, LoreleiError> {
        Ok(SongResponse {
            id: Uuid::new_v4(),
            created_at: chrono::Utc::now(),
            chunks: vec![SongChunk {
                index: 0,
                content: self.response.as_str().to_string(),
                is_final: true,
                tool_calls: Vec::new(),
            }],
            metadata: json!({"mock": true}),
        })
    }

    async fn song_stream(
        &self,
        _request: SongRequest,
    ) -> Result<BoxStream<'static, Result<SongChunk, LoreleiError>>, LoreleiError> {
        let chunks = vec![
            Ok(SongChunk {
                index: 0,
                content: self.response.as_str().to_string(),
                is_final: true,
                tool_calls: Vec::new(),
            }),
        ];
        Ok(Box::pin(stream::iter(chunks)))
    }
}
