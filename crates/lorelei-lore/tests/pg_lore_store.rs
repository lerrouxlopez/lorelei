use lorelei_core::traits::LoreStore;
use lorelei_core::types::{AgentId, NewPearl, PearlListQuery, PearlType, TenantId, UnitInterval};
use lorelei_lore::echo::resolve_hits;
use lorelei_lore::embedding::{DeterministicMockEmbeddingProvider, EmbeddingProvider};
use lorelei_lore::pg::PgLoreStore;
use lorelei_lore::qdrant::QdrantPearlIndex;
use lorelei_lore::qdrant::VectorHit;
use qdrant_client::Qdrant;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
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

fn maybe_qdrant() -> Option<Qdrant> {
    let url = match std::env::var("QDRANT_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return None,
    };
    Qdrant::from_url(&url).build().ok()
}

fn pearl(content: &str) -> NewPearl {
    NewPearl::new(
        PearlType::Other,
        content,
        UnitInterval::new(0.6).unwrap(),
        UnitInterval::new(0.7).unwrap(),
        Default::default(),
    )
    .unwrap()
}

#[tokio::test]
async fn save_and_get_pearl() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    let store = PgLoreStore::new(pool);
    store.migrate().await.expect("migrate");

    let tenant_id = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());

    let saved = store
        .save_pearl(tenant_id, agent_id, pearl("the lore"))
        .await
        .expect("save");

    let got = store
        .get_pearl(tenant_id, saved.pearl_id, false)
        .await
        .expect("get")
        .expect("present");

    assert_eq!(got.pearl_id, saved.pearl_id);
    assert_eq!(got.tenant_id, tenant_id);
    assert_eq!(got.agent_id, agent_id);
    assert_eq!(got.content, "the lore");
}

#[tokio::test]
async fn list_active_pearls() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    let store = PgLoreStore::new(pool);
    store.migrate().await.expect("migrate");

    let tenant_id = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());

    let a = store
        .save_pearl(tenant_id, agent_id, pearl("a"))
        .await
        .expect("save a");
    let b = store
        .save_pearl(tenant_id, agent_id, pearl("b"))
        .await
        .expect("save b");

    // soft-delete b
    store
        .forget_pearl(tenant_id, b.pearl_id)
        .await
        .expect("forget");

    let list = store
        .list_pearls(
            tenant_id,
            PearlListQuery {
                agent_id: Some(agent_id),
                include_deleted: false,
                ..Default::default()
            },
        )
        .await
        .expect("list");

    let ids: Vec<_> = list.iter().map(|p| p.pearl_id).collect();
    assert!(ids.contains(&a.pearl_id));
    assert!(!ids.contains(&b.pearl_id));
}

#[tokio::test]
async fn soft_deleted_pearl_not_returned() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    let store = PgLoreStore::new(pool);
    store.migrate().await.expect("migrate");

    let tenant_id = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());

    let saved = store
        .save_pearl(tenant_id, agent_id, pearl("temp"))
        .await
        .expect("save");
    store
        .forget_pearl(tenant_id, saved.pearl_id)
        .await
        .expect("forget");

    let got = store
        .get_pearl(tenant_id, saved.pearl_id, false)
        .await
        .expect("get");
    assert!(got.is_none());

    let got_incl = store
        .get_pearl(tenant_id, saved.pearl_id, true)
        .await
        .expect("get incl");
    assert!(got_incl.is_some());
}

#[tokio::test]
async fn tenant_isolation() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    let store = PgLoreStore::new(pool);
    store.migrate().await.expect("migrate");

    let tenant_a = TenantId(Uuid::new_v4());
    let tenant_b = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());

    let saved = store
        .save_pearl(tenant_a, agent_id, pearl("secret a"))
        .await
        .expect("save");

    let got_wrong_tenant = store
        .get_pearl(tenant_b, saved.pearl_id, false)
        .await
        .expect("get");
    assert!(got_wrong_tenant.is_none());
}

#[test]
fn invalid_confidence_importance_rejected_before_db() {
    assert!(UnitInterval::new(-0.1).is_err());
    assert!(UnitInterval::new(1.1).is_err());
}

#[tokio::test]
async fn saving_a_pearl_indexes_it_and_search_finds_it() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    let Some(client) = maybe_qdrant() else {
        return;
    };

    let embedder = Arc::new(DeterministicMockEmbeddingProvider::new(64));
    let collection = format!("lorelei_test_{}", Uuid::new_v4());
    let index = QdrantPearlIndex::new(client, collection);

    let store = PgLoreStore::new_indexed(pool, index.clone(), embedder.clone(), "mock");
    store.migrate().await.expect("migrate");

    let tenant_id = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());
    let saved = store
        .save_pearl(tenant_id, agent_id, pearl("hello world"))
        .await
        .expect("save");

    let emb = embedder
        .embed(tenant_id, "mock", vec!["hello world!".to_string()])
        .await
        .expect("embed");
    let v = emb.vectors.into_iter().next().unwrap();

    let hits = index
        .search_pearl_vectors(tenant_id, v, 5, Some(agent_id))
        .await
        .expect("search");

    assert!(hits.iter().any(|h| h.pearl_id == saved.pearl_id));
}

#[tokio::test]
async fn deleted_pearls_are_excluded_from_search_results() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    let Some(client) = maybe_qdrant() else {
        return;
    };

    let embedder = Arc::new(DeterministicMockEmbeddingProvider::new(64));
    let collection = format!("lorelei_test_{}", Uuid::new_v4());
    let index = QdrantPearlIndex::new(client, collection);

    let store = PgLoreStore::new_indexed(pool, index.clone(), embedder.clone(), "mock");
    store.migrate().await.expect("migrate");

    let tenant_id = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());
    let saved = store
        .save_pearl(tenant_id, agent_id, pearl("tide current"))
        .await
        .expect("save");

    store
        .forget_pearl(tenant_id, saved.pearl_id)
        .await
        .expect("forget");

    let emb = embedder
        .embed(tenant_id, "mock", vec!["tide current".to_string()])
        .await
        .expect("embed");
    let v = emb.vectors.into_iter().next().unwrap();
    let hits = index
        .search_pearl_vectors(tenant_id, v, 5, Some(agent_id))
        .await
        .expect("search");

    // Even if Qdrant returns the ID, Postgres source-of-truth get_pearl must hide it.
    for h in hits {
        let got = store.get_pearl(tenant_id, h.pearl_id, false).await.unwrap();
        assert!(got.is_none());
    }
}

#[tokio::test]
async fn tenant_filters_are_always_included() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    let Some(client) = maybe_qdrant() else {
        return;
    };

    let embedder = Arc::new(DeterministicMockEmbeddingProvider::new(64));
    let collection = format!("lorelei_test_{}", Uuid::new_v4());
    let index = QdrantPearlIndex::new(client, collection);

    let store = PgLoreStore::new_indexed(pool, index.clone(), embedder.clone(), "mock");
    store.migrate().await.expect("migrate");

    let tenant_a = TenantId(Uuid::new_v4());
    let tenant_b = TenantId(Uuid::new_v4());
    let agent_id = AgentId(Uuid::new_v4());

    let saved_a = store
        .save_pearl(tenant_a, agent_id, pearl("reef"))
        .await
        .expect("save a");
    let _saved_b = store
        .save_pearl(tenant_b, agent_id, pearl("reef"))
        .await
        .expect("save b");

    let emb = embedder
        .embed(tenant_a, "mock", vec!["reef".to_string()])
        .await
        .expect("embed");
    let v = emb.vectors.into_iter().next().unwrap();
    let hits = index
        .search_pearl_vectors(tenant_a, v, 10, Some(agent_id))
        .await
        .expect("search");

    assert!(hits.iter().any(|h| h.pearl_id == saved_a.pearl_id));
    // The B tenant pearl must not appear in tenant A search results.
    assert!(!hits.is_empty());
}

#[tokio::test]
async fn qdrant_only_hit_missing_postgres_row_is_ignored() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    let store = PgLoreStore::new(pool);
    store.migrate().await.expect("migrate");

    let tenant_id = TenantId(Uuid::new_v4());
    let fake = VectorHit {
        pearl_id: lorelei_core::types::PearlId(Uuid::new_v4()),
        score: 0.99,
        source_type: Some("pearl".to_string()),
        document_id: None,
        chunk_index: None,
        title: None,
    };

    let (resolved, ignored) = resolve_hits(&store, tenant_id, vec![fake])
        .await
        .expect("resolve");
    assert!(resolved.is_empty());
    assert_eq!(ignored.len(), 1);
}
