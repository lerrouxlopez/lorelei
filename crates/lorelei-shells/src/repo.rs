#![forbid(unsafe_code)]

use async_trait::async_trait;
use lorelei_core::error::LoreleiError;
use lorelei_core::types::{ShellCall, ShellResult};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

#[async_trait]
pub trait ShellCallRepository: Send + Sync {
    async fn record_start(
        &self,
        current_id: Option<Uuid>,
        call: &ShellCall,
    ) -> Result<(), LoreleiError>;
    async fn record_finish(&self, result: &ShellResult) -> Result<(), LoreleiError>;
}

#[derive(Clone)]
pub struct PgShellCallRepository {
    pool: PgPool,
}

impl PgShellCallRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ShellCallRepository for PgShellCallRepository {
    async fn record_start(
        &self,
        current_id: Option<Uuid>,
        call: &ShellCall,
    ) -> Result<(), LoreleiError> {
        let Some(current_id) = current_id else {
            return Err(LoreleiError::validation(
                "shell.current_id",
                "required for Postgres shell_calls persistence",
            ));
        };
        let tenant_id = call.tenant_id.0.to_string();
        let run_id = call.run_id.0;
        let shell_name = call.tool.clone();
        let input: Value = call.input.clone();
        let status = "started";
        let risk_level = format!("{:?}", call.risk).to_ascii_lowercase();

        sqlx::query(
            r#"
insert into shell_calls
  (id, tenant_id, run_id, current_id, shell_name, input, output, status, risk_level)
values
  ($1, $2, $3, $4, $5, $6, null, $7, $8)
"#,
        )
        .bind(call.call_id)
        .bind(tenant_id)
        .bind(run_id)
        .bind(current_id)
        .bind(shell_name)
        .bind(input)
        .bind(status)
        .bind(risk_level)
        .execute(&self.pool)
        .await
        .map_err(|e| LoreleiError::Internal(format!("failed to record shell call start: {e}")))?;

        Ok(())
    }

    async fn record_finish(&self, result: &ShellResult) -> Result<(), LoreleiError> {
        let status = if result.ok { "ok" } else { "error" };
        let output: Value = result.output.clone();

        sqlx::query(
            r#"
update shell_calls
set output = $2, status = $3
where id = $1
"#,
        )
        .bind(result.call_id)
        .bind(output)
        .bind(status)
        .execute(&self.pool)
        .await
        .map_err(|e| LoreleiError::Internal(format!("failed to record shell call finish: {e}")))?;

        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct NullShellCallRepository;

#[async_trait]
impl ShellCallRepository for NullShellCallRepository {
    async fn record_start(
        &self,
        _current_id: Option<Uuid>,
        _call: &ShellCall,
    ) -> Result<(), LoreleiError> {
        Ok(())
    }

    async fn record_finish(&self, _result: &ShellResult) -> Result<(), LoreleiError> {
        Ok(())
    }
}
