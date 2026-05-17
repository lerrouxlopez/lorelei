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
    /// Ask The Song (runs the Tide loop via Harbor)
    Ask(AskArgs),
    /// List Pearls via Harbor
    Pearls(PearlsArgs),
    /// Soft-delete a Pearl via Harbor
    Forget(ForgetArgs),
    /// List configured providers (via Harbor)
    Providers(HarborArgs),
    /// List available Shell tools (via Harbor)
    Shells(HarborArgs),
    /// Manage autonomous tasks
    Task(TaskArgs),
    /// List pending/decided approvals
    Approvals(ApprovalsArgs),
    /// Approve a pending approval
    Approve(ApproveArgs),
    /// Reef (docker compose) operations
    Reef {
        #[command(subcommand)]
        command: ReefCommand,
    },
    /// Build the Docker image(s)
    Ship(ShipArgs),
    /// Inspect runs and their artifacts (via Harbor)
    Run(RunArgs),
    /// Document ingestion + search (via Harbor)
    Docs(DocsArgs),
}

#[derive(Debug, Args)]
pub struct RunArgs {
    #[command(flatten)]
    pub config: ConfigArgs,
    #[command(flatten)]
    pub harbor: HarborArgs,
    #[command(subcommand)]
    pub command: RunCommand,
}

#[derive(Debug, Subcommand)]
pub enum RunCommand {
    Inspect(RunIdArg),
    Currents(RunIdArg),
    Memories(RunIdArg),
}

#[derive(Debug, Args)]
pub struct RunIdArg {
    pub run_id: String,
}

#[derive(Debug, Args)]
pub struct DocsArgs {
    #[command(flatten)]
    pub config: ConfigArgs,
    #[command(flatten)]
    pub harbor: HarborArgs,
    #[command(subcommand)]
    pub command: DocsCommand,
}

#[derive(Debug, Subcommand)]
pub enum DocsCommand {
    Ingest(DocsIngestArgs),
    Search(DocsSearchArgs),
}

#[derive(Debug, Args)]
pub struct DocsIngestArgs {
    pub path: String,
}

#[derive(Debug, Args)]
pub struct DocsSearchArgs {
    pub query: String,
    #[arg(long)]
    pub top_k: Option<usize>,
}

#[derive(Debug, Args)]
pub struct TaskArgs {
    #[command(flatten)]
    pub config: ConfigArgs,
    #[command(flatten)]
    pub harbor: HarborArgs,
    #[command(subcommand)]
    pub command: TaskCommand,
}

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    Add(TaskAddArgs),
    List(TaskListArgs),
    Pause(TaskPauseArgs),
    Resume(TaskResumeArgs),
}

#[derive(Debug, Args)]
pub struct TaskAddArgs {
    pub prompt: String,
    #[arg(long, default_value_t = false)]
    pub daily: bool,
    /// Time for daily schedule (HH:MM)
    #[arg(long)]
    pub at: Option<String>,
}

#[derive(Debug, Args)]
pub struct TaskListArgs {
    #[arg(long)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct TaskPauseArgs {
    pub task_id: String,
}

#[derive(Debug, Args)]
pub struct TaskResumeArgs {
    pub task_id: String,
}

#[derive(Debug, Args)]
pub struct ApprovalsArgs {
    #[command(flatten)]
    pub config: ConfigArgs,
    #[command(flatten)]
    pub harbor: HarborArgs,
    #[arg(long)]
    pub state: Option<String>,
}

#[derive(Debug, Args)]
pub struct ApproveArgs {
    #[command(flatten)]
    pub config: ConfigArgs,
    #[command(flatten)]
    pub harbor: HarborArgs,
    pub approval_id: String,
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
pub struct AskArgs {
    #[command(flatten)]
    pub config: ConfigArgs,
    #[command(flatten)]
    pub harbor: HarborArgs,
    /// Disable memory retrieval (Echo)
    #[arg(long)]
    pub no_memory: bool,
    /// Print live progress while Harbor runs the Tide loop
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub progress: bool,
    /// User prompt
    pub prompt: String,
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
    fn clap_parses_ask() {
        let cli = Cli::try_parse_from(["lore", "ask", "--no-memory", "hello"]).unwrap();
        match cli.command {
            Command::Ask(a) => {
                assert!(a.no_memory);
                assert_eq!(a.prompt, "hello");
            }
            _ => panic!("expected ask"),
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

    #[test]
    fn clap_parses_task_add_daily_at() {
        let cli = Cli::try_parse_from([
            "lore",
            "task",
            "add",
            "do the thing",
            "--daily",
            "--at",
            "09:00",
        ])
        .unwrap();
        match cli.command {
            Command::Task(t) => match t.command {
                TaskCommand::Add(a) => {
                    assert!(a.daily);
                    assert_eq!(a.at.as_deref(), Some("09:00"));
                }
                _ => panic!("expected task add"),
            },
            _ => panic!("expected task"),
        }
    }

    #[test]
    fn clap_parses_run_subcommands() {
        let cli = Cli::try_parse_from(["lore", "run", "inspect", "123"]).unwrap();
        match cli.command {
            Command::Run(r) => match r.command {
                RunCommand::Inspect(a) => assert_eq!(a.run_id, "123"),
                _ => panic!("expected run inspect"),
            },
            _ => panic!("expected run"),
        }

        let cli = Cli::try_parse_from(["lore", "run", "currents", "abc"]).unwrap();
        match cli.command {
            Command::Run(r) => match r.command {
                RunCommand::Currents(a) => assert_eq!(a.run_id, "abc"),
                _ => panic!("expected run currents"),
            },
            _ => panic!("expected run"),
        }

        let cli = Cli::try_parse_from(["lore", "run", "memories", "run-1"]).unwrap();
        match cli.command {
            Command::Run(r) => match r.command {
                RunCommand::Memories(a) => assert_eq!(a.run_id, "run-1"),
                _ => panic!("expected run memories"),
            },
            _ => panic!("expected run"),
        }
    }

    #[test]
    fn clap_parses_docs_commands() {
        let cli = Cli::try_parse_from(["lore", "docs", "ingest", "README.md"]).unwrap();
        match cli.command {
            Command::Docs(d) => match d.command {
                DocsCommand::Ingest(i) => assert_eq!(i.path, "README.md"),
                _ => panic!("expected docs ingest"),
            },
            _ => panic!("expected docs"),
        }

        let cli = Cli::try_parse_from(["lore", "docs", "search", "tea", "--top-k", "3"]).unwrap();
        match cli.command {
            Command::Docs(d) => match d.command {
                DocsCommand::Search(s) => {
                    assert_eq!(s.query, "tea");
                    assert_eq!(s.top_k, Some(3));
                }
                _ => panic!("expected docs search"),
            },
            _ => panic!("expected docs"),
        }
    }
}
