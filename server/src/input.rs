/// Touch input injection via Windows SendInput API.
///
/// Phase 4: Receive normalized touch coords from Android,
/// map to virtual monitor absolute position, inject mouse events.
///
/// Not yet implemented - placeholder for Phase 4.

pub struct InputInjector {
    _monitor_x: i32,
    _monitor_y: i32,
    _monitor_width: i32,
    _monitor_height: i32,
}

impl InputInjector {
    pub fn new(_monitor_name: &str) -> anyhow::Result<Self> {
        anyhow::bail!("Input injection not yet implemented (Phase 4)")
    }
}
