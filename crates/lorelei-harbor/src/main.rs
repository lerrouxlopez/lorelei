#[tokio::main]
async fn main() {
    if let Err(e) = lorelei_harbor::http::server::serve().await {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
