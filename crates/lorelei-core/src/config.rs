#![forbid(unsafe_code)]

use crate::error::LoreleiError;
use crate::types::{AgentId, TenantId, UnitInterval};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    OpenaiCompatible,
    Anthropic,
    GeminiNative,
    Bedrock,
    Local,
    Mock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    #[serde(default)]
    pub base_url: Option<String>,
    pub api_key_env: String,
    pub chat_model: String,
    #[serde(default)]
    pub embedding_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentConfig {
    pub tenant_id: TenantId,
    pub agent_id: AgentId,
    pub default_provider: String,
    pub default_embedding_provider: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarborConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoreConfig {
    pub postgres_url_env: String,
    pub qdrant_url_env: String,
    pub collection: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EchoConfig {
    pub top_k: usize,
    pub rerank_top_k: usize,
    #[serde(default)]
    pub min_confidence: Option<UnitInterval>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SirenConfig {
    pub require_approval_for_high_risk: bool,
    pub allow_shell_execution: bool,
    pub allow_network_tools: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DocsConfig {
    /// Directories allowed for local document ingestion.
    ///
    /// If empty, document ingestion is disabled.
    #[serde(default)]
    pub allowed_dirs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoreleiConfig {
    pub agent: AgentConfig,
    pub harbor: HarborConfig,
    pub lore: LoreConfig,
    pub echo: EchoConfig,
    pub siren: SirenConfig,
    #[serde(default)]
    pub docs: DocsConfig,
    pub providers: BTreeMap<String, ProviderConfig>,
}

impl LoreleiConfig {
    pub fn load_from_toml_path(path: impl AsRef<Path>) -> Result<Self, LoreleiError> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|e| {
            LoreleiError::Internal(format!(
                "failed to read config file {}: {}",
                path.display(),
                e
            ))
        })?;

        let cfg: Self = toml::from_str(&text)
            .map_err(|e| LoreleiError::validation("config", sanitize_toml_error(e)))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.agent.default_provider.trim().is_empty() {
            return Err(LoreleiError::validation(
                "agent.default_provider",
                "must not be empty",
            ));
        }
        if self.agent.default_embedding_provider.trim().is_empty() {
            return Err(LoreleiError::validation(
                "agent.default_embedding_provider",
                "must not be empty",
            ));
        }

        ensure_provider_exists(
            "agent.default_provider",
            &self.agent.default_provider,
            &self.providers,
        )?;
        ensure_provider_exists(
            "agent.default_embedding_provider",
            &self.agent.default_embedding_provider,
            &self.providers,
        )?;

        if self.lore.collection.trim().is_empty() {
            return Err(LoreleiError::validation(
                "lore.collection",
                "must not be empty",
            ));
        }
        if self.lore.postgres_url_env.trim().is_empty() {
            return Err(LoreleiError::validation(
                "lore.postgres_url_env",
                "must not be empty",
            ));
        }
        if self.lore.qdrant_url_env.trim().is_empty() {
            return Err(LoreleiError::validation(
                "lore.qdrant_url_env",
                "must not be empty",
            ));
        }

        if self.echo.top_k == 0 {
            return Err(LoreleiError::validation("echo.top_k", "must be > 0"));
        }
        if self.echo.rerank_top_k == 0 {
            return Err(LoreleiError::validation("echo.rerank_top_k", "must be > 0"));
        }
        if self.echo.rerank_top_k > self.echo.top_k {
            return Err(LoreleiError::validation(
                "echo.rerank_top_k",
                "must be <= echo.top_k",
            ));
        }

        for d in &self.docs.allowed_dirs {
            if d.trim().is_empty() {
                return Err(LoreleiError::validation(
                    "docs.allowed_dirs",
                    "must not contain empty entries",
                ));
            }
        }

        for (name, p) in &self.providers {
            if name.trim().is_empty() {
                return Err(LoreleiError::validation(
                    "providers",
                    "provider name must not be empty",
                ));
            }
            if p.api_key_env.trim().is_empty() {
                return Err(LoreleiError::validation(
                    "providers.<name>.api_key_env",
                    "must not be empty",
                ));
            }
            if p.chat_model.trim().is_empty() {
                return Err(LoreleiError::validation(
                    "providers.<name>.chat_model",
                    "must not be empty",
                ));
            }
        }

        Ok(())
    }
}

fn ensure_provider_exists(
    field: &'static str,
    name: &str,
    providers: &BTreeMap<String, ProviderConfig>,
) -> Result<(), LoreleiError> {
    if providers.contains_key(name) {
        Ok(())
    } else {
        Err(LoreleiError::validation(
            field,
            "references a provider that is not configured",
        ))
    }
}

fn sanitize_toml_error(err: toml::de::Error) -> String {
    // Avoid echoing any TOML snippet; it may contain secrets (or things the user
    // considers sensitive). Keep only location + general message.
    err.to_string()
}
