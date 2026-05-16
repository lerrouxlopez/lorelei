#![forbid(unsafe_code)]

use async_trait::async_trait;
use lorelei_core::config::LoreleiConfig;
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::{CurrentStore, EchoRetriever, ShellRegistry, SirenPolicy, SongProvider};
use lorelei_core::types::{
    CurrentEvent, CurrentEventType, EchoHit, EchoQuery, NormalizedToolCall, Run, RunId, RunStatus,
    ShellCall, ShellResult, ShellRisk, SirenDecision, SongRequest, SongResponse, TenantId,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::info_span;
use tracing_futures::Instrument;
use uuid::Uuid;

#[async_trait]
pub trait RunRepository: Send + Sync {
    async fn create_run(
        &self,
        tenant_id: TenantId,
        agent_id: lorelei_core::types::AgentId,
        goal: &str,
    ) -> Result<Run, LoreleiError>;

    async fn complete_run(
        &self,
        tenant_id: TenantId,
        agent_id: lorelei_core::types::AgentId,
        run_id: RunId,
        status: RunStatus,
    ) -> Result<(), LoreleiError>;

    async fn get_run(
        &self,
        tenant_id: TenantId,
        agent_id: lorelei_core::types::AgentId,
        run_id: RunId,
    ) -> Result<Option<Run>, LoreleiError>;
}

#[derive(Debug, Clone)]
pub struct TideResult {
    pub run_id: RunId,
    pub status: RunStatus,
    pub output: String,
}

#[derive(Clone)]
pub struct SingleAgentTideRuntime {
    pub config: LoreleiConfig,
    pub runs: Arc<dyn RunRepository>,
    pub currents: Arc<dyn CurrentStore>,
    pub echo: Arc<dyn EchoRetriever>,
    pub song: Arc<dyn SongProvider>,
    pub shells: Arc<dyn ShellRegistry>,
    pub siren: Arc<dyn SirenPolicy>,

    planner_template: &'static str,
    answer_template: &'static str,
}

impl SingleAgentTideRuntime {
    pub fn new(
        config: LoreleiConfig,
        runs: Arc<dyn RunRepository>,
        currents: Arc<dyn CurrentStore>,
        echo: Arc<dyn EchoRetriever>,
        song: Arc<dyn SongProvider>,
        shells: Arc<dyn ShellRegistry>,
        siren: Arc<dyn SirenPolicy>,
    ) -> Self {
        Self {
            config,
            runs,
            currents,
            echo,
            song,
            shells,
            siren,
            planner_template: include_str!("../../../prompts/song_planner.md"),
            answer_template: include_str!("../../../prompts/song_answer.md"),
        }
    }

    pub fn with_templates(mut self, planner: &'static str, answer: &'static str) -> Self {
        self.planner_template = planner;
        self.answer_template = answer;
        self
    }

    pub async fn run_once(
        &self,
        tenant_id: TenantId,
        agent_id: lorelei_core::types::AgentId,
        user_input: String,
    ) -> Result<TideResult, LoreleiError> {
        let span = info_span!("tide.run_once", tenant_id = %tenant_id.0, agent_id = %agent_id.0);
        async move { self.run_once_inner(tenant_id, agent_id, user_input).await }
            .instrument(span)
            .await
    }

    async fn run_once_inner(
        &self,
        tenant_id: TenantId,
        agent_id: lorelei_core::types::AgentId,
        user_input: String,
    ) -> Result<TideResult, LoreleiError> {
        // 1. Create run
        let run = self
            .runs
            .create_run(tenant_id, agent_id, &user_input)
            .instrument(info_span!("tide.create_run"))
            .await?;

        // 2. Write user Current event
        let user_event_id = lorelei_core::types::EchoId(Uuid::new_v4());
        self.append_current(
            tenant_id,
            agent_id,
            run.run_id,
            user_event_id,
            CurrentEventType::User,
            "user message",
            json!({ "text": user_input }),
        )
        .instrument(info_span!("tide.current_user"))
        .await?;

        // 3. Echo retrieve
        let echo_hits = self
            .echo
            .query(
                tenant_id,
                agent_id,
                EchoQuery {
                    query: user_input.clone(),
                    top_k: self.config.echo.top_k,
                    min_confidence: self.config.echo.min_confidence,
                    pearl_type: None,
                },
            )
            .instrument(info_span!("tide.echo"))
            .await?;

        // 4-7. Planner (JSON plan + repair once)
        let (plan, planner_raw) = self
            .plan(run.run_id, tenant_id, agent_id, &user_input, &echo_hits)
            .instrument(info_span!("tide.plan"))
            .await?;

        // 8-14. Execute + answer + complete
        let final_output: String;
        let status: RunStatus;

        match plan.action.as_str() {
            "answer" => {
                final_output = self
                    .answer(run.run_id, tenant_id, agent_id, &user_input, &echo_hits)
                    .instrument(info_span!("tide.answer"))
                    .await?;
                status = RunStatus::Succeeded;
            }
            "call_shell" => {
                let tool = plan
                    .tool
                    .ok_or_else(|| LoreleiError::validation("plan.tool", "missing tool"))?;
                let input = plan
                    .input
                    .ok_or_else(|| LoreleiError::validation("plan.input", "missing input"))?;

                // 9. Route to shells (but do not execute yet)
                // Use tool_call_id as both ShellCall.call_id and CurrentEvent id so shell_calls.current_id can FK to currents(id).
                let tool_call_id = Uuid::new_v4();
                let call = ShellCall {
                    call_id: tool_call_id,
                    tenant_id,
                    agent_id,
                    run_id: run.run_id,
                    shell: "builtin".to_string(),
                    tool: tool.clone(),
                    input: input.clone(),
                    risk: shell_risk(&tool),
                    requested_at: chrono::Utc::now(),
                };

                // 10. Ask Siren
                let request = SongRequest {
                    tenant_id,
                    agent_id,
                    run_id: run.run_id,
                    input: user_input.clone(),
                    context: echo_hits.iter().map(|h| h.content.clone()).collect(),
                    reasoning_summary: None,
                };
                let response = SongResponse {
                    output: planner_raw,
                    reasoning_summary: plan.reasoning_summary.clone(),
                    tool_calls: vec![],
                };
                let tool_calls = vec![NormalizedToolCall {
                    call_id: tool_call_id.to_string(),
                    name: tool.clone(),
                    arguments: input.clone(),
                }];
                let decision = self
                    .siren
                    .decide(
                        tenant_id,
                        agent_id,
                        run.run_id,
                        &request,
                        &response,
                        &tool_calls,
                        &[],
                    )
                    .instrument(info_span!("tide.siren"))
                    .await?;

                match decision {
                    SirenDecision::Allow { .. } => {
                        // 12. Write tool_call current
                        self.append_current(
                            tenant_id,
                            agent_id,
                            run.run_id,
                            lorelei_core::types::EchoId(tool_call_id),
                            CurrentEventType::ToolCall,
                            &format!("shell call: {tool}"),
                            json!({ "tool": tool, "input": input }),
                        )
                        .instrument(info_span!("tide.current_tool_call"))
                        .await?;

                        // 11. Execute shell (policy allowed)
                        let result = self
                            .shells
                            .call(call)
                            .instrument(info_span!("tide.shell_exec"))
                            .await?;

                        // 12. Write tool_result current (shell_calls row is written by ShellRegistry)
                        self.append_current(
                            tenant_id,
                            agent_id,
                            run.run_id,
                            lorelei_core::types::EchoId(Uuid::new_v4()),
                            CurrentEventType::ToolResult,
                            "shell result",
                            serde_json::to_value(&result).unwrap_or(Value::Null),
                        )
                        .instrument(info_span!("tide.current_tool_result"))
                        .await?;

                        // 13. Final answer using result + echo hits
                        final_output = self
                            .answer_with_tool_result(
                                run.run_id,
                                tenant_id,
                                agent_id,
                                &user_input,
                                &echo_hits,
                                &result,
                            )
                            .instrument(info_span!("tide.answer_tool"))
                            .await?;
                        status = RunStatus::Succeeded;
                    }
                    SirenDecision::Deny { reasoning_summary } => {
                        final_output = format!(
                            "run_id={}\nDenied by Siren: {}",
                            run.run_id.0, reasoning_summary
                        );
                        status = RunStatus::Failed;
                    }
                    SirenDecision::RequireApproval {
                        reasoning_summary,
                        approval_prompt,
                    } => {
                        final_output = format!(
                            "run_id={}\nApproval required: {}\n\n{}",
                            run.run_id.0, reasoning_summary, approval_prompt
                        );
                        status = RunStatus::Canceled;
                    }
                }
            }
            other => {
                return Err(LoreleiError::validation(
                    "plan.action",
                    format!("unknown action `{other}`"),
                ));
            }
        }

        // 14. Complete run
        self.runs
            .complete_run(tenant_id, agent_id, run.run_id, status)
            .instrument(info_span!("tide.complete_run"))
            .await?;

        // Also record assistant final answer as Current event
        self.append_current(
            tenant_id,
            agent_id,
            run.run_id,
            lorelei_core::types::EchoId(Uuid::new_v4()),
            CurrentEventType::Assistant,
            "final answer",
            json!({ "text": final_output }),
        )
        .instrument(info_span!("tide.current_assistant"))
        .await?;

        Ok(TideResult {
            run_id: run.run_id,
            status,
            output: final_output,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn append_current(
        &self,
        tenant_id: TenantId,
        agent_id: lorelei_core::types::AgentId,
        run_id: RunId,
        event_id: lorelei_core::types::EchoId,
        event_type: CurrentEventType,
        summary: &str,
        data: Value,
    ) -> Result<(), LoreleiError> {
        let event = CurrentEvent {
            event_id,
            tenant_id,
            agent_id,
            run_id,
            event_type,
            created_at: chrono::Utc::now(),
            summary: summary.to_string(),
            data,
        };
        self.currents
            .append_current_event(tenant_id, agent_id, run_id, event)
            .await
    }

    async fn plan(
        &self,
        run_id: RunId,
        tenant_id: TenantId,
        agent_id: lorelei_core::types::AgentId,
        user_input: &str,
        echo_hits: &[EchoHit],
    ) -> Result<(PlannerOutput, String), LoreleiError> {
        let context = echo_hits
            .iter()
            .map(|h| format!("- {} ({:?})", h.content, h.pearl_type))
            .collect::<Vec<_>>()
            .join("\n");
        let prompt = self
            .planner_template
            .replace("{{CONTEXT}}", &context)
            .replace("{{USER_INPUT}}", user_input);

        let req = SongRequest {
            tenant_id,
            agent_id,
            run_id,
            input: prompt,
            context: echo_hits.iter().map(|h| h.content.clone()).collect(),
            reasoning_summary: Some("planner".to_string()),
        };

        let resp = self.song.complete(req).await?;
        let raw = resp.output.clone();
        match parse_planner_output(&raw) {
            Ok(p) => Ok((p, raw)),
            Err(_) => {
                let repair_prompt = format!(
                    "LORELEI_MODE=planner_repair\nReturn only valid JSON for the planner schema.\n\nPrevious output:\n{}",
                    raw
                );
                let repair_req = SongRequest {
                    tenant_id,
                    agent_id,
                    run_id,
                    input: repair_prompt,
                    context: Vec::new(),
                    reasoning_summary: Some("planner_repair".to_string()),
                };
                let repair_resp = self.song.complete(repair_req).await?;
                let repaired_raw = repair_resp.output.clone();
                let plan = parse_planner_output(&repaired_raw)?;
                Ok((plan, repaired_raw))
            }
        }
    }

    async fn answer(
        &self,
        run_id: RunId,
        tenant_id: TenantId,
        agent_id: lorelei_core::types::AgentId,
        user_input: &str,
        echo_hits: &[EchoHit],
    ) -> Result<String, LoreleiError> {
        let echo_block = echo_hits
            .iter()
            .map(|h| format!("- {} ({:?})", h.content, h.pearl_type))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = self
            .answer_template
            .replace("{{ECHO_HITS}}", &echo_block)
            .replace("{{USER_INPUT}}", user_input);
        let req = SongRequest {
            tenant_id,
            agent_id,
            run_id,
            input: prompt,
            context: echo_hits.iter().map(|h| h.content.clone()).collect(),
            reasoning_summary: Some("answer".to_string()),
        };
        let resp = self.song.complete(req).await?;
        Ok(format!("run_id={}\n{}", run_id.0, resp.output))
    }

    async fn answer_with_tool_result(
        &self,
        run_id: RunId,
        tenant_id: TenantId,
        agent_id: lorelei_core::types::AgentId,
        user_input: &str,
        echo_hits: &[EchoHit],
        tool_result: &ShellResult,
    ) -> Result<String, LoreleiError> {
        let echo_block = echo_hits
            .iter()
            .map(|h| format!("- {} ({:?})", h.content, h.pearl_type))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            "LORELEI_MODE=answer\n\nEchoHits:\n{}\n\nToolResult:\n{}\n\nUser:\n{}",
            echo_block,
            serde_json::to_string_pretty(tool_result).unwrap_or_default(),
            user_input
        );
        let req = SongRequest {
            tenant_id,
            agent_id,
            run_id,
            input: prompt,
            context: echo_hits.iter().map(|h| h.content.clone()).collect(),
            reasoning_summary: Some("answer".to_string()),
        };
        let resp = self.song.complete(req).await?;
        Ok(format!("run_id={}\n{}", run_id.0, resp.output))
    }
}

fn shell_risk(tool: &str) -> ShellRisk {
    match tool {
        "forget_pearl" => ShellRisk::High,
        "http_get" => ShellRisk::Medium,
        "save_pearl" => ShellRisk::Medium,
        "echo_lore" | "list_pearls" | "echo" | "noop" => ShellRisk::Low,
        _ => ShellRisk::Medium,
    }
}

#[derive(Debug, Deserialize)]
struct PlannerOutput {
    action: String,
    #[serde(default)]
    reasoning_summary: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    answer: Option<String>,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    input: Option<Value>,
}

fn parse_planner_output(raw: &str) -> Result<PlannerOutput, LoreleiError> {
    serde_json::from_str(raw)
        .map_err(|e| LoreleiError::validation("planner.json", format!("invalid planner JSON: {e}")))
}
