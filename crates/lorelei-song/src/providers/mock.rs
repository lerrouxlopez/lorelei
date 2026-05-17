#![forbid(unsafe_code)]

use futures::stream::{self, BoxStream};
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::SongProvider;
use lorelei_core::types::{
    EmbeddingRequest, EmbeddingResponse, ProviderCapabilities, SongChunk, SongRequest, SongResponse,
};
use std::time::Instant;
use tracing::{debug, info};

#[derive(Default)]
pub struct MockSongProvider {
    pub capabilities: ProviderCapabilities,
}

impl MockSongProvider {
    pub fn deterministic() -> Self {
        Self {
            capabilities: ProviderCapabilities {
                supports_streaming: true,
                supports_tools: true,
                supports_json_mode: true,
                supports_embeddings: true,
                context_window: Some(8_192),
                metadata: Default::default(),
            },
        }
    }

    fn deterministic_text_response(input: &str) -> String {
        if input.contains("LORELEI_MODE=planner_json") {
            // Special test hook: allow acceptance scripts to force a shell call without a real LLM.
            // Format in user prompt:
            //   LORELEI_TEST_CALL_SHELL=<tool> [pearl_id=<uuid>]
            if let Some(idx) = input.find("LORELEI_TEST_CALL_SHELL=") {
                let rest = &input[idx + "LORELEI_TEST_CALL_SHELL=".len()..];
                let tool = rest
                    .split_whitespace()
                    .next()
                    .unwrap_or("noop")
                    .trim()
                    .to_string();

                if tool == "forget_pearl" {
                    let pearl_id = rest
                        .split_whitespace()
                        .find_map(|t| t.strip_prefix("pearl_id="))
                        .unwrap_or("00000000-0000-0000-0000-000000000000");
                    return format!(
                        r#"{{"action":"call_shell","reasoning_summary":"mock tool call","tool":"forget_pearl","input":{{"pearl_id":"{}"}}}}"#,
                        pearl_id
                    );
                }

                return format!(
                    r#"{{"action":"call_shell","reasoning_summary":"mock tool call","tool":"{}","input":{{}}}}"#,
                    tool
                );
            }

            return r#"{"action":"answer","reasoning_summary":"mock plan","answer":"hello from planner"}"#.to_string();
        }
        if input.contains("LORELEI_MODE=planner_json_invalid_once") {
            // Used by tests: first call returns invalid JSON; retry prompt should not contain this marker.
            return "not-json".to_string();
        }
        if input.contains("LORELEI_MODE=planner_repair") {
            return r#"{"action":"answer","reasoning_summary":"repaired","answer":"hello from repaired planner"}"#.to_string();
        }
        if input.contains("LORELEI_MODE=answer") {
            // Deterministic “memory-aware” answer for acceptance tests:
            // if the prompt contains EchoHits, repeat the first item.
            if let Some(mem_idx) = input.find("Memory (EchoHits):") {
                let after = &input[mem_idx..];
                for line in after.lines() {
                    let l = line.trim_start();
                    if let Some(rest) = l.strip_prefix("- ") {
                        // "- <content> (<type>)"
                        let content = rest.split(" (").next().unwrap_or(rest).trim();
                        if !content.is_empty() && content != "{{ECHO_HITS}}" {
                            return format!("Using memory: {content}");
                        }
                    }
                }
            }
            return "Say hello from The Song.".to_string();
        }
        if input.contains("memory extractor") || input.contains("candidate Pearls") {
            // Default: return no memories unless tests/scripted inputs provide otherwise.
            return "[]".to_string();
        }
        format!("mock: {input}")
    }

    fn log_prompts_enabled() -> bool {
        std::env::var("LORELEI_LOG_PROMPTS")
            .ok()
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    }

    fn text_to_vector(text: &str, dims: usize) -> Vec<f32> {
        let mut vec = vec![0f32; dims.max(1)];
        for (i, b) in text.as_bytes().iter().enumerate() {
            let idx = i % vec.len();
            vec[idx] += (*b as f32) / 255.0;
        }
        let norm = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut vec {
                *v /= norm;
            }
        }
        vec
    }
}

#[async_trait::async_trait]
impl SongProvider for MockSongProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }

    async fn complete(&self, request: SongRequest) -> Result<SongResponse, LoreleiError> {
        let started = Instant::now();
        if Self::log_prompts_enabled() {
            debug!(run_id = %request.run_id.0, provider = "mock", model = "mock-chat", prompt = %request.input, "song.prompt");
        } else {
            debug!(run_id = %request.run_id.0, provider = "mock", model = "mock-chat", prompt_len = request.input.len(), "song.prompt_redacted");
        }
        let output = Self::deterministic_text_response(&request.input);
        let latency = started.elapsed();
        info!(
            run_id = %request.run_id.0,
            provider = "mock",
            model = "mock-chat",
            latency_ms = latency.as_millis() as u64,
            retry_count = 0u32,
            prompt_tokens = Option::<u32>::None,
            completion_tokens = Option::<u32>::None,
            total_tokens = Option::<u32>::None,
            "song.complete"
        );
        Ok(SongResponse {
            output,
            reasoning_summary: request.reasoning_summary,
            tool_calls: vec![],
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

        let out = Self::deterministic_text_response(&request.input);
        let chunks: Vec<SongChunk> = out
            .as_bytes()
            .chunks(8)
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
        let started = Instant::now();
        let dims = 64;
        let resp = EmbeddingResponse {
            vectors: request
                .inputs
                .iter()
                .map(|t| Self::text_to_vector(t, dims))
                .collect(),
            model: Some("mock-embed".to_string()),
        };
        let latency = started.elapsed();
        info!(
            provider = "mock",
            model = "mock-embed",
            latency_ms = latency.as_millis() as u64,
            retry_count = 0u32,
            inputs = resp.vectors.len(),
            "song.embed"
        );
        Ok(resp)
    }
}
