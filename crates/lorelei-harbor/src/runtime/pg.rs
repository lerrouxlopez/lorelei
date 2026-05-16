#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::CurrentStore;
use lorelei_core::types::{
    AgentId, CurrentEvent, CurrentEventType, Run, RunId, RunStatus, TenantId,
};
use serde_json::json;
use sqlx::{postgres::PgRow, PgPool, Row};
use uuid::Uuid;

pub struct PgCurrentStore {
    pool: PgPool,
}

impl PgCurrentStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn migrate(&self) -> Result<(), LoreleiError> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|e| LoreleiError::Internal(format!("migration failed: {e}")))?;
        Ok(())
    }

    pub async fn create_run(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        goal: &str,
    ) -> Result<Run, LoreleiError> {
        if goal.trim().is_empty() {
            return Err(LoreleiError::validation("run.goal", "must not be empty"));
        }

        let run_id = RunId(Uuid::new_v4());
        let now = Utc::now();
        let status = RunStatus::Running;

        sqlx::query(
            r#"
insert into runs (id, tenant_id, agent_id, goal, status, created_at, completed_at)
values ($1, $2, $3, $4, $5, $6, null)
"#,
        )
        .bind(run_id.0)
        .bind(tenant_id.0.to_string())
        .bind(agent_id.0.to_string())
        .bind(goal)
        .bind(status_to_str(status))
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;

        Ok(Run {
            run_id,
            tenant_id,
            agent_id,
            status,
            created_at: now,
            updated_at: now,
        })
    }

    pub async fn complete_run(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        run_id: RunId,
        status: RunStatus,
    ) -> Result<(), LoreleiError> {
        let completed_at = Utc::now();
        let rows = sqlx::query(
            r#"
update runs
set status = $1, completed_at = $2
where id = $3 and tenant_id = $4 and agent_id = $5
"#,
        )
        .bind(status_to_str(status))
        .bind(completed_at)
        .bind(run_id.0)
        .bind(tenant_id.0.to_string())
        .bind(agent_id.0.to_string())
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?
        .rows_affected();

        if rows == 0 {
            return Err(LoreleiError::NotFound("run not found".to_string()));
        }
        Ok(())
    }

    pub async fn get_run(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        run_id: RunId,
    ) -> Result<Option<Run>, LoreleiError> {
        let row = sqlx::query(
            r#"
select id, tenant_id, agent_id, status, created_at, coalesce(completed_at, created_at) as updated_at
from runs
where id = $1 and tenant_id = $2 and agent_id = $3
"#,
        )
        .bind(run_id.0)
        .bind(tenant_id.0.to_string())
        .bind(agent_id.0.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx)?;

        let Some(row) = row else {
            return Ok(None);
        };

        let id: Uuid = row.get("id");
        let tenant_id_s: String = row.get("tenant_id");
        let agent_id_s: String = row.get("agent_id");
        let status_s: String = row.get("status");
        let created_at: DateTime<Utc> = row.get("created_at");
        let updated_at: DateTime<Utc> = row.get("updated_at");

        Ok(Some(Run {
            run_id: RunId(id),
            tenant_id: TenantId(Uuid::parse_str(&tenant_id_s).map_err(|_| {
                LoreleiError::Internal("invalid tenant_id in runs table".to_string())
            })?),
            agent_id: AgentId(Uuid::parse_str(&agent_id_s).map_err(|_| {
                LoreleiError::Internal("invalid agent_id in runs table".to_string())
            })?),
            status: parse_status(&status_s)?,
            created_at,
            updated_at,
        }))
    }
}

#[async_trait::async_trait]
impl lorelei_tide::runtime::RunRepository for PgCurrentStore {
    async fn create_run(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        goal: &str,
    ) -> Result<Run, LoreleiError> {
        PgCurrentStore::create_run(self, tenant_id, agent_id, goal).await
    }

    async fn complete_run(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        run_id: RunId,
        status: RunStatus,
    ) -> Result<(), LoreleiError> {
        PgCurrentStore::complete_run(self, tenant_id, agent_id, run_id, status).await
    }

    async fn get_run(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        run_id: RunId,
    ) -> Result<Option<Run>, LoreleiError> {
        PgCurrentStore::get_run(self, tenant_id, agent_id, run_id).await
    }
}

#[async_trait::async_trait]
impl CurrentStore for PgCurrentStore {
    async fn append_current_event(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        run_id: RunId,
        event: CurrentEvent,
    ) -> Result<(), LoreleiError> {
        // Enforce explicit IDs match the call context.
        if event.tenant_id != tenant_id {
            return Err(LoreleiError::validation(
                "current.tenant_id",
                "event tenant_id must match operation tenant_id",
            ));
        }
        if event.agent_id != agent_id {
            return Err(LoreleiError::validation(
                "current.agent_id",
                "event agent_id must match operation agent_id",
            ));
        }
        if event.run_id != run_id {
            return Err(LoreleiError::validation(
                "current.run_id",
                "event run_id must match operation run_id",
            ));
        }
        if event.summary.trim().is_empty() {
            return Err(LoreleiError::validation(
                "current.summary",
                "must not be empty",
            ));
        }

        let content = json!({
            "summary": event.summary,
            "data": event.data,
        });

        sqlx::query(
            r#"
insert into currents (id, tenant_id, run_id, agent_id, event_type, content, created_at)
values ($1, $2, $3, $4, $5, $6, $7)
"#,
        )
        .bind(event.event_id.0)
        .bind(tenant_id.0.to_string())
        .bind(run_id.0)
        .bind(agent_id.0.to_string())
        .bind(event_type_to_str(event.event_type))
        .bind(content)
        .bind(event.created_at)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;

        Ok(())
    }

    async fn list_current_events(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        run_id: RunId,
        limit: usize,
    ) -> Result<Vec<CurrentEvent>, LoreleiError> {
        let limit: i64 = limit
            .try_into()
            .map_err(|_| LoreleiError::validation("limit", "too large"))?;

        let rows: Vec<PgRow> = sqlx::query(
            r#"
select id, event_type, content, created_at
from currents
where tenant_id = $1 and agent_id = $2 and run_id = $3
order by created_at asc
limit $4
"#,
        )
        .bind(tenant_id.0.to_string())
        .bind(agent_id.0.to_string())
        .bind(run_id.0)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx)?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(row_to_current_event(&row, tenant_id, agent_id, run_id)?);
        }
        Ok(out)
    }
}

fn row_to_current_event(
    row: &PgRow,
    tenant_id: TenantId,
    agent_id: AgentId,
    run_id: RunId,
) -> Result<CurrentEvent, LoreleiError> {
    let id: Uuid = row.get("id");
    let event_type: String = row.get("event_type");
    let content: serde_json::Value = row.get("content");
    let created_at: DateTime<Utc> = row.get("created_at");

    let summary = content
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let data = content
        .get("data")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    Ok(CurrentEvent {
        event_id: lorelei_core::types::EchoId(id),
        tenant_id,
        agent_id,
        run_id,
        event_type: parse_event_type(&event_type)?,
        created_at,
        summary,
        data,
    })
}

fn parse_event_type(s: &str) -> Result<CurrentEventType, LoreleiError> {
    match s {
        "user" => Ok(CurrentEventType::User),
        "assistant" => Ok(CurrentEventType::Assistant),
        "tool_call" => Ok(CurrentEventType::ToolCall),
        "tool_result" => Ok(CurrentEventType::ToolResult),
        "system" => Ok(CurrentEventType::System),
        other => Err(LoreleiError::validation(
            "current.event_type",
            format!("unknown event_type `{other}`"),
        )),
    }
}

fn event_type_to_str(t: CurrentEventType) -> &'static str {
    match t {
        CurrentEventType::User => "user",
        CurrentEventType::Assistant => "assistant",
        CurrentEventType::ToolCall => "tool_call",
        CurrentEventType::ToolResult => "tool_result",
        CurrentEventType::System => "system",
    }
}

fn status_to_str(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Pending => "pending",
        RunStatus::Running => "running",
        RunStatus::Succeeded => "succeeded",
        RunStatus::Failed => "failed",
        RunStatus::Canceled => "canceled",
    }
}

fn parse_status(s: &str) -> Result<RunStatus, LoreleiError> {
    match s {
        "pending" => Ok(RunStatus::Pending),
        "running" => Ok(RunStatus::Running),
        "succeeded" => Ok(RunStatus::Succeeded),
        "failed" => Ok(RunStatus::Failed),
        "canceled" => Ok(RunStatus::Canceled),
        other => Err(LoreleiError::validation(
            "run.status",
            format!("unknown status `{other}`"),
        )),
    }
}

fn map_sqlx(err: sqlx::Error) -> LoreleiError {
    // Do not include query/params; they may contain secrets.
    LoreleiError::Internal(format!("database error: {err}"))
}
