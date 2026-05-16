#![forbid(unsafe_code)]

use futures::stream::{self, BoxStream};
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::SongProvider;
use lorelei_core::types::{
    EmbeddingRequest, EmbeddingResponse, NormalizedToolCall, ProviderCapabilities, SongChunk,
    SongRequest, SongResponse,
};
use serde_json::json;
use uuid::Uuid;

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
        format!("mock: {input}")
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
        let output = Self::deterministic_text_response(&request.input);
        Ok(SongResponse {
            output,
            reasoning_summary: request.reasoning_summary,
            tool_calls: vec![],
        })
    }

    async fn stream(&self, request: SongRequest) -> Result<BoxStream<'static, SongChunk>, LoreleiError> {
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
        let dims = 64;
        Ok(EmbeddingResponse {
            vectors: request
                .inputs
                .iter()
                .map(|t| Self::text_to_vector(t, dims))
                .collect(),
            model: Some("mock-embed".to_string()),
        })
    }
}

