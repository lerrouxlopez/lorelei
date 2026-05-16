#![forbid(unsafe_code)]

use crate::http::server::build_state;
use chrono::Duration;
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::CurrentStore;
use lorelei_core::types::{CurrentEventType, RunStatus, ShellRisk};
use serde_json::Value;
use std::sync::Arc;
use tokio::time::{sleep, Duration as TokioDuration};
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub poll_every: TokioDuration,
    pub lease_seconds: i64,
    pub batch_size: i64,
    pub once: bool,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            poll_every: TokioDuration::from_secs(5),
            lease_seconds: 60,
            batch_size: 5,
            once: false,
        }
    }
}

pub async fn run(cfg: WorkerConfig) -> Result<(), LoreleiError> {
    lorelei_core::observability::init_tracing("worker");

    let state = build_state().await?;
    run_with_state(Arc::new(state), cfg).await
}

pub async fn run_with_state(
    state: Arc<crate::http::server::AppState>,
    cfg: WorkerConfig,
) -> Result<(), LoreleiError> {
    let worker_id = Uuid::new_v4();
    info!("worker starting id={}", worker_id);

    loop {
        let claimed = state
            .autonomy
            .claim_due_tasks(
                worker_id,
                cfg.batch_size,
                Duration::seconds(cfg.lease_seconds),
            )
            .await?;

        for task in claimed {
            info!(
                "task due id={} next_run_at={}",
                task.task_id.0, task.next_run_at
            );

            let res = state
                .tide
                .run_task_once_with_options(
                    task.tenant_id,
                    task.agent_id,
                    task.task_id,
                    task.prompt.clone(),
                    true,
                )
                .await;

            match res {
                Ok(r) => {
                    match r.status {
                        RunStatus::Succeeded => {
                            state
                                .autonomy
                                .finish_task_run_success(
                                    task.tenant_id,
                                    task.task_id,
                                    r.run_id,
                                    task.schedule.clone(),
                                )
                                .await?;
                        }
                        RunStatus::Canceled => {
                            // If an approval is required, record it and stop the worker.
                            if let Some((tool, input, risk, prompt)) = find_approval_required(
                                state.currents.clone(),
                                task.tenant_id,
                                task.agent_id,
                                r.run_id,
                            )
                            .await?
                            {
                                let approval = state
                                    .autonomy
                                    .create_approval(
                                        task.tenant_id,
                                        task.agent_id,
                                        Some(task.task_id),
                                        r.run_id,
                                        &tool,
                                        input,
                                        risk,
                                        &prompt,
                                    )
                                    .await?;

                                // Pause the task until the approval is handled.
                                let _ = state
                                    .autonomy
                                    .pause_task(task.tenant_id, task.task_id)
                                    .await;

                                warn!(
                                    "approval required: approval_id={} task_id={} tool={} risk={:?}",
                                    approval.approval_id.0, task.task_id.0, approval.tool, approval.risk
                                );
                                return Ok(());
                            }

                            state
                                .autonomy
                                .finish_task_run_failure(
                                    task.tenant_id,
                                    task.task_id,
                                    "run canceled",
                                )
                                .await?;
                        }
                        RunStatus::Failed => {
                            state
                                .autonomy
                                .finish_task_run_failure(task.tenant_id, task.task_id, "run failed")
                                .await?;
                        }
                        other => {
                            state
                                .autonomy
                                .finish_task_run_failure(
                                    task.tenant_id,
                                    task.task_id,
                                    &format!("unexpected run status: {other:?}"),
                                )
                                .await?;
                        }
                    }
                }
                Err(e) => {
                    state
                        .autonomy
                        .finish_task_run_failure(task.tenant_id, task.task_id, &e.to_string())
                        .await?;
                }
            }
        }

        if cfg.once {
            return Ok(());
        }

        sleep(cfg.poll_every).await;
    }
}

async fn find_approval_required(
    currents: Arc<dyn CurrentStore>,
    tenant_id: lorelei_core::types::TenantId,
    agent_id: lorelei_core::types::AgentId,
    run_id: lorelei_core::types::RunId,
) -> Result<Option<(String, Value, ShellRisk, String)>, LoreleiError> {
    let events = currents
        .list_current_events(tenant_id, agent_id, run_id, 200)
        .await?;

    for e in events {
        if e.event_type != CurrentEventType::System || e.summary != "approval required" {
            continue;
        }
        let tool = e
            .data
            .get("tool")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let input = e.data.get("input").cloned().unwrap_or(Value::Null);
        let risk = e
            .data
            .get("risk")
            .and_then(|v| serde_json::from_value::<ShellRisk>(v.clone()).ok())
            .unwrap_or(ShellRisk::High);
        let prompt = e
            .data
            .get("approval_prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("Approval required.")
            .to_string();

        if tool.is_empty() {
            continue;
        }
        return Ok(Some((tool, input, risk, prompt)));
    }
    Ok(None)
}
