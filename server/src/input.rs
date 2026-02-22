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
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
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
use windows::Win32::Foundation::{BOOL, LPARAM, RECT};

const TOUCH_MOVE: u8 = 0;
const TOUCH_DOWN: u8 = 1;
const TOUCH_UP: u8 = 2;
const PACKET_SIZE: usize = 9;

/// Monitor bounds in physical pixels.
#[derive(Debug, Clone)]
struct MonitorBounds {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
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
    /// Create an injector targeting the monitor at the given index.
    pub fn new(monitor_index: usize) -> anyhow::Result<Self> {
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

        // Enumerate monitors
        let monitors = enumerate_monitors()?;
        anyhow::ensure!(!monitors.is_empty(), "No monitors found");
        anyhow::ensure!(monitor_index < monitors.len(),
            "Monitor index {} out of range (found {} monitors)",
            monitor_index, monitors.len());

        let monitor = monitors[monitor_index].clone();
        tracing::info!(
            "Target monitor #{}: {}x{} at ({},{})",
            monitor_index, monitor.width, monitor.height, monitor.x, monitor.y
        );

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

/// Enumerate all monitors and return their bounds.
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

        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };

        if unsafe { GetMonitorInfoW(hmonitor, &mut info) }.as_bool() {
            let rc = info.rcMonitor;
            monitors.push(MonitorBounds {
                x: rc.left,
                y: rc.top,
                width: rc.right - rc.left,
                height: rc.bottom - rc.top,
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
