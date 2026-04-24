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
///
/// Picks the scrap Display by matching `target_device_name` (e.g. `\\.\DISPLAY3`)
/// case-insensitively against `Display::device_name()` (exposed by the local
/// scrap-local fork). This is the same form returned by Win32
/// `GetMonitorInfoW(... szDevice)`, so touch and capture lock to the SAME
/// physical screen regardless of the (different) enumeration orders of scrap vs
/// EnumDisplayMonitors.
///
/// Falls back to `monitor_index_fallback` when no scrap display matches (e.g.
/// unit-test harness where device_name is empty, or a future Windows API change).
pub fn capture_loop(
    target_device_name: String,
    monitor_index_fallback: usize,
    target_fps: u32,
    tx: mpsc::Sender<CapturedFrame>,
) -> anyhow::Result<()> {
    let displays = Display::all().context("Failed to enumerate displays")?;

    if displays.is_empty() {
        bail!("No scrap displays found");
    }

    // Log every scrap display we found so DART logs show the full picture
    // alongside the EnumDisplayMonitors list emitted from input.rs.
    for (i, d) in displays.iter().enumerate() {
        tracing::info!(
            "scrap_display[{}] {}x{} device={:?}",
            i, d.width(), d.height(), d.device_name()
        );
    }

    // scrap::Display is !Clone, so re-enumerate and consume by index.
    let displays = Display::all()?;
    let target_lower = target_device_name.to_ascii_lowercase();
    let total = displays.len();

    let mut match_idx: Option<usize> = None;
    let mut match_reason: &'static str = "";

    if !target_lower.is_empty() {
        for (i, d) in displays.iter().enumerate() {
            if d.device_name().to_ascii_lowercase() == target_lower {
                match_idx = Some(i);
                match_reason = "DXGI DeviceName match";
                break;
            }
        }
    }

    if match_idx.is_none() {
        if monitor_index_fallback < total {
            match_idx = Some(monitor_index_fallback);
            match_reason = "--monitor-index fallback";
        } else {
            bail!(
                "Capture: no scrap display matched device_name={:?} and --monitor-index={} is out of range (found {} displays)",
                target_device_name, monitor_index_fallback, total
            );
        }
    }

    let idx = match_idx.unwrap();
    let display = displays
        .into_iter()
        .nth(idx)
        .context("Display disappeared between enumerations")?;

    let device_name = display.device_name();
    let width = display.width();
    let height = display.height();
    tracing::info!(
        "Capture target LOCKED to scrap_display[{}] device={} size={}x{} — reason: {} (target was {:?})",
        idx, device_name, width, height, match_reason, target_device_name
    );

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
