#![forbid(unsafe_code)]

use futures::stream::{self, BoxStream};
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::SongProvider;
use lorelei_core::types::{
    EmbeddingRequest, EmbeddingResponse, NormalizedToolCall, ProviderCapabilities, SongChunk,
    SongRequest, SongResponse,
};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{debug, info};

pub struct OpenAiCompatibleProvider {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub chat_model: String,
    pub embedding_model: Option<String>,
    pub capabilities: ProviderCapabilities,
    client: reqwest::Client,
}

impl OpenAiCompatibleProvider {
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        chat_model: impl Into<String>,
        embedding_model: Option<String>,
        capabilities: ProviderCapabilities,
    ) -> Result<Self, LoreleiError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| LoreleiError::Internal(format!("http client init failed: {e}")))?;
        Ok(Self {
            name: name.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            chat_model: chat_model.into(),
            embedding_model,
            capabilities,
            client,
        })
    }

    fn log_prompts_enabled() -> bool {
        std::env::var("LORELEI_LOG_PROMPTS")
            .ok()
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    }

    fn endpoint(&self, subpath: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        let subpath = subpath.trim_start_matches('/');
        if base.ends_with("/v1") {
            format!("{base}/{subpath}")
        } else {
            format!("{base}/v1/{subpath}")
        }
    }

    async fn request_with_retry<TReq: Serialize + ?Sized, TResp: for<'de> Deserialize<'de>>(
        &self,
        url: String,
        body: &TReq,
    ) -> Result<(TResp, u32, Duration), LoreleiError> {
        let started = Instant::now();
        let mut attempt = 0u32;
        let mut delay_ms = 250u64;
        loop {
            attempt += 1;
            let res = self
                .client
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(body)
                .send()
                .await;

            match res {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let parsed = resp.json::<TResp>().await.map_err(|e| {
                            LoreleiError::Provider(format!("invalid response: {e}"))
                        })?;
                        let retries = attempt.saturating_sub(1);
                        return Ok((parsed, retries, started.elapsed()));
                    }
                    if is_retryable_status(resp.status()) && attempt < 5 {
                        sleep(Duration::from_millis(delay_ms)).await;
                        delay_ms = (delay_ms * 2).min(4000);
                        continue;
                    }

                    let status = resp.status();
                    let msg = format!("provider `{}` http {}", self.name, status.as_u16());
                    return Err(LoreleiError::Provider(msg));
                }
                Err(e) => {
                    // Transport errors can be transient.
                    if attempt < 5 {
                        sleep(Duration::from_millis(delay_ms)).await;
                        delay_ms = (delay_ms * 2).min(4000);
                        continue;
                    }
                    return Err(LoreleiError::Provider(format!(
                        "provider `{}` request failed: {}",
                        self.name, e
                    )));
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl SongProvider for OpenAiCompatibleProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }

    async fn complete(&self, request: SongRequest) -> Result<SongResponse, LoreleiError> {
        let url = self.endpoint("chat/completions");

        if Self::log_prompts_enabled() {
            debug!(
                run_id = %request.run_id.0,
                provider = %self.name,
                model = %self.chat_model,
                prompt = %request.input,
                "song.prompt"
            );
        } else {
            debug!(
                run_id = %request.run_id.0,
                provider = %self.name,
                model = %self.chat_model,
                prompt_len = request.input.len(),
                "song.prompt_redacted"
            );
        }

        let body = ChatCompletionsRequest {
            model: self.chat_model.clone(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: request.input.clone(),
            }],
            stream: false,
            tools: None,
            tool_choice: None,
            response_format: None,
        };

        let (resp, retries, latency): (ChatCompletionsResponse, u32, Duration) =
            self.request_with_retry(url, &body).await?;
        let choice = resp
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| LoreleiError::Provider("empty response".to_string()))?;
        let message = choice.message;
        let tool_calls = normalize_tool_calls(message.tool_calls.unwrap_or_default());
        info!(
            run_id = %request.run_id.0,
            provider = %self.name,
            model = %self.chat_model,
            latency_ms = latency.as_millis() as u64,
            retry_count = retries,
            prompt_tokens = resp.usage.as_ref().map(|u| u.prompt_tokens),
            completion_tokens = resp.usage.as_ref().map(|u| u.completion_tokens),
            total_tokens = resp.usage.as_ref().map(|u| u.total_tokens),
            "song.complete"
        );
        Ok(SongResponse {
            output: message.content.unwrap_or_default(),
            reasoning_summary: None,
            tool_calls,
        })
    }

    async fn stream(
        &self,
        request: SongRequest,
    ) -> Result<BoxStream<'static, SongChunk>, LoreleiError> {
        if !self.capabilities.supports_streaming {
            return Err(LoreleiError::Unsupported(
                "provider does not support streaming".to_string(),
            ));
        }

        // Minimal streaming: fall back to non-streaming and chunk the result.
        // (Proper SSE streaming can be added later.)
        let full = self.complete(request).await?;
        let chunks: Vec<SongChunk> = full
            .output
            .as_bytes()
            .chunks(16)
            .map(|c| SongChunk {
                delta: String::from_utf8_lossy(c).to_string(),
                done: false,
            })
            .collect();
        let mut all = chunks;
        all.push(SongChunk {
            delta: String::new(),
            done: true,
        });
        Ok(Box::pin(stream::iter(all)))
    }

    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse, LoreleiError> {
        if !self.capabilities.supports_embeddings {
            return Err(LoreleiError::Unsupported(
                "provider does not support embeddings".to_string(),
            ));
        }
        let model = self
            .embedding_model
            .clone()
            .ok_or_else(|| LoreleiError::Validation {
                field: "providers.<name>.embedding_model",
                message: "embedding_model is required for embeddings".to_string(),
            })?;

        let url = self.endpoint("embeddings");
        let body = EmbeddingsRequest {
            model,
            input: request.inputs,
        };
        let (resp, retries, latency): (EmbeddingsResponse, u32, Duration) =
            self.request_with_retry(url, &body).await?;
        info!(
            provider = %self.name,
            model = %resp.model,
            latency_ms = latency.as_millis() as u64,
            retry_count = retries,
            inputs = resp.data.len(),
            "song.embed"
        );
        Ok(EmbeddingResponse {
            vectors: resp.data.into_iter().map(|d| d.embedding).collect(),
            model: Some(resp.model),
        })
    }
}

fn is_retryable_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::REQUEST_TIMEOUT
            | StatusCode::TOO_MANY_REQUESTS
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    ) || status.is_server_error()
}

#[derive(Debug, Serialize)]
struct ChatCompletionsRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<Value>,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionsResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
    #[serde(default)]
    total_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessageOut,
}

#[derive(Debug, Deserialize)]
struct ChatMessageOut {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCall {
    #[serde(default)]
    id: Option<String>,
    function: OpenAiToolFunction,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolFunction {
    name: String,
    arguments: String,
}

fn normalize_tool_calls(tool_calls: Vec<OpenAiToolCall>) -> Vec<NormalizedToolCall> {
    tool_calls
        .into_iter()
        .map(|tc| {
            let call_id = tc.id.unwrap_or_else(|| "call".to_string());
            let args: Value = serde_json::from_str(&tc.function.arguments).unwrap_or(Value::Null);
            NormalizedToolCall {
                call_id,
                name: tc.function.name,
                arguments: args,
            }
        })
        .collect()
}

#[derive(Debug, Serialize)]
struct EmbeddingsRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingsResponse {
    model: String,
    data: Vec<EmbeddingDatum>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingDatum {
    embedding: Vec<f32>,
}
