use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "lorelei-harbor")]
struct Args {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    Serve,
    Migrate,
    Worker {
        #[arg(long, default_value_t = 5)]
        poll_seconds: u64,
        #[arg(long, default_value_t = 60)]
        lease_seconds: i64,
        #[arg(long, default_value_t = 5)]
        batch_size: i64,
        #[arg(long, default_value_t = false)]
        once: bool,
    },
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let res = match args.cmd.unwrap_or(Cmd::Serve) {
        Cmd::Serve => lorelei_harbor::http::server::serve().await,
        Cmd::Migrate => lorelei_harbor::http::server::migrate().await,
        Cmd::Worker {
            poll_seconds,
            lease_seconds,
            batch_size,
            once,
        } => {
            lorelei_harbor::worker::run(lorelei_harbor::worker::WorkerConfig {
                poll_every: std::time::Duration::from_secs(poll_seconds),
                lease_seconds,
                batch_size,
                once,
            })
            .await
        }
    };

    if let Err(e) = res {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
