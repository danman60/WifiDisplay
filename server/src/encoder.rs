/// H.264 hardware encoding with fallback cascade.
///
/// Phase 2: Try NVENC → QSV → AMF → libx264
/// Config: low-latency, no B-frames, zerolatency tune, CBR

pub struct VideoEncoder {
    // TODO: ffmpeg-next integration
}

impl VideoEncoder {
    pub fn new(_bitrate: u32) -> anyhow::Result<Self> {
        anyhow::bail!("Video encoder not yet implemented (Phase 2)")
    }
}
