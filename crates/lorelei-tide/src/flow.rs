#![forbid(unsafe_code)]

use async_trait::async_trait;
use lorelei_core::error::LoreleiError;
use lorelei_core::traits::{ShellRegistry, SirenPolicy};
use lorelei_core::types::{
    NormalizedToolCall, ShellCall, ShellResult, SirenDecision, SongRequest, SongResponse,
};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct TideShellExecutor {
    policy: Arc<dyn SirenPolicy>,
    shells: Arc<dyn ShellRegistry>,
}

impl TideShellExecutor {
    pub fn new(policy: Arc<dyn SirenPolicy>, shells: Arc<dyn ShellRegistry>) -> Self {
        Self { policy, shells }
    }

    pub async fn execute_shell_call(
        &self,
        call: ShellCall,
        user_approved: bool,
    ) -> Result<ShellResult, LoreleiError> {
        let request = SongRequest {
            tenant_id: call.tenant_id,
            agent_id: call.agent_id,
            run_id: call.run_id,
            input: format!("tool: {}", call.tool),
            context: Vec::new(),
            reasoning_summary: None,
        };
        let response = SongResponse {
            output: String::new(),
            reasoning_summary: None,
            tool_calls: Vec::new(),
        };

        let tool_calls = vec![NormalizedToolCall {
            call_id: Uuid::new_v4().to_string(),
            name: call.tool.clone(),
            arguments: call.input.clone(),
        }];

        let decision = self
            .policy
            .decide(
                call.tenant_id,
                call.agent_id,
                call.run_id,
                None,
                &request,
                &response,
                &tool_calls,
                &[],
            )
            .await?;

        match decision {
            SirenDecision::Allow { .. } => self.shells.call(call).await,
            SirenDecision::Deny { reasoning_summary } => Err(LoreleiError::Shell(format!(
                "siren denied: {reasoning_summary}"
            ))),
            SirenDecision::RequireApproval {
                reasoning_summary,
                approval_prompt: _,
            } => {
                if user_approved {
                    self.shells.call(call).await
                } else {
                    Err(LoreleiError::Shell(format!(
                        "siren requires approval: {reasoning_summary}"
                    )))
                }
            }
        }
    }
}

#[async_trait]
pub trait TideGate: Send + Sync {
    async fn execute(
        &self,
        call: ShellCall,
        user_approved: bool,
    ) -> Result<ShellResult, LoreleiError>;
}

#[async_trait]
impl TideGate for TideShellExecutor {
    async fn execute(
        &self,
        call: ShellCall,
        user_approved: bool,
    ) -> Result<ShellResult, LoreleiError> {
        self.execute_shell_call(call, user_approved).await
    }
}
