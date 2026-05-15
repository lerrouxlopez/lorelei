use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use dotenvy::dotenv;
use lorelei_core::{Config as CoreConfig, NewPearl, PearlType};
use owo_colors::OwoColorize;
use serde_json::json;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

pub async fn run() -> Result<()> {
    // Load .env if present; do not error if missing.
    let _ = dotenv();
    init_tracing();

    let cli = Cli::parse();
    match cli.command {
        Commands::Init => cmd_init().await,
        Commands::Ask { message } => cmd_ask(message).await,
        Commands::Memo { content } => cmd_memo(content).await,
        Commands::Echo { query } => cmd_echo(query).await,
        Commands::Pearls => cmd_pearls().await,
        Commands::Forget { pearl_id } => cmd_forget(pearl_id).await,
        Commands::Providers => cmd_providers().await,
        Commands::Reef { command } => cmd_reef(command).await,
        Commands::Ship => cmd_ship().await,
        Commands::Doctor => cmd_doctor().await,
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let json = lorelei_core::log_json_enabled();

    let fmt = tracing_subscriber::fmt().with_env_filter(filter);
    if json {
        fmt.json()
            .with_current_span(true)
            .with_span_list(true)
            .init();
    } else {
        fmt.init();
    }
}

#[derive(Debug, Parser)]
#[command(name = "lore", version, about = "Lorelei CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Create `lorelei.toml` and `.env` from examples.
    Init,
    /// Ask Lorelei (currently: runs retrieval via Harbor and prints hits).
    Ask { message: String },
    /// Save a manual Pearl via Harbor.
    Memo { content: String },
    /// Test retrieval via Harbor only.
    Echo { query: String },
    /// List Pearls via Harbor.
    Pearls,
    /// Soft-delete a Pearl via Harbor.
    Forget { pearl_id: Uuid },
    /// List configured providers (from Harbor config).
    Providers,
    /// Manage the local docker-compose “reef”.
    Reef {
        #[command(subcommand)]
        command: ReefCmd,
    },
    /// Build the Docker image.
    Ship,
    /// Check system + configuration + dependencies.
    Doctor,
}

#[derive(Debug, Subcommand)]
enum ReefCmd {
    Up,
    Down,
    Logs,
}

async fn cmd_init() -> Result<()> {
    let root = std::env::current_dir()?;
    let toml_example = root.join("lorelei.toml.example");
    let env_example = root.join(".env.example");
    let toml_out = root.join("lorelei.toml");
    let env_out = root.join(".env");

    if !toml_example.exists() {
        return Err(anyhow!("missing `lorelei.toml.example` in {}", root.display()));
    }
    if !env_example.exists() {
        return Err(anyhow!("missing `.env.example` in {}", root.display()));
    }

    if !toml_out.exists() {
        std::fs::copy(&toml_example, &toml_out)
            .with_context(|| format!("copy {} -> {}", toml_example.display(), toml_out.display()))?;
        println!("{} {}", "created".green(), "lorelei.toml".bold());
    } else {
        println!("{} {}", "exists".yellow(), "lorelei.toml".bold());
    }

    if !env_out.exists() {
        let template = std::fs::read_to_string(&env_example)
            .with_context(|| format!("read {}", env_example.display()))?;
        let tenant_id = Uuid::new_v4();
        let agent_id = Uuid::new_v4();
        let rendered = template
            .replace("TENANT_ID=", &format!("TENANT_ID={tenant_id}"))
            .replace("AGENT_ID=", &format!("AGENT_ID={agent_id}"))
            .replace("LORELEI_CONFIG=", "LORELEI_CONFIG=lorelei.toml");
        std::fs::write(&env_out, rendered).with_context(|| format!("write {}", env_out.display()))?;
        println!("{} {}", "created".green(), ".env".bold());
    } else {
        println!("{} {}", "exists".yellow(), ".env".bold());
    }

    println!(
        "{} {}",
        "tip".cyan().bold(),
        "Use `lori` as an alias for `lore` (see README)."
    );
    Ok(())
}

async fn cmd_ask(message: String) -> Result<()> {
    println!("{} {}", "ask".cyan().bold(), message);
    // Currently Harbor doesn’t expose a full Tide “ask” endpoint; do retrieval + print.
    cmd_echo(message).await
}

async fn cmd_memo(content: String) -> Result<()> {
    let (harbor, tenant_id, agent_id) = load_cli_env()?;
    let run_id = create_run(&harbor, tenant_id).await?;

    let pearl = NewPearl {
        pearl_type: PearlType::Note,
        content,
        tags: vec!["manual".to_string()],
        confidence: None,
        importance: None,
        metadata: json!({}),
    };

    let resp = harbor
        .post_json(
            "/v1/pearls",
            json!({
              "tenant_id": tenant_id,
              "agent_id": agent_id,
              "run_id": run_id,
              "pearl": pearl
            }),
        )
        .await?;
    println!("{} {}", "saved".green(), resp);
    Ok(())
}

async fn cmd_echo(query: String) -> Result<()> {
    let (harbor, tenant_id, agent_id) = load_cli_env()?;
    let body = json!({
      "text": query,
      "tenant_id": tenant_id,
      "agent_id": agent_id,
      "run_id": null,
      "top_k": 10
    });
    let resp = harbor.post_json("/v1/echo", body).await?;
    println!("{}", "hits".cyan().bold());
    println!("{resp}");
    Ok(())
}

async fn cmd_pearls() -> Result<()> {
    let (harbor, tenant_id, _agent_id) = load_cli_env()?;
    let resp = harbor
        .get_json(&format!("/v1/pearls?tenant_id={tenant_id}&top_k=50"))
        .await?;
    println!("{}", "pearls".cyan().bold());
    println!("{resp}");
    Ok(())
}

async fn cmd_forget(pearl_id: Uuid) -> Result<()> {
    let (harbor, tenant_id, _agent_id) = load_cli_env()?;
    harbor
        .delete(&format!("/v1/pearls/{pearl_id}?tenant_id={tenant_id}"))
        .await?;
    println!("{} {}", "forgot".green(), pearl_id);
    Ok(())
}

async fn cmd_providers() -> Result<()> {
    let (harbor, _tenant_id, _agent_id) = load_cli_env()?;
    let resp = harbor.get_json("/v1/providers").await?;
    println!("{}", "providers".cyan().bold());
    println!("{resp}");
    Ok(())
}

async fn cmd_reef(command: ReefCmd) -> Result<()> {
    match command {
        ReefCmd::Up => run_cmd("docker", &["compose", "up", "-d"]),
        ReefCmd::Down => run_cmd("docker", &["compose", "down"]),
        ReefCmd::Logs => run_cmd("docker", &["compose", "logs", "-f", "--tail=200"]),
    }
}

async fn cmd_ship() -> Result<()> {
    run_cmd("docker", &["build", "-t", "lorelei:local", "."])
}

async fn cmd_doctor() -> Result<()> {
    let (harbor, _tenant_id, _agent_id) = load_cli_env()?;
    println!("{}", "doctor".cyan().bold());

    check_cmd("docker", &["version"]).context("docker not available")?;
    println!("{} {}", "ok".green(), "docker");

    let cfg_path = env_path("LORELEI_CONFIG")?;
    let cfg = CoreConfig::from_toml_file(&cfg_path).map_err(|e| anyhow!(e.to_string()))?;
    println!("{} {}", "ok".green(), format!("config {}", cfg_path.display()));

    // Provider keys
    for (name, p) in &cfg.providers {
        match p {
            lorelei_core::ProviderConfig::OpenAiCompatible(c) => check_key_source(name, &c.api_key)?,
            lorelei_core::ProviderConfig::Anthropic(c) => check_key_source(name, &c.api_key)?,
            _ => {}
        }
    }

    // API health + readiness
    harbor.get_json("/healthz").await?;
    println!("{} {}", "ok".green(), "harbor /healthz");
    harbor.get_json("/readyz").await?;
    println!("{} {}", "ok".green(), "harbor /readyz (db + qdrant)");

    println!("{}", "all checks passed".green().bold());
    Ok(())
}

fn check_key_source(provider_name: &str, src: &lorelei_core::ApiKeySource) -> Result<()> {
    match src {
        lorelei_core::ApiKeySource::Env { var } => {
            let v = std::env::var(var).unwrap_or_default();
            if v.trim().is_empty() {
                return Err(anyhow!(
                    "missing API key env var `{}` for provider `{}`",
                    var,
                    provider_name
                ));
            }
            println!("{} {}", "ok".green(), format!("key {} (env:{})", provider_name, var));
            Ok(())
        }
        lorelei_core::ApiKeySource::Literal { .. } => {
            println!(
                "{} {}",
                "warn".yellow(),
                format!("provider `{}` uses literal API key in config", provider_name)
            );
            Ok(())
        }
    }
}

fn run_cmd(program: &str, args: &[&str]) -> Result<()> {
    println!(
        "{} {} {}",
        "run".cyan().bold(),
        program.bold(),
        args.join(" ")
    );
    let status = Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("failed to start `{program}`"))?;
    if !status.success() {
        return Err(anyhow!("command failed with status {status}"));
    }
    Ok(())
}

fn check_cmd(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("failed to start `{program}`"))?;
    if !status.success() {
        return Err(anyhow!("command failed"));
    }
    Ok(())
}

fn env_path(name: &str) -> Result<PathBuf> {
    let v = std::env::var(name).with_context(|| format!("missing env var `{name}`"))?;
    Ok(PathBuf::from(v))
}

fn load_cli_env() -> Result<(HarborClient, Uuid, Option<Uuid>)> {
    let harbor_url =
        std::env::var("LORELEI_HARBOR_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
    let tenant_id = std::env::var("TENANT_ID")
        .context("missing TENANT_ID in env")?
        .parse::<Uuid>()
        .context("invalid TENANT_ID")?;
    let agent_id = std::env::var("AGENT_ID").ok().and_then(|s| s.parse::<Uuid>().ok());
    Ok((HarborClient::new(harbor_url)?, tenant_id, agent_id))
}

async fn create_run(harbor: &HarborClient, tenant_id: Uuid) -> Result<Uuid> {
    let resp = harbor
        .post_json("/v1/runs", json!({"tenant_id": tenant_id, "metadata": {}}))
        .await?;
    let id = resp
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("unexpected response from /v1/runs"))?;
    Ok(Uuid::parse_str(id)?)
}

#[derive(Clone)]
struct HarborClient {
    base: String,
    client: reqwest::Client,
}

impl HarborClient {
    fn new(base: String) -> Result<Self> {
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }

    async fn get_json(&self, path: &str) -> Result<serde_json::Value> {
        let resp = self.client.get(self.url(path)).send().await?;
        handle_json(resp).await
    }

    async fn post_json(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value> {
        let resp = self.client.post(self.url(path)).json(&body).send().await?;
        handle_json(resp).await
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let resp = self.client.delete(self.url(path)).send().await?;
        if resp.status().is_success() {
            return Ok(());
        }
        let v = resp.json::<serde_json::Value>().await.unwrap_or(json!({}));
        Err(anyhow!("delete failed: {v}"))
    }
}

async fn handle_json(resp: reqwest::Response) -> Result<serde_json::Value> {
    let status = resp.status();
    let v = resp.json::<serde_json::Value>().await.unwrap_or(json!({}));
    if status.is_success() {
        return Ok(v);
    }
    Err(anyhow!("http {}: {}", status, v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clap_parses_basic_commands() {
        let _ = Cli::try_parse_from(["lore", "init"]).unwrap();
        let _ = Cli::try_parse_from(["lore", "ask", "hello"]).unwrap();
        let _ = Cli::try_parse_from(["lore", "memo", "x"]).unwrap();
        let _ = Cli::try_parse_from(["lore", "echo", "q"]).unwrap();
        let _ = Cli::try_parse_from(["lore", "pearls"]).unwrap();
        let _ = Cli::try_parse_from(["lore", "providers"]).unwrap();
        let _ = Cli::try_parse_from(["lore", "reef", "up"]).unwrap();
        let _ = Cli::try_parse_from(["lore", "reef", "down"]).unwrap();
        let _ = Cli::try_parse_from(["lore", "reef", "logs"]).unwrap();
        let _ = Cli::try_parse_from(["lore", "ship"]).unwrap();
        let _ = Cli::try_parse_from(["lore", "doctor"]).unwrap();
    }
}
