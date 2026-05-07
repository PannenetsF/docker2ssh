mod cli;
mod config;
mod docker;
mod proxy;
mod ssh_server;

use anyhow::Context as _;
use clap::Parser as _;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,d2s=debug")),
        )
        .init();

    let cli = cli::Cli::parse();
    cli.run().await.context("d2s failed")?;
    Ok(())
}
