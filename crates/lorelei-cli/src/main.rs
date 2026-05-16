mod app;
mod cli;
mod harbor_client;

use clap::Parser;

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    lorelei_core::observability::init_tracing("cli");
    let cli = cli::Cli::parse();
    let code = app::run(cli).await;
    std::process::exit(code);
}
