use chrono::Utc;
use lorelei_core::{NewPearl, PearlType};
use lorelei_lore::{LoreConfig, LoreStores, PearlQueryFilter};
use qdrant_client::Qdrant;
use sqlx::PgPool;
use uuid::Uuid;

#[tokio::test]
async fn pg_is_source_of_truth_qdrant_is_index_only() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = match std::env::var("DATABASE_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return Ok(()),
    };
    let qdrant_url = match std::env::var("QDRANT_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return Ok(()),
    };

    let pool = PgPool::connect(&database_url).await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;

    let qdrant = Qdrant::from_url(&qdrant_url).build()?;
    let collection = format!("lorelei_test_{}", Uuid::new_v4());
    let stores = LoreStores::new(
        pool,
        qdrant,
        LoreConfig {
            qdrant_collection: collection,
        },
    )?;

    let tenant_id = Uuid::new_v4();
    let agent_id = Some(Uuid::new_v4());

    let run = stores.create_run(tenant_id, serde_json::json!({"t":"it"})).await?;

    let pearl = stores
        .save_pearl(
            tenant_id,
            agent_id,
            run.id,
            NewPearl {
                pearl_type: PearlType::Memory,
                content: "hello world".to_string(),
                tags: vec!["tag1".to_string()],
                confidence: Some(0.9),
                importance: Some(0.1),
                metadata: serde_json::json!({"m":1}),
            },
        )
        .await?;

    stores
        .upsert_pearl_vector(&pearl, vec![0.0, 1.0, 0.0, 0.0])
        .await?;

    let hits = stores
        .search_pearl_vectors(
            vec![0.0, 1.0, 0.0, 0.0],
            PearlQueryFilter {
                tenant_id,
                agent_id,
                pearl_type: Some(PearlType::Memory),
                tags_any: &["tag1".to_string()],
                include_deleted: false,
            },
            10,
        )
        .await?;

    assert!(hits.iter().any(|h| h.pearl_id == pearl.id));

    stores
        .update_last_echoed_at(tenant_id, pearl.id, Utc::now())
        .await?;

    let fetched = stores.get_pearl(tenant_id, pearl.id, false).await?;
    assert!(fetched.is_some());

    stores.forget_pearl(tenant_id, pearl.id).await?;
    let fetched = stores.get_pearl(tenant_id, pearl.id, false).await?;
    assert!(fetched.is_none());

    stores.delete_pearl_vector(pearl.id).await?;
    Ok(())
}

