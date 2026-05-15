use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::path::Path;

use serde::Deserialize;

use crate::core::LoreleiError;

#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(REDACTED)")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("REDACTED")
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Self(s))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", tag = "source")]
pub enum ApiKeySource {
    /// Inline config value. Avoid using this in real deployments.
    Literal { value: SecretString },
    /// Read API key from environment variable at runtime.
    Env { var: String },
}

impl ApiKeySource {
    pub fn resolve(&self) -> Result<SecretString, LoreleiError> {
        match self {
            ApiKeySource::Literal { value } => Ok(value.clone()),
            ApiKeySource::Env { var } => {
                if var == "LORELEI_LOCAL_API_KEY" {
                    // Local OpenAI-compatible endpoints often do not require auth.
                    // Treat missing/empty key as "no auth".
                    let v = env::var(var).unwrap_or_default();
                    return Ok(SecretString::new(v));
                }

                let v = env::var(var)
                    .map_err(|_| LoreleiError::validation(format!("missing API key env var `{var}`")))?;
                if v.trim().is_empty() {
                    return Err(LoreleiError::validation(format!(
                        "API key env var `{var}` must not be empty"
                    )));
                }
                Ok(SecretString::new(v))
            }
        }
    }

    pub fn validate(&self) -> Result<(), LoreleiError> {
        match self {
            ApiKeySource::Literal { value } => {
                if value.expose().trim().is_empty() {
                    return Err(LoreleiError::validation("literal API key must not be empty"));
                }
                Ok(())
            }
            ApiKeySource::Env { var } => {
                if var.trim().is_empty() {
                    return Err(LoreleiError::validation("env var name must not be empty"));
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OpenAiCompatibleConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: ApiKeySource,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

impl OpenAiCompatibleConfig {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.base_url.trim().is_empty() {
            return Err(LoreleiError::validation("openai-compatible base_url must not be empty"));
        }
        if self.model.trim().is_empty() {
            return Err(LoreleiError::validation("openai-compatible model must not be empty"));
        }
        self.api_key.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AnthropicConfig {
    pub model: String,
    pub api_key: ApiKeySource,
}

impl AnthropicConfig {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.model.trim().is_empty() {
            return Err(LoreleiError::validation("anthropic model must not be empty"));
        }
        self.api_key.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GeminiNativeConfig {
    pub model: String,
    /// Optional: some deployments use ADC / service accounts instead of an API key.
    #[serde(default)]
    pub api_key: Option<ApiKeySource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

impl GeminiNativeConfig {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.model.trim().is_empty() {
            return Err(LoreleiError::validation("gemini-native model must not be empty"));
        }
        if let Some(api_key) = &self.api_key {
            api_key.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BedrockConfig {
    pub region: String,
    pub model_id: String,
    /// Optional explicit credentials; typically sourced from env/instance profile.
    #[serde(default)]
    pub access_key_id: Option<ApiKeySource>,
    #[serde(default)]
    pub secret_access_key: Option<ApiKeySource>,
}

impl BedrockConfig {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.region.trim().is_empty() {
            return Err(LoreleiError::validation("bedrock region must not be empty"));
        }
        if self.model_id.trim().is_empty() {
            return Err(LoreleiError::validation("bedrock model_id must not be empty"));
        }
        if let Some(akid) = &self.access_key_id {
            akid.validate()?;
        }
        if let Some(sak) = &self.secret_access_key {
            sak.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LocalConfig {
    /// Optional local endpoint (e.g. an OpenAI-compatible server).
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Optional model name for local engines.
    #[serde(default)]
    pub model: Option<String>,
}

impl LocalConfig {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if let Some(endpoint) = &self.endpoint {
            if endpoint.trim().is_empty() {
                return Err(LoreleiError::validation("local endpoint must not be empty"));
            }
        }
        if let Some(model) = &self.model {
            if model.trim().is_empty() {
                return Err(LoreleiError::validation("local model must not be empty"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ProviderKind {
    #[serde(rename = "openai-compatible")]
    OpenAiCompatible,
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "gemini-native")]
    GeminiNative,
    #[serde(rename = "bedrock")]
    Bedrock,
    #[serde(rename = "local")]
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind")]
pub enum ProviderConfig {
    #[serde(rename = "openai-compatible")]
    OpenAiCompatible(OpenAiCompatibleConfig),
    #[serde(rename = "anthropic")]
    Anthropic(AnthropicConfig),
    #[serde(rename = "gemini-native")]
    GeminiNative(GeminiNativeConfig),
    #[serde(rename = "bedrock")]
    Bedrock(BedrockConfig),
    #[serde(rename = "local")]
    Local(LocalConfig),
}

impl ProviderConfig {
    pub fn kind(&self) -> ProviderKind {
        match self {
            ProviderConfig::OpenAiCompatible(_) => ProviderKind::OpenAiCompatible,
            ProviderConfig::Anthropic(_) => ProviderKind::Anthropic,
            ProviderConfig::GeminiNative(_) => ProviderKind::GeminiNative,
            ProviderConfig::Bedrock(_) => ProviderKind::Bedrock,
            ProviderConfig::Local(_) => ProviderKind::Local,
        }
    }

    pub fn validate(&self) -> Result<(), LoreleiError> {
        match self {
            ProviderConfig::OpenAiCompatible(cfg) => cfg.validate(),
            ProviderConfig::Anthropic(cfg) => cfg.validate(),
            ProviderConfig::GeminiNative(cfg) => cfg.validate(),
            ProviderConfig::Bedrock(cfg) => cfg.validate(),
            ProviderConfig::Local(cfg) => cfg.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProviderRef {
    pub name: String,
}

impl ProviderRef {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.name.trim().is_empty() {
            return Err(LoreleiError::validation("provider name must not be empty"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SongProviderConfig {
    pub provider: ProviderRef,
}

impl SongProviderConfig {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        self.provider.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    #[serde(default)]
    pub song: Option<SongProviderConfig>,
}

impl Config {
    pub fn from_toml_str(s: &str) -> Result<Self, LoreleiError> {
        let cfg: Self = toml::from_str(s)
            .map_err(|e| LoreleiError::validation(format!("config parse error: {e}")))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn from_toml_file(path: impl AsRef<Path>) -> Result<Self, LoreleiError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| {
            LoreleiError::validation(format!("failed to read config `{}`: {e}", path.display()))
        })?;
        Self::from_toml_str(&text)
    }

    pub fn validate(&self) -> Result<(), LoreleiError> {
        for (name, provider) in &self.providers {
            if name.trim().is_empty() {
                return Err(LoreleiError::validation("provider map keys must not be empty"));
            }
            provider.validate()?;
        }

        if let Some(song) = &self.song {
            song.validate()?;
            if !self.providers.contains_key(&song.provider.name) {
                return Err(LoreleiError::validation(format!(
                    "song.provider.name `{}` not found in [providers]",
                    song.provider.name
                )));
            }
        }

        Ok(())
    }
}
