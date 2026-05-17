#![forbid(unsafe_code)]

use crate::registry::{ShellSpec, ShellTool};
use async_trait::async_trait;
use lorelei_core::config::LoreleiConfig;
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::{DocumentStore, EchoRetriever, LoreStore};
use lorelei_core::types::{
    EchoQuery, EchoSources, NewPearl, PearlId, PearlListQuery, PearlType, ShellCall, ShellResult,
    ShellRisk, UnitInterval,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

pub fn builtin_tools(
    cfg: &LoreleiConfig,
    lore: Arc<dyn LoreStore>,
    echo: Arc<dyn EchoRetriever>,
    documents: Arc<dyn DocumentStore>,
) -> BTreeMap<String, Arc<dyn ShellTool>> {
    let mut out: BTreeMap<String, Arc<dyn ShellTool>> = BTreeMap::new();

    out.insert("noop".to_string(), Arc::new(NoopTool));
    out.insert("echo".to_string(), Arc::new(EchoTool));
    out.insert(
        "save_pearl".to_string(),
        Arc::new(SavePearlTool {
            lore: lore.clone(),
            cfg: cfg.clone(),
        }),
    );
    out.insert(
        "echo_lore".to_string(),
        Arc::new(EchoLoreTool {
            echo: echo.clone(),
            cfg: cfg.clone(),
        }),
    );
    out.insert(
        "list_pearls".to_string(),
        Arc::new(ListPearlsTool { lore: lore.clone() }),
    );
    out.insert(
        "forget_pearl".to_string(),
        Arc::new(ForgetPearlTool { lore }),
    );
    out.insert(
        "http_get".to_string(),
        Arc::new(HttpGetTool {
            cfg: cfg.clone(),
            http: reqwest::Client::builder()
                .user_agent("lorelei-shells/http_get")
                .build()
                .expect("http client"),
        }),
    );

    out.insert(
        "document_ingest".to_string(),
        Arc::new(DocumentIngestTool {
            documents: documents.clone(),
        }),
    );
    out.insert(
        "document_search".to_string(),
        Arc::new(DocumentSearchTool { echo }),
    );

    out
}

fn schema_for<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap_or(Value::Null)
}

fn decode_input<T: for<'de> Deserialize<'de>>(call: &ShellCall) -> Result<T, LoreleiError> {
    serde_json::from_value(call.input.clone()).map_err(|e| {
        LoreleiError::validation(
            "shell.input",
            format!("invalid JSON input for `{}`: {e}", call.tool),
        )
    })
}

fn ok(call_id: Uuid, output: Value) -> ShellResult {
    let now = chrono::Utc::now();
    ShellResult {
        call_id,
        ok: true,
        output,
        error: None,
        started_at: now,
        finished_at: now,
    }
}

fn err(call_id: Uuid, e: LoreleiError) -> ShellResult {
    let now = chrono::Utc::now();
    ShellResult {
        call_id,
        ok: false,
        output: Value::Null,
        error: Some(e.to_string()),
        started_at: now,
        finished_at: now,
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct NoopInput {
    value: Value,
}

struct NoopTool;

#[async_trait]
impl ShellTool for NoopTool {
    fn spec(&self) -> ShellSpec {
        ShellSpec {
            name: "noop",
            description: "Returns the provided input verbatim.",
            input_schema: schema_for::<NoopInput>(),
            risk: ShellRisk::Low,
        }
    }

    async fn execute(&self, call: ShellCall) -> Result<ShellResult, LoreleiError> {
        let input: NoopInput = decode_input(&call)?;
        Ok(ok(call.call_id, input.value))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EchoInput {
    message: String,
}

struct EchoTool;

#[async_trait]
impl ShellTool for EchoTool {
    fn spec(&self) -> ShellSpec {
        ShellSpec {
            name: "echo",
            description: "Returns a message.",
            input_schema: schema_for::<EchoInput>(),
            risk: ShellRisk::Low,
        }
    }

    async fn execute(&self, call: ShellCall) -> Result<ShellResult, LoreleiError> {
        let input: EchoInput = decode_input(&call)?;
        Ok(ok(call.call_id, json!({ "message": input.message })))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SavePearlInput {
    #[serde(default)]
    pearl_type: Option<String>,
    content: String,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    importance: Option<f64>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    metadata: BTreeMap<String, Value>,
}

struct SavePearlTool {
    lore: Arc<dyn LoreStore>,
    cfg: LoreleiConfig,
}

impl SavePearlTool {
    fn risk(&self) -> ShellRisk {
        // When the system is configured to require explicit approval for high-risk actions,
        // we treat memory writes as a low-risk operation. In "free-run" configurations, bump
        // it to medium to keep it visible in review tooling.
        if self.cfg.siren.require_approval_for_high_risk {
            ShellRisk::Low
        } else {
            ShellRisk::Medium
        }
    }
}

#[async_trait]
impl ShellTool for SavePearlTool {
    fn spec(&self) -> ShellSpec {
        ShellSpec {
            name: "save_pearl",
            description: "Save a Pearl to The Lore (tenant-scoped).",
            input_schema: schema_for::<SavePearlInput>(),
            risk: self.risk(),
        }
    }

    async fn execute(&self, call: ShellCall) -> Result<ShellResult, LoreleiError> {
        let input: SavePearlInput = decode_input(&call)?;
        if input.content.trim().is_empty() {
            return Ok(err(
                call.call_id,
                LoreleiError::validation("pearl.content", "must not be empty"),
            ));
        }

        let confidence = input.confidence.unwrap_or(0.8);
        let importance = input.importance.unwrap_or(0.5);
        let mut metadata = input.metadata;
        if !input.tags.is_empty() && !metadata.contains_key("tags") {
            metadata.insert(
                "tags".to_string(),
                serde_json::to_value(&input.tags).unwrap(),
            );
        }
        let new = NewPearl::new(
            input
                .pearl_type
                .as_deref()
                .map(parse_pearl_type)
                .transpose()?
                .unwrap_or(PearlType::Other),
            input.content,
            UnitInterval::new(importance)?,
            UnitInterval::new(confidence)?,
            metadata,
        )?;

        let pearl = self
            .lore
            .save_pearl(call.tenant_id, call.agent_id, new)
            .await?;

        Ok(ok(
            call.call_id,
            json!({
                "pearl_id": pearl.pearl_id.0,
                "tenant_id": pearl.tenant_id.0,
                "agent_id": pearl.agent_id.0,
                "pearl_type": pearl.pearl_type,
                "content": pearl.content,
                "confidence": pearl.confidence.get(),
                "importance": pearl.importance.get(),
                "tags": input.tags,
                "created_at": pearl.created_at.to_rfc3339(),
            }),
        ))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EchoLoreInput {
    query: String,
    #[serde(default)]
    top_k: Option<usize>,
    #[serde(default)]
    min_confidence: Option<f64>,
    #[serde(default)]
    pearl_type: Option<String>,
}

struct EchoLoreTool {
    echo: Arc<dyn EchoRetriever>,
    cfg: LoreleiConfig,
}

#[async_trait]
impl ShellTool for EchoLoreTool {
    fn spec(&self) -> ShellSpec {
        ShellSpec {
            name: "echo_lore",
            description: "Retrieve Pearls via Echo (tenant-scoped).",
            input_schema: schema_for::<EchoLoreInput>(),
            risk: ShellRisk::Low,
        }
    }

    async fn execute(&self, call: ShellCall) -> Result<ShellResult, LoreleiError> {
        let input: EchoLoreInput = decode_input(&call)?;
        let min_conf = match input.min_confidence {
            Some(v) => Some(UnitInterval::new(v)?),
            None => self.cfg.echo.min_confidence,
        };

        let hits = self
            .echo
            .query(
                call.tenant_id,
                call.agent_id,
                EchoQuery {
                    query: input.query,
                    top_k: input.top_k.unwrap_or(self.cfg.echo.top_k),
                    min_confidence: min_conf,
                    pearl_type: input
                        .pearl_type
                        .as_deref()
                        .map(parse_pearl_type)
                        .transpose()?,
                    sources: EchoSources::Pearls,
                },
            )
            .await?;

        Ok(ok(
            call.call_id,
            serde_json::to_value(hits).unwrap_or(Value::Null),
        ))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListPearlsInput {
    #[serde(default)]
    pearl_type: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    limit: Option<usize>,
}

struct ListPearlsTool {
    lore: Arc<dyn LoreStore>,
}

#[async_trait]
impl ShellTool for ListPearlsTool {
    fn spec(&self) -> ShellSpec {
        ShellSpec {
            name: "list_pearls",
            description: "List Pearls (tenant-scoped, read-only).",
            input_schema: schema_for::<ListPearlsInput>(),
            risk: ShellRisk::Low,
        }
    }

    async fn execute(&self, call: ShellCall) -> Result<ShellResult, LoreleiError> {
        let input: ListPearlsInput = decode_input(&call)?;
        let pearls = self
            .lore
            .list_pearls(
                call.tenant_id,
                PearlListQuery {
                    agent_id: Some(call.agent_id),
                    pearl_type: input
                        .pearl_type
                        .as_deref()
                        .map(parse_pearl_type)
                        .transpose()?,
                    tags: input.tags,
                    limit: input.limit,
                    include_deleted: false,
                },
            )
            .await?;
        Ok(ok(
            call.call_id,
            serde_json::to_value(pearls).unwrap_or(Value::Null),
        ))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ForgetPearlInput {
    pearl_id: Uuid,
}

struct ForgetPearlTool {
    lore: Arc<dyn LoreStore>,
}

#[async_trait]
impl ShellTool for ForgetPearlTool {
    fn spec(&self) -> ShellSpec {
        ShellSpec {
            name: "forget_pearl",
            description: "Soft-delete a Pearl (tenant-scoped).",
            input_schema: schema_for::<ForgetPearlInput>(),
            risk: ShellRisk::High,
        }
    }

    async fn execute(&self, call: ShellCall) -> Result<ShellResult, LoreleiError> {
        let input: ForgetPearlInput = decode_input(&call)?;
        self.lore
            .forget_pearl(call.tenant_id, PearlId(input.pearl_id))
            .await?;
        Ok(ok(call.call_id, json!({ "forgot": input.pearl_id })))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct HttpGetInput {
    url: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

struct HttpGetTool {
    cfg: LoreleiConfig,
    http: reqwest::Client,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DocumentIngestInput {
    path: String,
}

struct DocumentIngestTool {
    documents: Arc<dyn DocumentStore>,
}

#[async_trait]
impl ShellTool for DocumentIngestTool {
    fn spec(&self) -> ShellSpec {
        ShellSpec {
            name: "document_ingest",
            description: "Ingest a local text/Markdown document into The Reef (directory-limited).",
            input_schema: schema_for::<DocumentIngestInput>(),
            risk: ShellRisk::Medium,
        }
    }

    async fn execute(&self, call: ShellCall) -> Result<ShellResult, LoreleiError> {
        let input: DocumentIngestInput = decode_input(&call)?;
        let id = self
            .documents
            .ingest_document_path(
                call.tenant_id,
                call.agent_id,
                std::path::Path::new(&input.path),
            )
            .await?;
        Ok(ok(call.call_id, json!({ "document_id": id })))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DocumentSearchInput {
    query: String,
    #[serde(default)]
    top_k: Option<usize>,
}

struct DocumentSearchTool {
    echo: Arc<dyn EchoRetriever>,
}

#[async_trait]
impl ShellTool for DocumentSearchTool {
    fn spec(&self) -> ShellSpec {
        ShellSpec {
            name: "document_search",
            description: "Search ingested documents (tenant-scoped, read-only).",
            input_schema: schema_for::<DocumentSearchInput>(),
            risk: ShellRisk::Low,
        }
    }

    async fn execute(&self, call: ShellCall) -> Result<ShellResult, LoreleiError> {
        let input: DocumentSearchInput = decode_input(&call)?;
        let hits = self
            .echo
            .query(
                call.tenant_id,
                call.agent_id,
                EchoQuery {
                    query: input.query,
                    top_k: input.top_k.unwrap_or(10),
                    min_confidence: None,
                    pearl_type: None,
                    sources: EchoSources::Documents,
                },
            )
            .await?;
        Ok(ok(
            call.call_id,
            serde_json::to_value(hits).unwrap_or(Value::Null),
        ))
    }
}

fn parse_pearl_type(s: &str) -> Result<PearlType, LoreleiError> {
    let normalized = s.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "semantic" => Ok(PearlType::Fact),
        "fact" => Ok(PearlType::Fact),
        "preference" => Ok(PearlType::Preference),
        "skill" => Ok(PearlType::Skill),
        "plan" => Ok(PearlType::Plan),
        "other" => Ok(PearlType::Other),
        _ => Err(LoreleiError::validation(
            "pearl.pearl_type",
            "invalid pearl_type (try: semantic|fact|preference|skill|plan|other)",
        )),
    }
}

#[async_trait]
impl ShellTool for HttpGetTool {
    fn spec(&self) -> ShellSpec {
        ShellSpec {
            name: "http_get",
            description:
                "Perform an HTTP GET request (disabled unless siren.allow_network_tools=true).",
            input_schema: schema_for::<HttpGetInput>(),
            risk: ShellRisk::Medium,
        }
    }

    async fn execute(&self, call: ShellCall) -> Result<ShellResult, LoreleiError> {
        if !self.cfg.siren.allow_network_tools {
            return Err(LoreleiError::Unsupported(
                "http_get is disabled (siren.allow_network_tools=false)".to_string(),
            ));
        }
        let input: HttpGetInput = decode_input(&call)?;

        let mut req = self.http.get(&input.url);
        for (k, v) in input.headers {
            req = req.header(k, v);
        }

        let res = req
            .send()
            .await
            .map_err(|e| LoreleiError::Shell(format!("http_get request failed: {e}")))?;
        let status = res.status().as_u16();
        let text = res
            .text()
            .await
            .unwrap_or_else(|_| "<unreadable body>".to_string());

        // Avoid returning arbitrarily large outputs.
        let body = if text.len() > 8 * 1024 {
            text[..8 * 1024].to_string()
        } else {
            text
        };

        Ok(ok(call.call_id, json!({ "status": status, "body": body })))
    }
}
