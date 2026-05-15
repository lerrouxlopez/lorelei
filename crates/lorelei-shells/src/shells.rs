use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use jsonschema::Validator;
use lorelei_core::{LoreleiError, ShellCall, ShellResult, ShellRisk};
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltInShellName {
    SavePearl,
    EchoLore,
    ListPearls,
    ForgetPearl,
    HttpGet,
    DocumentIngest,
    Echo,
    Noop,
}

impl BuiltInShellName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SavePearl => "save_pearl",
            Self::EchoLore => "echo_lore",
            Self::ListPearls => "list_pearls",
            Self::ForgetPearl => "forget_pearl",
            Self::HttpGet => "http_get",
            Self::DocumentIngest => "document_ingest",
            Self::Echo => "echo",
            Self::Noop => "noop",
        }
    }
}

impl TryFrom<&str> for BuiltInShellName {
    type Error = LoreleiError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "save_pearl" => Ok(Self::SavePearl),
            "echo_lore" => Ok(Self::EchoLore),
            "list_pearls" => Ok(Self::ListPearls),
            "forget_pearl" => Ok(Self::ForgetPearl),
            "http_get" => Ok(Self::HttpGet),
            "document_ingest" => Ok(Self::DocumentIngest),
            "echo" => Ok(Self::Echo),
            "noop" => Ok(Self::Noop),
            _ => Err(LoreleiError::validation(format!("unknown shell `{value}`"))),
        }
    }
}

#[async_trait]
pub trait ShellSpec: Send + Sync {
    fn name(&self) -> &'static str;
    fn risk(&self) -> ShellRisk;
    fn schema(&self) -> JsonValue;
    async fn execute(&self, call: ShellCall, input: JsonValue) -> Result<ShellResult, LoreleiError>;
}

#[derive(Clone)]
pub struct ShellRegistryPg {
    pool: PgPool,
    shells: HashMap<&'static str, Arc<dyn ShellSpec>>,
}

impl ShellRegistryPg {
    pub fn new(pool: PgPool) -> Self {
        let mut shells: HashMap<&'static str, Arc<dyn ShellSpec>> = HashMap::new();
        register_builtin(&mut shells);
        Self { pool, shells }
    }

    pub fn validate_name(&self, name: &str) -> Result<(), LoreleiError> {
        if self.shells.contains_key(name) {
            Ok(())
        } else {
            Err(LoreleiError::validation(format!("unknown shell `{name}`")))
        }
    }

    pub fn schema(&self, name: &str) -> Result<JsonValue, LoreleiError> {
        self.validate_name(name)?;
        Ok(self.shells[name].schema())
    }

    pub fn risk(&self, name: &str) -> Result<ShellRisk, LoreleiError> {
        self.validate_name(name)?;
        Ok(self.shells[name].risk())
    }

    pub async fn execute(
        &self,
        tenant_id: Uuid,
        run_id: Uuid,
        shell_name: &str,
        call: ShellCall,
        input: JsonValue,
    ) -> Result<ShellResult, LoreleiError> {
        self.validate_name(shell_name)?;
        call.validate()?;

        let spec = self.shells[shell_name].clone();
        let risk = spec.risk();
        let schema = spec.schema();
        validate_schema(&schema, &input)?;

        let start = std::time::Instant::now();
        info!(
            tenant_id = %tenant_id,
            run_id = %run_id,
            shell_name,
            shell_risk = ?risk,
            "shell.call_start"
        );

        // Write shell_calls row with input first (output filled after).
        let id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO shell_calls (id, tenant_id, run_id, input, output, created_at)
            VALUES ($1, $2, $3, $4, NULL, now())
            "#,
        )
        .bind(id)
        .bind(tenant_id)
        .bind(run_id)
        .bind(sqlx::types::Json(input.clone()))
        .execute(&self.pool)
        .await
        .map_err(|e| LoreleiError::Shell {
            message: format!("failed to write shell_calls input: {e}"),
        })?;

        let result = spec.execute(call, input).await;

        match &result {
            Ok(r) => info!(
                tenant_id = %tenant_id,
                run_id = %run_id,
                shell_name,
                shell_risk = ?risk,
                exit_code = r.exit_code,
                latency_ms = start.elapsed().as_millis() as u64,
                "shell.call_end"
            ),
            Err(e) => warn!(
                tenant_id = %tenant_id,
                run_id = %run_id,
                shell_name,
                shell_risk = ?risk,
                latency_ms = start.elapsed().as_millis() as u64,
                error = %e,
                "shell.call_error"
            ),
        }

        // Persist output (even on error, store error info without secrets).
        let output = match &result {
            Ok(r) => json!({
                "exit_code": r.exit_code,
                "stdout": r.stdout,
                "stderr": r.stderr,
                "duration_ms": r.duration_ms,
            }),
            Err(e) => json!({
                "error": e.to_string(),
            }),
        };

        sqlx::query(
            r#"
            UPDATE shell_calls
            SET output = $1
            WHERE id = $2 AND tenant_id = $3 AND run_id = $4
            "#,
        )
        .bind(sqlx::types::Json(output))
        .bind(id)
        .bind(tenant_id)
        .bind(run_id)
        .execute(&self.pool)
        .await
        .map_err(|e| LoreleiError::Shell {
            message: format!("failed to write shell_calls output: {e}"),
        })?;

        result
    }
}

fn validate_schema(schema: &JsonValue, instance: &JsonValue) -> Result<(), LoreleiError> {
    let compiled = Validator::new(schema).map_err(|e| {
        LoreleiError::validation(format!("invalid shell schema: {e}"))
    })?;
    let mut errors = compiled.iter_errors(instance);
    if let Some(first) = errors.next() {
        return Err(LoreleiError::validation(format!(
            "shell input schema validation failed: {}",
            first
        )));
    }
    Ok(())
}

fn register_builtin(map: &mut HashMap<&'static str, Arc<dyn ShellSpec>>) {
    map.insert(SavePearlShell.name(), Arc::new(SavePearlShell));
    map.insert(EchoLoreShell.name(), Arc::new(EchoLoreShell));
    map.insert(ListPearlsShell.name(), Arc::new(ListPearlsShell));
    map.insert(ForgetPearlShell.name(), Arc::new(ForgetPearlShell));
    map.insert(HttpGetShell.name(), Arc::new(HttpGetShell));
    map.insert(DocumentIngestShell.name(), Arc::new(DocumentIngestShell));
    map.insert(EchoShell.name(), Arc::new(EchoShell));
    map.insert(NoopShell.name(), Arc::new(NoopShell));
}

struct SavePearlShell;
struct EchoLoreShell;
struct ListPearlsShell;
struct ForgetPearlShell;
struct HttpGetShell;
struct DocumentIngestShell;
struct EchoShell;
struct NoopShell;

impl SavePearlShell {
    const fn name(&self) -> &'static str {
        "save_pearl"
    }
}
impl EchoLoreShell {
    const fn name(&self) -> &'static str {
        "echo_lore"
    }
}
impl ListPearlsShell {
    const fn name(&self) -> &'static str {
        "list_pearls"
    }
}
impl ForgetPearlShell {
    const fn name(&self) -> &'static str {
        "forget_pearl"
    }
}
impl HttpGetShell {
    const fn name(&self) -> &'static str {
        "http_get"
    }
}
impl DocumentIngestShell {
    const fn name(&self) -> &'static str {
        "document_ingest"
    }
}
impl EchoShell {
    const fn name(&self) -> &'static str {
        "echo"
    }
}
impl NoopShell {
    const fn name(&self) -> &'static str {
        "noop"
    }
}

#[async_trait]
impl ShellSpec for SavePearlShell {
    fn name(&self) -> &'static str {
        self.name()
    }
    fn risk(&self) -> ShellRisk {
        ShellRisk::Medium
    }
    fn schema(&self) -> JsonValue {
        json!({
          "type": "object",
          "additionalProperties": false,
          "required": ["content", "pearl_type"],
          "properties": {
            "content": {"type": "string", "minLength": 1},
            "pearl_type": {"type":"string", "enum":["memory","note","insight"]},
            "tags": {"type":"array", "items":{"type":"string"}},
            "confidence": {"type":"number", "minimum": 0.0, "maximum": 1.0},
            "importance": {"type":"number", "minimum": 0.0, "maximum": 1.0},
            "metadata": {"type":"object"}
          }
        })
    }
    async fn execute(&self, _call: ShellCall, input: JsonValue) -> Result<ShellResult, LoreleiError> {
        Ok(ShellResult {
            exit_code: 0,
            stdout: input.to_string(),
            stderr: String::new(),
            duration_ms: None,
        })
    }
}

#[async_trait]
impl ShellSpec for EchoLoreShell {
    fn name(&self) -> &'static str {
        self.name()
    }
    fn risk(&self) -> ShellRisk {
        ShellRisk::Low
    }
    fn schema(&self) -> JsonValue {
        json!({
          "type":"object",
          "additionalProperties": false,
          "required":["text"],
          "properties": {
            "text": {"type":"string", "minLength": 1},
            "pearl_type": {"type":"string", "enum":["memory","note","insight"]},
            "min_confidence": {"type":"number", "minimum":0.0, "maximum":1.0},
            "top_k": {"type":"integer", "minimum": 1, "maximum": 1000}
          }
        })
    }
    async fn execute(&self, _call: ShellCall, input: JsonValue) -> Result<ShellResult, LoreleiError> {
        Ok(ShellResult {
            exit_code: 0,
            stdout: input.to_string(),
            stderr: String::new(),
            duration_ms: None,
        })
    }
}

#[async_trait]
impl ShellSpec for ListPearlsShell {
    fn name(&self) -> &'static str {
        self.name()
    }
    fn risk(&self) -> ShellRisk {
        ShellRisk::Low
    }
    fn schema(&self) -> JsonValue {
        json!({
          "type":"object",
          "additionalProperties": false,
          "properties": {
            "pearl_type": {"type":"string", "enum":["memory","note","insight"]},
            "top_k": {"type":"integer", "minimum": 1, "maximum": 1000}
          }
        })
    }
    async fn execute(&self, _call: ShellCall, input: JsonValue) -> Result<ShellResult, LoreleiError> {
        Ok(ShellResult {
            exit_code: 0,
            stdout: input.to_string(),
            stderr: String::new(),
            duration_ms: None,
        })
    }
}

#[async_trait]
impl ShellSpec for ForgetPearlShell {
    fn name(&self) -> &'static str {
        self.name()
    }
    fn risk(&self) -> ShellRisk {
        ShellRisk::High
    }
    fn schema(&self) -> JsonValue {
        json!({
          "type":"object",
          "additionalProperties": false,
          "required":["pearl_id"],
          "properties": {
            "pearl_id": {"type":"string", "minLength": 1}
          }
        })
    }
    async fn execute(&self, _call: ShellCall, input: JsonValue) -> Result<ShellResult, LoreleiError> {
        Ok(ShellResult {
            exit_code: 0,
            stdout: input.to_string(),
            stderr: String::new(),
            duration_ms: None,
        })
    }
}

#[async_trait]
impl ShellSpec for HttpGetShell {
    fn name(&self) -> &'static str {
        self.name()
    }
    fn risk(&self) -> ShellRisk {
        ShellRisk::Medium
    }
    fn schema(&self) -> JsonValue {
        json!({
          "type":"object",
          "additionalProperties": false,
          "required":["url"],
          "properties": {
            "url": {"type":"string", "minLength": 1}
          }
        })
    }
    async fn execute(&self, _call: ShellCall, input: JsonValue) -> Result<ShellResult, LoreleiError> {
        Ok(ShellResult {
            exit_code: 0,
            stdout: input.to_string(),
            stderr: String::new(),
            duration_ms: None,
        })
    }
}

#[async_trait]
impl ShellSpec for DocumentIngestShell {
    fn name(&self) -> &'static str {
        self.name()
    }
    fn risk(&self) -> ShellRisk {
        ShellRisk::Medium
    }
    fn schema(&self) -> JsonValue {
        json!({
          "type":"object",
          "additionalProperties": false,
          "required":["source"],
          "properties": {
            "source": {"type":"string", "minLength": 1},
            "tags": {"type":"array", "items":{"type":"string"}}
          }
        })
    }
    async fn execute(&self, _call: ShellCall, input: JsonValue) -> Result<ShellResult, LoreleiError> {
        Ok(ShellResult {
            exit_code: 0,
            stdout: input.to_string(),
            stderr: String::new(),
            duration_ms: None,
        })
    }
}

#[async_trait]
impl ShellSpec for EchoShell {
    fn name(&self) -> &'static str {
        self.name()
    }
    fn risk(&self) -> ShellRisk {
        ShellRisk::Low
    }
    fn schema(&self) -> JsonValue {
        json!({
          "type":"object",
          "additionalProperties": false,
          "required":["text"],
          "properties": {
            "text": {"type":"string", "minLength": 1}
          }
        })
    }
    async fn execute(&self, _call: ShellCall, input: JsonValue) -> Result<ShellResult, LoreleiError> {
        let text = input.get("text").and_then(|v| v.as_str()).unwrap_or("");
        Ok(ShellResult {
            exit_code: 0,
            stdout: text.to_string(),
            stderr: String::new(),
            duration_ms: None,
        })
    }
}

#[async_trait]
impl ShellSpec for NoopShell {
    fn name(&self) -> &'static str {
        self.name()
    }
    fn risk(&self) -> ShellRisk {
        ShellRisk::Low
    }
    fn schema(&self) -> JsonValue {
        json!({"type":"object"})
    }
    async fn execute(&self, _call: ShellCall, _input: JsonValue) -> Result<ShellResult, LoreleiError> {
        Ok(ShellResult {
            exit_code: 0,
            stdout: "noop".to_string(),
            stderr: String::new(),
            duration_ms: None,
        })
    }
}
