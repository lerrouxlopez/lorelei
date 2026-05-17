use chrono::Utc;
use lorelei_core::traits::CurrentStore;
use lorelei_core::types::{AgentId, CurrentEvent, CurrentEventType, EchoId, TenantId};
use lorelei_harbor::runtime::pg::PgCurrentStore;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

async fn maybe_pool() -> Option<sqlx::PgPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return None,
    };
    Some(
        PgPoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .expect("connect DATABASE_URL"),
    )
}

#[tokio::test]
async fn pg_current_store_smoke_create_write_list() {
    let Some(pool) = maybe_pool().await else {
        return; // gated: only run when DATABASE_URL is set
    };

    let store = PgCurrentStore::new(pool);
    store.migrate().await.expect("migrate");

    let tenant_id = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());
    let run = store
        .create_run(tenant_id, agent_id, "test goal")
        .await
        .expect("create_run");

    let event = CurrentEvent {
        event_id: EchoId(Uuid::new_v4()),
        tenant_id,
        agent_id,
        run_id: run.run_id,
        event_type: CurrentEventType::User,
        created_at: Utc::now(),
        summary: "hello".to_string(),
        data: serde_json::json!({"msg": "world"}),
    };

    store
        .append_current_event(tenant_id, agent_id, run.run_id, event.clone())
        .await
        .expect("append_current_event");

    let got = store
        .list_current_events(tenant_id, agent_id, run.run_id, 10)
        .await
        .expect("list_current_events");

    assert_eq!(got.len(), 1);
    assert_eq!(got[0].tenant_id, tenant_id);
    assert_eq!(got[0].agent_id, agent_id);
    assert_eq!(got[0].run_id, run.run_id);
    assert_eq!(got[0].event_type, CurrentEventType::User);
    assert_eq!(got[0].summary, "hello");
}

#[tokio::test]
async fn list_currents_scoped_by_run_and_tenant() {
    let Some(pool) = maybe_pool().await else {
        return;
    };

    let store = PgCurrentStore::new(pool);
    store.migrate().await.expect("migrate");

    let tenant_a = TenantId(Uuid::new_v4());
    let tenant_b = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());

    let run_a = store
        .create_run(tenant_a, agent_id, "run a")
        .await
        .expect("create_run a");
    let run_b = store
        .create_run(tenant_b, agent_id, "run b")
        .await
        .expect("create_run b");

    let event_a = CurrentEvent {
        event_id: EchoId(Uuid::new_v4()),
        tenant_id: tenant_a,
        agent_id,
        run_id: run_a.run_id,
        event_type: CurrentEventType::System,
        created_at: Utc::now(),
        summary: "a".to_string(),
        data: serde_json::json!({}),
    };
    store
        .append_current_event(tenant_a, agent_id, run_a.run_id, event_a)
        .await
        .expect("append a");

    let event_b = CurrentEvent {
        event_id: EchoId(Uuid::new_v4()),
        tenant_id: tenant_b,
        agent_id,
        run_id: run_b.run_id,
        event_type: CurrentEventType::System,
        created_at: Utc::now(),
        summary: "b".to_string(),
        data: serde_json::json!({}),
    };
    store
        .append_current_event(tenant_b, agent_id, run_b.run_id, event_b)
        .await
        .expect("append b");

    let got_a = store
        .list_current_events(tenant_a, agent_id, run_a.run_id, 10)
        .await
        .expect("list a");
    assert_eq!(got_a.len(), 1);
    assert_eq!(got_a[0].summary, "a");

    let got_b = store
        .list_current_events(tenant_b, agent_id, run_b.run_id, 10)
        .await
        .expect("list b");
    assert_eq!(got_b.len(), 1);
    assert_eq!(got_b[0].summary, "b");
}
