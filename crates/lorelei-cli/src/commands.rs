#![forbid(unsafe_code)]

use lorelei_core::config::LoreleiConfig;
use lorelei_core::traits::LoreStore;
use lorelei_core::types::{NewPearl, PearlListQuery, PearlType, UnitInterval};
use lorelei_lore::pg::PgLoreStore;
use sqlx::postgres::PgPoolOptions;
use std::path::PathBuf;
use uuid::Uuid;

pub async fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("doctor") => doctor(&args[1..]),
        Some("memo") => memo(&args[1..]).await,
        Some("pearls") => pearls(&args[1..]).await,
        Some("forget") => forget(&args[1..]).await,
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

    let store = PgLoreStore::new(pool);
    if let Err(e) = store.migrate().await {
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

    let saved = match store
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

    let store = PgLoreStore::new(pool);
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

    let store = PgLoreStore::new(pool);
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
