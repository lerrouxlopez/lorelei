#![forbid(unsafe_code)]

use chrono::{DateTime, Duration, NaiveTime, Utc};
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::ApprovalStore;
use lorelei_core::types::{
    AgentId, ApprovalId, ApprovalRequest, ApprovalState, AutonomousTask, AutonomousTaskId, RunId,
    ShellRisk, TaskSchedule, TaskStatus, TenantId,
};
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub struct PgAutonomy {
    pool: PgPool,
}

impl PgAutonomy {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn add_daily_task(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        prompt: &str,
        at_hhmm: &str,
    ) -> Result<AutonomousTask, LoreleiError> {
        if prompt.trim().is_empty() {
            return Err(LoreleiError::validation("task.prompt", "must not be empty"));
        }
        let at = parse_hhmm(at_hhmm)?;
        let now = Utc::now();
        let next_run_at = compute_next_daily(now, at);
        let id = AutonomousTaskId(Uuid::new_v4());

        let row = sqlx::query(
            r#"
insert into autonomous_tasks
  (id, tenant_id, agent_id, prompt, status, schedule_kind, schedule_at, next_run_at, created_at, updated_at)
values
  ($1, $2, $3, $4, 'active', 'daily', $5, $6, $7, $7)
returning id, tenant_id, agent_id, prompt, status, schedule_kind, schedule_at,
          next_run_at, last_run_at, created_at, updated_at
"#,
        )
        .bind(id.0)
        .bind(tenant_id.0.to_string())
        .bind(agent_id.0.to_string())
        .bind(prompt)
        .bind(at_hhmm)
        .bind(next_run_at)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx)?;

        task_from_row(row)
    }

    pub async fn list_tasks(
        &self,
        tenant_id: TenantId,
        agent_id: Option<AgentId>,
    ) -> Result<Vec<AutonomousTask>, LoreleiError> {
        let mut q = String::from(
            r#"
select id, tenant_id, agent_id, prompt, status, schedule_kind, schedule_at,
       next_run_at, last_run_at, created_at, updated_at
from autonomous_tasks
where tenant_id = $1
"#,
        );
        if agent_id.is_some() {
            q.push_str(" and agent_id = $2");
        }
        q.push_str(" order by created_at desc");

        let rows = if let Some(a) = agent_id {
            sqlx::query(&q)
                .bind(tenant_id.0.to_string())
                .bind(a.0.to_string())
                .fetch_all(&self.pool)
                .await
                .map_err(map_sqlx)?
        } else {
            sqlx::query(&q)
                .bind(tenant_id.0.to_string())
                .fetch_all(&self.pool)
                .await
                .map_err(map_sqlx)?
        };

        rows.into_iter().map(task_from_row).collect()
    }

    pub async fn pause_task(
        &self,
        tenant_id: TenantId,
        task_id: AutonomousTaskId,
    ) -> Result<(), LoreleiError> {
        let rows = sqlx::query(
            r#"
update autonomous_tasks
set status = 'paused', updated_at = now()
where id = $1 and tenant_id = $2
"#,
        )
        .bind(task_id.0)
        .bind(tenant_id.0.to_string())
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?
        .rows_affected();
        if rows == 0 {
            return Err(LoreleiError::NotFound("task not found".to_string()));
        }
        Ok(())
    }

    pub async fn resume_task(
        &self,
        tenant_id: TenantId,
        task_id: AutonomousTaskId,
    ) -> Result<(), LoreleiError> {
        let rows = sqlx::query(
            r#"
update autonomous_tasks
set status = 'active', updated_at = now()
where id = $1 and tenant_id = $2
"#,
        )
        .bind(task_id.0)
        .bind(tenant_id.0.to_string())
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?
        .rows_affected();
        if rows == 0 {
            return Err(LoreleiError::NotFound("task not found".to_string()));
        }
        Ok(())
    }

    pub async fn claim_due_tasks(
        &self,
        worker_id: Uuid,
        limit: i64,
        lease_for: Duration,
    ) -> Result<Vec<AutonomousTask>, LoreleiError> {
        let now = Utc::now();
        let locked_until = now + lease_for;

        let rows = sqlx::query(
            r#"
with due as (
  select id
  from autonomous_tasks
  where status = 'active'
    and next_run_at <= now()
    and (locked_until is null or locked_until < now())
  order by next_run_at asc
  limit $1
  for update skip locked
)
update autonomous_tasks t
set locked_until = $2, locked_by = $3, updated_at = $4
from due
where t.id = due.id
returning t.id, t.tenant_id, t.agent_id, t.prompt, t.status, t.schedule_kind, t.schedule_at,
          t.next_run_at, t.last_run_at, t.created_at, t.updated_at
"#,
        )
        .bind(limit)
        .bind(locked_until)
        .bind(worker_id)
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx)?;

        rows.into_iter().map(task_from_row).collect()
    }

    pub async fn finish_task_run_success(
        &self,
        tenant_id: TenantId,
        task_id: AutonomousTaskId,
        run_id: RunId,
        schedule: TaskSchedule,
    ) -> Result<(), LoreleiError> {
        let now = Utc::now();
        let next_run_at = match schedule {
            TaskSchedule::Daily { at_hhmm } => compute_next_daily(now, parse_hhmm(&at_hhmm)?),
        };

        let mut tx = self.pool.begin().await.map_err(map_sqlx)?;
        sqlx::query(
            r#"
insert into task_run_links (task_id, run_id, created_at)
values ($1, $2, $3)
on conflict do nothing
"#,
        )
        .bind(task_id.0)
        .bind(run_id.0)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx)?;

        sqlx::query(
            r#"
update autonomous_tasks
set last_run_at = $1,
    next_run_at = $2,
    locked_until = null,
    locked_by = null,
    last_error = null,
    consecutive_failures = 0,
    updated_at = $1
where id = $3 and tenant_id = $4
"#,
        )
        .bind(now)
        .bind(next_run_at)
        .bind(task_id.0)
        .bind(tenant_id.0.to_string())
        .execute(&mut *tx)
        .await
        .map_err(map_sqlx)?;

        tx.commit().await.map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn finish_task_run_failure(
        &self,
        tenant_id: TenantId,
        task_id: AutonomousTaskId,
        error: &str,
    ) -> Result<(), LoreleiError> {
        let now = Utc::now();
        // Simple backoff: 1m, 5m, 15m, 60m max.
        let row = sqlx::query(
            r#"
update autonomous_tasks
set locked_until = null,
    locked_by = null,
    last_error = $1,
    consecutive_failures = consecutive_failures + 1,
    updated_at = $2
where id = $3 and tenant_id = $4
returning consecutive_failures
"#,
        )
        .bind(truncate_err(error))
        .bind(now)
        .bind(task_id.0)
        .bind(tenant_id.0.to_string())
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx)?;

        let failures: i32 = row.get("consecutive_failures");
        let minutes = match failures {
            0 | 1 => 1,
            2 => 5,
            3 => 15,
            _ => 60,
        };
        let next_run_at = now + Duration::minutes(minutes);

        sqlx::query(
            r#"
update autonomous_tasks
set next_run_at = $1
where id = $2 and tenant_id = $3
"#,
        )
        .bind(next_run_at)
        .bind(task_id.0)
        .bind(tenant_id.0.to_string())
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_approval(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        task_id: Option<AutonomousTaskId>,
        run_id: RunId,
        tool: &str,
        input: Value,
        risk: ShellRisk,
        approval_prompt: &str,
    ) -> Result<ApprovalRequest, LoreleiError> {
        let id = ApprovalId(Uuid::new_v4());
        let now = Utc::now();

        let inserted = sqlx::query(
            r#"
insert into approvals
  (id, tenant_id, agent_id, task_id, run_id, tool, input, risk_level, approval_prompt, state, created_at, decided_at)
values
  ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'pending', $10, null)
on conflict do nothing
returning id, tenant_id, agent_id, task_id, run_id, tool, input, risk_level, approval_prompt, state, created_at, decided_at
"#,
        )
        .bind(id.0)
        .bind(tenant_id.0.to_string())
        .bind(agent_id.0.to_string())
        .bind(task_id.map(|t| t.0))
        .bind(run_id.0)
        .bind(tool)
        .bind(&input)
        .bind(shell_risk_to_str(risk))
        .bind(approval_prompt)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx)?;

        if let Some(row) = inserted {
            return approval_from_row(row);
        }

        // If already exists for (run_id, tool), fetch the existing pending approval.
        let row = sqlx::query(
            r#"
select id, tenant_id, agent_id, task_id, run_id, tool, input, risk_level, approval_prompt, state, created_at, decided_at
from approvals
where run_id = $1 and tool = $2 and state = 'pending'
limit 1
"#,
        )
        .bind(run_id.0)
        .bind(tool)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx)?;

        approval_from_row(row)
    }

    pub async fn list_approvals(
        &self,
        tenant_id: TenantId,
        state: Option<ApprovalState>,
    ) -> Result<Vec<ApprovalRequest>, LoreleiError> {
        let mut q = String::from(
            r#"
select id, tenant_id, agent_id, task_id, run_id, tool, input, risk_level, approval_prompt,
       state, created_at, decided_at
from approvals
where tenant_id = $1
"#,
        );
        if state.is_some() {
            q.push_str(" and state = $2");
        }
        q.push_str(" order by created_at desc");

        let rows = if let Some(s) = state {
            sqlx::query(&q)
                .bind(tenant_id.0.to_string())
                .bind(approval_state_to_str(s))
                .fetch_all(&self.pool)
                .await
                .map_err(map_sqlx)?
        } else {
            sqlx::query(&q)
                .bind(tenant_id.0.to_string())
                .fetch_all(&self.pool)
                .await
                .map_err(map_sqlx)?
        };

        rows.into_iter().map(approval_from_row).collect()
    }

    pub async fn approve(
        &self,
        tenant_id: TenantId,
        approval_id: ApprovalId,
    ) -> Result<(), LoreleiError> {
        let rows = sqlx::query(
            r#"
update approvals
set state = 'approved', decided_at = now()
where id = $1 and tenant_id = $2 and state = 'pending'
"#,
        )
        .bind(approval_id.0)
        .bind(tenant_id.0.to_string())
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?
        .rows_affected();

        if rows == 0 {
            return Err(LoreleiError::NotFound(
                "approval not found or not pending".to_string(),
            ));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl ApprovalStore for PgAutonomy {
    async fn is_approved(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        task_id: Option<AutonomousTaskId>,
        tool: &str,
        input: &Value,
    ) -> Result<bool, LoreleiError> {
        let row = sqlx::query(
            r#"
select count(*) as c
from approvals
where tenant_id = $1
  and agent_id = $2
  and tool = $3
  and input = $4
  and state = 'approved'
  and ($5::uuid is null or task_id = $5)
"#,
        )
        .bind(tenant_id.0.to_string())
        .bind(agent_id.0.to_string())
        .bind(tool)
        .bind(input)
        .bind(task_id.map(|t| t.0))
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx)?;

        let c: i64 = row.get("c");
        Ok(c > 0)
    }
}

fn parse_hhmm(s: &str) -> Result<NaiveTime, LoreleiError> {
    NaiveTime::parse_from_str(s, "%H:%M")
        .map_err(|_| LoreleiError::validation("task.schedule.at", "expected HH:MM"))
}

fn compute_next_daily(now: DateTime<Utc>, at: NaiveTime) -> DateTime<Utc> {
    let today = now.date_naive();
    let mut candidate = DateTime::<Utc>::from_naive_utc_and_offset(today.and_time(at), Utc);
    if candidate <= now {
        candidate += Duration::days(1);
    }
    candidate
}

fn task_from_row(row: sqlx::postgres::PgRow) -> Result<AutonomousTask, LoreleiError> {
    let id: Uuid = row.get("id");
    let tenant_id_s: String = row.get("tenant_id");
    let agent_id_s: String = row.get("agent_id");
    let prompt: String = row.get("prompt");
    let status_s: String = row.get("status");
    let kind: String = row.get("schedule_kind");
    let at: String = row.get("schedule_at");
    let next_run_at: DateTime<Utc> = row.get("next_run_at");
    let last_run_at: Option<DateTime<Utc>> = row.get("last_run_at");
    let created_at: DateTime<Utc> = row.get("created_at");
    let updated_at: DateTime<Utc> = row.get("updated_at");

    let tenant_id = TenantId(Uuid::parse_str(&tenant_id_s).map_err(|_| {
        LoreleiError::Internal("invalid tenant_id in autonomous_tasks".to_string())
    })?);
    let agent_id =
        AgentId(Uuid::parse_str(&agent_id_s).map_err(|_| {
            LoreleiError::Internal("invalid agent_id in autonomous_tasks".to_string())
        })?);

    let status = match status_s.as_str() {
        "active" => TaskStatus::Active,
        "paused" => TaskStatus::Paused,
        _ => {
            return Err(LoreleiError::Internal(
                "invalid task status in autonomous_tasks".to_string(),
            ))
        }
    };
    let schedule = match kind.as_str() {
        "daily" => TaskSchedule::Daily { at_hhmm: at },
        _ => {
            return Err(LoreleiError::Internal(
                "invalid schedule in autonomous_tasks".to_string(),
            ))
        }
    };

    Ok(AutonomousTask {
        task_id: AutonomousTaskId(id),
        tenant_id,
        agent_id,
        prompt,
        status,
        schedule,
        next_run_at,
        last_run_at,
        created_at,
        updated_at,
    })
}

fn approval_from_row(row: sqlx::postgres::PgRow) -> Result<ApprovalRequest, LoreleiError> {
    let id: Uuid = row.get("id");
    let tenant_id_s: String = row.get("tenant_id");
    let agent_id_s: String = row.get("agent_id");
    let task_id: Option<Uuid> = row.get("task_id");
    let run_id: Uuid = row.get("run_id");
    let tool: String = row.get("tool");
    let input: Value = row.get("input");
    let risk_s: String = row.get("risk_level");
    let approval_prompt: String = row.get("approval_prompt");
    let state_s: String = row.get("state");
    let created_at: DateTime<Utc> = row.get("created_at");
    let decided_at: Option<DateTime<Utc>> = row.get("decided_at");

    let tenant_id = TenantId(
        Uuid::parse_str(&tenant_id_s)
            .map_err(|_| LoreleiError::Internal("invalid tenant_id in approvals".to_string()))?,
    );
    let agent_id = AgentId(
        Uuid::parse_str(&agent_id_s)
            .map_err(|_| LoreleiError::Internal("invalid agent_id in approvals".to_string()))?,
    );

    let risk = shell_risk_from_str(&risk_s)?;
    let state = match state_s.as_str() {
        "pending" => ApprovalState::Pending,
        "approved" => ApprovalState::Approved,
        "denied" => ApprovalState::Denied,
        _ => {
            return Err(LoreleiError::Internal(
                "invalid approval state in approvals".to_string(),
            ))
        }
    };

    Ok(ApprovalRequest {
        approval_id: ApprovalId(id),
        tenant_id,
        agent_id,
        task_id: task_id.map(AutonomousTaskId),
        run_id: RunId(run_id),
        tool,
        input,
        risk,
        state,
        approval_prompt,
        created_at,
        decided_at,
    })
}

fn shell_risk_to_str(r: ShellRisk) -> &'static str {
    match r {
        ShellRisk::None => "none",
        ShellRisk::Low => "low",
        ShellRisk::Medium => "medium",
        ShellRisk::High => "high",
        ShellRisk::Critical => "critical",
    }
}

fn shell_risk_from_str(s: &str) -> Result<ShellRisk, LoreleiError> {
    match s {
        "none" => Ok(ShellRisk::None),
        "low" => Ok(ShellRisk::Low),
        "medium" => Ok(ShellRisk::Medium),
        "high" => Ok(ShellRisk::High),
        "critical" => Ok(ShellRisk::Critical),
        _ => Err(LoreleiError::Internal(
            "invalid risk_level in approvals".to_string(),
        )),
    }
}

fn approval_state_to_str(s: ApprovalState) -> &'static str {
    match s {
        ApprovalState::Pending => "pending",
        ApprovalState::Approved => "approved",
        ApprovalState::Denied => "denied",
    }
}

fn truncate_err(s: &str) -> String {
    const MAX: usize = 500;
    let s = s.trim();
    if s.len() <= MAX {
        s.to_string()
    } else {
        s.chars().take(MAX).collect()
    }
}

fn map_sqlx(e: sqlx::Error) -> LoreleiError {
    LoreleiError::Internal(format!("db error: {e}"))
}
