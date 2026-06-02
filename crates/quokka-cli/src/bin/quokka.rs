#[tokio::main]
async fn main() -> anyhow::Result<()> {
    quokka_cli::run().await
}
