#![forbid(unsafe_code)]

use crate::error::LoreleiError;
use crate::types::{
    AgentId, CurrentEvent, EchoHit, EchoQuery, NormalizedToolCall, ProviderCapabilities, Run,
    RunId, ShellCall, ShellResult, SirenDecision, SongChunk, SongRequest, SongResponse, TenantId,
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
}

#[async_trait]
pub trait LoreStore: Send + Sync {
    async fn put_pearl(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        pearl: crate::types::NewPearl,
    ) -> Result<crate::types::Pearl, LoreleiError>;
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
