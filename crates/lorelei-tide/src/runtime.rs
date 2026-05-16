#![forbid(unsafe_code)]

use async_trait::async_trait;
use lorelei_core::config::LoreleiConfig;
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::{
    CurrentStore, EchoRetriever, LoreStore, ShellRegistry, SirenPolicy, SongProvider,
};
use lorelei_core::types::{
    CurrentEvent, CurrentEventType, EchoHit, EchoQuery, NormalizedToolCall, Run, RunId, RunStatus,
    ShellCall, ShellResult, ShellRisk, SirenDecision, SongRequest, SongResponse, TenantId,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use tracing::info_span;
use tracing::field;
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
                    },
                )
                .instrument(info_span!("tide.echo"))
                .await?
        } else {
            Vec::new()
        };

        let mut shell_result: Option<ShellResult> = None;

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
                        shell_result = Some(result.clone());

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
                        // Emit a structured current so workers/clients can create an approval request.
                        self.append_current(
                            tenant_id,
                            agent_id,
                            run.run_id,
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

        // Reflection + memory formation (best-effort).
        if enable_memory {
            let memory_decisions = self
                .form_memories(
                    run.run_id,
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
                run.run_id,
                lorelei_core::types::EchoId(Uuid::new_v4()),
                CurrentEventType::System,
                "memory formation",
                json!({ "decisions": memory_decisions }),
            )
            .await?;
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

            serde_json::from_str(&resp.output).map_err(|e| {
                LoreleiError::validation(
                    "lore_extractor.json",
                    format!("invalid candidate JSON: {e}"),
                )
            })?
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
    confidence: f64,
    importance: f64,
    #[serde(default)]
    tags: Vec<String>,
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
    serde_json::from_str(raw)
        .map_err(|e| LoreleiError::validation("planner.json", format!("invalid planner JSON: {e}")))
}

fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
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
