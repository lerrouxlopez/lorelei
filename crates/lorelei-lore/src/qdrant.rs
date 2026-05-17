#![forbid(unsafe_code)]

use lorelei_core::error::LoreleiError;
use lorelei_core::types::{AgentId, Pearl, PearlId, PearlType, TenantId};
use qdrant_client::qdrant::{
    CreateCollectionBuilder, DeletePointsBuilder, Distance, Filter, PointId, PointStruct,
    SearchPointsBuilder, UpsertPointsBuilder, VectorParamsBuilder,
};
use qdrant_client::Qdrant;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Clone)]
pub struct QdrantPearlIndex {
    client: Qdrant,
    collection: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorHit {
    pub pearl_id: PearlId,
    pub score: f32,
    pub source_type: Option<String>,
    pub document_id: Option<Uuid>,
    pub chunk_index: Option<i32>,
    pub title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DocumentChunkVectorMeta {
    pub tenant_id: TenantId,
    pub agent_id: AgentId,
    pub document_id: Uuid,
    pub chunk_id: Uuid,
    pub chunk_index: i32,
    pub title: String,
}

impl QdrantPearlIndex {
    pub fn new(client: Qdrant, collection: impl Into<String>) -> Self {
        Self {
            client,
            collection: collection.into(),
        }
    }

    pub async fn ensure_collection(&self, vector_size: u64) -> Result<(), LoreleiError> {
        // Create if it does not exist; if it exists, leave it as-is.
        // If vector size mismatches, Qdrant will error on insert/search.
        let create = CreateCollectionBuilder::new(self.collection.clone())
            .vectors_config(VectorParamsBuilder::new(vector_size, Distance::Cosine));

        match self.client.create_collection(create).await {
            Ok(_) => Ok(()),
            Err(_) => Ok(()),
        }
    }

    pub async fn upsert_pearl_vector(
        &self,
        pearl: &Pearl,
        vector: Vec<f32>,
    ) -> Result<(), LoreleiError> {
        let mut payload = pearl_payload(pearl);
        payload.insert("source_type".to_string(), "pearl".into());
        let point = PointStruct::new(pearl_id_to_point_id(pearl.pearl_id), vector, payload);
        self.client
            .upsert_points(UpsertPointsBuilder::new(
                self.collection.clone(),
                vec![point],
            ))
            .await
            .map_err(|e| map_qdrant_error("upsert", &self.collection, e))?;
        Ok(())
    }

    pub async fn upsert_document_chunk_vector(
        &self,
        meta: DocumentChunkVectorMeta,
        vector: Vec<f32>,
    ) -> Result<(), LoreleiError> {
        let mut payload: HashMap<String, serde_json::Value> = HashMap::new();
        payload.insert("source_type".to_string(), "document_chunk".into());
        payload.insert("pearl_id".to_string(), meta.chunk_id.to_string().into());
        payload.insert("tenant_id".to_string(), meta.tenant_id.0.to_string().into());
        payload.insert("agent_id".to_string(), meta.agent_id.0.to_string().into());
        payload.insert(
            "document_id".to_string(),
            meta.document_id.to_string().into(),
        );
        payload.insert("chunk_index".to_string(), meta.chunk_index.into());
        payload.insert("title".to_string(), meta.title.into());

        let point_id = PointId {
            point_id_options: Some(qdrant_client::qdrant::point_id::PointIdOptions::Uuid(
                meta.chunk_id.to_string(),
            )),
        };
        let point = PointStruct::new(point_id, vector, payload);
        self.client
            .upsert_points(UpsertPointsBuilder::new(
                self.collection.clone(),
                vec![point],
            ))
            .await
            .map_err(|e| map_qdrant_error("upsert", &self.collection, e))?;
        Ok(())
    }

    pub async fn search_pearl_vectors(
        &self,
        tenant_id: TenantId,
        query_vector: Vec<f32>,
        top_k: u64,
        agent_id: Option<AgentId>,
    ) -> Result<Vec<VectorHit>, LoreleiError> {
        let dim = query_vector.len() as u64;
        if dim == 0 {
            return Ok(Vec::new());
        }
        self.ensure_collection(dim).await?;

        let filter = tenant_filter_with_source(tenant_id, agent_id, "pearl");
        let res = self
            .client
            .search_points(
                SearchPointsBuilder::new(self.collection.clone(), query_vector, top_k)
                    .with_payload(true)
                    .filter(filter),
            )
            .await
            .map_err(|e| map_qdrant_error("search", &self.collection, e))?;

        let mut out = Vec::with_capacity(res.result.len());
        for p in res.result {
            let Some(id) = p.id else { continue };
            let pearl_id = point_id_to_pearl_id(id)?;
            out.push(VectorHit {
                pearl_id,
                score: p.score,
                source_type: p
                    .payload
                    .get("source_type")
                    .and_then(|v| v.as_str().map(|s| s.to_string())),
                document_id: p
                    .payload
                    .get("document_id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok()),
                chunk_index: p
                    .payload
                    .get("chunk_index")
                    .and_then(|v| v.as_integer())
                    .map(|i| i as i32),
                title: p
                    .payload
                    .get("title")
                    .and_then(|v| v.as_str().map(|s| s.to_string())),
            });
        }
        Ok(out)
    }

    pub async fn search_document_chunk_vectors(
        &self,
        tenant_id: TenantId,
        query_vector: Vec<f32>,
        top_k: u64,
        agent_id: Option<AgentId>,
    ) -> Result<Vec<VectorHit>, LoreleiError> {
        let dim = query_vector.len() as u64;
        if dim == 0 {
            return Ok(Vec::new());
        }
        self.ensure_collection(dim).await?;

        let filter = tenant_filter_with_source(tenant_id, agent_id, "document_chunk");
        let res = self
            .client
            .search_points(
                SearchPointsBuilder::new(self.collection.clone(), query_vector, top_k)
                    .with_payload(true)
                    .filter(filter),
            )
            .await
            .map_err(|e| map_qdrant_error("search", &self.collection, e))?;

        let mut out = Vec::with_capacity(res.result.len());
        for p in res.result {
            let Some(id) = p.id else { continue };
            let chunk_id = point_id_to_pearl_id(id)?;
            out.push(VectorHit {
                pearl_id: chunk_id,
                score: p.score,
                source_type: p
                    .payload
                    .get("source_type")
                    .and_then(|v| v.as_str().map(|s| s.to_string())),
                document_id: p
                    .payload
                    .get("document_id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok()),
                chunk_index: p
                    .payload
                    .get("chunk_index")
                    .and_then(|v| v.as_integer())
                    .map(|i| i as i32),
                title: p
                    .payload
                    .get("title")
                    .and_then(|v| v.as_str().map(|s| s.to_string())),
            });
        }
        Ok(out)
    }

    pub async fn delete_pearl_vector(
        &self,
        tenant_id: TenantId,
        pearl_id: PearlId,
    ) -> Result<(), LoreleiError> {
        // Tenant-scoped delete via payload filter.
        let filter = Filter::must([
            qdrant_client::qdrant::Condition::matches("tenant_id", tenant_id.0.to_string()),
            qdrant_client::qdrant::Condition::matches("pearl_id", pearl_id.0.to_string()),
            qdrant_client::qdrant::Condition::matches("source_type", "pearl".to_string()),
        ]);

        self.client
            .delete_points(DeletePointsBuilder::new(self.collection.clone()).points(filter))
            .await
            .map_err(|e| map_qdrant_error("delete", &self.collection, e))?;
        Ok(())
    }

    pub async fn delete_document_vectors(
        &self,
        tenant_id: TenantId,
        document_id: Uuid,
    ) -> Result<(), LoreleiError> {
        let filter = Filter::must([
            qdrant_client::qdrant::Condition::matches("tenant_id", tenant_id.0.to_string()),
            qdrant_client::qdrant::Condition::matches("document_id", document_id.to_string()),
            qdrant_client::qdrant::Condition::matches("source_type", "document_chunk".to_string()),
        ]);
        self.client
            .delete_points(DeletePointsBuilder::new(self.collection.clone()).points(filter))
            .await
            .map_err(|e| map_qdrant_error("delete", &self.collection, e))?;
        Ok(())
    }
}

fn map_qdrant_error(op: &str, collection: &str, err: impl std::fmt::Display) -> LoreleiError {
    let msg = err.to_string();
    if msg.contains("Vector dimension error") {
        return LoreleiError::validation(
            "lore.collection",
            format!(
                "qdrant collection `{collection}` vector size mismatch. Set `lore.collection` to a new name (recommended) or wipe the Qdrant volume, then re-index."
            ),
        );
    }
    LoreleiError::Internal(format!("qdrant {op} failed: {msg}"))
}

fn tenant_filter_with_source(
    tenant_id: TenantId,
    agent_id: Option<AgentId>,
    source: &str,
) -> Filter {
    if let Some(agent) = agent_id {
        Filter::must([
            qdrant_client::qdrant::Condition::matches("tenant_id", tenant_id.0.to_string()),
            qdrant_client::qdrant::Condition::matches("agent_id", agent.0.to_string()),
            qdrant_client::qdrant::Condition::matches("source_type", source.to_string()),
        ])
    } else {
        Filter::must([
            qdrant_client::qdrant::Condition::matches("tenant_id", tenant_id.0.to_string()),
            qdrant_client::qdrant::Condition::matches("source_type", source.to_string()),
        ])
    }
}

fn pearl_id_to_point_id(pearl_id: PearlId) -> PointId {
    PointId {
        point_id_options: Some(qdrant_client::qdrant::point_id::PointIdOptions::Uuid(
            pearl_id.0.to_string(),
        )),
    }
}

fn point_id_to_pearl_id(id: PointId) -> Result<PearlId, LoreleiError> {
    let Some(opts) = id.point_id_options else {
        return Err(LoreleiError::Internal(
            "missing qdrant point id".to_string(),
        ));
    };
    let s = match opts {
        qdrant_client::qdrant::point_id::PointIdOptions::Uuid(u) => u,
        qdrant_client::qdrant::point_id::PointIdOptions::Num(_) => {
            return Err(LoreleiError::Internal(
                "unexpected numeric point id".to_string(),
            ));
        }
    };
    let u = Uuid::parse_str(&s)
        .map_err(|_| LoreleiError::Internal("invalid pearl_id in qdrant".to_string()))?;
    Ok(PearlId(u))
}

fn pearl_payload(pearl: &Pearl) -> HashMap<String, serde_json::Value> {
    let mut m = HashMap::new();
    m.insert("pearl_id".to_string(), pearl.pearl_id.0.to_string().into());
    m.insert(
        "tenant_id".to_string(),
        pearl.tenant_id.0.to_string().into(),
    );
    m.insert("agent_id".to_string(), pearl.agent_id.0.to_string().into());
    m.insert(
        "pearl_type".to_string(),
        pearl_type_to_str(pearl.pearl_type).into(),
    );
    m.insert("confidence".to_string(), f64::from(pearl.confidence).into());
    m.insert("importance".to_string(), f64::from(pearl.importance).into());
    let tags = pearl
        .metadata
        .get("tags")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    m.insert("tags".to_string(), serde_json::Value::Array(tags));
    m.insert(
        "created_at".to_string(),
        pearl.created_at.to_rfc3339().into(),
    );
    m
}

fn pearl_type_to_str(t: PearlType) -> &'static str {
    match t {
        PearlType::Fact => "fact",
        PearlType::Preference => "preference",
        PearlType::Skill => "skill",
        PearlType::Plan => "plan",
        PearlType::Other => "other",
    }
}
