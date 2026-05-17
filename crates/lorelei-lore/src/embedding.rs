#![forbid(unsafe_code)]

use async_trait::async_trait;
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::SongProvider;
use lorelei_core::types::{EmbeddingRequest, EmbeddingResponse, TenantId};

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(
        &self,
        tenant_id: TenantId,
        provider: &str,
        inputs: Vec<String>,
    ) -> Result<EmbeddingResponse, LoreleiError>;
}

pub struct SongProviderEmbeddingAdapter<P: SongProvider> {
    provider: P,
}

impl<P: SongProvider> SongProviderEmbeddingAdapter<P> {
    pub fn new(provider: P) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl<P: SongProvider> EmbeddingProvider for SongProviderEmbeddingAdapter<P> {
    async fn embed(
        &self,
        tenant_id: TenantId,
        provider: &str,
        inputs: Vec<String>,
    ) -> Result<EmbeddingResponse, LoreleiError> {
        if !self.provider.capabilities().supports_embeddings {
            return Err(LoreleiError::Unsupported(
                "provider does not support embeddings".to_string(),
            ));
        }
        self.provider
            .embed(EmbeddingRequest {
                tenant_id,
                provider: provider.to_string(),
                inputs,
            })
            .await
    }
}

pub struct DynSongProviderEmbeddingAdapter {
    provider: std::sync::Arc<dyn SongProvider>,
}

impl DynSongProviderEmbeddingAdapter {
    pub fn new(provider: std::sync::Arc<dyn SongProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl EmbeddingProvider for DynSongProviderEmbeddingAdapter {
    async fn embed(
        &self,
        tenant_id: TenantId,
        provider: &str,
        inputs: Vec<String>,
    ) -> Result<EmbeddingResponse, LoreleiError> {
        if !self.provider.capabilities().supports_embeddings {
            return Err(LoreleiError::Unsupported(
                "provider does not support embeddings".to_string(),
            ));
        }
        self.provider
            .embed(EmbeddingRequest {
                tenant_id,
                provider: provider.to_string(),
                inputs,
            })
            .await
    }
}

pub struct DeterministicMockEmbeddingProvider {
    dims: usize,
}

impl DeterministicMockEmbeddingProvider {
    pub fn new(dims: usize) -> Self {
        Self { dims }
    }
}

#[async_trait]
impl EmbeddingProvider for DeterministicMockEmbeddingProvider {
    async fn embed(
        &self,
        _tenant_id: TenantId,
        _provider: &str,
        inputs: Vec<String>,
    ) -> Result<EmbeddingResponse, LoreleiError> {
        let mut vectors = Vec::with_capacity(inputs.len());
        for input in inputs {
            vectors.push(text_to_vector(&input, self.dims));
        }
        Ok(EmbeddingResponse {
            vectors,
            model: Some("deterministic-mock".to_string()),
        })
    }
}

fn text_to_vector(text: &str, dims: usize) -> Vec<f32> {
    // Deterministic "similarity-ish" embedding:
    // bucket byte values into dimensions, then L2-normalize.
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
