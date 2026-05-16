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
        let payload = pearl_payload(pearl);
        let point = PointStruct::new(pearl_id_to_point_id(pearl.pearl_id), vector, payload);
        self.client
            .upsert_points(UpsertPointsBuilder::new(
                self.collection.clone(),
                vec![point],
            ))
            .await
            .map_err(|e| LoreleiError::Internal(format!("qdrant upsert failed: {e}")))?;
        Ok(())
    }

    pub async fn search_pearl_vectors(
        &self,
        tenant_id: TenantId,
        query_vector: Vec<f32>,
        top_k: u64,
        agent_id: Option<AgentId>,
    ) -> Result<Vec<VectorHit>, LoreleiError> {
        let filter = tenant_filter(tenant_id, agent_id);
        let res = self
            .client
            .search_points(
                SearchPointsBuilder::new(self.collection.clone(), query_vector, top_k)
                    .with_payload(true)
                    .filter(filter),
            )
            .await
            .map_err(|e| LoreleiError::Internal(format!("qdrant search failed: {e}")))?;

        let mut out = Vec::with_capacity(res.result.len());
        for p in res.result {
            let Some(id) = p.id else { continue };
            let pearl_id = point_id_to_pearl_id(id)?;
            out.push(VectorHit {
                pearl_id,
                score: p.score,
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
        ]);

        self.client
            .delete_points(DeletePointsBuilder::new(self.collection.clone()).points(filter))
            .await
            .map_err(|e| LoreleiError::Internal(format!("qdrant delete failed: {e}")))?;
        Ok(())
    }
}

fn tenant_filter(tenant_id: TenantId, agent_id: Option<AgentId>) -> Filter {
    if let Some(agent) = agent_id {
        Filter::must([
            qdrant_client::qdrant::Condition::matches("tenant_id", tenant_id.0.to_string()),
            qdrant_client::qdrant::Condition::matches("agent_id", agent.0.to_string()),
        ])
    } else {
        Filter::must([qdrant_client::qdrant::Condition::matches(
            "tenant_id",
            tenant_id.0.to_string(),
        )])
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
