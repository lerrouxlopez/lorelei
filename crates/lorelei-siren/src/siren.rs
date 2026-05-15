use std::path::Path;

use async_trait::async_trait;
use lorelei_core::{LoreleiError, ProposedAction, ShellRisk, SirenDecision, SirenPolicy};
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SirenConfig {
    #[serde(default)]
    pub allow_shell_execution: bool,
    #[serde(default)]
    pub allow_network_tools: bool,
}

impl Default for SirenConfig {
    fn default() -> Self {
        Self {
            allow_shell_execution: false,
            allow_network_tools: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SirenPrompt {
    pub text: String,
}

impl SirenPrompt {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, LoreleiError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| LoreleiError::SirenPolicy {
            message: format!("failed to read siren prompt `{}`: {e}", path.display()),
        })?;
        Ok(Self { text })
    }
}

#[derive(Debug, Clone)]
pub struct DeterministicSirenPolicy {
    pub cfg: SirenConfig,
    pub prompt: Option<SirenPrompt>,
}

impl DeterministicSirenPolicy {
    pub fn new(cfg: SirenConfig) -> Self {
        Self { cfg, prompt: None }
    }

    pub fn with_prompt(mut self, prompt: SirenPrompt) -> Self {
        self.prompt = Some(prompt);
        self
    }

    fn decide_deterministic(&self, action: &ProposedAction) -> SirenDecision {
        let mut reasons = Vec::new();

        if action.tenant_id != action.target_tenant_id {
            reasons.push("cross-tenant access denied".to_string());
            return SirenDecision {
                allow: false,
                risk: ShellRisk::Forbidden,
                reasons,
                proposed: None,
            };
        }

        if !self.cfg.allow_shell_execution {
            reasons.push("shell execution disabled (allow_shell_execution=false)".to_string());
            return SirenDecision {
                allow: false,
                risk: ShellRisk::Forbidden,
                reasons,
                proposed: None,
            };
        }

        let classified = classify_shell_risk(&action.call.program, &action.call.args);

        if !self.cfg.allow_network_tools && classified.is_network_tool {
            reasons.push("external network tools disabled (allow_network_tools=false)".to_string());
            return SirenDecision {
                allow: false,
                risk: ShellRisk::High,
                reasons,
                proposed: Some(action.clone()),
            };
        }

        match classified.risk {
            ShellRisk::None => {
                reasons.push("risk classified as none".to_string());
                SirenDecision {
                    allow: true,
                    risk: ShellRisk::Low,
                    reasons,
                    proposed: Some(action.clone()),
                }
            }
            ShellRisk::Low | ShellRisk::Medium => {
                reasons.push(format!("risk classified as {:?}", classified.risk).to_lowercase());
                SirenDecision {
                    allow: true,
                    risk: classified.risk,
                    reasons,
                    proposed: Some(action.clone()),
                }
            }
            ShellRisk::High => {
                reasons.push("high-risk action requires approval".to_string());
                SirenDecision {
                    allow: false,
                    risk: ShellRisk::High,
                    reasons,
                    proposed: Some(action.clone()),
                }
            }
            ShellRisk::Forbidden => SirenDecision {
                allow: false,
                risk: ShellRisk::Forbidden,
                reasons,
                proposed: None,
            },
        }
    }
}

#[async_trait]
impl SirenPolicy for DeterministicSirenPolicy {
    async fn decide(&self, action: ProposedAction) -> Result<SirenDecision, LoreleiError> {
        action.validate()?;
        let start = std::time::Instant::now();

        // Deterministic checks first.
        let decision = self.decide_deterministic(&action);
        info!(
            tenant_id = %action.tenant_id,
            target_tenant_id = %action.target_tenant_id,
            shell_name = %action.call.program,
            shell_risk = ?decision.risk,
            siren_allow = decision.allow,
            latency_ms = start.elapsed().as_millis() as u64,
            "siren.decision"
        );
        if !decision.allow {
            return Ok(decision);
        }

        // Optional prompt is loaded/available for future LLM policy checks, but we do not
        // execute any LLM here yet.
        Ok(decision)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Classification {
    risk: ShellRisk,
    is_network_tool: bool,
}

fn classify_shell_risk(program: &str, args: &[String]) -> Classification {
    let p = program.trim().to_lowercase();
    let a = args
        .iter()
        .map(|s| s.trim().to_lowercase())
        .collect::<Vec<_>>();

    // Network-ish tools (blocked unless allow_network_tools=true).
    let is_network_tool = matches!(
        p.as_str(),
        "curl" | "wget" | "powershell" | "pwsh" | "invoke-webrequest" | "http"
    ) || a.iter().any(|x| x.contains("http://") || x.contains("https://"));

    // Obvious destructive commands.
    let is_delete = matches!(
        p.as_str(),
        "rm" | "del" | "erase" | "rmdir" | "remove-item" | "unlink" | "shred"
    ) || (p == "git" && a.first().is_some_and(|x| x == "clean"));

    if is_delete {
        return Classification {
            risk: ShellRisk::High,
            is_network_tool,
        };
    }

    // File writes/changes (medium).
    let is_write = matches!(
        p.as_str(),
        "mv" | "move" | "copy" | "cp" | "mkdir" | "touch" | "tee" | "set-content" | "out-file"
    );

    if is_write {
        return Classification {
            risk: ShellRisk::Medium,
            is_network_tool,
        };
    }

    // Read-only-ish.
    Classification {
        risk: ShellRisk::Low,
        is_network_tool,
    }
}
