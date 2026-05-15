use std::collections::{BTreeMap, HashMap};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use lorelei_core::{CurrentEvent, CurrentStore, LoreleiError, LoreStore, NewPearl, Pearl, PearlType};
use qdrant_client::qdrant::{
    condition::ConditionOneOf, value::Kind as QdrantValueKind, vectors_config::Config,
    with_payload_selector::SelectorOptions, Condition, FieldCondition, Filter, Match, PointId,
    MinShould, PointsSelector, ScoredPoint, SearchPoints, UpsertPoints, Value as QdrantValue,
    VectorsConfig,
};
use qdrant_client::Qdrant;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoreConfig {
    pub qdrant_collection: String,
}

impl LoreConfig {
    pub fn validate(&self) -> Result<(), LoreleiError> {
        if self.qdrant_collection.trim().is_empty() {
            return Err(LoreleiError::validation("qdrant_collection must not be empty"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PearlQueryFilter<'a> {
    pub tenant_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub pearl_type: Option<PearlType>,
    pub tags_any: &'a [String],
    pub include_deleted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorHit {
    pub pearl_id: Uuid,
    pub score: f32,
    pub payload: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone)]
pub struct LoreStores {
    pub pool: PgPool,
    pub qdrant: Qdrant,
    pub config: LoreConfig,
}

fn validate_01(name: &str, value: Option<f64>) -> Result<(), LoreleiError> {
    if let Some(v) = value {
        if !v.is_finite() || v < 0.0 || v > 1.0 {
            return Err(LoreleiError::validation(format!("{name} must be between 0 and 1")));
        }
    }
    Ok(())
}

impl LoreStores {
    pub fn new(pool: PgPool, qdrant: Qdrant, config: LoreConfig) -> Result<Self, LoreleiError> {
        config.validate()?;
        Ok(Self { pool, qdrant, config })
    }

    pub async fn create_run(
        &self,
        tenant_id: Uuid,
        metadata: serde_json::Value,
    ) -> Result<RunRow, LoreleiError> {
        let id = Uuid::new_v4();
        let started_at = Utc::now();

        sqlx::query(
            r#"
            INSERT INTO runs (id, tenant_id, started_at, ended_at, metadata)
            VALUES ($1, $2, $3, NULL, $4)
            "#,
        )
        .bind(id)
        .bind(tenant_id)
        .bind(started_at)
        .bind(sqlx::types::Json(metadata.clone()))
        .execute(&self.pool)
        .await
        .map_err(|e| LoreleiError::LoreStore {
            message: format!("create_run failed: {e}"),
        })?;

        Ok(RunRow {
            id,
            tenant_id,
            started_at,
            ended_at: None,
            metadata,
        })
    }

    pub async fn complete_run(
        &self,
        tenant_id: Uuid,
        run_id: Uuid,
        ended_at: DateTime<Utc>,
    ) -> Result<(), LoreleiError> {
        let res = sqlx::query(
            r#"
            UPDATE runs
            SET ended_at = $1
            WHERE tenant_id = $2 AND id = $3
            "#,
        )
        .bind(ended_at)
        .bind(tenant_id)
        .bind(run_id)
        .execute(&self.pool)
        .await
        .map_err(|e| LoreleiError::LoreStore {
            message: format!("complete_run failed: {e}"),
        })?;

        if res.rows_affected() == 0 {
            return Err(LoreleiError::validation("run not found for tenant"));
        }
        Ok(())
    }

    pub async fn write_current(
        &self,
        tenant_id: Uuid,
        event: CurrentEvent,
        confidence: Option<f64>,
        importance: Option<f64>,
    ) -> Result<(), LoreleiError> {
        event.validate()?;
        validate_01("confidence", confidence)?;
        validate_01("importance", importance)?;

        sqlx::query(
            r#"
            INSERT INTO currents (id, tenant_id, run_id, kind, content, confidence, importance, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(event.id)
        .bind(tenant_id)
        .bind(event.run_id)
        .bind(event.kind)
        .bind(sqlx::types::Json(event.payload))
        .bind(confidence)
        .bind(importance)
        .bind(event.at)
        .execute(&self.pool)
        .await
        .map_err(|e| LoreleiError::CurrentStore {
            message: format!("write_current failed: {e}"),
        })?;

        Ok(())
    }

    pub async fn list_currents(
        &self,
        tenant_id: Uuid,
        run_id: Uuid,
    ) -> Result<Vec<CurrentEvent>, LoreleiError> {
        let rows = sqlx::query(
            r#"
            SELECT id, run_id, created_at, kind, content
            FROM currents
            WHERE tenant_id = $1 AND run_id = $2
            ORDER BY created_at ASC
            "#,
        )
        .bind(tenant_id)
        .bind(run_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| LoreleiError::CurrentStore {
            message: format!("list_currents failed: {e}"),
        })?;

        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let id: Uuid = r.get("id");
            let run_id: Uuid = r.get("run_id");
            let at: DateTime<Utc> = r.get("created_at");
            let kind: String = r.get("kind");
            let payload: sqlx::types::Json<serde_json::Value> = r.get("content");
            out.push(CurrentEvent {
                id,
                run_id,
                at,
                kind,
                payload: payload.0,
            });
        }
        Ok(out)
    }

    pub async fn save_pearl(
        &self,
        tenant_id: Uuid,
        agent_id: Option<Uuid>,
        run_id: Uuid,
        pearl: NewPearl,
    ) -> Result<Pearl, LoreleiError> {
        pearl.validate()?;

        let id = Uuid::new_v4();
        let created_at = Utc::now();

        sqlx::query(
            r#"
            INSERT INTO pearls (
              id, tenant_id, agent_id, run_id, pearl_type, content,
              confidence, importance, tags, metadata, created_at, deleted_at, last_echoed_at
            )
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,NULL,NULL)
            "#,
        )
        .bind(id)
        .bind(tenant_id)
        .bind(agent_id)
        .bind(run_id)
        .bind(pearl_type_to_db(pearl.pearl_type))
        .bind(pearl.content.clone())
        .bind(pearl.confidence)
        .bind(pearl.importance)
        .bind(&pearl.tags)
        .bind(sqlx::types::Json(pearl.metadata.clone()))
        .bind(created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| LoreleiError::LoreStore {
            message: format!("save_pearl failed: {e}"),
        })?;

        Ok(Pearl {
            id,
            tenant_id,
            agent_id,
            run_id,
            pearl_type: pearl.pearl_type,
            created_at,
            deleted_at: None,
            last_echoed_at: None,
            content: pearl.content,
            tags: pearl.tags,
            confidence: pearl.confidence,
            importance: pearl.importance,
            metadata: pearl.metadata,
        })
    }

    pub async fn get_pearl(
        &self,
        tenant_id: Uuid,
        pearl_id: Uuid,
        include_deleted: bool,
    ) -> Result<Option<Pearl>, LoreleiError> {
        let row = if include_deleted {
            sqlx::query(
                r#"
                SELECT id, tenant_id, agent_id, run_id, pearl_type, content,
                       confidence, importance, tags, created_at, deleted_at, last_echoed_at, metadata
                FROM pearls
                WHERE tenant_id = $1 AND id = $2
                "#,
            )
            .bind(tenant_id)
            .bind(pearl_id)
            .fetch_optional(&self.pool)
            .await
        } else {
            sqlx::query(
                r#"
                SELECT id, tenant_id, agent_id, run_id, pearl_type, content,
                       confidence, importance, tags, created_at, deleted_at, last_echoed_at, metadata
                FROM pearls
                WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL
                "#,
            )
            .bind(tenant_id)
            .bind(pearl_id)
            .fetch_optional(&self.pool)
            .await
        }
        .map_err(|e| LoreleiError::LoreStore {
            message: format!("get_pearl failed: {e}"),
        })?;

        Ok(row.map(row_to_pearl))
    }

    pub async fn forget_pearl(&self, tenant_id: Uuid, pearl_id: Uuid) -> Result<(), LoreleiError> {
        let res = sqlx::query(
            r#"
            UPDATE pearls
            SET deleted_at = now()
            WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL
            "#,
        )
        .bind(tenant_id)
        .bind(pearl_id)
        .execute(&self.pool)
        .await
        .map_err(|e| LoreleiError::LoreStore {
            message: format!("forget_pearl failed: {e}"),
        })?;

        if res.rows_affected() == 0 {
            return Err(LoreleiError::validation("pearl not found (or already deleted)"));
        }
        Ok(())
    }

    pub async fn update_last_echoed_at(
        &self,
        tenant_id: Uuid,
        pearl_id: Uuid,
        at: DateTime<Utc>,
    ) -> Result<(), LoreleiError> {
        let res = sqlx::query(
            r#"
            UPDATE pearls
            SET last_echoed_at = $1
            WHERE tenant_id = $2 AND id = $3 AND deleted_at IS NULL
            "#,
        )
        .bind(at)
        .bind(tenant_id)
        .bind(pearl_id)
        .execute(&self.pool)
        .await
        .map_err(|e| LoreleiError::LoreStore {
            message: format!("update_last_echoed_at failed: {e}"),
        })?;

        if res.rows_affected() == 0 {
            return Err(LoreleiError::validation("pearl not found (or deleted)"));
        }
        Ok(())
    }

    pub async fn upsert_pearl_vector(
        &self,
        pearl: &Pearl,
        vector: Vec<f32>,
    ) -> Result<(), LoreleiError> {
        if vector.is_empty() {
            return Err(LoreleiError::validation("vector must not be empty"));
        }
        self.ensure_collection(vector.len() as u64).await?;

        let payload = pearl_to_qdrant_payload(pearl)?;
        let point_id = PointId::from(pearl.id.to_string());

        self.qdrant
            .upsert_points(UpsertPoints {
                collection_name: self.config.qdrant_collection.clone(),
                wait: Some(true),
                points: vec![qdrant_client::qdrant::PointStruct {
                    id: Some(point_id),
                    vectors: Some(vector.into()),
                    payload,
                }],
                ..Default::default()
            })
            .await
            .map_err(|e| LoreleiError::LoreStore {
                message: format!("qdrant upsert failed: {e}"),
            })?;
        Ok(())
    }

    pub async fn search_pearl_vectors(
        &self,
        vector: Vec<f32>,
        filter: PearlQueryFilter<'_>,
        limit: u32,
    ) -> Result<Vec<VectorHit>, LoreleiError> {
        if vector.is_empty() {
            return Err(LoreleiError::validation("vector must not be empty"));
        }
        if limit == 0 || limit > 1_000 {
            return Err(LoreleiError::validation("limit must be between 1 and 1000"));
        }
        self.ensure_collection(vector.len() as u64).await?;

        let qfilter = build_qdrant_filter(filter)?;

        let resp = self
            .qdrant
            .search_points(SearchPoints {
                collection_name: self.config.qdrant_collection.clone(),
                vector,
                filter: Some(qfilter),
                limit: limit.into(),
                with_payload: Some(qdrant_client::qdrant::WithPayloadSelector {
                    selector_options: Some(SelectorOptions::Enable(true)),
                }),
                ..Default::default()
            })
            .await
            .map_err(|e| LoreleiError::LoreStore {
                message: format!("qdrant search failed: {e}"),
            })?;

        Ok(resp
            .result
            .into_iter()
            .filter_map(scored_point_to_hit)
            .collect())
    }

    pub async fn delete_pearl_vector(
        &self,
        pearl_id: Uuid,
    ) -> Result<(), LoreleiError> {
        self.qdrant
            .delete_points(qdrant_client::qdrant::DeletePoints {
                collection_name: self.config.qdrant_collection.clone(),
                wait: Some(true),
                points: Some(PointsSelector {
                    points_selector_one_of: Some(qdrant_client::qdrant::points_selector::PointsSelectorOneOf::Points(
                        qdrant_client::qdrant::PointsIdsList {
                            ids: vec![PointId::from(pearl_id.to_string())],
                        },
                    )),
                }),
                ..Default::default()
            })
            .await
            .map_err(|e| LoreleiError::LoreStore {
                message: format!("qdrant delete failed: {e}"),
            })?;
        Ok(())
    }

    async fn ensure_collection(&self, vector_size: u64) -> Result<(), LoreleiError> {
        // Best-effort: try to create. If it already exists, Qdrant returns an error; ignore that specific case.
        let create = qdrant_client::qdrant::CreateCollection {
            collection_name: self.config.qdrant_collection.clone(),
            vectors_config: Some(VectorsConfig {
                config: Some(Config::Params(qdrant_client::qdrant::VectorParams {
                    size: vector_size,
                    distance: qdrant_client::qdrant::Distance::Cosine.into(),
                    ..Default::default()
                })),
            }),
            ..Default::default()
        };

        match self.qdrant.create_collection(create).await {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if msg.to_lowercase().contains("already exists") {
                    Ok(())
                } else {
                    Err(LoreleiError::LoreStore {
                        message: format!("qdrant create_collection failed: {e}"),
                    })
                }
            }
        }
    }
}

fn pearl_type_to_db(p: PearlType) -> &'static str {
    match p {
        PearlType::Memory => "memory",
        PearlType::Note => "note",
        PearlType::Insight => "insight",
    }
}

fn pearl_type_from_db(s: &str) -> Result<PearlType, LoreleiError> {
    match s {
        "memory" => Ok(PearlType::Memory),
        "note" => Ok(PearlType::Note),
        "insight" => Ok(PearlType::Insight),
        _ => Err(LoreleiError::validation(format!(
            "unknown pearl_type `{s}`"
        ))),
    }
}

fn row_to_pearl(row: sqlx::postgres::PgRow) -> Pearl {
    let pearl_type: String = row.get("pearl_type");
    let pearl_type = pearl_type_from_db(&pearl_type).unwrap_or(PearlType::Note);
    let tags: Vec<String> = row.get("tags");
    let metadata: sqlx::types::Json<serde_json::Value> = row.get("metadata");
    Pearl {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        agent_id: row.get("agent_id"),
        run_id: row.get("run_id"),
        pearl_type,
        content: row.get("content"),
        confidence: row.get("confidence"),
        importance: row.get("importance"),
        tags,
        created_at: row.get("created_at"),
        deleted_at: row.get("deleted_at"),
        last_echoed_at: row.get("last_echoed_at"),
        metadata: metadata.0,
    }
}

fn pearl_to_qdrant_payload(
    pearl: &Pearl,
) -> Result<HashMap<String, QdrantValue>, LoreleiError> {
    let agent_id = pearl
        .agent_id
        .map(|v| v.to_string())
        .unwrap_or_else(|| "".to_string());

    let mut payload = HashMap::new();
    payload.insert("pearl_id".to_string(), qval_str(pearl.id.to_string()));
    payload.insert(
        "tenant_id".to_string(),
        qval_str(pearl.tenant_id.to_string()),
    );
    payload.insert("agent_id".to_string(), qval_str(agent_id));
    payload.insert("pearl_type".to_string(), qval_str(pearl_type_to_db(pearl.pearl_type)));
    if let Some(c) = pearl.confidence {
        payload.insert("confidence".to_string(), qval_f64(c)?);
    }
    if let Some(i) = pearl.importance {
        payload.insert("importance".to_string(), qval_f64(i)?);
    }
    payload.insert(
        "tags".to_string(),
        QdrantValue {
            kind: Some(QdrantValueKind::ListValue(qdrant_client::qdrant::ListValue {
                values: pearl.tags.iter().cloned().map(qval_str).collect(),
            })),
        },
    );
    payload.insert(
        "created_at".to_string(),
        qval_str(pearl.created_at.to_rfc3339()),
    );
    Ok(payload)
}

fn qval_str(s: impl Into<String>) -> QdrantValue {
    QdrantValue {
        kind: Some(QdrantValueKind::StringValue(s.into())),
    }
}

fn qval_f64(v: f64) -> Result<QdrantValue, LoreleiError> {
    if !v.is_finite() {
        return Err(LoreleiError::validation("payload float must be finite"));
    }
    Ok(QdrantValue {
        kind: Some(QdrantValueKind::DoubleValue(v)),
    })
}

fn build_qdrant_filter(filter: PearlQueryFilter<'_>) -> Result<Filter, LoreleiError> {
    let mut must: Vec<Condition> = Vec::new();
    let must_not: Vec<Condition> = Vec::new();
    let should: Vec<Condition> = Vec::new();
    let mut min_should: Option<MinShould> = None;
    must.push(eq_condition("tenant_id", filter.tenant_id.to_string()));
    if let Some(agent_id) = filter.agent_id {
        must.push(eq_condition("agent_id", agent_id.to_string()));
    }
    if let Some(pearl_type) = filter.pearl_type {
        must.push(eq_condition("pearl_type", pearl_type_to_db(pearl_type).to_string()));
    }
    if !filter.tags_any.is_empty() {
        // "tags" is a list payload; require at least one match.
        let any = filter
            .tags_any
            .iter()
            .map(|t| eq_condition("tags", t.clone()))
            .collect::<Vec<_>>();
        min_should = Some(MinShould {
            conditions: any,
            min_count: 1,
        });
    }
    if !filter.include_deleted {
        // deleted_at isn't stored in Qdrant; source-of-truth filtering must happen via Postgres fetch.
        // Keep Qdrant filter minimal here.
    }
    Ok(Filter {
        should,
        must,
        must_not,
        min_should,
        ..Default::default()
    })
}

fn eq_condition(field: &str, value: String) -> Condition {
    Condition {
        condition_one_of: Some(ConditionOneOf::Field(FieldCondition {
            key: field.to_string(),
            r#match: Some(Match {
                match_value: Some(qdrant_client::qdrant::r#match::MatchValue::Keyword(value)),
            }),
            ..Default::default()
        })),
    }
}

fn scored_point_to_hit(p: ScoredPoint) -> Option<VectorHit> {
    let point_id = p.id?;
    let id_str = match point_id.point_id_options? {
        qdrant_client::qdrant::point_id::PointIdOptions::Uuid(u) => u,
        qdrant_client::qdrant::point_id::PointIdOptions::Num(_) => return None,
    };
    let pearl_id = Uuid::parse_str(&id_str).ok()?;
    let payload = p
        .payload
        .into_iter()
        .map(|(k, v)| (k, qdrant_value_to_json(v)))
        .collect::<BTreeMap<_, _>>();

    Some(VectorHit {
        pearl_id,
        score: p.score,
        payload,
    })
}

fn qdrant_value_to_json(v: QdrantValue) -> serde_json::Value {
    match v.kind {
        Some(QdrantValueKind::NullValue(_)) => serde_json::Value::Null,
        Some(QdrantValueKind::BoolValue(b)) => serde_json::Value::Bool(b),
        Some(QdrantValueKind::IntegerValue(i)) => serde_json::Value::Number(i.into()),
        Some(QdrantValueKind::DoubleValue(f)) => json!(f),
        Some(QdrantValueKind::StringValue(s)) => serde_json::Value::String(s),
        Some(QdrantValueKind::ListValue(list)) => serde_json::Value::Array(
            list.values.into_iter().map(qdrant_value_to_json).collect(),
        ),
        Some(QdrantValueKind::StructValue(st)) => serde_json::Value::Object(
            st.fields
                .into_iter()
                .map(|(k, v)| (k, qdrant_value_to_json(v)))
                .collect(),
        ),
        None => serde_json::Value::Null,
    }
}

// Trait implementations: Postgres is the source of truth. These use tenant_id from the caller's event/run context.

#[async_trait]
impl CurrentStore for LoreStores {
    async fn write_current(
        &self,
        tenant_id: Uuid,
        event: CurrentEvent,
        confidence: Option<f64>,
        importance: Option<f64>,
    ) -> Result<(), LoreleiError> {
        LoreStores::write_current(self, tenant_id, event, confidence, importance).await
    }

    async fn list_currents(
        &self,
        tenant_id: Uuid,
        run_id: Uuid,
    ) -> Result<Vec<CurrentEvent>, LoreleiError> {
        LoreStores::list_currents(self, tenant_id, run_id).await
    }
}

#[async_trait]
impl LoreStore for LoreStores {
    async fn save_pearl(
        &self,
        tenant_id: Uuid,
        agent_id: Option<Uuid>,
        run_id: Uuid,
        pearl: NewPearl,
    ) -> Result<Pearl, LoreleiError> {
        LoreStores::save_pearl(self, tenant_id, agent_id, run_id, pearl).await
    }

    async fn get_pearl(
        &self,
        tenant_id: Uuid,
        id: Uuid,
        include_deleted: bool,
    ) -> Result<Option<Pearl>, LoreleiError> {
        LoreStores::get_pearl(self, tenant_id, id, include_deleted).await
    }

    async fn forget_pearl(&self, tenant_id: Uuid, id: Uuid) -> Result<(), LoreleiError> {
        LoreStores::forget_pearl(self, tenant_id, id).await
    }

    async fn update_last_echoed_at(
        &self,
        tenant_id: Uuid,
        id: Uuid,
        at: DateTime<Utc>,
    ) -> Result<(), LoreleiError> {
        LoreStores::update_last_echoed_at(self, tenant_id, id, at).await
    }
}
