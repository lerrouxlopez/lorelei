#![forbid(unsafe_code)]

use crate::embedding::EmbeddingProvider;
use crate::qdrant::QdrantPearlIndex;
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::DocumentStore;
use lorelei_core::types::{AgentId, EchoCitation, TenantId};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

pub struct PgDocumentStore {
    pool: PgPool,
    index: QdrantPearlIndex,
    embedder: Arc<dyn EmbeddingProvider>,
    embedding_provider: String,
    allowed_dirs: Vec<PathBuf>,
}

impl PgDocumentStore {
    pub fn new(
        pool: PgPool,
        index: QdrantPearlIndex,
        embedder: Arc<dyn EmbeddingProvider>,
        embedding_provider: impl Into<String>,
        allowed_dirs: Vec<PathBuf>,
    ) -> Self {
        Self {
            pool,
            index,
            embedder,
            embedding_provider: embedding_provider.into(),
            allowed_dirs,
        }
    }

    fn ensure_allowed(&self, path: &Path) -> Result<(), LoreleiError> {
        let canon = path
            .canonicalize()
            .map_err(|_| LoreleiError::validation("document.path", "path not found"))?;
        for dir in &self.allowed_dirs {
            if let Ok(d) = dir.canonicalize() {
                if canon.starts_with(&d) {
                    return Ok(());
                }
            }
        }
        Err(LoreleiError::validation(
            "document.path",
            "path not allowed (configure docs.allowed_dirs)",
        ))
    }

    fn mime_for_path(path: &Path) -> &'static str {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "md" | "markdown" => "text/markdown",
            _ => "text/plain",
        }
    }

    fn checksum_bytes(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        format!("{:x}", h.finalize())
    }

    fn token_estimate(text: &str) -> i32 {
        ((text.len() as f64) / 4.0).ceil().max(1.0) as i32
    }

    fn chunk_text(text: &str, max_tokens: i32) -> Vec<String> {
        let mut chunks = Vec::new();
        let mut cur = String::new();
        let mut cur_tokens = 0i32;

        for para in text.split("\n\n") {
            let p = para.trim();
            if p.is_empty() {
                continue;
            }
            let p_tokens = Self::token_estimate(p);
            if !cur.is_empty() && cur_tokens + p_tokens > max_tokens {
                chunks.push(cur.trim().to_string());
                cur.clear();
                cur_tokens = 0;
            }
            if !cur.is_empty() {
                cur.push_str("\n\n");
            }
            cur.push_str(p);
            cur_tokens += p_tokens;
        }
        if !cur.trim().is_empty() {
            chunks.push(cur.trim().to_string());
        }
        chunks
    }

    async fn upsert_chunks(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        document_id: Uuid,
        title: &str,
        chunks: &[String],
    ) -> Result<(), LoreleiError> {
        let vectors = self
            .embedder
            .embed(tenant_id, &self.embedding_provider, chunks.to_vec())
            .await?;

        let size = vectors.vectors.first().map(|v| v.len()).unwrap_or(0) as u64;
        if size == 0 {
            return Ok(());
        }
        self.index.ensure_collection(size).await?;

        for (idx, content) in chunks.iter().enumerate() {
            let chunk_id = Uuid::new_v4();
            let token_estimate = Self::token_estimate(content);

            sqlx::query(
                r#"
insert into document_chunks (id, document_id, tenant_id, agent_id, chunk_index, content, token_estimate)
values ($1,$2,$3,$4,$5,$6,$7)
"#,
            )
            .bind(chunk_id)
            .bind(document_id)
            .bind(tenant_id.0.to_string())
            .bind(agent_id.0.to_string())
            .bind(idx as i32)
            .bind(content)
            .bind(token_estimate)
            .execute(&self.pool)
            .await
            .map_err(|e| LoreleiError::Internal(format!("failed to insert document chunk: {e}")))?;

            if let Some(vec) = vectors.vectors.get(idx).cloned() {
                self.index
                    .upsert_document_chunk_vector(
                        crate::qdrant::DocumentChunkVectorMeta {
                            tenant_id,
                            agent_id,
                            document_id,
                            chunk_id,
                            chunk_index: idx as i32,
                            title: title.to_string(),
                        },
                        vec,
                    )
                    .await?;
            }
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl DocumentStore for PgDocumentStore {
    async fn ingest_document_path(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        path: &Path,
    ) -> Result<Uuid, LoreleiError> {
        self.ensure_allowed(path)?;
        let bytes = std::fs::read(path)
            .map_err(|_| LoreleiError::validation("document.path", "failed to read file"))?;
        let checksum = Self::checksum_bytes(&bytes);
        let mime = Self::mime_for_path(path).to_string();
        let title = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("document")
            .to_string();
        let source_uri = path.to_string_lossy().to_string();
        let text = String::from_utf8(bytes)
            .map_err(|_| LoreleiError::validation("document.content", "file must be UTF-8"))?;

        // Deduplicate by (tenant, agent, checksum) across active docs.
        if let Some(row) = sqlx::query(
            r#"
select id
from documents
where tenant_id = $1 and agent_id = $2 and checksum = $3 and deleted_at is null
limit 1
"#,
        )
        .bind(tenant_id.0.to_string())
        .bind(agent_id.0.to_string())
        .bind(&checksum)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| LoreleiError::Internal(format!("document lookup failed: {e}")))?
        {
            let id: Uuid = row.get("id");
            return Ok(id);
        }

        let document_id = Uuid::new_v4();
        sqlx::query(
            r#"
insert into documents (id, tenant_id, agent_id, title, source_uri, mime_type, checksum, created_at, deleted_at)
values ($1,$2,$3,$4,$5,$6,$7, now(), null)
"#,
        )
        .bind(document_id)
        .bind(tenant_id.0.to_string())
        .bind(agent_id.0.to_string())
        .bind(&title)
        .bind(&source_uri)
        .bind(&mime)
        .bind(&checksum)
        .execute(&self.pool)
        .await
        .map_err(|e| LoreleiError::Internal(format!("document insert failed: {e}")))?;

        let chunks = Self::chunk_text(&text, 400);
        self.upsert_chunks(tenant_id, agent_id, document_id, &title, &chunks)
            .await?;

        Ok(document_id)
    }

    async fn get_document_chunk_for_echo(
        &self,
        tenant_id: TenantId,
        chunk_id: Uuid,
    ) -> Result<Option<(String, EchoCitation, chrono::DateTime<chrono::Utc>)>, LoreleiError> {
        let row = sqlx::query(
            r#"
select c.content, c.chunk_index, c.created_at, d.title
from document_chunks c
join documents d on d.id = c.document_id
where c.id = $1 and c.tenant_id = $2 and d.deleted_at is null
limit 1
"#,
        )
        .bind(chunk_id)
        .bind(tenant_id.0.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| LoreleiError::Internal(format!("document chunk lookup failed: {e}")))?;

        let Some(row) = row else { return Ok(None) };
        let content: String = row.get("content");
        let chunk_index: i32 = row.get("chunk_index");
        let created_at: chrono::DateTime<chrono::Utc> = row.get("created_at");
        let title: String = row.get("title");
        Ok(Some((
            content,
            EchoCitation { title, chunk_index },
            created_at,
        )))
    }

    async fn soft_delete_document(
        &self,
        tenant_id: TenantId,
        document_id: Uuid,
    ) -> Result<(), LoreleiError> {
        let rows = sqlx::query(
            r#"
update documents
set deleted_at = now()
where id = $1 and tenant_id = $2 and deleted_at is null
"#,
        )
        .bind(document_id)
        .bind(tenant_id.0.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| LoreleiError::Internal(format!("document delete failed: {e}")))?
        .rows_affected();

        if rows == 0 {
            return Err(LoreleiError::NotFound("document not found".to_string()));
        }
        // Best-effort: remove vectors for this document.
        let _ = self
            .index
            .delete_document_vectors(tenant_id, document_id)
            .await;
        Ok(())
    }
}
