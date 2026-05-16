#![forbid(unsafe_code)]

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "lore", version, about = "Lorelei CLI")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create `lorelei.toml` + `.env` from examples
    Init(InitArgs),
    /// Validate config + environment + Reef readiness
    Doctor(ConfigArgs),
    /// Save a Pearl (memory) via Harbor
    Memo(MemoArgs),
    /// Retrieve Pearls from the Echo (RAG) via Harbor
    Echo(EchoArgs),
    /// List Pearls via Harbor
    Pearls(PearlsArgs),
    /// Soft-delete a Pearl via Harbor
    Forget(ForgetArgs),
    /// List configured providers (via Harbor)
    Providers(HarborArgs),
    /// List available Shell tools (via Harbor)
    Shells(HarborArgs),
    /// Reef (docker compose) operations
    Reef {
        #[command(subcommand)]
        command: ReefCommand,
    },
    /// Build the Docker image(s)
    Ship(ShipArgs),
}

#[derive(Debug, Args, Clone)]
pub struct ConfigArgs {
    /// Path to config TOML
    #[arg(long, default_value = "lorelei.toml")]
    pub config: PathBuf,
}

#[derive(Debug, Args, Clone)]
pub struct HarborArgs {
    /// Harbor base URL (defaults to $LORELEI_HARBOR_URL or http://localhost:8080)
    #[arg(long)]
    pub harbor_url: Option<String>,
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Overwrite existing files
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct MemoArgs {
    #[command(flatten)]
    pub config: ConfigArgs,
    #[command(flatten)]
    pub harbor: HarborArgs,
    /// Pearl content
    pub content: String,
    /// Pearl type (default: other)
    #[arg(long)]
    pub pearl_type: Option<String>,
    #[arg(long)]
    pub confidence: Option<f64>,
    #[arg(long)]
    pub importance: Option<f64>,
}

#[derive(Debug, Args)]
pub struct EchoArgs {
    #[command(flatten)]
    pub config: ConfigArgs,
    #[command(flatten)]
    pub harbor: HarborArgs,
    pub query: String,
    #[arg(long)]
    pub top_k: Option<usize>,
    #[arg(long)]
    pub min_confidence: Option<f64>,
    #[arg(long)]
    pub pearl_type: Option<String>,
}

#[derive(Debug, Args)]
pub struct PearlsArgs {
    #[command(flatten)]
    pub config: ConfigArgs,
    #[command(flatten)]
    pub harbor: HarborArgs,
    #[arg(long)]
    pub agent_id: Option<String>,
    #[arg(long)]
    pub pearl_type: Option<String>,
    #[arg(long)]
    pub limit: Option<usize>,
}

#[derive(Debug, Args)]
pub struct ForgetArgs {
    #[command(flatten)]
    pub config: ConfigArgs,
    #[command(flatten)]
    pub harbor: HarborArgs,
    pub pearl_id: String,
}

#[derive(Debug, Subcommand)]
pub enum ReefCommand {
    Up(ReefUpArgs),
    Down(ReefArgs),
    Logs(ReefLogsArgs),
}

#[derive(Debug, Args)]
pub struct ReefArgs {
    /// docker compose file (defaults to `docker-compose.yml`)
    #[arg(long, default_value = "docker-compose.yml")]
    pub compose_file: PathBuf,
}

#[derive(Debug, Args)]
pub struct ReefUpArgs {
    #[command(flatten)]
    pub common: ReefArgs,
}

#[derive(Debug, Args)]
pub struct ReefLogsArgs {
    #[command(flatten)]
    pub common: ReefArgs,
    /// Follow logs
    #[arg(long)]
    pub follow: bool,
    /// Number of lines to show
    #[arg(long, default_value_t = 200)]
    pub tail: usize,
}

#[derive(Debug, Args)]
pub struct ShipArgs {
    /// docker compose file (defaults to `docker-compose.yml`)
    #[arg(long, default_value = "docker-compose.yml")]
    pub compose_file: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn clap_parses_memo() {
        let cli = Cli::try_parse_from(["lore", "memo", "hello"]).unwrap();
        match cli.command {
            Command::Memo(m) => assert_eq!(m.content, "hello"),
            _ => panic!("expected memo"),
        }
    }

    #[test]
    fn clap_parses_shells() {
        let cli = Cli::try_parse_from(["lore", "shells"]).unwrap();
        match cli.command {
            Command::Shells(_) => {}
            _ => panic!("expected shells"),
        }
    }

    #[test]
    fn clap_parses_reef_logs_flags() {
        let cli =
            Cli::try_parse_from(["lore", "reef", "logs", "--follow", "--tail", "50"]).unwrap();
        match cli.command {
            Command::Reef {
                command: ReefCommand::Logs(a),
            } => {
                assert!(a.follow);
                assert_eq!(a.tail, 50);
            }
            _ => panic!("expected reef logs"),
        }
    }
}
