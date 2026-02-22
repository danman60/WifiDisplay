use anyhow::{bail, Context};
use scrap::{Capturer, Display};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

pub struct CapturedFrame {
    pub bgra: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// List available displays and their info.
pub fn list_displays() -> anyhow::Result<Vec<(usize, usize, usize)>> {
    let displays = Display::all().context("Failed to enumerate displays")?;
    Ok(displays
        .iter()
        .map(|d| (0, d.width(), d.height()))
        .collect())
}

/// Blocking capture loop. Runs on a dedicated thread.
/// Captures frames from the specified monitor and sends them through the channel.
pub fn capture_loop(
    monitor_index: usize,
    target_fps: u32,
    tx: mpsc::Sender<CapturedFrame>,
) -> anyhow::Result<()> {
    let displays = Display::all().context("Failed to enumerate displays")?;

    if monitor_index >= displays.len() {
        bail!(
            "Monitor index {} out of range. Found {} display(s).",
            monitor_index,
            displays.len()
        );
    }

    // scrap::Display doesn't implement Clone, need to re-enumerate
    // and pick by index
    let displays = Display::all()?;
    let display = displays
        .into_iter()
        .nth(monitor_index)
        .context("Display disappeared")?;

    let width = display.width();
    let height = display.height();
    tracing::info!("Capturing monitor #{monitor_index}: {width}x{height}");

    let mut capturer = Capturer::new(display).context("Failed to create capturer")?;

    let frame_duration = Duration::from_nanos(1_000_000_000 / target_fps as u64);

    loop {
        let frame_start = Instant::now();

        match capturer.frame() {
            Ok(frame) => {
                // frame is BGRA packed pixels, length = width * height * 4
                let bgra = frame.to_vec();

                let captured = CapturedFrame {
                    bgra,
                    width,
                    height,
                };

                // If channel is full (receiver slow), drop the frame
                if tx.try_send(captured).is_err() {
                    tracing::trace!("Frame dropped (receiver busy)");
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No new frame yet (screen hasn't changed), sleep briefly
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            Err(e) => {
                tracing::error!("Capture error: {e}");
                // Desktop Duplication can fail transiently (e.g. secure desktop)
                // Wait and retry
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        }

        // Pace to target FPS
        let elapsed = frame_start.elapsed();
        if elapsed < frame_duration {
            std::thread::sleep(frame_duration - elapsed);
        }
    }
}
