use async_trait::async_trait;
use lorelei_core::{LoreleiError, ProviderCapabilities, SongChunk, SongProvider, SongRequest, SongResponse};
#[derive(Clone)]
pub struct FallbackSongProvider<P: SongProvider, F: SongProvider> {
    pub primary: P,
    pub fallback: F,
}

#[async_trait]
impl<P: SongProvider, F: SongProvider> SongProvider for FallbackSongProvider<P, F> {
    async fn capabilities(&self) -> Result<ProviderCapabilities, LoreleiError> {
        // Prefer primary capabilities; if it fails, use fallback.
        match self.primary.capabilities().await {
            Ok(c) => Ok(c),
            Err(_) => self.fallback.capabilities().await,
        }
    }

    async fn song(&self, request: SongRequest) -> Result<SongResponse, LoreleiError> {
        match self.primary.song(request.clone()).await {
            Ok(r) => Ok(r),
            Err(_) => self.fallback.song(request).await,
        }
    }

    async fn song_stream(
        &self,
        request: SongRequest,
    ) -> Result<futures::stream::BoxStream<'static, Result<SongChunk, LoreleiError>>, LoreleiError> {
        match self.primary.song_stream(request.clone()).await {
            Ok(s) => Ok(s),
            Err(_) => self.fallback.song_stream(request).await,
        }
    }
}
