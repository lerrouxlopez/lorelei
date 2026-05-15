use lorelei_core::{ProposedAction, ShellCall, ShellRisk, SirenPolicy};
use lorelei_siren::{DeterministicSirenPolicy, SirenConfig};
use uuid::Uuid;

fn call(program: &str, args: &[&str]) -> ShellCall {
    ShellCall {
        program: program.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        cwd: None,
        env: Default::default(),
        stdin: None,
        timeout_ms: None,
    }
}

#[tokio::test]
async fn read_only_allowed() {
    let tenant = Uuid::new_v4();
    let policy = DeterministicSirenPolicy::new(SirenConfig {
        allow_shell_execution: true,
        allow_network_tools: false,
    });

    let action = ProposedAction {
        tenant_id: tenant,
        target_tenant_id: tenant,
        call: call("ls", &["-la"]),
        risk: ShellRisk::Low,
        rationale: "list files".to_string(),
    };

    let d = policy.decide(action).await.unwrap();
    assert!(d.allow);
    assert_eq!(d.risk, ShellRisk::Low);
}

#[tokio::test]
async fn delete_requires_approval() {
    let tenant = Uuid::new_v4();
    let policy = DeterministicSirenPolicy::new(SirenConfig {
        allow_shell_execution: true,
        allow_network_tools: true,
    });

    let action = ProposedAction {
        tenant_id: tenant,
        target_tenant_id: tenant,
        call: call("rm", &["-rf", "somewhere"]),
        risk: ShellRisk::Low,
        rationale: "cleanup".to_string(),
    };

    let d = policy.decide(action).await.unwrap();
    assert!(!d.allow);
    assert_eq!(d.risk, ShellRisk::High);
}

#[tokio::test]
async fn shell_execution_denied_by_default() {
    let tenant = Uuid::new_v4();
    let policy = DeterministicSirenPolicy::new(SirenConfig::default());

    let action = ProposedAction {
        tenant_id: tenant,
        target_tenant_id: tenant,
        call: call("ls", &[]),
        risk: ShellRisk::Low,
        rationale: "try".to_string(),
    };

    let d = policy.decide(action).await.unwrap();
    assert!(!d.allow);
    assert_eq!(d.risk, ShellRisk::Forbidden);
}

#[tokio::test]
async fn cross_tenant_access_denied() {
    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();
    let policy = DeterministicSirenPolicy::new(SirenConfig {
        allow_shell_execution: true,
        allow_network_tools: true,
    });

    let action = ProposedAction {
        tenant_id: tenant_a,
        target_tenant_id: tenant_b,
        call: call("cat", &["file"]),
        risk: ShellRisk::Low,
        rationale: "nope".to_string(),
    };

    let d = policy.decide(action).await.unwrap();
    assert!(!d.allow);
    assert_eq!(d.risk, ShellRisk::Forbidden);
}

