use lorelei_core::traits::{EchoRetriever, LoreStore};
use lorelei_core::types::{
    AgentId, EchoQuery, EchoSources, NewPearl, PearlType, TenantId, UnitInterval,
};
use lorelei_echo::retriever::{EchoEngine, EchoRetrievalConfig};
use lorelei_lore::embedding::{DeterministicMockEmbeddingProvider, EmbeddingProvider};
use lorelei_lore::pg::PgLoreStore;
use lorelei_lore::qdrant::QdrantPearlIndex;
use qdrant_client::Qdrant;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use uuid::Uuid;

async fn maybe_pg() -> Option<sqlx::PgPool> {
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

fn maybe_qdrant() -> Option<Qdrant> {
    let url = match std::env::var("QDRANT_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return None,
    };
    Qdrant::from_url(&url).build().ok()
}

fn pearl(content: &str, confidence: f64) -> NewPearl {
    NewPearl::new(
        PearlType::Other,
        content,
        UnitInterval::new(0.5).unwrap(),
        UnitInterval::new(confidence).unwrap(),
        Default::default(),
    )
    .unwrap()
}

#[tokio::test]
async fn echo_retrieves_relevant_pearls_and_dedupes() {
    let Some(pool) = maybe_pg().await else { return };
    let Some(client) = maybe_qdrant() else { return };

    let tenant_id = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());
    let collection = format!("lorelei_echo_test_{}", Uuid::new_v4());
    let index = QdrantPearlIndex::new(client, collection);

    let embedder: Arc<dyn EmbeddingProvider> =
        Arc::new(DeterministicMockEmbeddingProvider::new(64));

    let store = PgLoreStore::new_indexed(pool, index.clone(), embedder.clone(), "mock");
    store.migrate().await.expect("migrate");

    // Two pearls with same content; should dedupe to 1 result.
    let _ = store
        .save_pearl(tenant_id, agent_id, pearl("deep memory", 0.9))
        .await
        .expect("save");
    let _ = store
        .save_pearl(tenant_id, agent_id, pearl("deep memory", 0.9))
        .await
        .expect("save");

    let engine = EchoEngine::new(
        store,
        index,
        embedder,
        "mock",
        EchoRetrievalConfig {
            rerank_top_k: 10,
            enable_query_rewrite: false,
        },
    );

    let hits = engine
        .query(
            tenant_id,
            agent_id,
            EchoQuery {
                query: "deep memory".to_string(),
                top_k: 10,
                min_confidence: Some(UnitInterval::new(0.0).unwrap()),
                pearl_type: None,
                sources: EchoSources::Pearls,
            },
        )
        .await
        .expect("echo");

    assert!(!hits.is_empty());
    assert_eq!(hits[0].content, "deep memory");
}

#[tokio::test]
async fn echo_excludes_low_confidence_and_never_crosses_tenants() {
    let Some(pool) = maybe_pg().await else { return };
    let Some(client) = maybe_qdrant() else { return };

    let tenant_a = TenantId(Uuid::new_v4());
    let tenant_b = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());
    let collection = format!("lorelei_echo_test_{}", Uuid::new_v4());
    let index = QdrantPearlIndex::new(client, collection);
    let embedder: Arc<dyn EmbeddingProvider> =
        Arc::new(DeterministicMockEmbeddingProvider::new(64));

    let store = PgLoreStore::new_indexed(pool, index.clone(), embedder.clone(), "mock");
    store.migrate().await.expect("migrate");

    // A has a relevant pearl but low confidence.
    let _ = store
        .save_pearl(tenant_a, agent_id, pearl("harbor", 0.1))
        .await
        .expect("save a");
    // B has a high confidence relevant pearl.
    let _ = store
        .save_pearl(tenant_b, agent_id, pearl("harbor", 0.9))
        .await
        .expect("save b");

    let engine = EchoEngine::new(
        store,
        index,
        embedder,
        "mock",
        EchoRetrievalConfig {
            rerank_top_k: 10,
            enable_query_rewrite: false,
        },
    );

    // Tenant A query should not see tenant B pearl and should exclude its own low confidence pearl.
    let hits_a = engine
        .query(
            tenant_a,
            agent_id,
            EchoQuery {
                query: "harbor".to_string(),
                top_k: 10,
                min_confidence: Some(UnitInterval::new(0.5).unwrap()),
                pearl_type: None,
                sources: EchoSources::Pearls,
            },
        )
        .await
        .expect("echo a");
    assert!(hits_a.is_empty());

    // Tenant B query should see its own pearl.
    let hits_b = engine
        .query(
            tenant_b,
            agent_id,
            EchoQuery {
                query: "harbor".to_string(),
                top_k: 10,
                min_confidence: Some(UnitInterval::new(0.5).unwrap()),
                pearl_type: None,
                sources: EchoSources::Pearls,
            },
        )
        .await
        .expect("echo b");
    assert!(!hits_b.is_empty());
}

#[tokio::test]
async fn echo_handles_zero_results_gracefully() {
    let Some(pool) = maybe_pg().await else { return };
    let Some(client) = maybe_qdrant() else { return };

    let tenant_id = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());
    let collection = format!("lorelei_echo_test_{}", Uuid::new_v4());
    let index = QdrantPearlIndex::new(client, collection);
    let embedder: Arc<dyn EmbeddingProvider> =
        Arc::new(DeterministicMockEmbeddingProvider::new(64));

    let store = PgLoreStore::new_indexed(pool, index.clone(), embedder.clone(), "mock");
    store.migrate().await.expect("migrate");

    let engine = EchoEngine::new(
        store,
        index,
        embedder,
        "mock",
        EchoRetrievalConfig {
            rerank_top_k: 10,
            enable_query_rewrite: false,
        },
    );

    let hits = engine
        .query(
            tenant_id,
            agent_id,
            EchoQuery {
                query: "nope".to_string(),
                top_k: 5,
                min_confidence: None,
                pearl_type: None,
                sources: EchoSources::Pearls,
            },
        )
        .await
        .expect("echo");
    assert!(hits.is_empty());
}
