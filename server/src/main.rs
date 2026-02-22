mod capture;
mod config;
mod encoder;
mod input;
mod transport;

use anyhow::Context;
use clap::Parser;
use config::Config;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::parse();
    tracing::info!("WiFi Display Server v{}", env!("CARGO_PKG_VERSION"));
    tracing::info!(
        "Monitor: #{}, Target: {}",
        config.monitor_index,
        config.client.as_deref().unwrap_or("broadcast")
    );
    tracing::info!(
        "Video: :{}, Touch: :{}, Bitrate: {}kbps, FPS: {}",
        config.video_port,
        config.touch_port,
        config.bitrate,
        config.fps
    );

    // Channel: capture thread sends raw BGRA frames to async encode+send pipeline
    let (frame_tx, mut frame_rx) = mpsc::channel::<capture::CapturedFrame>(2);

    // Start capture on a dedicated thread (scrap uses blocking API)
    let monitor_index = config.monitor_index;
    let target_fps = config.fps;
    std::thread::spawn(move || {
        if let Err(e) = capture::capture_loop(monitor_index, target_fps, frame_tx) {
            tracing::error!("Capture thread failed: {e:#}");
        }
    });

    // Initialize encoder
    let first_frame = frame_rx
        .recv()
        .await
        .context("Capture thread exited before sending a frame")?;

    let width = first_frame.width;
    let height = first_frame.height;
    tracing::info!("Captured resolution: {width}x{height}");

    let mut enc = encoder::H264Encoder::new(width, height, config.bitrate, target_fps)
        .context("Failed to create H.264 encoder")?;

    // Initialize transport
    let transport = Arc::new(
        transport::UdpTransport::new(config.video_port, config.client.as_deref())
            .await
            .context("Failed to create UDP transport")?,
    );

    // Start touch input listener — match by captured resolution to handle
    // different enumeration order between scrap and Win32 EnumDisplayMonitors
    let touch_port = config.touch_port;
    let injector = Arc::new(
        input::InputInjector::new(config.monitor_index, width, height)
            .context("Failed to create input injector")?,
    );
    let touch_injector = injector.clone();
    tokio::spawn(async move {
        if let Err(e) = input::touch_listener(touch_port, touch_injector).await {
            tracing::error!("Touch listener failed: {e:#}");
        }
    });

    tracing::info!("Streaming started. Press Ctrl+C to stop.");

    // Stats tracking
    let mut frames_sent: u64 = 0;
    let mut bytes_sent: u64 = 0;
    let mut last_stats = Instant::now();

    // Encode and send the first frame
    let nals = enc.encode(&first_frame.bgra, width, height)?;
    let sent = transport.send_nals(&nals).await?;
    frames_sent += 1;
    bytes_sent += sent as u64;

    // Main loop: receive frames, encode, send
    loop {
        tokio::select! {
            frame = frame_rx.recv() => {
                let Some(frame) = frame else {
                    tracing::info!("Capture thread stopped");
                    break;
                };

                match enc.encode(&frame.bgra, frame.width, frame.height) {
                    Ok(nals) => {
                        match transport.send_nals(&nals).await {
                            Ok(sent) => {
                                frames_sent += 1;
                                bytes_sent += sent as u64;
                            }
                            Err(e) => tracing::warn!("Send error: {e}"),
                        }
                    }
                    Err(e) => tracing::warn!("Encode error: {e}"),
                }

                // Print stats every 5 seconds
                if last_stats.elapsed() >= Duration::from_secs(5) {
                    let elapsed = last_stats.elapsed().as_secs_f64();
                    let fps = frames_sent as f64 / elapsed;
                    let mbps = (bytes_sent as f64 * 8.0) / (elapsed * 1_000_000.0);
                    tracing::info!("Stats: {fps:.1} fps, {mbps:.1} Mbps, {frames_sent} frames");
                    frames_sent = 0;
                    bytes_sent = 0;
                    last_stats = Instant::now();
                }
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Shutting down");
                break;
            }
        }
    }

    Ok(())
}
