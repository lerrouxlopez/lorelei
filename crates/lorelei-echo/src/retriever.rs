#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::{DocumentStore, EchoRetriever, LoreStore};
use lorelei_core::types::{
    AgentId, EchoHit, EchoQuery, EchoSources, Pearl, PearlId, PearlType, TenantId, UnitInterval,
};
use lorelei_lore::embedding::EmbeddingProvider;
use lorelei_lore::qdrant::QdrantPearlIndex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};
use uuid::Uuid;

pub struct EchoRetrievalConfig {
    pub rerank_top_k: usize,
    pub enable_query_rewrite: bool,
}

impl Default for EchoRetrievalConfig {
    fn default() -> Self {
        Self {
            rerank_top_k: 10,
            enable_query_rewrite: false,
        }
    }
}

#[async_trait::async_trait]
pub trait QueryRewriter: Send + Sync {
    async fn rewrite(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        query: &str,
    ) -> Result<String, LoreleiError>;
}

pub struct EchoEngine {
    store: Arc<dyn LoreStore>,
    documents: Option<Arc<dyn DocumentStore>>,
    index: QdrantPearlIndex,
    embedder: Arc<dyn EmbeddingProvider>,
    embedding_provider: String,
    cfg: EchoRetrievalConfig,
    rewriter: Option<Arc<dyn QueryRewriter>>,
}

impl EchoEngine {
    pub fn new(
        store: impl LoreStore + 'static,
        index: QdrantPearlIndex,
        embedder: Arc<dyn EmbeddingProvider>,
        embedding_provider: impl Into<String>,
        cfg: EchoRetrievalConfig,
    ) -> Self {
        Self {
            store: Arc::new(store),
            documents: None,
            index,
            embedder,
            embedding_provider: embedding_provider.into(),
            cfg,
            rewriter: None,
        }
    }

    pub fn with_documents(mut self, store: Arc<dyn DocumentStore>) -> Self {
        self.documents = Some(store);
        self
    }

    pub fn with_rewriter(mut self, rewriter: Arc<dyn QueryRewriter>) -> Self {
        self.rewriter = Some(rewriter);
        self
    }

    async fn effective_query(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        raw: &str,
    ) -> Result<String, LoreleiError> {
        if self.cfg.enable_query_rewrite {
            if let Some(r) = &self.rewriter {
                return r.rewrite(tenant_id, agent_id, raw).await;
            }
        }
        Ok(raw.to_string())
    }

    fn query_variants(raw: &str) -> Vec<String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return vec![];
        }
        let mut out = vec![raw.to_string()];
        // Simple keyword variants: lowercased and token-joined.
        let lower = raw.to_lowercase();
        if lower != raw {
            out.push(lower);
        }
        let tokens: Vec<&str> = raw.split_whitespace().collect();
        if tokens.len() > 1 {
            out.push(tokens.join(" "));
        }
        out.sort();
        out.dedup();
        out
    }

    fn recency_score(created_at: DateTime<Utc>) -> f64 {
        let age = Utc::now().signed_duration_since(created_at);
        let days = age.num_seconds().max(0) as f64 / 86_400.0;
        1.0 / (1.0 + days)
    }

    fn pearl_type_boost(pearl_type: PearlType, filter: Option<PearlType>) -> f64 {
        if let Some(f) = filter {
            if pearl_type == f {
                return 0.10;
            }
            return -0.05;
        }
        match pearl_type {
            PearlType::Fact => 0.08,
            PearlType::Preference => 0.02,
            PearlType::Skill => 0.04,
            PearlType::Plan => 0.01,
            PearlType::Other => 0.0,
        }
    }

    fn combined_score(
        vector_score: f32,
        pearl: &Pearl,
        pearl_type_filter: Option<PearlType>,
        duplicate_penalty: f64,
    ) -> Result<(UnitInterval, String), LoreleiError> {
        let v = vector_score.clamp(0.0, 1.0) as f64;
        let conf = f64::from(pearl.confidence);
        let imp = f64::from(pearl.importance);
        let rec = Self::recency_score(pearl.created_at);
        let type_boost = Self::pearl_type_boost(pearl.pearl_type, pearl_type_filter);

        let mut score = 0.55 * v + 0.15 * conf + 0.15 * imp + 0.15 * rec;
        score += type_boost;
        score -= duplicate_penalty;
        score = score.clamp(0.0, 1.0);

        let reason = format!(
            "v={:.3} conf={:.3} imp={:.3} rec={:.3} type={:+.3} dup={:+.3}",
            v, conf, imp, rec, type_boost, -duplicate_penalty
        );
        Ok((UnitInterval::new(score)?, reason))
    }

    async fn fetch_pearl(
        &self,
        tenant_id: TenantId,
        pearl_id: PearlId,
    ) -> Result<Option<Pearl>, LoreleiError> {
        self.store.get_pearl(tenant_id, pearl_id, false).await
    }
}

#[async_trait::async_trait]
impl EchoRetriever for EchoEngine {
    async fn query(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        query: EchoQuery,
    ) -> Result<Vec<EchoHit>, LoreleiError> {
        let started = Instant::now();
        let effective = self
            .effective_query(tenant_id, agent_id, &query.query)
            .await?;
        let variants = Self::query_variants(&effective);
        if variants.is_empty() || query.top_k == 0 {
            return Ok(Vec::new());
        }
        debug!(
            tenant_id = %tenant_id.0,
            agent_id = %agent_id.0,
            query_len = query.query.len(),
            variants = variants.len(),
            "echo.query_variants"
        );

        let emb = self
            .embedder
            .embed(tenant_id, &self.embedding_provider, variants.clone())
            .await?;
        if emb.vectors.is_empty() {
            return Ok(Vec::new());
        }

        // Collect hits across variants; keep max vector score per id.
        let mut best_vec: HashMap<PearlId, f32> = HashMap::new();
        let mut best_meta: HashMap<PearlId, (Option<Uuid>, Option<i32>, Option<String>)> =
            HashMap::new();
        for v in emb.vectors {
            let all_hits = match query.sources {
                EchoSources::Pearls => {
                    self.index
                        .search_pearl_vectors(tenant_id, v, query.top_k as u64, Some(agent_id))
                        .await?
                }
                EchoSources::Documents => {
                    self.index
                        .search_document_chunk_vectors(
                            tenant_id,
                            v,
                            query.top_k as u64,
                            Some(agent_id),
                        )
                        .await?
                }
                EchoSources::All => {
                    let mut p = self
                        .index
                        .search_pearl_vectors(
                            tenant_id,
                            v.clone(),
                            query.top_k as u64,
                            Some(agent_id),
                        )
                        .await?;
                    let mut d = self
                        .index
                        .search_document_chunk_vectors(
                            tenant_id,
                            v,
                            query.top_k as u64,
                            Some(agent_id),
                        )
                        .await?;
                    p.append(&mut d);
                    p
                }
            };
            for h in all_hits {
                best_vec
                    .entry(h.pearl_id)
                    .and_modify(|s| *s = (*s).max(h.score))
                    .or_insert(h.score);
                best_meta.insert(h.pearl_id, (h.document_id, h.chunk_index, h.title));
            }
        }

        if best_vec.is_empty() {
            info!(
                tenant_id = %tenant_id.0,
                agent_id = %agent_id.0,
                query_variants = variants.len(),
                candidates = 0usize,
                hits = 0usize,
                latency_ms = started.elapsed().as_millis() as u64,
                "echo.query"
            );
            return Ok(Vec::new());
        }

        // Fetch pearls (and/or document chunks) from Postgres (source of truth) and filter.
        let mut pearls: Vec<(Pearl, f32)> = Vec::new();
        let mut doc_chunks: Vec<(
            PearlId,
            f32,
            lorelei_core::types::EchoCitation,
            chrono::DateTime<chrono::Utc>,
            String,
        )> = Vec::new();
        let mut missing = Vec::new();
        let candidate_count = best_vec.len();
        for (id, vec_score) in best_vec {
            let meta = best_meta.get(&id).cloned().unwrap_or((None, None, None));
            let is_doc = meta.0.is_some();
            if matches!(query.sources, EchoSources::Documents)
                || (matches!(query.sources, EchoSources::All) && is_doc)
            {
                let Some(docs) = &self.documents else {
                    missing.push(id);
                    continue;
                };
                match docs.get_document_chunk_for_echo(tenant_id, id.0).await? {
                    Some((content, citation, created_at)) => {
                        doc_chunks.push((id, vec_score, citation, created_at, content));
                    }
                    None => missing.push(id),
                }
                continue;
            }

            match self.fetch_pearl(tenant_id, id).await? {
                Some(p) => {
                    if let Some(min_conf) = query.min_confidence {
                        if p.confidence.get() < min_conf.get() {
                            continue;
                        }
                    }
                    if let Some(t) = query.pearl_type {
                        if p.pearl_type != t {
                            continue;
                        }
                    }
                    pearls.push((p, vec_score));
                }
                None => missing.push(id),
            }
        }
        for id in missing {
            warn!(tenant_id = %tenant_id.0, pearl_id = %id.0, "echo.ignored_qdrant_only_hit");
        }

        // Deduplicate by identical content (lightweight).
        let mut seen_content: HashSet<String> = HashSet::new();
        let mut ranked: Vec<(EchoHit, f64)> = Vec::new();
        for (pearl, vec_score) in pearls {
            let dup = if seen_content.contains(&pearl.content) {
                0.15
            } else {
                0.0
            };
            let (score, reason) = Self::combined_score(vec_score, &pearl, query.pearl_type, dup)?;
            let hit = EchoHit {
                score,
                pearl_id: pearl.pearl_id,
                content: pearl.content.clone(),
                pearl_type: pearl.pearl_type,
                reason,
                created_at: pearl.created_at,
                citation: None,
            };
            let final_score = score.get();
            if seen_content.insert(pearl.content) {
                ranked.push((hit, final_score));
            } else {
                // keep duplicates out
            }
        }

        for (chunk_id, vec_score, citation, created_at, content) in doc_chunks {
            let dup = if seen_content.contains(&content) {
                0.15
            } else {
                0.0
            };
            let (score, reason) = Self::combined_score(
                vec_score,
                &Pearl {
                    pearl_id: chunk_id,
                    tenant_id,
                    agent_id,
                    pearl_type: PearlType::Other,
                    content: content.clone(),
                    importance: UnitInterval::new(0.5)?,
                    confidence: UnitInterval::new(1.0)?,
                    created_at,
                    metadata: Default::default(),
                },
                None,
                dup,
            )?;
            let hit = EchoHit {
                score,
                pearl_id: chunk_id,
                content: content.clone(),
                pearl_type: PearlType::Other,
                reason,
                created_at,
                citation: Some(citation),
            };
            let final_score = score.get();
            if seen_content.insert(content) {
                ranked.push((hit, final_score));
            }
        }

        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let take = query
            .top_k
            .min(self.cfg.rerank_top_k.max(1))
            .min(ranked.len());
        let out: Vec<EchoHit> = ranked.into_iter().take(take).map(|(h, _)| h).collect();
        info!(
            tenant_id = %tenant_id.0,
            agent_id = %agent_id.0,
            query_variants = variants.len(),
            candidates = candidate_count,
            hits = out.len(),
            latency_ms = started.elapsed().as_millis() as u64,
            "echo.query"
        );
        Ok(out)
    }
}
