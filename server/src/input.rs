/// Touch input injection via Windows SendInput API.
///
/// Phase 4: Receive normalized touch coords from Android,
/// map to virtual monitor absolute position, inject mouse events.

pub struct InputInjector {
    // TODO: monitor rect, SendInput calls
}

impl InputInjector {
    pub fn new(_monitor_name: &str) -> anyhow::Result<Self> {
        anyhow::bail!("Input injection not yet implemented (Phase 4)")
    }
}
