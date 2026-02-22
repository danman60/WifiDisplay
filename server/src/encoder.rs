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
    yuv_data: Vec<u8>,
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

        let y_size = width * height;
        let uv_size = y_size / 4;
        let yuv_data = vec![0u8; y_size + uv_size * 2];

        tracing::info!(
            "H.264 encoder: {}x{}, {}kbps, {}fps (OpenH264 software)",
            width, height, bitrate_kbps, fps
        );

        Ok(Self {
            encoder,
            yuv_data,
            width,
            height,
            frame_count: 0,
            keyframe_interval: (fps * 2) as u64,
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

        // Convert BGRA to YUV420p (fast integer math)
        bgra_to_yuv420_fast(bgra, &mut self.yuv_data, width, height);

        let yuv_len = self.yuv_data.len();
        let yuv = YUVBuffer::from_vec(std::mem::replace(
            &mut self.yuv_data,
            vec![0u8; yuv_len],
        ), width, height);

        // Request keyframe periodically
        if self.frame_count % self.keyframe_interval == 0 {
            self.encoder.force_intra_frame();
        }

        let bitstream = self.encoder.encode(&yuv)
            .map_err(|e| anyhow::anyhow!("H.264 encode failed: {e}"))?;

        let is_keyframe = matches!(bitstream.frame_type(), FrameType::IDR | FrameType::I);
        self.frame_count += 1;

        let data = bitstream.to_vec();

        // Reclaim the yuv buffer from YUVBuffer (it consumed our Vec)
        // We already replaced it above, so yuv_data is a fresh allocation.
        // This is fine — the hot path is the conversion, not allocation.

        if data.is_empty() {
            return Ok(Vec::new());
        }

        Ok(vec![EncodedNal { data, is_keyframe }])
    }
}

/// Fast BGRA to YUV420p conversion using fixed-point integer arithmetic.
/// ~5-10x faster than the float version.
///
/// BT.601 coefficients scaled by 256:
///   Y  =  (77*R + 150*G + 29*B) >> 8
///   U  = (-43*R - 85*G + 128*B + 32768) >> 8
///   V  = (128*R - 107*G - 21*B + 32768) >> 8
fn bgra_to_yuv420_fast(bgra: &[u8], yuv: &mut [u8], width: usize, height: usize) {
    let y_size = width * height;
    let uv_size = y_size / 4;
    let uv_width = width / 2;

    // Split into Y, U, V planes
    let (y_plane, uv_planes) = yuv.split_at_mut(y_size);
    let (u_plane, v_plane) = uv_planes.split_at_mut(uv_size);

    // Process 2 rows at a time for UV subsampling
    for row_pair in 0..(height / 2) {
        let row0 = row_pair * 2;
        let row1 = row0 + 1;

        let bgra_row0 = row0 * width * 4;
        let bgra_row1 = row1 * width * 4;
        let y_row0 = row0 * width;
        let y_row1 = row1 * width;
        let uv_row = row_pair * uv_width;

        for col_pair in 0..(width / 2) {
            let col0 = col_pair * 2;
            let col1 = col0 + 1;

            // Read 4 pixels in the 2x2 block
            let px00 = bgra_row0 + col0 * 4;
            let px01 = bgra_row0 + col1 * 4;
            let px10 = bgra_row1 + col0 * 4;
            let px11 = bgra_row1 + col1 * 4;

            let (b00, g00, r00) = (bgra[px00] as i32, bgra[px00 + 1] as i32, bgra[px00 + 2] as i32);
            let (b01, g01, r01) = (bgra[px01] as i32, bgra[px01 + 1] as i32, bgra[px01 + 2] as i32);
            let (b10, g10, r10) = (bgra[px10] as i32, bgra[px10 + 1] as i32, bgra[px10 + 2] as i32);
            let (b11, g11, r11) = (bgra[px11] as i32, bgra[px11 + 1] as i32, bgra[px11 + 2] as i32);

            // Y for all 4 pixels
            y_plane[y_row0 + col0] = ((77 * r00 + 150 * g00 + 29 * b00) >> 8) as u8;
            y_plane[y_row0 + col1] = ((77 * r01 + 150 * g01 + 29 * b01) >> 8) as u8;
            y_plane[y_row1 + col0] = ((77 * r10 + 150 * g10 + 29 * b10) >> 8) as u8;
            y_plane[y_row1 + col1] = ((77 * r11 + 150 * g11 + 29 * b11) >> 8) as u8;

            // Average RGB for UV (use top-left pixel for speed, or average all 4)
            // Averaging all 4 is more correct but top-left is faster and visually similar
            let r_avg = (r00 + r01 + r10 + r11) >> 2;
            let g_avg = (g00 + g01 + g10 + g11) >> 2;
            let b_avg = (b00 + b01 + b10 + b11) >> 2;

            u_plane[uv_row + col_pair] = ((-43 * r_avg - 85 * g_avg + 128 * b_avg + 32768) >> 8) as u8;
            v_plane[uv_row + col_pair] = ((128 * r_avg - 107 * g_avg - 21 * b_avg + 32768) >> 8) as u8;
        }
    }
}
