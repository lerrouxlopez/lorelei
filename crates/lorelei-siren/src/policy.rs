#![forbid(unsafe_code)]

use async_trait::async_trait;
use lorelei_core::config::LoreleiConfig;
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::ApprovalStore;
use lorelei_core::traits::SirenPolicy;
use lorelei_core::types::{
    AgentId, AutonomousTaskId, NormalizedToolCall, RunId, SirenDecision, SongRequest, SongResponse,
    TenantId,
};
use std::sync::Arc;
use tracing::info;

#[derive(Clone)]
pub struct DeterministicSirenPolicy {
    config: LoreleiConfig,
    enable_llm_policy: bool,
    approvals: Option<Arc<dyn ApprovalStore>>,
}

impl DeterministicSirenPolicy {
    pub fn new(config: LoreleiConfig) -> Self {
        Self {
            config,
            enable_llm_policy: false,
            approvals: None,
        }
    }

    pub fn with_approval_store(mut self, store: Arc<dyn ApprovalStore>) -> Self {
        self.approvals = Some(store);
        self
    }

    pub fn with_llm_policy_enabled(mut self, enabled: bool) -> Self {
        self.enable_llm_policy = enabled;
        self
    }

    fn approval_prompt(&self, request: &SongRequest, tool_names: &[String]) -> String {
        // Keep this short, deterministic, and safe to display.
        let tools = if tool_names.is_empty() {
            "<none>".to_string()
        } else {
            tool_names.join(", ")
        };
        format!(
            "Approval required.\n\nTenant: {}\nAgent: {}\nRun: {}\nRequested tools: {}\n\nUser message:\n{}",
            request.tenant_id.0, request.agent_id.0, request.run_id.0, tools, request.input
        )
    }

    fn extract_tool_names(tool_calls: &[NormalizedToolCall]) -> Vec<String> {
        tool_calls
            .iter()
            .map(|t| t.name.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    fn medium_risk_tools(&self) -> &'static [&'static str] {
        &["http_get", "save_pearl", "document_ingest"]
    }

    fn high_risk_tools(&self) -> &'static [&'static str] {
        &["forget_pearl"]
    }

    fn risk_for_tools(&self, tools: &[String]) -> lorelei_core::types::ShellRisk {
        if tools
            .iter()
            .any(|t| self.high_risk_tools().contains(&t.as_str()))
        {
            return lorelei_core::types::ShellRisk::High;
        }
        if tools
            .iter()
            .any(|t| self.medium_risk_tools().contains(&t.as_str()))
        {
            return lorelei_core::types::ShellRisk::Medium;
        }
        if tools.iter().any(|t| Self::is_readonly_low_risk(t)) {
            return lorelei_core::types::ShellRisk::Low;
        }
        lorelei_core::types::ShellRisk::Medium
    }

    fn is_readonly_low_risk(tool: &str) -> bool {
        matches!(
            tool,
            "noop" | "echo" | "echo_lore" | "list_pearls" | "document_search"
        )
    }

    fn requires_clear_user_intent(&self, request: &SongRequest, tool: &str) -> bool {
        // Conservative heuristic: if the user asked a direct action containing a verb or
        // the tool name, we treat it as clear intent.
        let msg = request.input.to_ascii_lowercase();
        let tool_lc = tool.to_ascii_lowercase();
        msg.contains(&tool_lc)
            || msg.contains("fetch")
            || msg.contains("http")
            || msg.contains("download")
            || msg.contains("request")
            || msg.contains("save")
            || msg.contains("store")
            || msg.contains("remember")
            || msg.contains("forget")
            || msg.contains("delete")
    }

    #[allow(clippy::too_many_arguments)]
    async fn deterministic_decision(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        _run_id: RunId,
        task_id: Option<AutonomousTaskId>,
        request: &SongRequest,
        tool_calls: &[NormalizedToolCall],
        shell_names: &[String],
    ) -> Result<SirenDecision, LoreleiError> {
        // Cross-tenant access is always denied.
        if request.tenant_id != tenant_id || request.agent_id != agent_id {
            return Ok(SirenDecision::Deny {
                reasoning_summary: "cross-tenant or cross-agent mismatch".to_string(),
            });
        }

        let mut requested_tools = Self::extract_tool_names(tool_calls);
        requested_tools.extend(
            shell_names
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
        requested_tools.sort();
        requested_tools.dedup();

        if requested_tools.is_empty() {
            return Ok(SirenDecision::Allow {
                reasoning_summary: "no tools requested".to_string(),
            });
        }

        // Shell execution is denied unless explicitly enabled.
        if !self.config.siren.allow_shell_execution {
            return Ok(SirenDecision::Deny {
                reasoning_summary:
                    "shell execution is disabled (siren.allow_shell_execution=false)".to_string(),
            });
        }

        // Network tools are denied unless explicitly enabled.
        if requested_tools.iter().any(|t| t == "http_get") && !self.config.siren.allow_network_tools
        {
            return Ok(SirenDecision::Deny {
                reasoning_summary: "network tools are disabled (siren.allow_network_tools=false)"
                    .to_string(),
            });
        }

        // High-risk tools require explicit approval (unless already approved for this task).
        if requested_tools
            .iter()
            .any(|t| self.high_risk_tools().contains(&t.as_str()))
        {
            if let Some(store) = &self.approvals {
                // If all high-risk tool calls are approved, allow.
                let mut all_ok = true;
                for call in tool_calls
                    .iter()
                    .filter(|c| self.high_risk_tools().contains(&c.name.as_str()))
                {
                    let ok = store
                        .is_approved(tenant_id, agent_id, task_id, &call.name, &call.arguments)
                        .await?;
                    if !ok {
                        all_ok = false;
                        break;
                    }
                }
                if all_ok {
                    return Ok(SirenDecision::Allow {
                        reasoning_summary: "approved high-risk tool".to_string(),
                    });
                }
            }
            let prompt = self.approval_prompt(request, &requested_tools);
            return Ok(SirenDecision::RequireApproval {
                reasoning_summary: "high-risk tool requires explicit approval".to_string(),
                approval_prompt: prompt,
            });
        }

        // Medium-risk tools require permission and clear user intent; if the configuration
        // requires approvals for high-risk actions, we still keep medium-risk gated.
        if requested_tools
            .iter()
            .any(|t| self.medium_risk_tools().contains(&t.as_str()))
        {
            for tool in requested_tools
                .iter()
                .filter(|t| self.medium_risk_tools().contains(&t.as_str()))
            {
                if !self.requires_clear_user_intent(request, tool) {
                    let prompt = self.approval_prompt(request, &requested_tools);
                    return Ok(SirenDecision::RequireApproval {
                        reasoning_summary: "medium-risk tool requires clear user intent"
                            .to_string(),
                        approval_prompt: prompt,
                    });
                }
            }
        }

        // Low-risk read-only shells may proceed.
        if requested_tools.iter().all(|t| {
            Self::is_readonly_low_risk(t) || self.medium_risk_tools().contains(&t.as_str())
        }) {
            return Ok(SirenDecision::Allow {
                reasoning_summary: "deterministic checks passed".to_string(),
            });
        }

        // Unknown or unclassified tools are denied by default.
        Ok(SirenDecision::Deny {
            reasoning_summary: "unknown or unclassified tool request denied".to_string(),
        })
    }
}

#[async_trait]
impl SirenPolicy for DeterministicSirenPolicy {
    async fn decide(
        &self,
        tenant_id: TenantId,
        agent_id: AgentId,
        run_id: RunId,
        task_id: Option<AutonomousTaskId>,
        request: &SongRequest,
        _response: &SongResponse,
        tool_calls: &[NormalizedToolCall],
        shell_names: &[String],
    ) -> Result<SirenDecision, LoreleiError> {
        let det = self
            .deterministic_decision(
                tenant_id,
                agent_id,
                run_id,
                task_id,
                request,
                tool_calls,
                shell_names,
            )
            .await?;

        let mut requested_tools = Self::extract_tool_names(tool_calls);
        requested_tools.extend(shell_names.iter().cloned());
        requested_tools.sort();
        requested_tools.dedup();
        let risk = self.risk_for_tools(&requested_tools);
        let decision = match &det {
            SirenDecision::Allow { .. } => "allow",
            SirenDecision::Deny { .. } => "deny",
            SirenDecision::RequireApproval { .. } => "require_approval",
        };
        let reason = match &det {
            SirenDecision::Allow { reasoning_summary } => reasoning_summary.as_str(),
            SirenDecision::Deny { reasoning_summary } => reasoning_summary.as_str(),
            SirenDecision::RequireApproval {
                reasoning_summary, ..
            } => reasoning_summary.as_str(),
        };
        let task_id_str = task_id.map(|t| t.0.to_string());
        info!(
            run_id = %run_id.0,
            tenant_id = %tenant_id.0,
            agent_id = %agent_id.0,
            task_id = task_id_str.as_deref(),
            decision = decision,
            risk = ?risk,
            reason = reason,
            tools = requested_tools.len(),
            "siren.decision"
        );

        if !self.enable_llm_policy {
            return Ok(det);
        }

        // Optional LLM policy support is intentionally disabled by default and not wired yet.
        // The deterministic result remains authoritative.
        Ok(det)
    }
}
