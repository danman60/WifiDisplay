/// Touch input injection via Windows SendInput API.
///
/// Receives normalized touch coordinates from Android over UDP,
/// maps them to the target monitor's absolute position, and injects
/// mouse events using SendInput.

use anyhow::Context;
use std::sync::Arc;
use tokio::net::UdpSocket;

#[cfg(windows)]
use windows::Win32::{
    Graphics::Gdi::{
        EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, DISPLAY_DEVICEW,
        HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    },
    UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE,
        MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK,
        MOUSEINPUT,
    },
    UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
        SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN},
};

#[cfg(windows)]
use windows::core::PCWSTR;

#[cfg(windows)]
use windows::Win32::Foundation::{BOOL, LPARAM, RECT};

const TOUCH_MOVE: u8 = 0;
const TOUCH_DOWN: u8 = 1;
const TOUCH_UP: u8 = 2;
const PACKET_SIZE: usize = 9;

/// Monitor bounds in physical pixels plus adapter/device identity for VDD matching.
#[derive(Debug, Clone)]
struct MonitorBounds {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    /// Monitor display name, e.g. `\\.\DISPLAY3`.
    device_name: String,
    /// Adapter DeviceString, e.g. `"IddSampleDriver Device"`, `"Parsec Virtual Display Adapter"`, `"NVIDIA GeForce RTX 4070"`.
    adapter_string: String,
}

/// Returns true if this adapter/device looks like a Virtual Display Driver
/// (IDD-based sample drivers, Parsec VDD, amyuni, Virtual Display, etc.).
fn looks_like_vdd(adapter_string: &str, device_name: &str) -> bool {
    let a = adapter_string.to_ascii_lowercase();
    let d = device_name.to_ascii_lowercase();
    // Common VDD adapter DeviceString tokens:
    //   "IddSampleDriver Device"       — Microsoft IDD sample
    //   "Virtual Display Driver"       — open-source VirtualDisplayDriver
    //   "Parsec Virtual Display Adapter"
    //   "Amyuni USB Mobile Monitor"
    //   "MttVDD"                       — some OEM names
    let tokens = ["virtual", "idd", "vdd", "parsec", "amyuni", "deskreen"];
    tokens.iter().any(|t| a.contains(t) || d.contains(t))
}

/// Virtual desktop bounds for absolute coordinate mapping.
#[derive(Debug, Clone)]
struct VirtualDesktop {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

pub struct InputInjector {
    monitor: MonitorBounds,
    desktop: VirtualDesktop,
}

#[cfg(windows)]
impl InputInjector {
    /// Create an injector targeting the monitor that matches the captured resolution.
    /// Falls back to monitor_index if no resolution match is found.
    pub fn new(monitor_index: usize, captured_width: usize, captured_height: usize) -> anyhow::Result<Self> {
        // Query virtual desktop dimensions
        let desktop = unsafe {
            VirtualDesktop {
                x: GetSystemMetrics(SM_XVIRTUALSCREEN),
                y: GetSystemMetrics(SM_YVIRTUALSCREEN),
                width: GetSystemMetrics(SM_CXVIRTUALSCREEN),
                height: GetSystemMetrics(SM_CYVIRTUALSCREEN),
            }
        };

        anyhow::ensure!(desktop.width > 0 && desktop.height > 0,
            "Failed to query virtual desktop dimensions");

        tracing::info!(
            "Virtual desktop: {}x{} at ({},{})",
            desktop.width, desktop.height, desktop.x, desktop.y
        );

        // Enumerate monitors via Win32 API
        let monitors = enumerate_monitors()?;
        anyhow::ensure!(!monitors.is_empty(), "No monitors found");

        // Log all enumerated monitors up-front so we have full diagnostic output on DART.
        for (i, m) in monitors.iter().enumerate() {
            tracing::info!(
                "monitor[{}] {}x{} at ({},{}) device={} adapter={:?}",
                i, m.width, m.height, m.x, m.y, m.device_name, m.adapter_string
            );
        }

        // ── VDD selection priority ─────────────────────────────────────────────
        // (1) env WIFI_DISPLAY_VDD_MATCH=<substring> — operator override, matches
        //     case-insensitively against device_name or adapter_string.
        // (2) heuristic: adapter/device string looks like a virtual display driver
        //     (IddSampleDriver, Parsec VDD, Amyuni, VirtualDisplayDriver, etc.)
        // (3) fallback: use monitor_index.
        //
        // Why: scrap::Display and EnumDisplayMonitors may iterate monitors in
        // different orders, so touch can land on the wrong physical screen.
        // The VDD is the "tablet mirror" target in practice, so locking to it
        // by identity (not index) is the reliable fix.
        let cw = captured_width as i32;
        let ch = captured_height as i32;

        let override_substr = std::env::var("WIFI_DISPLAY_VDD_MATCH").ok();

        let mut selected_idx: Option<usize> = None;
        let mut reason: &'static str = "";

        if let Some(needle) = override_substr.as_deref() {
            let n = needle.trim();
            if !n.is_empty() {
                let nl = n.to_ascii_lowercase();
                if let Some((i, _)) = monitors.iter().enumerate().find(|(_, m)| {
                    m.device_name.to_ascii_lowercase().contains(&nl)
                        || m.adapter_string.to_ascii_lowercase().contains(&nl)
                }) {
                    selected_idx = Some(i);
                    reason = "env WIFI_DISPLAY_VDD_MATCH";
                } else {
                    tracing::warn!("WIFI_DISPLAY_VDD_MATCH={n:?} did not match any monitor adapter/device string");
                }
            }
        }

        if selected_idx.is_none() {
            if let Some((i, _)) = monitors.iter().enumerate()
                .find(|(_, m)| looks_like_vdd(&m.adapter_string, &m.device_name))
            {
                selected_idx = Some(i);
                reason = "VDD heuristic match";
            }
        }

        if selected_idx.is_none() {
            if monitor_index < monitors.len() {
                selected_idx = Some(monitor_index);
                reason = "--monitor-index fallback";
            } else if let Some((i, _)) = monitors.iter().enumerate()
                .find(|(_, m)| m.width == cw && m.height == ch)
            {
                selected_idx = Some(i);
                reason = "resolution-match fallback";
            }
        }

        let idx = selected_idx.ok_or_else(|| anyhow::anyhow!(
            "Could not select any monitor for touch injection ({} monitors enumerated, index={}, capture={}x{})",
            monitors.len(), monitor_index, captured_width, captured_height
        ))?;

        let monitor = monitors[idx].clone();
        if monitor.width == cw && monitor.height == ch {
            tracing::info!(
                "Touch target LOCKED to monitor[{}] {}x{} at ({},{}) device={} adapter={:?} — reason: {}",
                idx, monitor.width, monitor.height, monitor.x, monitor.y,
                monitor.device_name, monitor.adapter_string, reason
            );
        } else {
            tracing::warn!(
                "Touch target LOCKED to monitor[{}] {}x{} at ({},{}) device={} adapter={:?} — reason: {} (capture is {}x{}; scrap/EnumDisplayMonitors may disagree on order)",
                idx, monitor.width, monitor.height, monitor.x, monitor.y,
                monitor.device_name, monitor.adapter_string, reason, cw, ch
            );
        }

        Ok(Self { monitor, desktop })
    }

    /// Map normalized (0.0-1.0) coordinates to absolute virtual desktop coords (0-65535).
    fn to_absolute(&self, norm_x: f32, norm_y: f32) -> (i32, i32) {
        // Clamp to valid range
        let nx = norm_x.clamp(0.0, 1.0);
        let ny = norm_y.clamp(0.0, 1.0);

        // Map to pixel position on target monitor
        let pixel_x = self.monitor.x + (nx * self.monitor.width as f32) as i32;
        let pixel_y = self.monitor.y + (ny * self.monitor.height as f32) as i32;

        // Convert pixel position to 0-65535 absolute coordinates
        // relative to the virtual desktop
        let abs_x = ((pixel_x - self.desktop.x) as f64 / self.desktop.width as f64 * 65535.0) as i32;
        let abs_y = ((pixel_y - self.desktop.y) as f64 / self.desktop.height as f64 * 65535.0) as i32;

        (abs_x, abs_y)
    }

    fn make_mouse_input(&self, norm_x: f32, norm_y: f32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
        let (abs_x, abs_y) = self.to_absolute(norm_x, norm_y);

        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: abs_x,
                    dy: abs_y,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    pub fn inject_move(&self, norm_x: f32, norm_y: f32) {
        let input = self.make_mouse_input(norm_x, norm_y, MOUSEEVENTF_MOVE);
        unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    }

    pub fn inject_down(&self, norm_x: f32, norm_y: f32) {
        let input = self.make_mouse_input(norm_x, norm_y, MOUSEEVENTF_MOVE | MOUSEEVENTF_LEFTDOWN);
        let (abs_x, abs_y) = self.to_absolute(norm_x, norm_y);
        tracing::info!(
            "TOUCH_DOWN norm=({:.3},{:.3}) monitor=({},{})@{}x{} abs=({},{})",
            norm_x, norm_y, self.monitor.x, self.monitor.y, self.monitor.width, self.monitor.height,
            abs_x, abs_y
        );
        unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    }

    pub fn inject_up(&self, norm_x: f32, norm_y: f32) {
        let input = self.make_mouse_input(norm_x, norm_y, MOUSEEVENTF_MOVE | MOUSEEVENTF_LEFTUP);
        unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    }

    /// Parse a 9-byte touch packet and inject the corresponding mouse event.
    fn handle_touch_packet(&self, data: &[u8]) {
        if data.len() < PACKET_SIZE {
            return;
        }

        let touch_type = data[0];
        let norm_x = f32::from_le_bytes([data[1], data[2], data[3], data[4]]);
        let norm_y = f32::from_le_bytes([data[5], data[6], data[7], data[8]]);

        match touch_type {
            TOUCH_MOVE => self.inject_move(norm_x, norm_y),
            TOUCH_DOWN => self.inject_down(norm_x, norm_y),
            TOUCH_UP => self.inject_up(norm_x, norm_y),
            _ => tracing::warn!("Unknown touch type: {touch_type}"),
        }
    }
}

/// Decode a null-terminated UTF-16 fixed buffer into a String.
#[cfg(windows)]
fn wide_to_string(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// Look up the adapter DeviceString for a monitor display name (e.g. `\\.\DISPLAY3`).
/// Returns empty string if the lookup fails.
#[cfg(windows)]
fn adapter_string_for_device(device_name: &str) -> String {
    let mut adapter = DISPLAY_DEVICEW {
        cb: size_of::<DISPLAY_DEVICEW>() as u32,
        ..Default::default()
    };

    // Convert device_name to UTF-16 null-terminated.
    let wide: Vec<u16> = device_name.encode_utf16().chain(std::iter::once(0)).collect();

    // Index 0 = the adapter for this display device (Win32 quirk).
    let ok = unsafe { EnumDisplayDevicesW(PCWSTR(wide.as_ptr()), 0, &mut adapter, 0).as_bool() };
    if !ok {
        return String::new();
    }
    wide_to_string(&adapter.DeviceString)
}

/// Enumerate all monitors and return their bounds + identity fields.
#[cfg(windows)]
fn enumerate_monitors() -> anyhow::Result<Vec<MonitorBounds>> {
    let mut monitors: Vec<MonitorBounds> = Vec::new();

    unsafe extern "system" fn callback(
        hmonitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let monitors = &mut *(lparam.0 as *mut Vec<MonitorBounds>);

        let mut info = MONITORINFOEXW {
            monitorInfo: MONITORINFO {
                cbSize: size_of::<MONITORINFOEXW>() as u32,
                ..Default::default()
            },
            ..Default::default()
        };

        let info_ptr = &mut info as *mut MONITORINFOEXW as *mut MONITORINFO;
        if unsafe { GetMonitorInfoW(hmonitor, info_ptr) }.as_bool() {
            let rc = info.monitorInfo.rcMonitor;
            let device_name = wide_to_string(&info.szDevice);
            let adapter_string = adapter_string_for_device(&device_name);
            monitors.push(MonitorBounds {
                x: rc.left,
                y: rc.top,
                width: rc.right - rc.left,
                height: rc.bottom - rc.top,
                device_name,
                adapter_string,
            });
        }

        BOOL(1) // continue enumeration
    }

    unsafe {
        EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(callback),
            LPARAM(&mut monitors as *mut Vec<MonitorBounds> as isize),
        )
        .ok()
        .context("EnumDisplayMonitors failed")?;
    }

    Ok(monitors)
}

/// Listen for touch packets on the given UDP port and inject mouse events.
pub async fn touch_listener(port: u16, injector: Arc<InputInjector>) -> anyhow::Result<()> {
    let socket = UdpSocket::bind(format!("0.0.0.0:{port}"))
        .await
        .with_context(|| format!("Failed to bind touch input socket on port {port}"))?;

    tracing::info!("Touch input listener on port {port}");

    let mut buf = [0u8; 64];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((len, _addr)) => {
                #[cfg(windows)]
                injector.handle_touch_packet(&buf[..len]);
            }
            Err(e) => {
                tracing::warn!("Touch recv error: {e}");
            }
        }
    }
}
