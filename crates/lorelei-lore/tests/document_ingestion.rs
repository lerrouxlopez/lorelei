use lorelei_core::traits::{DocumentStore, EchoRetriever};
use lorelei_core::types::{AgentId, EchoQuery, EchoSources, TenantId, UnitInterval};
use lorelei_echo::retriever::{EchoEngine, EchoRetrievalConfig};
use lorelei_lore::docs::PgDocumentStore;
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
    PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .ok()
}

fn maybe_qdrant() -> Option<Qdrant> {
    let url = match std::env::var("QDRANT_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return None,
    };
    Qdrant::from_url(&url).build().ok()
}

#[tokio::test]
async fn ingest_markdown_and_search_and_soft_delete() {
    let Some(pool) = maybe_pg().await else { return };
    let Some(client) = maybe_qdrant() else { return };

    let tenant_id = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());

    let collection = format!("lorelei_docs_test_{}", Uuid::new_v4());
    let index = QdrantPearlIndex::new(client, collection);
    let embedder: Arc<dyn EmbeddingProvider> =
        Arc::new(DeterministicMockEmbeddingProvider::new(64));

    // Ensure schema exists.
    let lore = PgLoreStore::new(pool.clone());
    lore.migrate().await.expect("migrate");

    // Create a temp markdown file.
    let dir = std::env::temp_dir().join(format!("lorelei_docs_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tea.md");
    std::fs::write(&path, "# Tea\n\nThe Lore starts in Postgres.\n").unwrap();

    let docs = Arc::new(PgDocumentStore::new(
        pool.clone(),
        index.clone(),
        embedder.clone(),
        "mock",
        vec![dir.clone()],
    ));

    let doc_id_1 = docs
        .ingest_document_path(tenant_id, agent_id, &path)
        .await
        .expect("ingest");
    let doc_id_2 = docs
        .ingest_document_path(tenant_id, agent_id, &path)
        .await
        .expect("dedupe ingest");
    assert_eq!(doc_id_1, doc_id_2);

    let engine = EchoEngine::new(
        PgLoreStore::new(pool.clone()),
        index.clone(),
        embedder.clone(),
        "mock",
        EchoRetrievalConfig {
            rerank_top_k: 10,
            enable_query_rewrite: false,
        },
    )
    .with_documents(docs.clone());

    let hits = engine
        .query(
            tenant_id,
            agent_id,
            EchoQuery {
                query: "Postgres".to_string(),
                top_k: 5,
                min_confidence: Some(UnitInterval::new(0.0).unwrap()),
                pearl_type: None,
                sources: EchoSources::Documents,
            },
        )
        .await
        .expect("search");

    assert!(!hits.is_empty());
    assert!(hits[0].content.contains("Postgres"));
    assert!(hits[0].citation.is_some());

    // Soft-delete and ensure excluded.
    docs.soft_delete_document(tenant_id, doc_id_1)
        .await
        .expect("delete");

    let hits2 = engine
        .query(
            tenant_id,
            agent_id,
            EchoQuery {
                query: "Postgres".to_string(),
                top_k: 5,
                min_confidence: Some(UnitInterval::new(0.0).unwrap()),
                pearl_type: None,
                sources: EchoSources::Documents,
            },
        )
        .await
        .expect("search2");
    assert!(hits2.is_empty());
}

#[tokio::test]
async fn tenant_isolation_applies_to_documents() {
    let Some(pool) = maybe_pg().await else { return };
    let Some(client) = maybe_qdrant() else { return };

    let tenant_a = TenantId(Uuid::new_v4());
    let tenant_b = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());

    let collection = format!("lorelei_docs_test_{}", Uuid::new_v4());
    let index = QdrantPearlIndex::new(client, collection);
    let embedder: Arc<dyn EmbeddingProvider> =
        Arc::new(DeterministicMockEmbeddingProvider::new(64));

    let lore = PgLoreStore::new(pool.clone());
    lore.migrate().await.expect("migrate");

    let dir = std::env::temp_dir().join(format!("lorelei_docs_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.md");
    std::fs::write(&path, "Tenant A only content about tea.").unwrap();

    let docs = Arc::new(PgDocumentStore::new(
        pool.clone(),
        index.clone(),
        embedder.clone(),
        "mock",
        vec![dir.clone()],
    ));
    let _ = docs
        .ingest_document_path(tenant_a, agent_id, &path)
        .await
        .expect("ingest a");

    let engine = EchoEngine::new(
        PgLoreStore::new(pool),
        index,
        embedder,
        "mock",
        EchoRetrievalConfig {
            rerank_top_k: 10,
            enable_query_rewrite: false,
        },
    )
    .with_documents(docs);

    let hits_b = engine
        .query(
            tenant_b,
            agent_id,
            EchoQuery {
                query: "tea".to_string(),
                top_k: 5,
                min_confidence: Some(UnitInterval::new(0.0).unwrap()),
                pearl_type: None,
                sources: EchoSources::Documents,
            },
        )
        .await
        .expect("search b");
    assert!(hits_b.is_empty());
}
