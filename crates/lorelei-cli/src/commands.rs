#![forbid(unsafe_code)]

use lorelei_core::config::LoreleiConfig;
use lorelei_core::traits::LoreStore;
use lorelei_core::types::{NewPearl, PearlListQuery, PearlType, UnitInterval};
use lorelei_lore::echo::resolve_hits;
use lorelei_lore::embedding::{DeterministicMockEmbeddingProvider, EmbeddingProvider};
use lorelei_lore::pg::PgLoreStore;
use lorelei_lore::qdrant::QdrantPearlIndex;
use qdrant_client::Qdrant;
use sqlx::postgres::PgPoolOptions;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

pub async fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("doctor") => doctor(&args[1..]),
        Some("memo") => memo(&args[1..]).await,
        Some("pearls") => pearls(&args[1..]).await,
        Some("forget") => forget(&args[1..]).await,
        Some("echo") => echo(&args[1..]).await,
        Some("-h") | Some("--help") | Some("help") | None => {
            print_help();
            0
        }
        Some(other) => {
            eprintln!("unknown command: {other}");
            eprintln!("try: lore help");
            2
        }
    }
}

fn print_help() {
    println!("lore (Lorelei CLI)");
    println!();
    println!("Usage:");
    println!("  lore doctor [--config <path>]");
    println!("  lore memo <content> [--config <path>]");
    println!("  lore pearls [--config <path>]");
    println!("  lore forget <pearl_id> [--config <path>]");
    println!("  lore echo <query> [--config <path>]");
    println!("  lore help");
}

fn doctor(args: &[String]) -> i32 {
    // Load `.env` if present so local development matches user expectations.
    // This does not print any secret values.
    let _ = dotenvy::dotenv();

    let config_path = match parse_config_path(args) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("{msg}");
            return 2;
        }
    };

    let cfg = match LoreleiConfig::load_from_toml_path(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config invalid: {e}");
            eprintln!("hint: start from `lorelei.toml.example`");
            return 1;
        }
    };

    let mut missing_env = Vec::new();

    check_env(&cfg.lore.postgres_url_env, &mut missing_env);
    check_env(&cfg.lore.qdrant_url_env, &mut missing_env);

    for (name, p) in &cfg.providers {
        if p.kind == lorelei_core::config::ProviderKind::Mock {
            // Still check env presence, but never print the value.
            check_env(&p.api_key_env, &mut missing_env);
        } else {
            check_env(&p.api_key_env, &mut missing_env);
        }

        if p.chat_model.trim().is_empty() {
            eprintln!("provider `{name}` chat_model is empty");
            return 1;
        }
    }

    if missing_env.is_empty() {
        println!("doctor ok: config parsed and required env vars are set");
        println!("config: {}", config_path.display());
        0
    } else {
        eprintln!("doctor: missing required env vars (values not shown):");
        for k in missing_env {
            eprintln!("- {k}");
        }
        eprintln!("hint: copy `.env.example` to `.env` and set values");
        1
    }
}

async fn memo(args: &[String]) -> i32 {
    let _ = dotenvy::dotenv();

    let (config_path, rest) = match parse_config_path_with_rest(args) {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("{msg}");
            return 2;
        }
    };
    let content = rest.join(" ").trim().to_string();
    if content.is_empty() {
        eprintln!("memo requires content");
        return 2;
    }

    let cfg = match LoreleiConfig::load_from_toml_path(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config invalid: {e}");
            return 1;
        }
    };

    let url_key = cfg.lore.postgres_url_env.clone();
    let url = match std::env::var(&url_key) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("missing required env var (value not shown): {url_key}");
            return 1;
        }
    };

    let pool = match PgPoolOptions::new().max_connections(5).connect(&url).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("database connection failed: {e}");
            return 1;
        }
    };

    let qdrant_store = match build_indexed_store(&cfg, pool).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    if let Err(e) = qdrant_store.migrate().await {
        eprintln!("migration failed: {e}");
        return 1;
    }

    let pearl = match NewPearl::new(
        PearlType::Other,
        content,
        UnitInterval::new(0.5).unwrap(),
        UnitInterval::new(0.8).unwrap(),
        Default::default(),
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("invalid pearl: {e}");
            return 1;
        }
    };

    let saved = match qdrant_store
        .save_pearl(cfg.agent.tenant_id, cfg.agent.agent_id, pearl)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("save failed: {e}");
            return 1;
        }
    };

    println!("saved pearl: {}", saved.pearl_id.0);
    0
}

async fn pearls(args: &[String]) -> i32 {
    let _ = dotenvy::dotenv();

    let config_path = match parse_config_path(args) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("{msg}");
            return 2;
        }
    };

    let cfg = match LoreleiConfig::load_from_toml_path(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config invalid: {e}");
            return 1;
        }
    };

    let url_key = cfg.lore.postgres_url_env.clone();
    let url = match std::env::var(&url_key) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("missing required env var (value not shown): {url_key}");
            return 1;
        }
    };

    let pool = match PgPoolOptions::new().max_connections(5).connect(&url).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("database connection failed: {e}");
            return 1;
        }
    };

    let store = match build_indexed_store(&cfg, pool).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    if let Err(e) = store.migrate().await {
        eprintln!("migration failed: {e}");
        return 1;
    }

    let query = PearlListQuery {
        agent_id: Some(cfg.agent.agent_id),
        include_deleted: false,
        ..Default::default()
    };

    let pearls = match store.list_pearls(cfg.agent.tenant_id, query).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("list failed: {e}");
            return 1;
        }
    };

    for p in pearls {
        println!("{}\t{:?}\t{}", p.pearl_id.0, p.pearl_type, p.content);
    }
    0
}

async fn forget(args: &[String]) -> i32 {
    let _ = dotenvy::dotenv();

    let (config_path, rest) = match parse_config_path_with_rest(args) {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("{msg}");
            return 2;
        }
    };

    let Some(id_str) = rest.first() else {
        eprintln!("forget requires <pearl_id>");
        return 2;
    };
    let pearl_uuid = match Uuid::parse_str(id_str) {
        Ok(u) => u,
        Err(_) => {
            eprintln!("invalid pearl_id");
            return 2;
        }
    };

    let cfg = match LoreleiConfig::load_from_toml_path(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config invalid: {e}");
            return 1;
        }
    };

    let url_key = cfg.lore.postgres_url_env.clone();
    let url = match std::env::var(&url_key) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("missing required env var (value not shown): {url_key}");
            return 1;
        }
    };

    let pool = match PgPoolOptions::new().max_connections(5).connect(&url).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("database connection failed: {e}");
            return 1;
        }
    };

    let store = match build_indexed_store(&cfg, pool).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    if let Err(e) = store.migrate().await {
        eprintln!("migration failed: {e}");
        return 1;
    }

    let pearl_id = lorelei_core::types::PearlId(pearl_uuid);
    if let Err(e) = store.forget_pearl(cfg.agent.tenant_id, pearl_id).await {
        eprintln!("forget failed: {e}");
        return 1;
    }
    println!("forgot pearl: {pearl_uuid}");
    0
}

async fn echo(args: &[String]) -> i32 {
    let _ = dotenvy::dotenv();

    let (config_path, rest) = match parse_config_path_with_rest(args) {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("{msg}");
            return 2;
        }
    };
    let query = rest.join(" ").trim().to_string();
    if query.is_empty() {
        eprintln!("echo requires <query>");
        return 2;
    }

    let cfg = match LoreleiConfig::load_from_toml_path(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config invalid: {e}");
            return 1;
        }
    };

    let pg_url = match std::env::var(&cfg.lore.postgres_url_env) {
        Ok(v) => v,
        Err(_) => {
            eprintln!(
                "missing required env var (value not shown): {}",
                cfg.lore.postgres_url_env
            );
            return 1;
        }
    };
    let qdrant_url = match std::env::var(&cfg.lore.qdrant_url_env) {
        Ok(v) => v,
        Err(_) => {
            eprintln!(
                "missing required env var (value not shown): {}",
                cfg.lore.qdrant_url_env
            );
            return 1;
        }
    };

    let pool = match PgPoolOptions::new()
        .max_connections(5)
        .connect(&pg_url)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("database connection failed: {e}");
            return 1;
        }
    };

    let Some(embedding_provider_cfg) = cfg.providers.get(&cfg.agent.default_embedding_provider)
    else {
        eprintln!("config invalid: embedding provider is not configured");
        return 1;
    };
    if embedding_provider_cfg.kind != lorelei_core::config::ProviderKind::Mock {
        eprintln!("echo not implemented for non-mock embedding providers yet");
        return 1;
    }

    // For now, only a deterministic mock embedder is wired. This validates the
    // Qdrant integration without any vendor SDKs.
    let embedder = Arc::new(DeterministicMockEmbeddingProvider::new(64));
    let client = match Qdrant::from_url(&qdrant_url).build() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("qdrant client init failed: {e}");
            return 1;
        }
    };
    let index = QdrantPearlIndex::new(client, cfg.lore.collection.clone());
    let store = PgLoreStore::new(pool);
    if let Err(e) = store.migrate().await {
        eprintln!("migration failed: {e}");
        return 1;
    }

    let emb = match embedder
        .embed(
            cfg.agent.tenant_id,
            &cfg.agent.default_embedding_provider,
            vec![query.clone()],
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("embed failed: {e}");
            return 1;
        }
    };
    let Some(vector) = emb.vectors.into_iter().next() else {
        eprintln!("embed failed: empty vectors");
        return 1;
    };
    if let Err(e) = index.ensure_collection(vector.len() as u64).await {
        eprintln!("qdrant collection error: {e}");
        return 1;
    }

    let hits = match index
        .search_pearl_vectors(
            cfg.agent.tenant_id,
            vector,
            cfg.echo.top_k as u64,
            Some(cfg.agent.agent_id),
        )
        .await
    {
        Ok(h) => h,
        Err(e) => {
            eprintln!("search failed: {e}");
            return 1;
        }
    };

    let (resolved, ignored) = match resolve_hits(&store, cfg.agent.tenant_id, hits).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("failed to resolve hits: {e}");
            return 1;
        }
    };

    for id in ignored {
        eprintln!("ignored qdrant-only hit for pearl_id {}", id.0);
    }

    for r in resolved {
        println!(
            "{:.4}\t{}\t{}",
            r.score, r.pearl.pearl_id.0, r.pearl.content
        );
    }

    0
}

async fn build_indexed_store(
    cfg: &LoreleiConfig,
    pool: sqlx::PgPool,
) -> Result<PgLoreStore, String> {
    let qdrant_url = std::env::var(&cfg.lore.qdrant_url_env).map_err(|_| {
        format!(
            "missing required env var (value not shown): {}",
            cfg.lore.qdrant_url_env
        )
    })?;

    let embedding_provider_cfg = cfg
        .providers
        .get(&cfg.agent.default_embedding_provider)
        .ok_or_else(|| "config invalid: embedding provider is not configured".to_string())?;
    if embedding_provider_cfg.kind != lorelei_core::config::ProviderKind::Mock {
        return Err("indexing not implemented for non-mock embedding providers yet".to_string());
    }

    let embedder = Arc::new(DeterministicMockEmbeddingProvider::new(64));
    let client = Qdrant::from_url(&qdrant_url)
        .build()
        .map_err(|e| format!("qdrant client init failed: {e}"))?;
    let index = QdrantPearlIndex::new(client, cfg.lore.collection.clone());

    Ok(PgLoreStore::new_indexed(
        pool,
        index,
        embedder,
        cfg.agent.default_embedding_provider.clone(),
    ))
}

fn parse_config_path(args: &[String]) -> Result<PathBuf, String> {
    let mut i = 0usize;
    let mut config: Option<PathBuf> = None;

    while i < args.len() {
        match args[i].as_str() {
            "--config" => {
                let Some(val) = args.get(i + 1) else {
                    return Err("missing value for --config".to_string());
                };
                config = Some(PathBuf::from(val));
                i += 2;
            }
            other => {
                return Err(format!("unexpected argument: {other}"));
            }
        }
    }

    Ok(config.unwrap_or_else(|| PathBuf::from("lorelei.toml")))
}

fn parse_config_path_with_rest(args: &[String]) -> Result<(PathBuf, Vec<String>), String> {
    let mut i = 0usize;
    let mut config: Option<PathBuf> = None;
    let mut rest: Vec<String> = Vec::new();

    while i < args.len() {
        match args[i].as_str() {
            "--config" => {
                let Some(val) = args.get(i + 1) else {
                    return Err("missing value for --config".to_string());
                };
                config = Some(PathBuf::from(val));
                i += 2;
            }
            other => {
                rest.push(other.to_string());
                i += 1;
            }
        }
    }

    Ok((
        config.unwrap_or_else(|| PathBuf::from("lorelei.toml")),
        rest,
    ))
}

fn check_env(key: &str, missing: &mut Vec<String>) {
    if std::env::var_os(key).is_none() {
        missing.push(key.to_string());
    }
}
