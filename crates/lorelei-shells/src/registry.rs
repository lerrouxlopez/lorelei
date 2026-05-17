#![forbid(unsafe_code)]

use crate::repo::ShellCallRepository;
use async_trait::async_trait;
use lorelei_core::config::LoreleiConfig;
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::{EchoRetriever, LoreStore, ShellRegistry};
use lorelei_core::types::{ShellCall, ShellResult, ShellRisk};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};
use uuid::Uuid;

type CurrentIdProvider = Arc<dyn Fn(&ShellCall) -> Option<Uuid> + Send + Sync>;

#[derive(Debug, Clone, Serialize)]
pub struct ShellSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub risk: ShellRisk,
}

#[async_trait]
pub trait ShellTool: Send + Sync {
    fn spec(&self) -> ShellSpec;
    async fn execute(&self, call: ShellCall) -> Result<ShellResult, LoreleiError>;
}

#[derive(Clone)]
pub struct BuiltinShellRegistry {
    config: LoreleiConfig,
    tools: BTreeMap<String, Arc<dyn ShellTool>>,
    calls: Arc<dyn ShellCallRepository>,
    current_id_provider: CurrentIdProvider,
}

impl BuiltinShellRegistry {
    pub fn new(
        config: LoreleiConfig,
        lore: Arc<dyn LoreStore>,
        echo: Arc<dyn EchoRetriever>,
        documents: Arc<dyn lorelei_core::traits::DocumentStore>,
        calls: Arc<dyn ShellCallRepository>,
    ) -> Self {
        let tools = crate::builtin::builtin_tools(&config, lore, echo, documents);
        Self {
            config,
            tools,
            calls,
            current_id_provider: Arc::new(|_call: &ShellCall| None),
        }
    }

    pub fn with_current_id_provider(
        mut self,
        f: impl Fn(&ShellCall) -> Option<Uuid> + Send + Sync + 'static,
    ) -> Self {
        self.current_id_provider = Arc::new(f);
        self
    }

    pub fn specs(&self) -> Vec<ShellSpec> {
        self.tools.values().map(|t| t.spec()).collect()
    }

    fn get_tool(&self, name: &str) -> Result<Arc<dyn ShellTool>, LoreleiError> {
        self.tools.get(name).cloned().ok_or_else(|| {
            LoreleiError::NotFound(format!(
                "unknown shell tool `{name}` (expected built-in tool)"
            ))
        })
    }

    async fn run_one(&self, call: ShellCall) -> Result<ShellResult, LoreleiError> {
        let started = Instant::now();
        // Enforce network tools config centrally.
        if call.tool == "http_get" && !self.config.siren.allow_network_tools {
            return Err(LoreleiError::Unsupported(
                "http_get is disabled (siren.allow_network_tools=false)".to_string(),
            ));
        }

        let tool = self.get_tool(&call.tool)?;
        let current_id = (self.current_id_provider)(&call);

        let call_id = call.call_id;

        // Record start *after* we've validated tool existence / config gating.
        self.calls.record_start(current_id, &call).await?;

        let run_id = call.run_id.0;
        let tool_name = call.tool.clone();
        let risk = call.risk;

        match tool.execute(call).await {
            Ok(result) => {
                let latency = started.elapsed();
                self.calls.record_finish(&result).await?;
                info!(
                    run_id = %run_id,
                    tool = %tool_name,
                    risk = ?risk,
                    status = "ok",
                    latency_ms = latency.as_millis() as u64,
                    "shell.call"
                );
                Ok(result)
            }
            Err(e) => {
                let now = chrono::Utc::now();
                let failed = ShellResult {
                    call_id,
                    ok: false,
                    output: Value::Null,
                    error: Some(e.to_string()),
                    started_at: now,
                    finished_at: now,
                };
                self.calls.record_finish(&failed).await?;
                let latency = started.elapsed();
                warn!(
                    run_id = %run_id,
                    tool = %tool_name,
                    risk = ?risk,
                    status = "error",
                    latency_ms = latency.as_millis() as u64,
                    "shell.call"
                );
                Err(e)
            }
        }
    }
}

#[async_trait]
impl ShellRegistry for BuiltinShellRegistry {
    async fn list_shells(&self) -> Result<Vec<String>, LoreleiError> {
        Ok(self.tools.keys().cloned().collect())
    }

    async fn call(&self, call: ShellCall) -> Result<ShellResult, LoreleiError> {
        self.run_one(call).await
    }
}
