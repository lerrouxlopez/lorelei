use std::collections::BTreeMap;

use chrono::{TimeZone, Utc};
use lorelei_core::{
    CurrentEvent, EchoHit, EchoQuery, NewPearl, Pearl, PearlType, ProposedAction, Run, ShellCall,
    ShellRisk, SirenDecision, SongChunk, SongRequest, SongResponse,
};
use serde_json::json;
use uuid::Uuid;

#[test]
fn serde_roundtrip_run() {
    let run = Run {
        id: Uuid::new_v4(),
        started_at: Utc.with_ymd_and_hms(2026, 5, 15, 0, 0, 0).unwrap(),
        ended_at: None,
        metadata: json!({"tag": "test"}),
    };

    let encoded = serde_json::to_string(&run).unwrap();
    let decoded: Run = serde_json::from_str(&encoded).unwrap();
    assert_eq!(run, decoded);
    decoded.validate().unwrap();
}

#[test]
fn serde_roundtrip_pearl_and_new_pearl() {
    let new_pearl = NewPearl {
        pearl_type: PearlType::Memory,
        content: "hello".to_string(),
        tags: vec!["t1".to_string()],
        confidence: Some(0.5),
        importance: None,
        metadata: json!({"k":"v"}),
    };
    new_pearl.validate().unwrap();

    let pearl = Pearl {
        id: Uuid::new_v4(),
        tenant_id: Uuid::new_v4(),
        agent_id: Some(Uuid::new_v4()),
        run_id: Uuid::new_v4(),
        pearl_type: PearlType::Insight,
        created_at: Utc.with_ymd_and_hms(2026, 5, 15, 1, 2, 3).unwrap(),
        deleted_at: None,
        last_echoed_at: None,
        content: "world".to_string(),
        tags: vec!["t2".to_string()],
        confidence: Some(1.0),
        importance: Some(0.0),
        metadata: json!({"m":1}),
    };

    let encoded = serde_json::to_string(&pearl).unwrap();
    let decoded: Pearl = serde_json::from_str(&encoded).unwrap();
    assert_eq!(pearl, decoded);
    decoded.validate().unwrap();
}

#[test]
fn serde_roundtrip_song_request_response_chunk() {
    let mut parameters = BTreeMap::new();
    parameters.insert("temperature".to_string(), json!(0.2));

    let req = SongRequest {
        prompt: "sing".to_string(),
        context: vec![],
        max_chunks: Some(3),
        parameters,
    };
    req.validate().unwrap();

    let chunk = SongChunk {
        index: 0,
        content: "la".to_string(),
        is_final: true,
        tool_calls: vec![],
    };
    chunk.validate().unwrap();

    let resp = SongResponse {
        id: Uuid::new_v4(),
        created_at: Utc.with_ymd_and_hms(2026, 5, 15, 2, 3, 4).unwrap(),
        chunks: vec![chunk],
        metadata: json!({"provider": "test"}),
    };
    resp.validate().unwrap();

    let encoded = serde_json::to_string(&resp).unwrap();
    let decoded: SongResponse = serde_json::from_str(&encoded).unwrap();
    assert_eq!(resp, decoded);
}

#[test]
fn serde_roundtrip_events_hits_and_decisions() {
    let event = CurrentEvent {
        id: Uuid::new_v4(),
        run_id: Uuid::new_v4(),
        at: Utc.with_ymd_and_hms(2026, 5, 15, 3, 0, 0).unwrap(),
        kind: "boot".to_string(),
        payload: json!({"ok": true}),
    };
    event.validate().unwrap();

    let hit = EchoHit {
        pearl_id: Uuid::new_v4(),
        score: 0.5,
        snippet: Some("...".to_string()),
        metadata: json!({"source": "test"}),
    };
    hit.validate().unwrap();

    let action = ProposedAction {
        tenant_id: Uuid::new_v4(),
        target_tenant_id: Uuid::new_v4(),
        call: ShellCall {
            program: "echo".to_string(),
            args: vec!["hi".to_string()],
            cwd: None,
            env: BTreeMap::new(),
            stdin: None,
            timeout_ms: Some(10_000),
        },
        risk: ShellRisk::Low,
        rationale: "test".to_string(),
    };
    action.validate().unwrap();

    let decision = SirenDecision {
        allow: true,
        risk: ShellRisk::Low,
        reasons: vec!["ok".to_string()],
        proposed: Some(action),
    };

    let encoded = serde_json::to_string(&(event.clone(), hit.clone(), decision.clone())).unwrap();
    let decoded: (CurrentEvent, EchoHit, SirenDecision) = serde_json::from_str(&encoded).unwrap();
    assert_eq!(event, decoded.0);
    assert_eq!(hit, decoded.1);
    assert_eq!(decision, decoded.2);
}

#[test]
fn validation_rejects_empty_strings_and_bad_ranges() {
    assert!(NewPearl {
        pearl_type: PearlType::Note,
        content: "   ".to_string(),
        tags: vec![],
        confidence: None,
        importance: None,
        metadata: json!(null),
    }
    .validate()
    .is_err());

    assert!(EchoQuery {
        text: "x".to_string(),
        tenant_id: Uuid::new_v4(),
        agent_id: None,
        run_id: None,
        pearl_type: None,
        min_confidence: None,
        limit: 0,
    }
    .validate()
    .is_err());

    assert!(ShellCall {
        program: "".to_string(),
        args: vec![],
        cwd: None,
        env: BTreeMap::new(),
        stdin: None,
        timeout_ms: None,
    }
    .validate()
    .is_err());

    let run = Run {
        id: Uuid::new_v4(),
        started_at: Utc.with_ymd_and_hms(2026, 5, 15, 10, 0, 0).unwrap(),
        ended_at: Some(Utc.with_ymd_and_hms(2026, 5, 15, 9, 0, 0).unwrap()),
        metadata: json!(null),
    };
    assert!(run.validate().is_err());
}
