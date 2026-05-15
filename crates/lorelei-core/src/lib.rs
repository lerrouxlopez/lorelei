pub mod core;
pub mod config;
pub mod observability;

pub use crate::core::{
    CurrentEvent, CurrentStore, EchoHit, EchoQuery, EchoRetriever, LoreStore, LoreleiError,
    NewPearl, Pearl, PearlType, ProposedAction, ProviderCapabilities, Run, Shell, ShellCall,
    ShellRegistry, ShellResult, ShellRisk, SirenDecision, SirenPolicy, SongChunk, SongProvider,
    SongRequest, SongResponse, TideRunner, ToolCall,
};

pub use crate::config::{
    AnthropicConfig, ApiKeySource, BedrockConfig, Config, GeminiNativeConfig, LocalConfig,
    OpenAiCompatibleConfig, ProviderConfig, ProviderKind, ProviderRef, SecretString,
    SongProviderConfig,
};

pub use crate::observability::{log_json_enabled, log_prompts_enabled};
