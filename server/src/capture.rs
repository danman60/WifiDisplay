/// Screen capture using Windows Graphics Capture API.
///
/// Phase 2: Detect virtual monitor by name, start capture,
/// deliver frames to encoder pipeline.

pub struct ScreenCapture {
    // TODO: windows-capture integration
}

impl ScreenCapture {
    pub fn new(_monitor_name: &str) -> anyhow::Result<Self> {
        anyhow::bail!("Screen capture not yet implemented (Phase 2)")
    }
}
