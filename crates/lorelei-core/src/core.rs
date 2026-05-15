use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum LoreleiError {
    #[error("validation error: {message}")]
    Validation { message: String },

    #[error("song provider error: {message}")]
    SongProvider { message: String },

    #[error("lore store error: {message}")]
    LoreStore { message: String },

    #[error("current store error: {message}")]
    CurrentStore { message: String },

    #[error("echo retriever error: {message}")]
    EchoRetriever { message: String },

    #[error("shell error: {message}")]
    Shell { message: String },

    #[error("siren policy error: {message}")]
    SirenPolicy { message: String },

    #[error("tide runner error: {message}")]
    TideRunner { message: String },
}

impl LoreleiError {
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PearlType {
    Memory,
    Note,
    Insight,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewPearl {
    pub pearl_type: PearlType,
    pub content: String,
}

impl NewPearl {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.content.trim().is_empty() {
            return Err(LoreleiError::validation("new pearl content must not be empty"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pearl {
    pub id: Uuid,
    pub run_id: Uuid,
    pub pearl_type: PearlType,
    pub created_at: DateTime<Utc>,
    pub content: String,
}

impl Pearl {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.content.trim().is_empty() {
            return Err(LoreleiError::validation("pearl content must not be empty"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub metadata: JsonValue,
}

impl Run {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if let Some(ended_at) = self.ended_at {
            if ended_at < self.started_at {
                return Err(LoreleiError::validation(
                    "run ended_at must not be earlier than started_at",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentEvent {
    pub id: Uuid,
    pub run_id: Uuid,
    pub at: DateTime<Utc>,
    pub kind: String,
    #[serde(default)]
    pub payload: JsonValue,
}

impl CurrentEvent {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.kind.trim().is_empty() {
            return Err(LoreleiError::validation("current event kind must not be empty"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EchoQuery {
    pub text: String,
    #[serde(default)]
    pub run_id: Option<Uuid>,
    #[serde(default = "EchoQuery::default_limit")]
    pub limit: usize,
}

impl EchoQuery {
    fn default_limit() -> usize {
        25
    }

    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.text.trim().is_empty() {
            return Err(LoreleiError::validation("echo query text must not be empty"));
        }
        if self.limit == 0 || self.limit > 1_000 {
            return Err(LoreleiError::validation(
                "echo query limit must be between 1 and 1000",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EchoHit {
    pub pearl_id: Uuid,
    pub score: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(default)]
    pub metadata: JsonValue,
}

impl EchoHit {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if !self.score.is_finite() || self.score < 0.0 {
            return Err(LoreleiError::validation("echo hit score must be finite and >= 0"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SongRequest {
    pub prompt: String,
    #[serde(default)]
    pub context: Vec<Pearl>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_chunks: Option<u32>,
    #[serde(default)]
    pub parameters: BTreeMap<String, JsonValue>,
}

impl SongRequest {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.prompt.trim().is_empty() {
            return Err(LoreleiError::validation("song request prompt must not be empty"));
        }
        if let Some(max_chunks) = self.max_chunks {
            if max_chunks == 0 {
                return Err(LoreleiError::validation("song request max_chunks must be > 0"));
            }
        }
        for pearl in &self.context {
            pearl.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SongChunk {
    pub index: u32,
    pub content: String,
    #[serde(default)]
    pub is_final: bool,
}

impl SongChunk {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.content.trim().is_empty() {
            return Err(LoreleiError::validation("song chunk content must not be empty"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SongResponse {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub chunks: Vec<SongChunk>,
    #[serde(default)]
    pub metadata: JsonValue,
}

impl SongResponse {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        for chunk in &self.chunks {
            chunk.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    #[serde(default)]
    pub streaming: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u32>,
    #[serde(default)]
    pub json_mode: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellCall {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl ShellCall {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.program.trim().is_empty() {
            return Err(LoreleiError::validation("shell call program must not be empty"));
        }
        if let Some(timeout_ms) = self.timeout_ms {
            if timeout_ms == 0 {
                return Err(LoreleiError::validation("shell call timeout_ms must be > 0"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellRisk {
    None,
    Low,
    Medium,
    High,
    Forbidden,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellResult {
    pub exit_code: i32,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedAction {
    pub call: ShellCall,
    pub risk: ShellRisk,
    #[serde(default)]
    pub rationale: String,
}

impl ProposedAction {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        self.call.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SirenDecision {
    pub allow: bool,
    pub risk: ShellRisk,
    #[serde(default)]
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed: Option<ProposedAction>,
}

#[async_trait]
pub trait SongProvider: Send + Sync {
    async fn capabilities(&self) -> Result<ProviderCapabilities, LoreleiError>;
    async fn song(&self, request: SongRequest) -> Result<SongResponse, LoreleiError>;
    async fn song_stream(
        &self,
        request: SongRequest,
    ) -> Result<BoxStream<'static, Result<SongChunk, LoreleiError>>, LoreleiError>;
}

#[async_trait]
pub trait LoreStore: Send + Sync {
    async fn put_pearl(&self, run_id: Uuid, pearl: NewPearl) -> Result<Pearl, LoreleiError>;
    async fn get_pearl(&self, id: Uuid) -> Result<Option<Pearl>, LoreleiError>;
}

#[async_trait]
pub trait CurrentStore: Send + Sync {
    async fn append_event(&self, event: CurrentEvent) -> Result<(), LoreleiError>;
    async fn list_events(&self, run_id: Uuid) -> Result<Vec<CurrentEvent>, LoreleiError>;
}

#[async_trait]
pub trait EchoRetriever: Send + Sync {
    async fn query(&self, query: EchoQuery) -> Result<Vec<EchoHit>, LoreleiError>;
}

#[async_trait]
pub trait Shell: Send + Sync {
    async fn call(&self, call: ShellCall) -> Result<ShellResult, LoreleiError>;
}

#[async_trait]
pub trait ShellRegistry: Send + Sync {
    async fn resolve(&self, name: &str) -> Result<Box<dyn Shell>, LoreleiError>;
}

#[async_trait]
pub trait SirenPolicy: Send + Sync {
    async fn decide(&self, action: ProposedAction) -> Result<SirenDecision, LoreleiError>;
}

#[async_trait]
pub trait TideRunner: Send + Sync {
    async fn run(&self, run: Run) -> Result<(), LoreleiError>;
}

