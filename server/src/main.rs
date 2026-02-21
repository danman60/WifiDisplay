mod capture;
mod config;
mod encoder;
mod input;
mod transport;

use clap::Parser;
use config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::init();

    let config = Config::parse();
    tracing::info!("WiFi Display Server starting");
    tracing::info!("Video port: {}, Touch port: {}", config.video_port, config.touch_port);
    tracing::info!("Target monitor: {:?}", config.monitor);
    tracing::info!("Bitrate: {} kbps", config.bitrate);

    // Phase 2: capture → encode → stream pipeline
    // Phase 4: touch input listener

    tracing::info!("Server ready. Waiting for connections...");

    // Placeholder: keep running
    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutting down");

    Ok(())
}
