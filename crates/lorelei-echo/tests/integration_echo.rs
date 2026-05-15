use lorelei_core::{EchoQuery, NewPearl, PearlType};
use lorelei_echo::{EchoConfig, EchoService};
use lorelei_lore::{LoreConfig, LoreStores};
use qdrant_client::Qdrant;
use sqlx::PgPool;
use uuid::Uuid;

struct TestEmbedder {
    vector: Vec<f32>,
}

#[async_trait::async_trait]
impl lorelei_echo::echo::Embedder for TestEmbedder {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, lorelei_core::LoreleiError> {
        Ok(self.vector.clone())
    }
}

#[tokio::test]
async fn tenant_isolation_and_deleted_exclusion() -> Result<(), Box<dyn std::error::Error>> {
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
    let collection = format!("lorelei_echo_test_{}", Uuid::new_v4());
    let lore = LoreStores::new(
        pool,
        qdrant,
        LoreConfig {
            qdrant_collection: collection.clone(),
        },
    )?;

    let echo = EchoService::new_for_test(
        lore.clone(),
        EchoConfig {
            qdrant_collection: collection,
            embedding_provider: "unused".to_string(),
            embedding_model: None,
            llm_rerank: false,
        },
        Box::new(TestEmbedder {
            vector: vec![0.0, 1.0, 0.0, 0.0],
        }),
    )?;

    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();
    let agent = Some(Uuid::new_v4());

    let run_a = lore.create_run(tenant_a, serde_json::json!({})).await?;
    let run_b = lore.create_run(tenant_b, serde_json::json!({})).await?;

    let pearl_a = lore
        .save_pearl(
            tenant_a,
            agent,
            run_a.id,
            NewPearl {
                pearl_type: PearlType::Memory,
                content: "tenant_a secret".to_string(),
                tags: vec![],
                confidence: Some(0.9),
                importance: None,
                metadata: serde_json::json!({}),
            },
        )
        .await?;
    let pearl_b = lore
        .save_pearl(
            tenant_b,
            agent,
            run_b.id,
            NewPearl {
                pearl_type: PearlType::Memory,
                content: "tenant_b secret".to_string(),
                tags: vec![],
                confidence: Some(0.9),
                importance: None,
                metadata: serde_json::json!({}),
            },
        )
        .await?;

    lore.upsert_pearl_vector(&pearl_a, vec![0.0, 1.0, 0.0, 0.0])
        .await?;
    lore.upsert_pearl_vector(&pearl_b, vec![0.0, 1.0, 0.0, 0.0])
        .await?;

    // Soft-delete A but keep vector around; Echo must exclude it via Postgres truth.
    lore.forget_pearl(tenant_a, pearl_a.id).await?;

    let hits_a = echo
        .retrieve(EchoQuery {
            text: "find secrets".to_string(),
            tenant_id: tenant_a,
            agent_id: agent,
            run_id: None,
            pearl_type: Some(PearlType::Memory),
            min_confidence: Some(0.0),
            limit: 10,
        })
        .await?;
    assert!(
        hits_a.iter().all(|h| h.pearl_id != pearl_a.id),
        "deleted pearls must be excluded"
    );
    assert!(
        hits_a.iter().all(|h| h.pearl_id != pearl_b.id),
        "must not leak cross-tenant pearls"
    );

    let hits_b = echo
        .retrieve(EchoQuery {
            text: "find secrets".to_string(),
            tenant_id: tenant_b,
            agent_id: agent,
            run_id: None,
            pearl_type: Some(PearlType::Memory),
            min_confidence: Some(0.0),
            limit: 10,
        })
        .await?;
    assert!(
        hits_b.iter().any(|h| h.pearl_id == pearl_b.id),
        "tenant_b should see its own pearl"
    );

    Ok(())
}

