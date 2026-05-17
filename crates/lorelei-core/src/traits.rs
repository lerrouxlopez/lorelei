#![forbid(unsafe_code)]

use crate::error::LoreleiError;
use crate::types::{
    AgentId, CurrentEvent, EchoHit, EchoQuery, EmbeddingRequest, EmbeddingResponse,
    NormalizedToolCall, Pearl, PearlId, PearlListQuery, ProviderCapabilities, Run, RunId,
    ShellCall, ShellResult, SirenDecision, SongChunk, SongRequest, SongResponse, TenantId,
};
use async_trait::async_trait;
use futures::stream::BoxStream;

#[async_trait]
pub trait SongProvider: Send + Sync {
    fn capabilities(&self) -> ProviderCapabilities;

    async fn complete(&self, request: SongRequest) -> Result<SongResponse, LoreleiError>;

    async fn stream(
        &self,
        request: SongRequest,
    ) -> Result<BoxStream<'static, SongChunk>, LoreleiError>;

    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse, LoreleiError>;
}

#[async_trait]
pub trait LoreStore: Send + Sync {
    async fn save_pearl(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        pearl: crate::types::NewPearl,
    ) -> Result<Pearl, LoreleiError>;

    async fn get_pearl(
        &self,
        tenant_id: TenantId,
        pearl_id: PearlId,
        include_deleted: bool,
    ) -> Result<Option<Pearl>, LoreleiError>;

    async fn list_pearls(
        &self,
        tenant_id: TenantId,
        query: PearlListQuery,
    ) -> Result<Vec<Pearl>, LoreleiError>;

    async fn forget_pearl(
        &self,
        tenant_id: TenantId,
        pearl_id: PearlId,
    ) -> Result<(), LoreleiError>;

    async fn update_last_echoed_at(
        &self,
        tenant_id: TenantId,
        pearl_id: PearlId,
        last_echoed_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), LoreleiError>;
}

#[async_trait]
pub trait CurrentStore: Send + Sync {
    async fn append_current_event(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        run_id: RunId,
        event: CurrentEvent,
    ) -> Result<(), LoreleiError>;

    async fn list_current_events(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        run_id: RunId,
        limit: usize,
    ) -> Result<Vec<CurrentEvent>, LoreleiError>;
}

#[async_trait]
pub trait EchoRetriever: Send + Sync {
    async fn query(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        query: EchoQuery,
    ) -> Result<Vec<EchoHit>, LoreleiError>;
}

#[async_trait]
pub trait DocumentStore: Send + Sync {
    async fn ingest_document_path(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        path: &std::path::Path,
    ) -> Result<uuid::Uuid, LoreleiError>;

    /// Load a single chunk for Echo display, including citation metadata.
    async fn get_document_chunk_for_echo(
        &self,
        tenant_id: TenantId,
        chunk_id: uuid::Uuid,
    ) -> Result<
        Option<(
            String,
            crate::types::EchoCitation,
            chrono::DateTime<chrono::Utc>,
        )>,
        LoreleiError,
    >;

    async fn soft_delete_document(
        &self,
        tenant_id: TenantId,
        document_id: uuid::Uuid,
    ) -> Result<(), LoreleiError>;
}

#[async_trait]
pub trait Shell: Send + Sync {
    async fn call(&self, call: ShellCall) -> Result<ShellResult, LoreleiError>;
}

#[async_trait]
pub trait ShellRegistry: Send + Sync {
    async fn list_shells(&self) -> Result<Vec<String>, LoreleiError>;

    async fn call(&self, call: ShellCall) -> Result<ShellResult, LoreleiError>;
}

#[async_trait]
pub trait SirenPolicy: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    async fn decide(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        run_id: RunId,
        task_id: Option<crate::types::AutonomousTaskId>,
        request: &SongRequest,
        response: &SongResponse,
        tool_calls: &[NormalizedToolCall],
        shell_names: &[String],
    ) -> Result<SirenDecision, LoreleiError>;
}

#[async_trait]
pub trait TideRunner: Send + Sync {
    async fn start_run(&self, tenant_id: TenantId, agent_id: AgentId) -> Result<Run, LoreleiError>;
}

#[async_trait]
pub trait ApprovalStore: Send + Sync {
    async fn is_approved(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        task_id: Option<crate::types::AutonomousTaskId>,
        tool: &str,
        input: &serde_json::Value,
    ) -> Result<bool, LoreleiError>;
}
