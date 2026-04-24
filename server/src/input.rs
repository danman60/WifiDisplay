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
pub struct MonitorBounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Monitor display name, e.g. `\\.\DISPLAY3`.
    pub device_name: String,
    /// Adapter DeviceString, e.g. `"IddSampleDriver Device"`, `"Parsec Virtual Display Adapter"`, `"NVIDIA GeForce RTX 4070"`.
    pub adapter_string: String,
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
    /// Create an injector for the already-selected monitor bounds. main.rs runs the
    /// shared `select_target_monitor` once and threads the bounds here AND threads
    /// the device_name to the capture loop, guaranteeing touch+capture lock to the
    /// same physical screen regardless of scrap vs EnumDisplayMonitors ordering.
    pub fn new(monitor: MonitorBounds, captured_width: usize, captured_height: usize, reason: &str) -> anyhow::Result<Self> {
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

        let cw = captured_width as i32;
        let ch = captured_height as i32;

        if monitor.width == cw && monitor.height == ch {
            tracing::info!(
                "Touch target LOCKED to {}x{} at ({},{}) device={} adapter={:?} — reason: {}",
                monitor.width, monitor.height, monitor.x, monitor.y,
                monitor.device_name, monitor.adapter_string, reason
            );
        } else {
            tracing::warn!(
                "Touch target LOCKED to {}x{} at ({},{}) device={} adapter={:?} — reason: {} (capture is {}x{}; scrap/EnumDisplayMonitors may disagree on order)",
                monitor.width, monitor.height, monitor.x, monitor.y,
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

/// Build a `DeviceName (\\.\DISPLAYn) → DeviceString (e.g. "Virtual Display Driver")` map
/// by iterating adapters with `EnumDisplayDevicesW(NULL, iDevNum, ...)`.
///
/// Prior implementation called `EnumDisplayDevicesW(PCWSTR(wide.as_ptr()), 0, ...)` passing
/// the monitor's `szDevice` as the first arg. That variant returns monitor-panel info
/// ("Generic PnP Monitor"), not the adapter. This iterates adapters instead.
#[cfg(windows)]
fn build_adapter_map() -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut i_dev_num: u32 = 0;
    loop {
        let mut adapter = DISPLAY_DEVICEW {
            cb: size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // lpDevice = null → enumerate adapters (display devices), not monitors.
        let ok = unsafe {
            EnumDisplayDevicesW(PCWSTR::null(), i_dev_num, &mut adapter, 0).as_bool()
        };
        if !ok {
            break;
        }
        let name = wide_to_string(&adapter.DeviceName);       // e.g. \\.\DISPLAY3
        let string = wide_to_string(&adapter.DeviceString);   // e.g. "Virtual Display Driver"
        if !name.is_empty() {
            map.insert(name, string);
        }
        i_dev_num += 1;
    }
    map
}

/// Look up the adapter DeviceString for a monitor display name (e.g. `\\.\DISPLAY3`).
/// Returns empty string if the lookup fails.
#[cfg(windows)]
fn adapter_string_for_device(device_name: &str, map: &std::collections::HashMap<String, String>) -> String {
    map.get(device_name).cloned().unwrap_or_default()
}

/// Payload for EnumDisplayMonitors callback: monitor list + adapter name→string map.
#[cfg(windows)]
struct EnumPayload {
    monitors: Vec<MonitorBounds>,
    adapter_map: std::collections::HashMap<String, String>,
}

/// Enumerate all monitors and return their bounds + identity fields.
#[cfg(windows)]
fn enumerate_monitors() -> anyhow::Result<Vec<MonitorBounds>> {
    let mut payload = EnumPayload {
        monitors: Vec::new(),
        adapter_map: build_adapter_map(),
    };

    unsafe extern "system" fn callback(
        hmonitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let payload = &mut *(lparam.0 as *mut EnumPayload);

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
            let adapter_string = adapter_string_for_device(&device_name, &payload.adapter_map);
            payload.monitors.push(MonitorBounds {
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
            LPARAM(&mut payload as *mut EnumPayload as isize),
        )
        .ok()
        .context("EnumDisplayMonitors failed")?;
    }

    Ok(payload.monitors)
}

/// Result of VDD / target-monitor selection.
#[cfg(windows)]
#[derive(Debug, Clone)]
pub struct TargetSelection {
    pub bounds: MonitorBounds,
    pub reason: String,
}

/// Shared VDD selection logic used by BOTH touch injection and screen capture.
///
/// Priority:
///   1. env `WIFI_DISPLAY_VDD_MATCH=<substring>` — operator override (case-insensitive match
///      against device_name OR adapter_string).
///   2. Heuristic: adapter/device string looks like a virtual display driver (IddSampleDriver,
///      Parsec VDD, Amyuni, VirtualDisplayDriver, etc.) — see `looks_like_vdd`.
///   3. Fallback: `monitor_index_fallback` (the `--monitor-index` CLI arg).
///
/// Logs every enumerated monitor and the final selection. Returns error only if zero
/// monitors enumerated or index fallback is also out of range.
#[cfg(windows)]
pub fn select_target_monitor(monitor_index_fallback: usize) -> anyhow::Result<TargetSelection> {
    let monitors = enumerate_monitors()?;
    anyhow::ensure!(!monitors.is_empty(), "No monitors found");

    for (i, m) in monitors.iter().enumerate() {
        tracing::info!(
            "monitor[{}] {}x{} at ({},{}) device={} adapter={:?}",
            i, m.width, m.height, m.x, m.y, m.device_name, m.adapter_string
        );
    }

    let override_substr = std::env::var("WIFI_DISPLAY_VDD_MATCH").ok();

    let mut selected_idx: Option<usize> = None;
    let mut reason: String = String::new();

    if let Some(needle) = override_substr.as_deref() {
        let n = needle.trim();
        if !n.is_empty() {
            let nl = n.to_ascii_lowercase();
            if let Some((i, _)) = monitors.iter().enumerate().find(|(_, m)| {
                m.device_name.to_ascii_lowercase().contains(&nl)
                    || m.adapter_string.to_ascii_lowercase().contains(&nl)
            }) {
                selected_idx = Some(i);
                reason = format!("env WIFI_DISPLAY_VDD_MATCH={:?}", n);
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
            reason = "VDD heuristic match".to_string();
        }
    }

    if selected_idx.is_none() {
        if monitor_index_fallback < monitors.len() {
            selected_idx = Some(monitor_index_fallback);
            reason = "--monitor-index fallback".to_string();
        }
    }

    let idx = selected_idx.ok_or_else(|| anyhow::anyhow!(
        "Could not select any monitor ({} monitors enumerated, index={})",
        monitors.len(), monitor_index_fallback
    ))?;

    let bounds = monitors[idx].clone();
    Ok(TargetSelection { bounds, reason })
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
