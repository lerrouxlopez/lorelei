#![forbid(unsafe_code)]

use async_trait::async_trait;
use lorelei_core::config::LoreleiConfig;
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::{
    CurrentStore, EchoRetriever, LoreStore, ShellRegistry, SirenPolicy, SongProvider,
};
use lorelei_core::types::{
    CurrentEvent, CurrentEventType, EchoHit, EchoQuery, EchoSources, NormalizedToolCall, Run,
    RunId, RunStatus, ShellCall, ShellResult, ShellRisk, SirenDecision, SongRequest, SongResponse,
    TenantId,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use tracing::field;
use tracing::info_span;
use tracing_futures::Instrument;
use uuid::Uuid;

use regex::Regex;

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
    pub lore: Arc<dyn LoreStore>,
    pub song: Arc<dyn SongProvider>,
    pub shells: Arc<dyn ShellRegistry>,
    pub siren: Arc<dyn SirenPolicy>,

    planner_template: &'static str,
    answer_template: &'static str,
    extractor_template: &'static str,
    critic_template: &'static str,
}

impl SingleAgentTideRuntime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: LoreleiConfig,
        runs: Arc<dyn RunRepository>,
        currents: Arc<dyn CurrentStore>,
        echo: Arc<dyn EchoRetriever>,
        lore: Arc<dyn LoreStore>,
        song: Arc<dyn SongProvider>,
        shells: Arc<dyn ShellRegistry>,
        siren: Arc<dyn SirenPolicy>,
    ) -> Self {
        Self {
            config,
            runs,
            currents,
            echo,
            lore,
            song,
            shells,
            siren,
            planner_template: include_str!("../../../prompts/song_planner.md"),
            answer_template: include_str!("../../../prompts/song_answer.md"),
            extractor_template: include_str!("../../../prompts/lore_extractor.md"),
            critic_template: include_str!("../../../prompts/lore_critic.md"),
        }
    }

    pub fn with_templates(mut self, planner: &'static str, answer: &'static str) -> Self {
        self.planner_template = planner;
        self.answer_template = answer;
        self
    }

    pub fn with_memory_templates(mut self, extractor: &'static str, critic: &'static str) -> Self {
        self.extractor_template = extractor;
        self.critic_template = critic;
        self
    }

    pub async fn run_once(
        &self,
        tenant_id: TenantId,
        agent_id: lorelei_core::types::AgentId,
        user_input: String,
    ) -> Result<TideResult, LoreleiError> {
        self.run_once_with_options(tenant_id, agent_id, user_input, true)
            .await
    }

    pub async fn run_once_with_options(
        &self,
        tenant_id: TenantId,
        agent_id: lorelei_core::types::AgentId,
        user_input: String,
        enable_memory: bool,
    ) -> Result<TideResult, LoreleiError> {
        let span = info_span!(
            "tide.run_once",
            tenant_id = %tenant_id.0,
            agent_id = %agent_id.0,
            run_id = field::Empty
        );
        async move {
            self.run_once_inner(tenant_id, agent_id, None, user_input, enable_memory)
                .await
        }
        .instrument(span)
        .await
    }

    /// Starts a run in the background and returns `run_id` immediately.
    ///
    /// Intended for clients that want to poll `/v1/runs/:run_id` and
    /// `/v1/runs/:run_id/currents` for live progress.
    pub async fn spawn_run_once_with_options(
        self: Arc<Self>,
        tenant_id: TenantId,
        agent_id: lorelei_core::types::AgentId,
        user_input: String,
        enable_memory: bool,
    ) -> Result<RunId, LoreleiError> {
        let span = info_span!(
            "tide.run_once_spawned",
            tenant_id = %tenant_id.0,
            agent_id = %agent_id.0,
            run_id = field::Empty
        );

        async move {
            // Create run and record user event synchronously so the run is visible immediately.
            let run = self
                .runs
                .create_run(tenant_id, agent_id, &user_input)
                .instrument(info_span!("tide.create_run"))
                .await?;
            tracing::Span::current().record("run_id", tracing::field::display(run.run_id.0));

            let user_event_id = lorelei_core::types::EchoId(Uuid::new_v4());
            self.append_current(
                tenant_id,
                agent_id,
                run.run_id,
                user_event_id,
                CurrentEventType::User,
                "user message",
                json!({ "text": user_input.clone() }),
            )
            .instrument(info_span!("tide.current_user"))
            .await?;

            let runtime = Arc::clone(&self);
            let run_id = run.run_id;
            tokio::spawn(async move {
                let res = runtime
                    .run_existing_inner(
                        tenant_id,
                        agent_id,
                        None,
                        run_id,
                        user_input,
                        enable_memory,
                    )
                    .await;
                if let Err(e) = res {
                    let msg = format!("{e}");
                    let _ = runtime
                        .append_current(
                            tenant_id,
                            agent_id,
                            run_id,
                            lorelei_core::types::EchoId(Uuid::new_v4()),
                            CurrentEventType::System,
                            "run failed",
                            json!({ "error": msg }),
                        )
                        .await;
                    let _ = runtime
                        .runs
                        .complete_run(tenant_id, agent_id, run_id, RunStatus::Failed)
                        .await;
                }
            });

            Ok(run.run_id)
        }
        .instrument(span)
        .await
    }

    pub async fn run_task_once_with_options(
        &self,
        tenant_id: TenantId,
        agent_id: lorelei_core::types::AgentId,
        task_id: lorelei_core::types::AutonomousTaskId,
        user_input: String,
        enable_memory: bool,
    ) -> Result<TideResult, LoreleiError> {
        let span = info_span!(
            "tide.run_task_once",
            tenant_id = %tenant_id.0,
            agent_id = %agent_id.0,
            task_id = %task_id.0,
            run_id = field::Empty
        );
        async move {
            self.run_once_inner(
                tenant_id,
                agent_id,
                Some(task_id),
                user_input,
                enable_memory,
            )
            .await
        }
        .instrument(span)
        .await
    }

    async fn run_once_inner(
        &self,
        tenant_id: TenantId,
        agent_id: lorelei_core::types::AgentId,
        task_id: Option<lorelei_core::types::AutonomousTaskId>,
        user_input: String,
        enable_memory: bool,
    ) -> Result<TideResult, LoreleiError> {
        // 1. Create run
        let run = self
            .runs
            .create_run(tenant_id, agent_id, &user_input)
            .instrument(info_span!("tide.create_run"))
            .await?;
        tracing::Span::current().record("run_id", tracing::field::display(run.run_id.0));

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

        self.run_existing_inner(
            tenant_id,
            agent_id,
            task_id,
            run.run_id,
            user_input,
            enable_memory,
        )
            .await
    }

    async fn run_existing_inner(
        &self,
        tenant_id: TenantId,
        agent_id: lorelei_core::types::AgentId,
        task_id: Option<lorelei_core::types::AutonomousTaskId>,
        run_id: RunId,
        user_input: String,
        enable_memory: bool,
    ) -> Result<TideResult, LoreleiError> {
        // 3. Echo retrieve
        self.append_current(
            tenant_id,
            agent_id,
            run_id,
            lorelei_core::types::EchoId(Uuid::new_v4()),
            CurrentEventType::System,
            if enable_memory {
                "retrieving memory"
            } else {
                "memory disabled"
            },
            json!({}),
        )
        .await?;

        // 3. Echo retrieve
        let echo_hits = if enable_memory {
            self.echo
                .query(
                    tenant_id,
                    agent_id,
                    EchoQuery {
                        query: user_input.clone(),
                        top_k: self.config.echo.top_k,
                        min_confidence: self.config.echo.min_confidence,
                        pearl_type: None,
                        sources: EchoSources::Pearls,
                    },
                )
                .instrument(info_span!("tide.echo"))
                .await?
        } else {
            Vec::new()
        };

        let mut shell_result: Option<ShellResult> = None;

        // 4-7. Planner (JSON plan + repair once)
        self.append_current(
            tenant_id,
            agent_id,
            run_id,
            lorelei_core::types::EchoId(Uuid::new_v4()),
            CurrentEventType::System,
            "planning",
            json!({}),
        )
        .await?;
        let (plan, planner_raw) = self
            .plan(run_id, tenant_id, agent_id, &user_input, &echo_hits)
            .instrument(info_span!("tide.plan"))
            .await?;

        // 8-14. Execute + answer + complete
        let final_output: String;
        let status: RunStatus;

        match plan.action.as_str() {
            "answer" => {
                self.append_current(
                    tenant_id,
                    agent_id,
                    run_id,
                    lorelei_core::types::EchoId(Uuid::new_v4()),
                    CurrentEventType::System,
                    "answering",
                    json!({}),
                )
                .await?;
                final_output = self
                    .answer(run_id, tenant_id, agent_id, &user_input, &echo_hits)
                    .instrument(info_span!("tide.answer"))
                    .await?;
                status = RunStatus::Succeeded;
            }
            "call_shell" => {
                let tool = plan
                    .tool
                    .ok_or_else(|| LoreleiError::validation("plan.tool", "missing tool"))?;
                let tool = tool.trim().to_string();
                if tool.is_empty() {
                    return Err(LoreleiError::validation("plan.tool", "missing tool"));
                }
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
                    run_id,
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
                    run_id,
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
                        run_id,
                        task_id,
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
                            run_id,
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
                        shell_result = Some(result.clone());

                        // 12. Write tool_result current (shell_calls row is written by ShellRegistry)
                        self.append_current(
                            tenant_id,
                            agent_id,
                            run_id,
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
                                run_id,
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
                        final_output =
                            format!("run_id={}\nDenied by Siren: {}", run_id.0, reasoning_summary);
                        status = RunStatus::Failed;
                    }
                    SirenDecision::RequireApproval {
                        reasoning_summary,
                        approval_prompt,
                    } => {
                        // Emit a structured current so workers/clients can create an approval request.
                        self.append_current(
                            tenant_id,
                            agent_id,
                            run_id,
                            lorelei_core::types::EchoId(Uuid::new_v4()),
                            CurrentEventType::System,
                            "approval required",
                            json!({
                                "tool": tool,
                                "input": input,
                                "risk": shell_risk(&tool),
                                "reasoning_summary": reasoning_summary,
                                "approval_prompt": approval_prompt,
                            }),
                        )
                        .instrument(info_span!("tide.current_approval_required"))
                        .await?;

                        final_output = format!(
                            "run_id={}\nApproval required: {}\n\n{}",
                            run_id.0, reasoning_summary, approval_prompt
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

        // Reflection + memory formation (best-effort).
        // Emit assistant answer as soon as it's available (before memory formation),
        // so clients can display the response while background work continues.
        self.append_current(
            tenant_id,
            agent_id,
            run_id,
            lorelei_core::types::EchoId(Uuid::new_v4()),
            CurrentEventType::Assistant,
            "final answer",
            json!({ "text": final_output }),
        )
        .instrument(info_span!("tide.current_assistant"))
        .await?;

        if enable_memory {
            self.append_current(
                tenant_id,
                agent_id,
                run_id,
                lorelei_core::types::EchoId(Uuid::new_v4()),
                CurrentEventType::System,
                "forming memories",
                json!({}),
            )
            .await?;
            let memory_decisions = self
                .form_memories(
                    run_id,
                    tenant_id,
                    agent_id,
                    &user_input,
                    &final_output,
                    shell_result.as_ref(),
                    &echo_hits,
                )
                .instrument(info_span!("tide.memory"))
                .await?;

            self.append_current(
                tenant_id,
                agent_id,
                run_id,
                lorelei_core::types::EchoId(Uuid::new_v4()),
                CurrentEventType::System,
                "memory formation",
                json!({ "decisions": memory_decisions }),
            )
            .await?;
        }

        // 14. Complete run
        self.runs
            .complete_run(tenant_id, agent_id, run_id, status)
            .instrument(info_span!("tide.complete_run"))
            .await?;

        Ok(TideResult {
            run_id,
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
            Err(first_err) => {
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
                match parse_planner_output(&repaired_raw) {
                    Ok(plan) => Ok((plan, repaired_raw)),
                    Err(second_err) => {
                        let repair_prompt2 = format!(
                            "LORELEI_MODE=planner_repair\nReturn exactly ONE JSON object and nothing else.\nConstraints:\n- Must be valid JSON (double quotes, no trailing commas)\n- Must include `action` = \"answer\" or \"call_shell\"\n- If `action` = \"call_shell\", must include `tool` and `input`\n\nParse error: {second_err}\n\nPrevious output:\n{repaired_raw}"
                        );
                        let repair_req2 = SongRequest {
                            tenant_id,
                            agent_id,
                            run_id,
                            input: repair_prompt2,
                            context: Vec::new(),
                            reasoning_summary: Some("planner_repair2".to_string()),
                        };
                        let repair_resp2 = self.song.complete(repair_req2).await?;
                        let repaired_raw2 = repair_resp2.output.clone();
                        let plan = parse_planner_output(&repaired_raw2).map_err(|final_err| {
                            LoreleiError::validation(
                                "planner.json",
                                format!(
                                    "planner repair failed: {first_err}; second repair failed: {final_err}"
                                ),
                            )
                        })?;
                        Ok((plan, repaired_raw2))
                    }
                }
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

    #[allow(clippy::too_many_arguments)]
    async fn form_memories(
        &self,
        run_id: RunId,
        tenant_id: TenantId,
        agent_id: lorelei_core::types::AgentId,
        user_input: &str,
        final_answer: &str,
        shell_result: Option<&ShellResult>,
        echo_hits: &[EchoHit],
    ) -> Result<Value, LoreleiError> {
        let shell_results_json = match shell_result {
            Some(r) => serde_json::to_string_pretty(r).unwrap_or_default(),
            None => "(none)".to_string(),
        };

        let run_summary = format!(
            "run_id={} echo_hits={} shell_used={}",
            run_id.0,
            echo_hits.len(),
            shell_result.is_some()
        );

        // In normal runs with the mock provider we perform deterministic extraction so `lore ask`
        // can exercise memory formation end-to-end without an external LLM.
        // In tests, we keep extraction driven by the SongProvider so tests can script candidates.
        let default_kind_is_mock = self
            .config
            .providers
            .get(&self.config.agent.default_provider)
            .is_some_and(|p| p.kind == lorelei_core::config::ProviderKind::Mock);
        let deterministic_extract_mode = default_kind_is_mock
            && std::env::var("LORELEI_DETERMINISTIC_EXTRACT")
                .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
                .unwrap_or(true);

        let candidates: Vec<CandidatePearl> = if deterministic_extract_mode {
            deterministic_extract_candidates(user_input)
        } else {
            let extractor_prompt = self
                .extractor_template
                .replace("{{USER_INPUT}}", user_input)
                .replace("{{FINAL_ANSWER}}", final_answer)
                .replace("{{SHELL_RESULTS}}", &shell_results_json)
                .replace("{{RUN_SUMMARY}}", &run_summary);

            let req = SongRequest {
                tenant_id,
                agent_id,
                run_id,
                input: extractor_prompt,
                context: Vec::new(),
                reasoning_summary: Some("lore_extractor".to_string()),
            };
            let resp = self.song.complete(req).await?;
            let raw = resp.output;

            match parse_candidate_list(&raw) {
                Ok(v) => v,
                Err(first_err) => {
                    let repair_prompt = format!(
                        "LORELEI_MODE=lore_extractor_repair\nReturn ONLY a valid JSON array of candidate pearls.\nSchema example: [{{\"pearl_type\":\"Fact\",\"content\":\"...\",\"confidence\":0.8,\"importance\":0.5,\"tags\":[\"...\"]}}]\n\nPrevious output:\n{}",
                        raw
                    );
                    let repair_req = SongRequest {
                        tenant_id,
                        agent_id,
                        run_id,
                        input: repair_prompt,
                        context: Vec::new(),
                        reasoning_summary: Some("lore_extractor_repair".to_string()),
                    };
                    let repair_resp = self.song.complete(repair_req).await?;
                    parse_candidate_list(&repair_resp.output).map_err(|second_err| {
                        LoreleiError::validation(
                            "lore_extractor.json",
                            format!(
                                "invalid candidate JSON: {first_err}; repair failed: {second_err}"
                            ),
                        )
                    })?
                }
            }
        };
        let candidates_json =
            serde_json::to_string_pretty(&candidates).unwrap_or_else(|_| "[]".to_string());

        let deterministic_critic_mode = default_kind_is_mock
            && std::env::var("LORELEI_DETERMINISTIC_CRITIC")
                .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
                .unwrap_or(true);
        let critic = if deterministic_critic_mode {
            None
        } else {
            let critic_prompt = self
                .critic_template
                .replace("{{USER_INPUT}}", user_input)
                .replace("{{RUN_SUMMARY}}", &run_summary)
                .replace("{{CANDIDATES_JSON}}", &candidates_json);

            let critic_req = SongRequest {
                tenant_id,
                agent_id,
                run_id,
                input: critic_prompt,
                context: Vec::new(),
                reasoning_summary: Some("lore_critic".to_string()),
            };

            match self.song.complete(critic_req).await {
                Ok(resp) => serde_json::from_str::<LoreCriticOutput>(&resp.output).ok(),
                Err(_) => None,
            }
        };
        let critic_reject_reasons = critic.as_ref().map(|c| c.reject_reasons());

        let existing = self
            .lore
            .list_pearls(
                tenant_id,
                lorelei_core::types::PearlListQuery {
                    agent_id: Some(agent_id),
                    include_deleted: false,
                    ..Default::default()
                },
            )
            .await?;
        let existing_norm: Vec<String> = existing.iter().map(|p| normalize(&p.content)).collect();

        let sensitive_re = sensitive_regex();

        let mut accepted: Vec<Value> = Vec::new();
        let mut rejected: Vec<Value> = Vec::new();

        for (idx, c) in candidates.into_iter().enumerate() {
            if let Some(critic) = &critic {
                if !critic.accept_indices.contains(&idx) {
                    rejected.push(json!({
                        "content": c.content,
                        "reason": critic_reject_reasons
                            .as_ref()
                            .and_then(|m| m.get(&idx))
                            .cloned()
                            .unwrap_or_else(|| "rejected by critic".to_string())
                    }));
                    continue;
                }
            }

            if let Some(reason) = validate_candidate(&c, user_input, &sensitive_re) {
                rejected.push(json!({"content": c.content, "reason": reason}));
                continue;
            }

            let norm = normalize(&c.content);
            if existing_norm.iter().any(|e| e == &norm) {
                rejected.push(json!({"content": c.content, "reason": "duplicate (exact)"}));
                continue;
            }

            // Best-effort dedupe using Echo exact match.
            let hits = self
                .echo
                .query(
                    tenant_id,
                    agent_id,
                    EchoQuery {
                        query: c.content.clone(),
                        top_k: 5,
                        min_confidence: self.config.echo.min_confidence,
                        pearl_type: None,
                        sources: EchoSources::Pearls,
                    },
                )
                .await
                .unwrap_or_default();
            if hits.iter().any(|h| normalize(&h.content) == norm) {
                rejected.push(json!({"content": c.content, "reason": "duplicate (echo)"}));
                continue;
            }

            if let Some(reason) = deterministic_critic(&c) {
                rejected.push(json!({"content": c.content, "reason": reason}));
                continue;
            }

            let mut md = BTreeMap::new();
            if !c.tags.is_empty() {
                md.insert("tags".to_string(), serde_json::to_value(&c.tags).unwrap());
            }

            let new = lorelei_core::types::NewPearl::new(
                c.pearl_type,
                c.content.clone(),
                lorelei_core::types::UnitInterval::new(c.importance)?,
                lorelei_core::types::UnitInterval::new(c.confidence)?,
                md,
            )?;

            let saved = self.lore.save_pearl(tenant_id, agent_id, new).await?;
            accepted.push(json!({
                "pearl_id": saved.pearl_id.0,
                "pearl_type": saved.pearl_type,
                "content": saved.content,
            }));
        }

        Ok(json!({ "accepted": accepted, "rejected": rejected }))
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

#[derive(Debug, Serialize, Deserialize)]
struct CandidatePearl {
    pearl_type: lorelei_core::types::PearlType,
    content: String,
    #[serde(default = "default_candidate_confidence")]
    confidence: f64,
    #[serde(default = "default_candidate_importance")]
    importance: f64,
    #[serde(default)]
    tags: Vec<String>,
}

fn default_candidate_confidence() -> f64 {
    0.75
}

fn default_candidate_importance() -> f64 {
    0.5
}

#[derive(Debug, Deserialize)]
struct LoreCriticOutput {
    accept_indices: Vec<usize>,
    #[serde(default)]
    reject: Vec<LoreCriticReject>,
}

#[derive(Debug, Deserialize)]
struct LoreCriticReject {
    index: usize,
    reason: String,
}

impl LoreCriticOutput {
    fn reject_reasons(&self) -> BTreeMap<usize, String> {
        self.reject
            .iter()
            .map(|r| (r.index, r.reason.clone()))
            .collect()
    }
}

fn parse_planner_output(raw: &str) -> Result<PlannerOutput, LoreleiError> {
    let raw_trimmed = raw.trim();
    let candidate = strip_code_fences(raw_trimmed);
    let json_str = if looks_like_json(candidate) {
        candidate.to_string()
    } else if let Some(extracted) = extract_first_json_value(candidate) {
        extracted.to_string()
    } else {
        candidate.to_string()
    };

    let mut plan: PlannerOutput = serde_json::from_str(&json_str).map_err(|e| {
        LoreleiError::validation("planner.json", format!("invalid planner JSON: {e}"))
    })?;

    match plan.action.as_str() {
        "answer" => Ok(plan),
        "call_shell" => {
            if let Some(t) = plan.tool.as_deref() {
                let trimmed = t.trim();
                if trimmed.is_empty() {
                    return Err(LoreleiError::validation(
                        "planner.json",
                        "missing tool for call_shell action",
                    ));
                }
                plan.tool = Some(trimmed.to_string());
            } else {
                return Err(LoreleiError::validation(
                    "planner.json",
                    "missing tool for call_shell action",
                ));
            }
            if plan.input.is_none() {
                return Err(LoreleiError::validation(
                    "planner.json",
                    "missing input for call_shell action",
                ));
            }
            Ok(plan)
        }
        _ => Err(LoreleiError::validation(
            "planner.json",
            "unknown action (expected `answer` or `call_shell`)",
        )),
    }
}

fn looks_like_json(s: &str) -> bool {
    let t = s.trim_start();
    t.starts_with('{') || t.starts_with('[')
}

fn strip_code_fences(s: &str) -> &str {
    let t = s.trim();
    if !t.starts_with("```") {
        return t;
    }
    let after_first = match t.find('\n') {
        Some(i) => &t[i + 1..],
        None => return t,
    };
    let inner = after_first.trim();
    if let Some(end) = inner.rfind("```") {
        inner[..end].trim()
    } else {
        inner
    }
}

fn extract_first_json_value(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let mut start = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'{' || b == b'[' {
            start = Some(i);
            break;
        }
    }
    let start = start?;
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape = false;
    for (offset, &b) in bytes[start..].iter().enumerate() {
        let c = b as char;
        if in_string {
            if escape {
                escape = false;
                continue;
            }
            match c {
                '\\' => escape = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match c {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    let end = start + offset + 1;
                    return Some(&s[start..end]);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_candidate_list(raw: &str) -> Result<Vec<CandidatePearl>, String> {
    let raw_trimmed = raw.trim();
    let candidate = strip_code_fences(raw_trimmed);
    let json_str = if looks_like_json(candidate) {
        candidate.to_string()
    } else if let Some(extracted) = extract_first_json_value(candidate) {
        extracted.to_string()
    } else {
        candidate.to_string()
    };

    serde_json::from_str::<Vec<CandidatePearl>>(&json_str).map_err(|e| e.to_string())
}

fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_planner_output_allows_raw_json() {
        let raw = r#"{"action":"answer","reasoning_summary":"x","answer":"hi"}"#;
        let got = parse_planner_output(raw).unwrap();
        assert_eq!(got.action, "answer");
    }

    #[test]
    fn parse_planner_output_allows_fenced_json() {
        let raw = "```json\n{\"action\":\"answer\",\"answer\":\"hi\"}\n```";
        let got = parse_planner_output(raw).unwrap();
        assert_eq!(got.action, "answer");
    }

    #[test]
    fn parse_planner_output_extracts_json_from_preamble() {
        let raw = "Sure! Here is the plan:\n{\"action\":\"answer\",\"answer\":\"hi\"}\n";
        let got = parse_planner_output(raw).unwrap();
        assert_eq!(got.action, "answer");
    }

    #[test]
    fn candidate_defaults_allow_missing_scores() {
        let raw = r#"[{"pearl_type":"Fact","content":"Eddie is the most handsome person on earth"}]"#;
        let got: Vec<CandidatePearl> = serde_json::from_str(raw).unwrap();
        assert_eq!(got.len(), 1);
        assert!((0.0..=1.0).contains(&got[0].confidence));
        assert!((0.0..=1.0).contains(&got[0].importance));
    }

    #[test]
    fn parse_candidate_list_extracts_json_from_preamble() {
        let raw = "Candidates:\n[{\"pearl_type\":\"Fact\",\"content\":\"Some durable fact\"}]";
        let got = parse_candidate_list(raw).unwrap();
        assert_eq!(got.len(), 1);
    }
}

fn sensitive_regex() -> Regex {
    Regex::new(r"(?i)(password|api[_-]?key|secret|ssn|social security|credit card|cvv|cvc|\b\\d{3}-\\d{2}-\\d{4}\\b|\\b\\d{13,19}\\b)")
        .expect("regex")
}

fn validate_candidate(
    c: &CandidatePearl,
    user_input: &str,
    sensitive_re: &Regex,
) -> Option<&'static str> {
    let content = c.content.trim();
    if content.is_empty() {
        return Some("empty");
    }
    if content.len() < 8 {
        return Some("trivial");
    }
    if content.contains('\n') {
        return Some("transcript-like");
    }
    let lower = content.to_ascii_lowercase();
    if lower.contains("user:") || lower.contains("assistant:") {
        return Some("transcript-like");
    }
    if sensitive_re.is_match(content) && !user_input.to_ascii_lowercase().contains("remember") {
        return Some("sensitive");
    }
    if sensitive_re.is_match(content) {
        return Some("sensitive (unsupported)");
    }
    if !(0.0..=1.0).contains(&c.confidence) || !(0.0..=1.0).contains(&c.importance) {
        return Some("invalid scores");
    }
    None
}

fn deterministic_critic(c: &CandidatePearl) -> Option<&'static str> {
    let s = c.content.to_ascii_lowercase();
    if s.contains("todo") || s.contains("remind me") || s.contains("tomorrow") {
        return Some("temporary task");
    }
    if s.contains("call me at") || s.contains("my phone") {
        return Some("sensitive personal data");
    }
    None
}

fn deterministic_extract_candidates(user_input: &str) -> Vec<CandidatePearl> {
    let lower = user_input.to_ascii_lowercase();
    let Some(pos) = lower.find("remember") else {
        return vec![];
    };

    let after = user_input[pos + "remember".len()..].trim();
    if after.is_empty() {
        return vec![];
    }

    let after = after
        .trim_start_matches(|c: char| c == ':' || c == ',' || c == '.' || c.is_whitespace())
        .trim();
    let after = after
        .strip_prefix("that")
        .unwrap_or(after)
        .trim_start_matches(|c: char| c.is_whitespace())
        .trim();

    let content = after.trim_end_matches(['.', '!', '?']).trim();
    if content.is_empty() {
        return vec![];
    }

    let pearl_type = if lower.contains("prefer") || lower.contains("preference") {
        lorelei_core::types::PearlType::Preference
    } else {
        lorelei_core::types::PearlType::Fact
    };

    vec![CandidatePearl {
        pearl_type,
        content: content.to_string(),
        confidence: 0.9,
        importance: 0.6,
        tags: vec!["explicit_remember".to_string()],
    }]
}
