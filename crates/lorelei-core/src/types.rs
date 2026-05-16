#![forbid(unsafe_code)]

use crate::error::LoreleiError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TenantId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PearlId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EchoId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct UnitInterval(f64);

impl UnitInterval {
    pub fn new(value: f64) -> Result<Self, LoreleiError> {
        if (0.0..=1.0).contains(&value) && value.is_finite() {
            Ok(Self(value))
        } else {
            Err(LoreleiError::validation(
                "unit_interval",
                "must be a finite number between 0.0 and 1.0",
            ))
        }
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for UnitInterval {
    type Error = LoreleiError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<UnitInterval> for f64 {
    fn from(value: UnitInterval) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub run_id: RunId,
    pub tenant_id: TenantId,
    pub agent_id: AgentId,
    pub status: RunStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CurrentEventType {
    User,
    Assistant,
    ToolCall,
    ToolResult,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentEvent {
    pub event_id: EchoId,
    pub tenant_id: TenantId,
    pub agent_id: AgentId,
    pub run_id: RunId,
    pub event_type: CurrentEventType,
    pub created_at: DateTime<Utc>,
    pub summary: String,
    pub data: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PearlType {
    Fact,
    Preference,
    Skill,
    Plan,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PearlSerde")]
pub struct Pearl {
    pub pearl_id: PearlId,
    pub tenant_id: TenantId,
    pub agent_id: AgentId,
    pub pearl_type: PearlType,
    pub content: String,
    pub importance: UnitInterval,
    pub confidence: UnitInterval,
    pub created_at: DateTime<Utc>,
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewPearl {
    pub pearl_type: PearlType,
    pub content: String,
    pub importance: UnitInterval,
    pub confidence: UnitInterval,
    pub metadata: BTreeMap<String, Value>,
}

impl NewPearl {
    pub fn new(
        pearl_type: PearlType,
        content: impl Into<String>,
        importance: UnitInterval,
        confidence: UnitInterval,
        metadata: BTreeMap<String, Value>,
    ) -> Result<Self, LoreleiError> {
        let content = content.into();
        if content.trim().is_empty() {
            return Err(LoreleiError::validation(
                "pearl.content",
                "must not be empty",
            ));
        }
        Ok(Self {
            pearl_type,
            content,
            importance,
            confidence,
            metadata,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
struct PearlSerde {
    pearl_id: PearlId,
    tenant_id: TenantId,
    agent_id: AgentId,
    pearl_type: PearlType,
    content: String,
    importance: UnitInterval,
    confidence: UnitInterval,
    created_at: DateTime<Utc>,
    metadata: BTreeMap<String, Value>,
}

impl TryFrom<PearlSerde> for Pearl {
    type Error = LoreleiError;

    fn try_from(value: PearlSerde) -> Result<Self, Self::Error> {
        if value.content.trim().is_empty() {
            return Err(LoreleiError::validation(
                "pearl.content",
                "must not be empty",
            ));
        }
        Ok(Self {
            pearl_id: value.pearl_id,
            tenant_id: value.tenant_id,
            agent_id: value.agent_id,
            pearl_type: value.pearl_type,
            content: value.content,
            importance: value.importance,
            confidence: value.confidence,
            created_at: value.created_at,
            metadata: value.metadata,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EchoQuery {
    pub query: String,
    pub top_k: usize,
    pub min_confidence: Option<UnitInterval>,
    pub pearl_type: Option<PearlType>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EchoHit {
    pub score: UnitInterval,
    pub pearl_id: PearlId,
    pub content: String,
    pub pearl_type: PearlType,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SongRequest {
    pub tenant_id: TenantId,
    pub agent_id: AgentId,
    pub run_id: RunId,
    pub input: String,
    pub context: Vec<String>,
    pub reasoning_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SongResponse {
    pub output: String,
    pub reasoning_summary: Option<String>,
    pub tool_calls: Vec<NormalizedToolCall>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SongChunk {
    pub delta: String,
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingRequest {
    pub tenant_id: TenantId,
    pub provider: String,
    pub inputs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingResponse {
    pub vectors: Vec<Vec<f32>>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProviderCapabilities {
    pub streaming: bool,
    pub tool_calls: bool,
    pub embeddings: bool,
    pub max_output_tokens: Option<u32>,
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedToolCall {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellCall {
    pub call_id: Uuid,
    pub tenant_id: TenantId,
    pub agent_id: AgentId,
    pub run_id: RunId,
    pub shell: String,
    pub tool: String,
    pub input: Value,
    pub risk: ShellRisk,
    pub requested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellResult {
    pub call_id: Uuid,
    pub ok: bool,
    pub output: Value,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShellRisk {
    None,
    Low,
    Medium,
    High,
    Critical,
}

impl ShellRisk {
    fn severity(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
            Self::Critical => 4,
        }
    }
}

impl Ord for ShellRisk {
    fn cmp(&self, other: &Self) -> Ordering {
        self.severity().cmp(&other.severity())
    }
}

impl PartialOrd for ShellRisk {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedAction {
    pub summary: String,
    pub tool_calls: Vec<NormalizedToolCall>,
    pub risk: ShellRisk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SirenDecision {
    Allow { reasoning_summary: String },
    Deny { reasoning_summary: String },
    RequireReview { reasoning_summary: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PearlListQuery {
    pub agent_id: Option<AgentId>,
    pub pearl_type: Option<PearlType>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub limit: Option<usize>,
    pub include_deleted: bool,
}
