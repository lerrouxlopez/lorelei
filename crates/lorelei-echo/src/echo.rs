use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use lorelei_core::{
    ApiKeySource, Config, EchoHit, EchoQuery, EchoRetriever, LoreleiError, OpenAiCompatibleConfig,
    ProviderConfig,
};
use lorelei_lore::{LoreStores, PearlQueryFilter};
use reqwest::{header, StatusCode};
use serde_json::{json, Value as JsonValue};
use thiserror::Error;
use tokio::time::sleep;
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum LoreleiEchoError {
    #[error("config error: {0}")]
    Config(String),
    #[error("embedding error: {0}")]
    Embedding(String),
    #[error("retrieval error: {0}")]
    Retrieval(String),
}

impl From<LoreleiEchoError> for LoreleiError {
    fn from(value: LoreleiEchoError) -> Self {
        LoreleiError::EchoRetriever {
            message: value.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EchoConfig {
    pub qdrant_collection: String,
    /// Provider name (from `[providers.<name>]`) used for embeddings.
    pub embedding_provider: String,
    /// Embedding model override (optional).
    pub embedding_model: Option<String>,
    /// Optional LLM re-ranker (provider name). Not implemented yet; flag is accepted.
    pub llm_rerank: bool,
}

impl EchoConfig {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.qdrant_collection.trim().is_empty() {
            return Err(LoreleiError::validation("echo qdrant_collection must not be empty"));
        }
        if self.embedding_provider.trim().is_empty() {
            return Err(LoreleiError::validation("echo embedding_provider must not be empty"));
        }
        Ok(())
    }
}

pub struct EchoService {
    pub lore: LoreStores,
    embedder: Box<dyn Embedder>,
    cfg: EchoConfig,
}

impl EchoService {
    pub fn from_config(
        cfg: &Config,
        lore_pool_and_qdrant: LoreStores,
        echo: EchoConfig,
    ) -> Result<Self, LoreleiError> {
        echo.validate()?;

        let provider = cfg.providers.get(&echo.embedding_provider).ok_or_else(|| {
            LoreleiError::validation(format!(
                "echo.embedding_provider `{}` not found in [providers]",
                echo.embedding_provider
            ))
        })?;

        let embedder_cfg = match provider {
            ProviderConfig::OpenAiCompatible(p) => {
                let mut p = p.clone();
                if let Some(m) = &echo.embedding_model {
                    p.model = m.clone();
                }
                p
            }
            ProviderConfig::Local(p) => {
                let endpoint = p.endpoint.clone().ok_or_else(|| {
                    LoreleiError::validation("local provider requires `endpoint` for embeddings")
                })?;
                let model = echo
                    .embedding_model
                    .clone()
                    .or_else(|| p.model.clone())
                    .ok_or_else(|| LoreleiError::validation("local provider requires embedding_model or model"))?;
                OpenAiCompatibleConfig {
                    base_url: endpoint,
                    model,
                    api_key: ApiKeySource::Env {
                        var: "LORELEI_LOCAL_API_KEY".to_string(),
                    },
                    headers: Default::default(),
                }
            }
            other => {
                return Err(LoreleiError::validation(format!(
                    "embedding_provider kind `{:?}` not supported yet (use openai-compatible or local)",
                    other.kind()
                )));
            }
        };

        let embedder: Box<dyn Embedder> = Box::new(OpenAiCompatibleEmbedder::new(embedder_cfg)?);

        Ok(Self {
            lore: lore_pool_and_qdrant,
            embedder,
            cfg: echo,
        })
    }

    pub fn new_for_test(
        lore: LoreStores,
        cfg: EchoConfig,
        embedder: Box<dyn Embedder>,
    ) -> Result<Self, LoreleiError> {
        cfg.validate()?;
        Ok(Self { lore, embedder, cfg })
    }

    pub async fn retrieve(&self, q: EchoQuery) -> Result<Vec<EchoHit>, LoreleiError> {
        q.validate()?;
        let start = Instant::now();

        // 1) Rewrite user goal into retrieval queries (placeholder: single query).
        let queries = vec![q.text.clone()];

        // 2) Embed queries.
        let mut hits: Vec<EchoHit> = Vec::new();
        for text in queries {
            let vector = self.embedder.embed(&text).await?;

            // 3) Search Qdrant with tenant and agent filters.
            let tags_any: Vec<String> = Vec::new();
            let vhits = self
                .lore
                .search_pearl_vectors(
                    vector,
                    PearlQueryFilter {
                        tenant_id: q.tenant_id,
                        agent_id: q.agent_id,
                        pearl_type: q.pearl_type,
                        tags_any: &tags_any,
                        include_deleted: false,
                    },
                    q.limit as u32,
                )
                .await?;

            // 4) Fetch full Pearls from Postgres (source of truth), excluding deleted.
            for vh in vhits {
                let pearl = self.lore.get_pearl(q.tenant_id, vh.pearl_id, false).await?;
                let Some(pearl) = pearl else { continue };

                // 5) Heuristic rerank (confidence/importance/recency boost).
                if let Some(min_conf) = q.min_confidence {
                    if pearl.confidence.unwrap_or(0.0) < min_conf {
                        continue;
                    }
                }
                let score = heuristic_score(vh.score, pearl.created_at, pearl.confidence, pearl.importance);
                hits.push(EchoHit {
                    pearl_id: pearl.id,
                    score,
                    snippet: Some(truncate(&pearl.content, 240)),
                    metadata: json!({
                        "vector_score": vh.score,
                        "pearl_type": format!("{:?}", pearl.pearl_type).to_lowercase(),
                        "created_at": pearl.created_at,
                        "confidence": pearl.confidence,
                        "importance": pearl.importance,
                        "tags": pearl.tags,
                    }),
                });
            }
        }

        // 6) Optional LLM reranking behind config flag (not implemented yet).
        if self.cfg.llm_rerank {
            // TODO: implement reranking via lorelei-song provider behind config.
        }

        // 7) Return EchoHit list.
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(q.limit);

        info!(
            tenant_id = %q.tenant_id,
            agent_id = ?q.agent_id,
            echo_query_count = 1u64,
            echo_hit_count = hits.len() as u64,
            latency_ms = start.elapsed().as_millis() as u64,
            "echo.retrieve"
        );
        Ok(hits)
    }
}

#[async_trait]
impl EchoRetriever for EchoService {
    async fn query(&self, query: EchoQuery) -> Result<Vec<EchoHit>, LoreleiError> {
        self.retrieve(query).await
    }
}

fn heuristic_score(base: f32, created_at: DateTime<Utc>, confidence: Option<f64>, importance: Option<f64>) -> f32 {
    let now = Utc::now();
    let age_days = (now - created_at).num_seconds().max(0) as f32 / 86_400.0;
    let recency = (1.0 / (1.0 + age_days)).clamp(0.0, 1.0); // newer => closer to 1
    let conf = confidence.unwrap_or(0.0).clamp(0.0, 1.0) as f32;
    let imp = importance.unwrap_or(0.0).clamp(0.0, 1.0) as f32;
    base + 0.15 * recency + 0.15 * conf + 0.10 * imp
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut out = s[..max].to_string();
    out.push_str("…");
    out
}

#[derive(Clone)]
struct OpenAiCompatibleEmbedder {
    cfg: OpenAiCompatibleConfig,
    client: reqwest::Client,
}

#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, LoreleiError>;
}

impl OpenAiCompatibleEmbedder {
    fn new(cfg: OpenAiCompatibleConfig) -> Result<Self, LoreleiError> {
        cfg.validate()?;
        let client = reqwest::Client::builder()
            .user_agent("lorelei/0.1")
            .build()
            .map_err(|e| LoreleiError::EchoRetriever {
                message: format!("reqwest client init failed: {e}"),
            })?;
        Ok(Self { cfg, client })
    }

    fn auth_header(&self) -> Result<header::HeaderValue, LoreleiError> {
        let key = self.cfg.api_key.resolve()?;
        let v = format!("Bearer {}", key.expose());
        header::HeaderValue::from_str(&v).map_err(|e| LoreleiError::EchoRetriever {
            message: format!("invalid auth header: {e}"),
        })
    }

    fn base_url(&self, path: &str) -> String {
        format!("{}/{}", self.cfg.base_url.trim_end_matches('/'), path.trim_start_matches('/'))
    }

    async fn request_with_retry(&self, body: JsonValue) -> Result<JsonValue, LoreleiError> {
        let url = self.base_url("/embeddings");
        let auth = self.auth_header()?;
        let mut attempt = 0u32;
        let mut delay = Duration::from_millis(200);
        loop {
            let start = Instant::now();
            let mut b = self
                .client
                .post(url.clone())
                .header(header::AUTHORIZATION, auth.clone())
                .json(&body);
            for (k, v) in &self.cfg.headers {
                b = b.header(k, v);
            }
            let resp = b.send().await.map_err(|e| LoreleiError::EchoRetriever {
                message: format!("embedding request failed: {e}"),
            })?;

            if resp.status() == StatusCode::TOO_MANY_REQUESTS
                || resp.status().is_server_error()
                || resp.status() == StatusCode::REQUEST_TIMEOUT
            {
                if attempt >= 5 {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    return Err(LoreleiError::EchoRetriever {
                        message: format!("embedding failed after retries: HTTP {status}: {}", truncate(&text, 400)),
                    });
                }
                attempt += 1;
                sleep(delay).await;
                delay = std::cmp::min(delay * 2, Duration::from_secs(5));
                continue;
            }

            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                warn!(
                    model = %self.cfg.model,
                    status = %status,
                    latency_ms = start.elapsed().as_millis() as u64,
                    "echo.embedding_http_error"
                );
                return Err(LoreleiError::EchoRetriever {
                    message: format!("embedding failed: HTTP {status}: {}", truncate(&text, 400)),
                });
            }

            info!(
                model = %self.cfg.model,
                latency_ms = start.elapsed().as_millis() as u64,
                "echo.embedding_request"
            );
            return resp.json().await.map_err(|e| LoreleiError::EchoRetriever {
                message: format!("embedding invalid JSON: {e}"),
            });
        }
    }

    async fn embed_inner(&self, text: &str) -> Result<Vec<f32>, LoreleiError> {
        if text.trim().is_empty() {
            return Err(LoreleiError::validation("embed text must not be empty"));
        }
        let body = json!({
            "model": self.cfg.model,
            "input": text,
        });
        let v = self.request_with_retry(body).await?;
        let vec = v
            .get("data")
            .and_then(|d| d.get(0))
            .and_then(|d| d.get("embedding"))
            .and_then(|e| e.as_array())
            .ok_or_else(|| LoreleiError::EchoRetriever {
                message: "embedding response missing data[0].embedding".to_string(),
            })?
            .iter()
            .filter_map(|n| n.as_f64())
            .map(|f| f as f32)
            .collect::<Vec<_>>();

        if vec.is_empty() {
            return Err(LoreleiError::EchoRetriever {
                message: "embedding vector empty".to_string(),
            });
        }
        Ok(vec)
    }
}

#[async_trait]
impl Embedder for OpenAiCompatibleEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, LoreleiError> {
        self.embed_inner(text).await
    }
}
