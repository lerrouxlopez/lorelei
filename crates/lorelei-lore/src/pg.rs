#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::LoreStore;
use lorelei_core::types::{
    AgentId, NewPearl, Pearl, PearlId, PearlListQuery, PearlType, TenantId, UnitInterval,
};
use serde_json::Value;
use sqlx::{PgPool, Row};
use std::collections::BTreeMap;
use uuid::Uuid;

pub struct PgLoreStore {
    pool: PgPool,
}

impl PgLoreStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn migrate(&self) -> Result<(), LoreleiError> {
        // Reuse the Harbor migrations for now to avoid schema divergence.
        sqlx::migrate!("../lorelei-harbor/migrations")
            .run(&self.pool)
            .await
            .map_err(|e| LoreleiError::Internal(format!("migration failed: {e}")))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl LoreStore for PgLoreStore {
    async fn save_pearl(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        pearl: NewPearl,
    ) -> Result<Pearl, LoreleiError> {
        // Validate before DB insertion.
        if pearl.content.trim().is_empty() {
            return Err(LoreleiError::validation(
                "pearl.content",
                "must not be empty",
            ));
        }

        let pearl_id = PearlId(Uuid::new_v4());
        let now = Utc::now();

        // Minimal mapping for Part I:
        // - tags from metadata["tags"] if present (array of strings), else empty.
        // - metadata stored as JSON in pearls.content is not desired; keep in-memory only.
        let tags = tags_from_metadata(&pearl.metadata)?;

        sqlx::query(
            r#"
insert into pearls (
  id, tenant_id, agent_id, pearl_type, content,
  source_current_id, confidence, importance, tags,
  created_at, last_echoed_at, deleted_at
)
values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,null,null)
"#,
        )
        .bind(pearl_id.0)
        .bind(tenant_id.0.to_string())
        .bind(agent_id.0.to_string())
        .bind(pearl_type_to_str(pearl.pearl_type))
        .bind(&pearl.content)
        .bind(Option::<Uuid>::None)
        .bind(f64::from(pearl.confidence) as f32)
        .bind(f64::from(pearl.importance) as f32)
        .bind(&tags)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;

        Ok(Pearl {
            pearl_id,
            tenant_id,
            agent_id,
            pearl_type: pearl.pearl_type,
            content: pearl.content,
            importance: pearl.importance,
            confidence: pearl.confidence,
            created_at: now,
            metadata: pearl.metadata,
        })
    }

    async fn get_pearl(
        &self,
        tenant_id: TenantId,
        pearl_id: PearlId,
        include_deleted: bool,
    ) -> Result<Option<Pearl>, LoreleiError> {
        let row = if include_deleted {
            sqlx::query(
                r#"
select id, tenant_id, agent_id, pearl_type, content, confidence, importance, created_at, tags, deleted_at
from pearls
where tenant_id = $1 and id = $2
limit 1
"#,
            )
            .bind(tenant_id.0.to_string())
            .bind(pearl_id.0)
            .fetch_optional(&self.pool)
            .await
        } else {
            sqlx::query(
                r#"
select id, tenant_id, agent_id, pearl_type, content, confidence, importance, created_at, tags, deleted_at
from pearls
where tenant_id = $1 and id = $2 and deleted_at is null
limit 1
"#,
            )
            .bind(tenant_id.0.to_string())
            .bind(pearl_id.0)
            .fetch_optional(&self.pool)
            .await
        }
        .map_err(map_sqlx)?;

        let Some(row) = row else {
            return Ok(None);
        };
        let pearl = row_to_pearl(&row)?;
        Ok(Some(pearl))
    }

    async fn list_pearls(
        &self,
        tenant_id: TenantId,
        query: PearlListQuery,
    ) -> Result<Vec<Pearl>, LoreleiError> {
        // Simple dynamic SQL building (no user-provided SQL fragments).
        let mut sql = String::from(
            r#"
select id, tenant_id, agent_id, pearl_type, content, confidence, importance, created_at, tags, deleted_at
from pearls
where tenant_id = $1
"#,
        );
        let mut binds: Vec<Bind> = Vec::new();
        binds.push(Bind::Text(tenant_id.0.to_string()));

        if let Some(agent_id) = query.agent_id {
            sql.push_str("  and agent_id = $2\n");
            binds.push(Bind::Text(agent_id.0.to_string()));
        }

        let mut next_index = binds.len() + 1;

        if let Some(pearl_type) = query.pearl_type {
            sql.push_str(&format!("  and pearl_type = ${next_index}\n"));
            binds.push(Bind::Text(pearl_type_to_str(pearl_type).to_string()));
            next_index += 1;
        }

        if !query.tags.is_empty() {
            // tags && $n::text[] (overlap)
            sql.push_str(&format!("  and tags && ${next_index}::text[]\n"));
            binds.push(Bind::TextArray(query.tags.clone()));
            next_index += 1;
        }

        if !query.include_deleted {
            sql.push_str("  and deleted_at is null\n");
        }

        sql.push_str("order by created_at desc\n");

        if let Some(limit) = query.limit {
            if limit == 0 {
                return Ok(Vec::new());
            }
            sql.push_str(&format!("limit ${next_index}\n"));
            let limit_i64: i64 = limit
                .try_into()
                .map_err(|_| LoreleiError::validation("limit", "too large"))?;
            binds.push(Bind::I64(limit_i64));
        }

        let mut q = sqlx::query(&sql);
        for b in binds {
            q = match b {
                Bind::Text(v) => q.bind(v),
                Bind::TextArray(v) => q.bind(v),
                Bind::I64(v) => q.bind(v),
            };
        }

        let rows = q.fetch_all(&self.pool).await.map_err(map_sqlx)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(row_to_pearl(&row)?);
        }
        Ok(out)
    }

    async fn forget_pearl(
        &self,
        tenant_id: TenantId,
        pearl_id: PearlId,
    ) -> Result<(), LoreleiError> {
        let now = Utc::now();
        let rows = sqlx::query(
            r#"
update pearls
set deleted_at = $1
where tenant_id = $2 and id = $3 and deleted_at is null
"#,
        )
        .bind(now)
        .bind(tenant_id.0.to_string())
        .bind(pearl_id.0)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?
        .rows_affected();

        if rows == 0 {
            return Err(LoreleiError::NotFound("pearl not found".to_string()));
        }
        Ok(())
    }

    async fn update_last_echoed_at(
        &self,
        tenant_id: TenantId,
        pearl_id: PearlId,
        last_echoed_at: DateTime<Utc>,
    ) -> Result<(), LoreleiError> {
        let rows = sqlx::query(
            r#"
update pearls
set last_echoed_at = $1
where tenant_id = $2 and id = $3 and deleted_at is null
"#,
        )
        .bind(last_echoed_at)
        .bind(tenant_id.0.to_string())
        .bind(pearl_id.0)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?
        .rows_affected();

        if rows == 0 {
            return Err(LoreleiError::NotFound("pearl not found".to_string()));
        }
        Ok(())
    }
}

enum Bind {
    Text(String),
    TextArray(Vec<String>),
    I64(i64),
}

fn row_to_pearl(row: &sqlx::postgres::PgRow) -> Result<Pearl, LoreleiError> {
    let id: Uuid = row.get("id");
    let tenant_id: String = row.get("tenant_id");
    let agent_id: String = row.get("agent_id");
    let pearl_type: String = row.get("pearl_type");
    let content: String = row.get("content");
    let confidence: f32 = row.get("confidence");
    let importance: f32 = row.get("importance");
    let created_at: DateTime<Utc> = row.get("created_at");
    let tags: Vec<String> = row.get("tags");

    let tenant_uuid = Uuid::parse_str(&tenant_id)
        .map_err(|_| LoreleiError::Internal("invalid tenant_id in database".to_string()))?;
    let agent_uuid = Uuid::parse_str(&agent_id)
        .map_err(|_| LoreleiError::Internal("invalid agent_id in database".to_string()))?;

    let mut metadata = BTreeMap::new();
    if !tags.is_empty() {
        metadata.insert(
            "tags".to_string(),
            serde_json::to_value(tags)
                .map_err(|_| LoreleiError::Internal("failed to serialize tags".to_string()))?,
        );
    }

    Ok(Pearl {
        pearl_id: PearlId(id),
        tenant_id: TenantId(tenant_uuid),
        agent_id: AgentId(agent_uuid),
        pearl_type: parse_pearl_type(&pearl_type)?,
        content,
        importance: UnitInterval::new(importance as f64)?,
        confidence: UnitInterval::new(confidence as f64)?,
        created_at,
        metadata,
    })
}

fn parse_pearl_type(s: &str) -> Result<PearlType, LoreleiError> {
    match s {
        "fact" => Ok(PearlType::Fact),
        "preference" => Ok(PearlType::Preference),
        "skill" => Ok(PearlType::Skill),
        "plan" => Ok(PearlType::Plan),
        "other" => Ok(PearlType::Other),
        other => Err(LoreleiError::Internal(format!(
            "unknown pearl_type in database: {other}"
        ))),
    }
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

fn tags_from_metadata(metadata: &BTreeMap<String, Value>) -> Result<Vec<String>, LoreleiError> {
    let Some(tags_val) = metadata.get("tags") else {
        return Ok(Vec::new());
    };
    let Some(arr) = tags_val.as_array() else {
        return Err(LoreleiError::validation(
            "pearl.metadata.tags",
            "must be an array",
        ));
    };
    let mut out = Vec::with_capacity(arr.len());
    for v in arr {
        let Some(s) = v.as_str() else {
            return Err(LoreleiError::validation(
                "pearl.metadata.tags",
                "all tags must be strings",
            ));
        };
        out.push(s.to_string());
    }
    Ok(out)
}

fn map_sqlx(err: sqlx::Error) -> LoreleiError {
    LoreleiError::Internal(format!("database error: {err}"))
}
