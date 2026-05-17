use chrono::{TimeZone, Utc};
use lorelei_core::error::LoreleiError;
use lorelei_core::types::*;
use serde_json::json;
use std::collections::BTreeMap;
use uuid::Uuid;

fn fixed_time() -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000, 0).unwrap()
}

fn ids() -> (TenantId, AgentId, RunId) {
    (
        TenantId(Uuid::from_u128(1)),
        AgentId(Uuid::from_u128(2)),
        RunId(Uuid::from_u128(3)),
    )
}

#[test]
fn serde_round_trip_pearl() {
    let (tenant_id, agent_id, _) = ids();
    let pearl = Pearl {
        pearl_id: PearlId(Uuid::from_u128(10)),
        tenant_id,
        agent_id,
        pearl_type: PearlType::Fact,
        content: "A pearl of lore".to_string(),
        importance: UnitInterval::new(0.9).unwrap(),
        confidence: UnitInterval::new(0.8).unwrap(),
        created_at: fixed_time(),
        metadata: BTreeMap::new(),
    };

    let encoded = serde_json::to_string(&pearl).unwrap();
    let decoded: Pearl = serde_json::from_str(&encoded).unwrap();
    assert_eq!(pearl, decoded);
}

#[test]
fn serde_round_trip_echo_hit() {
    let hit = EchoHit {
        pearl_id: PearlId(Uuid::from_u128(11)),
        score: UnitInterval::new(0.7).unwrap(),
        content: "an echo".to_string(),
        pearl_type: PearlType::Other,
        reason: "v=0.7".to_string(),
        created_at: fixed_time(),
        citation: None,
    };

    let encoded = serde_json::to_string(&hit).unwrap();
    let decoded: EchoHit = serde_json::from_str(&encoded).unwrap();
    assert_eq!(hit, decoded);
}

#[test]
fn serde_round_trip_song_request() {
    let (tenant_id, agent_id, run_id) = ids();
    let req = SongRequest {
        tenant_id,
        agent_id,
        run_id,
        input: "Sing".to_string(),
        context: vec!["The Lore".to_string()],
        reasoning_summary: Some("brief".to_string()),
    };

    let encoded = serde_json::to_string(&req).unwrap();
    let decoded: SongRequest = serde_json::from_str(&encoded).unwrap();
    assert_eq!(req, decoded);
}

#[test]
fn serde_round_trip_shell_call() {
    let (tenant_id, agent_id, run_id) = ids();
    let call = ShellCall {
        call_id: Uuid::from_u128(12),
        tenant_id,
        agent_id,
        run_id,
        shell: "harbor".to_string(),
        tool: "echo".to_string(),
        input: json!({"q": "pearl"}),
        risk: ShellRisk::Low,
        requested_at: fixed_time(),
    };

    let encoded = serde_json::to_string(&call).unwrap();
    let decoded: ShellCall = serde_json::from_str(&encoded).unwrap();
    assert_eq!(call, decoded);
}

#[test]
fn serde_round_trip_siren_decision() {
    let decision = SirenDecision::Allow {
        reasoning_summary: "ok".to_string(),
    };

    let encoded = serde_json::to_string(&decision).unwrap();
    let decoded: SirenDecision = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decision, decoded);

    let decision2 = SirenDecision::RequireApproval {
        reasoning_summary: "need approval".to_string(),
        approval_prompt: "approve?".to_string(),
    };
    let encoded2 = serde_json::to_string(&decision2).unwrap();
    let decoded2: SirenDecision = serde_json::from_str(&encoded2).unwrap();
    assert_eq!(decision2, decoded2);
}

#[test]
fn invalid_confidence_is_rejected() {
    let err = UnitInterval::new(1.1).unwrap_err();
    assert!(matches!(err, LoreleiError::Validation { .. }));

    let err2: Result<UnitInterval, _> = serde_json::from_str("1.1");
    assert!(err2.is_err());
}

#[test]
fn empty_pearl_content_is_rejected() {
    let err = NewPearl::new(
        PearlType::Fact,
        "   ",
        UnitInterval::new(0.5).unwrap(),
        UnitInterval::new(0.5).unwrap(),
        BTreeMap::new(),
    )
    .unwrap_err();
    assert!(matches!(err, LoreleiError::Validation { .. }));

    let (tenant_id, agent_id, _) = ids();
    let raw = json!({
        "pearl_id": PearlId(Uuid::from_u128(13)),
        "tenant_id": tenant_id,
        "agent_id": agent_id,
        "pearl_type": "Fact",
        "content": "",
        "importance": 0.5,
        "confidence": 0.5,
        "created_at": fixed_time(),
        "metadata": {}
    });

    let decoded: Result<Pearl, _> = serde_json::from_value(raw);
    assert!(decoded.is_err());
}

#[test]
fn shell_risk_ordering_is_predictable() {
    assert!(ShellRisk::None < ShellRisk::Low);
    assert!(ShellRisk::Low < ShellRisk::Medium);
    assert!(ShellRisk::Medium < ShellRisk::High);
    assert!(ShellRisk::High < ShellRisk::Critical);
}
