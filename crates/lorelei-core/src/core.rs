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

fn validate_01(name: &str, value: Option<f64>) -> Result<(), LoreleiError> {
    if let Some(v) = value {
        if !v.is_finite() || v < 0.0 || v > 1.0 {
            return Err(LoreleiError::validation(format!("{name} must be between 0 and 1")));
        }
    }
    Ok(())
}

fn vec_string_null_to_empty<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = Option::<Vec<String>>::deserialize(deserializer)?;
    Ok(v.unwrap_or_default())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PearlType {
    Memory,
    Note,
    Insight,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewPearl {
    pub pearl_type: PearlType,
    pub content: String,
    #[serde(default, deserialize_with = "vec_string_null_to_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub importance: Option<f64>,
    #[serde(default)]
    pub metadata: JsonValue,
}

impl NewPearl {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.content.trim().is_empty() {
            return Err(LoreleiError::validation("new pearl content must not be empty"));
        }
        validate_01("confidence", self.confidence)?;
        validate_01("importance", self.importance)?;
        for t in &self.tags {
            if t.trim().is_empty() {
                return Err(LoreleiError::validation("new pearl tag must not be empty"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pearl {
    pub id: Uuid,
    pub tenant_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<Uuid>,
    pub run_id: Uuid,
    pub pearl_type: PearlType,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_echoed_at: Option<DateTime<Utc>>,
    pub content: String,
    #[serde(default, deserialize_with = "vec_string_null_to_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub importance: Option<f64>,
    #[serde(default)]
    pub metadata: JsonValue,
}

impl Pearl {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.content.trim().is_empty() {
            return Err(LoreleiError::validation("pearl content must not be empty"));
        }
        validate_01("confidence", self.confidence)?;
        validate_01("importance", self.importance)?;
        for t in &self.tags {
            if t.trim().is_empty() {
                return Err(LoreleiError::validation("pearl tag must not be empty"));
            }
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EchoQuery {
    pub text: String,
    pub tenant_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pearl_type: Option<PearlType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_confidence: Option<f64>,
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
        validate_01("min_confidence", self.min_confidence)?;
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

impl SongChunk {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.content.trim().is_empty() && self.tool_calls.is_empty() {
            return Err(LoreleiError::validation("song chunk content must not be empty"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: JsonValue,
}

impl ToolCall {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.id.trim().is_empty() {
            return Err(LoreleiError::validation("tool call id must not be empty"));
        }
        if self.name.trim().is_empty() {
            return Err(LoreleiError::validation("tool call name must not be empty"));
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
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub embeddings: bool,
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
    pub tenant_id: Uuid,
    pub target_tenant_id: Uuid,
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
    async fn save_pearl(
        &self,
        tenant_id: Uuid,
        agent_id: Option<Uuid>,
        run_id: Uuid,
        pearl: NewPearl,
    ) -> Result<Pearl, LoreleiError>;
    async fn get_pearl(
        &self,
        tenant_id: Uuid,
        id: Uuid,
        include_deleted: bool,
    ) -> Result<Option<Pearl>, LoreleiError>;
    async fn forget_pearl(&self, tenant_id: Uuid, id: Uuid) -> Result<(), LoreleiError>;
    async fn update_last_echoed_at(
        &self,
        tenant_id: Uuid,
        id: Uuid,
        at: DateTime<Utc>,
    ) -> Result<(), LoreleiError>;
}

#[async_trait]
pub trait CurrentStore: Send + Sync {
    async fn write_current(
        &self,
        tenant_id: Uuid,
        event: CurrentEvent,
        confidence: Option<f64>,
        importance: Option<f64>,
    ) -> Result<(), LoreleiError>;
    async fn list_currents(
        &self,
        tenant_id: Uuid,
        run_id: Uuid,
    ) -> Result<Vec<CurrentEvent>, LoreleiError>;
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
