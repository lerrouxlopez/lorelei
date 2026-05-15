mod app;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    app::run().await?;
    Ok(())
}
