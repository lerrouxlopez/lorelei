#![forbid(unsafe_code)]

use futures::stream::BoxStream;
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::SongProvider;
use lorelei_core::types::{
    EmbeddingRequest, EmbeddingResponse, ProviderCapabilities, SongChunk, SongRequest, SongResponse,
};

pub struct UnsupportedProvider {
    pub name: String,
    pub kind: String,
    pub capabilities: ProviderCapabilities,
}

#[async_trait::async_trait]
impl SongProvider for UnsupportedProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }

    async fn complete(&self, _request: SongRequest) -> Result<SongResponse, LoreleiError> {
        Err(LoreleiError::Unsupported(format!(
            "provider `{}` ({}) is not implemented yet",
            self.name, self.kind
        )))
    }

    async fn stream(
        &self,
        _request: SongRequest,
    ) -> Result<BoxStream<'static, SongChunk>, LoreleiError> {
        Err(LoreleiError::Unsupported(format!(
            "provider `{}` ({}) streaming is not implemented yet",
            self.name, self.kind
        )))
    }

    async fn embed(&self, _request: EmbeddingRequest) -> Result<EmbeddingResponse, LoreleiError> {
        Err(LoreleiError::Unsupported(format!(
            "provider `{}` ({}) embeddings are not implemented yet",
            self.name, self.kind
        )))
    }
}
