use anyhow::Context;
use openh264::encoder::{EncoderConfig, Encoder, FrameType};
use openh264::formats::YUVBuffer;
use openh264::OpenH264API;

/// Encoded NAL unit ready for transport.
pub struct EncodedNal {
    pub data: Vec<u8>,
    pub is_keyframe: bool,
}

pub struct H264Encoder {
    encoder: Encoder,
    width: usize,
    height: usize,
    frame_count: u64,
    keyframe_interval: u64,
}

impl H264Encoder {
    pub fn new(width: usize, height: usize, bitrate_kbps: u32, fps: u32) -> anyhow::Result<Self> {
        let config = EncoderConfig::new()
            .max_frame_rate(fps as f32)
            .rate_control_mode(openh264::encoder::RateControlMode::Bitrate)
            .set_bitrate_bps(bitrate_kbps * 1000)
            .enable_skip_frame(true);

        let api = OpenH264API::from_source();
        let encoder = Encoder::with_api_config(api, config)
            .map_err(|e| anyhow::anyhow!("Failed to initialize OpenH264 encoder: {e}"))?;

        tracing::info!(
            "H.264 encoder: {}x{}, {}kbps, {}fps (OpenH264 software)",
            width, height, bitrate_kbps, fps
        );

        Ok(Self {
            encoder,
            width,
            height,
            frame_count: 0,
            keyframe_interval: (fps * 2) as u64, // IDR every 2 seconds
        })
    }

    /// Encode a BGRA frame into H.264 NAL units.
    pub fn encode(
        &mut self,
        bgra: &[u8],
        width: usize,
        height: usize,
    ) -> anyhow::Result<Vec<EncodedNal>> {
        assert_eq!(width, self.width);
        assert_eq!(height, self.height);
        assert_eq!(bgra.len(), width * height * 4);

        // Convert BGRA to YUV420p
        let yuv_data = bgra_to_yuv420(bgra, width, height);
        let yuv = YUVBuffer::from_vec(yuv_data, width, height);

        // Request keyframe periodically
        if self.frame_count % self.keyframe_interval == 0 {
            self.encoder.force_intra_frame();
        }

        // Encode
        let bitstream = self.encoder.encode(&yuv)
            .map_err(|e| anyhow::anyhow!("H.264 encode failed: {e}"))?;

        let is_keyframe = matches!(bitstream.frame_type(), FrameType::IDR | FrameType::I);
        self.frame_count += 1;

        // Extract all NAL units from the bitstream into a single blob
        let data = bitstream.to_vec();
        if data.is_empty() {
            return Ok(Vec::new());
        }

        Ok(vec![EncodedNal { data, is_keyframe }])
    }
}

/// Convert BGRA pixels to YUV420 planar format.
/// Returns Vec with layout: [Y plane: w*h] [U plane: w*h/4] [V plane: w*h/4]
fn bgra_to_yuv420(bgra: &[u8], width: usize, height: usize) -> Vec<u8> {
    let y_size = width * height;
    let uv_size = y_size / 4;
    let mut yuv = vec![0u8; y_size + uv_size * 2];

    let uv_width = width / 2;

    for row in 0..height {
        for col in 0..width {
            let px = (row * width + col) * 4;
            let b = bgra[px] as f32;
            let g = bgra[px + 1] as f32;
            let r = bgra[px + 2] as f32;

            // BT.601 conversion
            let y = (0.299 * r + 0.587 * g + 0.114 * b) as u8;
            yuv[row * width + col] = y;

            // Subsample U and V: one value per 2x2 block
            if row % 2 == 0 && col % 2 == 0 {
                let u = (-0.169 * r - 0.331 * g + 0.500 * b + 128.0).clamp(0.0, 255.0) as u8;
                let v = (0.500 * r - 0.419 * g - 0.081 * b + 128.0).clamp(0.0, 255.0) as u8;
                let uv_idx = (row / 2) * uv_width + (col / 2);
                yuv[y_size + uv_idx] = u;
                yuv[y_size + uv_size + uv_idx] = v;
            }
        }
    }

    yuv
}
