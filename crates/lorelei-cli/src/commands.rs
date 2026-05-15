#![forbid(unsafe_code)]

use lorelei_core::config::LoreleiConfig;
use std::path::PathBuf;

pub fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("doctor") => doctor(&args[1..]),
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

fn check_env(key: &str, missing: &mut Vec<String>) {
    if std::env::var_os(key).is_none() {
        missing.push(key.to_string());
    }
}
