pub mod core;
pub mod config;

pub use crate::core::{
    CurrentEvent, CurrentStore, EchoHit, EchoQuery, EchoRetriever, LoreStore, LoreleiError,
    NewPearl, Pearl, PearlType, ProposedAction, ProviderCapabilities, Run, Shell, ShellCall,
    ShellRegistry, ShellResult, ShellRisk, SirenDecision, SirenPolicy, SongChunk, SongProvider,
    SongRequest, SongResponse, TideRunner,
};

pub use crate::config::{
    ApiKeySource, BedrockConfig, Config, GeminiNativeConfig, LocalConfig, OpenAiCompatibleConfig,
    ProviderConfig, ProviderKind, ProviderRef, SecretString, SongProviderConfig,
};
