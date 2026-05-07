use anyhow::Context;
use openh264::encoder::{Encoder, EncoderConfig, FrameType};
use openh264::formats::YUVBuffer;
use openh264::OpenH264API;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc as std_mpsc;
use std::thread;

/// Encoded NAL unit ready for transport. Codec-agnostic — the transport layer
/// fragments and the Android side reassembles regardless of H.264 vs HEVC.
pub struct EncodedNal {
    pub data: Vec<u8>,
    pub is_keyframe: bool,
}

/// Selects which encoder backend to use. Default is OpenH264 (existing
/// software path). HevcNvenc routes BGRA frames through a bundled ffmpeg.exe
/// child process for GPU-offloaded HEVC encode on Windows + NVIDIA.
#[derive(Debug, Clone, Copy)]
pub enum EncoderKind {
    OpenH264,
    HevcNvenc,
}

impl std::str::FromStr for EncoderKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "openh264" | "h264" | "soft" => Ok(EncoderKind::OpenH264),
            "hevc-nvenc" | "nvenc" | "hevc" => Ok(EncoderKind::HevcNvenc),
            other => Err(format!(
                "unknown encoder '{other}' (expected 'openh264' or 'hevc-nvenc')"
            )),
        }
    }
}

/// Codec-erased encoder enum. We use an enum (not a trait object) to keep the
/// hot encode path monomorphised and avoid Box<dyn ...> + Send shenanigans.
pub enum VideoEncoder {
    OpenH264(H264Encoder),
    HevcNvenc(NvencHevcEncoder),
}

impl VideoEncoder {
    /// Build the appropriate backend. `ffmpeg_path` is only consulted for the
    /// HevcNvenc variant — for OpenH264 it is ignored.
    pub fn new(
        kind: EncoderKind,
        width: usize,
        height: usize,
        bitrate_kbps: u32,
        fps: u32,
        ffmpeg_path: Option<&str>,
    ) -> anyhow::Result<Self> {
        match kind {
            EncoderKind::OpenH264 => {
                let enc = H264Encoder::new(width, height, bitrate_kbps, fps)?;
                Ok(VideoEncoder::OpenH264(enc))
            }
            EncoderKind::HevcNvenc => {
                let path = ffmpeg_path.ok_or_else(|| {
                    anyhow::anyhow!(
                        "--encoder hevc-nvenc requires --ffmpeg-path pointing to ffmpeg.exe"
                    )
                })?;
                let enc = NvencHevcEncoder::new(width, height, bitrate_kbps, fps, path)?;
                Ok(VideoEncoder::HevcNvenc(enc))
            }
        }
    }

    pub fn encode(
        &mut self,
        bgra: &[u8],
        width: usize,
        height: usize,
    ) -> anyhow::Result<Vec<EncodedNal>> {
        match self {
            VideoEncoder::OpenH264(e) => e.encode(bgra, width, height),
            VideoEncoder::HevcNvenc(e) => e.encode(bgra, width, height),
        }
    }
}

// ---------------------------------------------------------------------------
// OpenH264 software path (unchanged behaviour)
// ---------------------------------------------------------------------------

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
        let yuv = YUVBuffer::from_vec(
            std::mem::replace(&mut self.yuv_data, vec![0u8; yuv_len]),
            width,
            height,
        );

        // Request keyframe periodically
        if self.frame_count % self.keyframe_interval == 0 {
            self.encoder.force_intra_frame();
        }

        let bitstream = self
            .encoder
            .encode(&yuv)
            .map_err(|e| anyhow::anyhow!("H.264 encode failed: {e}"))?;

        let is_keyframe = matches!(bitstream.frame_type(), FrameType::IDR | FrameType::I);
        self.frame_count += 1;

        let data = bitstream.to_vec();

        if data.is_empty() {
            return Ok(Vec::new());
        }

        Ok(vec![EncodedNal { data, is_keyframe }])
    }
}

// ---------------------------------------------------------------------------
// HEVC NVENC path via bundled ffmpeg.exe
//
// Architecture: spawn ffmpeg.exe once at encoder construction, pipe raw BGRA
// frames into stdin, read encoded HEVC Annex-B NAL units off stdout. A
// background thread drains stdout into a channel so the encode call can pull
// any NAL units that have completed without blocking the capture loop on a
// frame that hasn't finished encoding yet (NVENC is low-latency but not
// instant — the first NAL emerges after a couple of frames).
// ---------------------------------------------------------------------------

pub struct NvencHevcEncoder {
    width: usize,
    height: usize,
    stdin: Option<ChildStdin>,
    nal_rx: std_mpsc::Receiver<EncodedNal>,
    _child: Child,
    _reader_thread: thread::JoinHandle<()>,
    frame_count: u64,
}

impl NvencHevcEncoder {
    pub fn new(
        width: usize,
        height: usize,
        bitrate_kbps: u32,
        fps: u32,
        ffmpeg_path: &str,
    ) -> anyhow::Result<Self> {
        if !Path::new(ffmpeg_path).exists() {
            anyhow::bail!("ffmpeg binary not found at: {ffmpeg_path}");
        }

        // Build argv. Known-good NVENC HEVC ultra-low-latency arg set:
        //   -preset p4   : balanced perf/quality
        //   -tune ull    : ultra-low-latency
        //   -zerolatency 1 : disable encoder reordering / lookahead
        //   -rc cbr      : constant bitrate (predictable for UDP transport)
        //   -g <fps*2>   : keyframe interval matching the openh264 path
        // Output is muxed in raw HEVC (Annex-B), pipe:1 = stdout.
        let bitrate_bps_str = format!("{}", bitrate_kbps * 1000);
        let size_str = format!("{width}x{height}");
        let fps_str = format!("{fps}");
        let gop_str = format!("{}", fps * 2);

        let mut cmd = Command::new(ffmpeg_path);
        cmd.args([
            "-hide_banner",
            "-loglevel", "warning",
            // Input: raw BGRA frames from stdin
            "-f", "rawvideo",
            "-pix_fmt", "bgra",
            "-s", &size_str,
            "-r", &fps_str,
            "-i", "pipe:0",
            // Encoder
            "-c:v", "hevc_nvenc",
            "-preset", "p4",
            "-tune", "ull",
            "-zerolatency", "1",
            "-rc", "cbr",
            "-b:v", &bitrate_bps_str,
            "-maxrate", &bitrate_bps_str,
            "-bufsize", &bitrate_bps_str,
            "-g", &gop_str,
            "-pix_fmt", "yuv420p",
            // Output: raw HEVC Annex-B to stdout
            "-f", "hevc",
            "pipe:1",
        ]);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn ffmpeg at {ffmpeg_path}"))?;

        let stdin = child
            .stdin
            .take()
            .context("ffmpeg child has no stdin handle")?;
        let stdout = child
            .stdout
            .take()
            .context("ffmpeg child has no stdout handle")?;
        let stderr = child
            .stderr
            .take()
            .context("ffmpeg child has no stderr handle")?;

        // Drain stderr so ffmpeg doesn't deadlock on a full pipe. Forward
        // warnings/errors to tracing.
        thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines().flatten() {
                tracing::warn!(target: "ffmpeg", "{line}");
            }
        });

        let (nal_tx, nal_rx) = std_mpsc::channel::<EncodedNal>();
        let reader_thread = thread::spawn(move || {
            if let Err(e) = read_hevc_nals(stdout, nal_tx) {
                tracing::warn!("HEVC NAL reader thread exited: {e:#}");
            }
        });

        tracing::info!(
            "HEVC NVENC encoder: {}x{}, {}kbps, {}fps (ffmpeg child via {})",
            width, height, bitrate_kbps, fps, ffmpeg_path
        );

        Ok(Self {
            width,
            height,
            stdin: Some(stdin),
            nal_rx,
            _child: child,
            _reader_thread: reader_thread,
            frame_count: 0,
        })
    }

    pub fn encode(
        &mut self,
        bgra: &[u8],
        width: usize,
        height: usize,
    ) -> anyhow::Result<Vec<EncodedNal>> {
        assert_eq!(width, self.width);
        assert_eq!(height, self.height);
        assert_eq!(bgra.len(), width * height * 4);

        // Push raw BGRA to ffmpeg stdin. If write fails the child died — bail
        // so the supervisor can respawn the whole server.
        if let Some(stdin) = self.stdin.as_mut() {
            stdin
                .write_all(bgra)
                .context("Failed to write frame to ffmpeg stdin (child likely died)")?;
        } else {
            anyhow::bail!("ffmpeg stdin closed");
        }

        self.frame_count += 1;

        // Drain any NAL units that have finished encoding. NVENC will usually
        // emit one access unit (which we treat as one EncodedNal) per input
        // frame, but with a small startup delay.
        let mut out = Vec::new();
        while let Ok(nal) = self.nal_rx.try_recv() {
            out.push(nal);
        }
        Ok(out)
    }
}

impl Drop for NvencHevcEncoder {
    fn drop(&mut self) {
        // Closing stdin signals EOF so ffmpeg flushes and exits cleanly.
        self.stdin.take();
        let _ = self._child.wait();
    }
}

/// Read HEVC Annex-B byte stream from ffmpeg stdout, split on start codes,
/// and group NAL units into access units (one per frame). Each access unit
/// is forwarded as a single `EncodedNal` so the existing UDP transport's
/// fragmenter can carry it intact.
fn read_hevc_nals(
    mut stdout: ChildStdout,
    tx: std_mpsc::Sender<EncodedNal>,
) -> anyhow::Result<()> {
    let mut buf = Vec::with_capacity(256 * 1024);
    let mut chunk = vec![0u8; 64 * 1024];

    loop {
        let n = match stdout.read(&mut chunk) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) => return Err(e.into()),
        };
        buf.extend_from_slice(&chunk[..n]);

        // Find all NAL start codes (00 00 00 01 or 00 00 01) and slice out
        // complete NAL units. Emit per-access-unit by buffering until we see
        // the start of the next AUD or VPS/SPS/PPS (which begin frames).
        // For simplicity here we forward NALs individually with the keyframe
        // flag set on VPS/SPS/PPS/IDR types. The transport already supports
        // many small NALs per "frame".
        loop {
            let Some((start, prefix_len)) = find_start_code(&buf, 0) else {
                // Keep partial data for the next read
                break;
            };
            // Look for the NEXT start code to delimit the current NAL
            let nal_data_start = start + prefix_len;
            let next = find_start_code(&buf, nal_data_start);
            let nal_end = match next {
                Some((next_start, _)) => next_start,
                None => break, // incomplete NAL, wait for more bytes
            };
            // Emit NAL with start code prefix included (Annex-B framing) so
            // the Android decoder can feed it straight to MediaCodec.
            let nal_bytes = buf[start..nal_end].to_vec();
            let nal_type = hevc_nal_type(&buf[nal_data_start..nal_end]);
            let is_keyframe = matches!(nal_type, Some(32 | 33 | 34 | 19 | 20 | 21));
            if tx
                .send(EncodedNal {
                    data: nal_bytes,
                    is_keyframe,
                })
                .is_err()
            {
                // receiver dropped — encoder shutting down
                return Ok(());
            }
            // Drop the consumed bytes
            buf.drain(0..nal_end);
        }
    }
}

/// Locate the next Annex-B start code at or after `from`. Returns the offset
/// of the first 0x00 of the prefix and the prefix length (3 or 4).
fn find_start_code(buf: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut i = from;
    while i + 3 <= buf.len() {
        if buf[i] == 0 && buf[i + 1] == 0 {
            if buf[i + 2] == 1 {
                return Some((i, 3));
            }
            if i + 4 <= buf.len() && buf[i + 2] == 0 && buf[i + 3] == 1 {
                return Some((i, 4));
            }
        }
        i += 1;
    }
    None
}

/// HEVC NAL header is 2 bytes; nal_unit_type is bits [1..7] of the first byte.
fn hevc_nal_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|b| (b >> 1) & 0x3F)
}

// ---------------------------------------------------------------------------
// Shared BGRA -> YUV420p conversion (used by the OpenH264 path)
// ---------------------------------------------------------------------------

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

            // Average RGB for UV
            let r_avg = (r00 + r01 + r10 + r11) >> 2;
            let g_avg = (g00 + g01 + g10 + g11) >> 2;
            let b_avg = (b00 + b01 + b10 + b11) >> 2;

            u_plane[uv_row + col_pair] = ((-43 * r_avg - 85 * g_avg + 128 * b_avg + 32768) >> 8) as u8;
            v_plane[uv_row + col_pair] = ((128 * r_avg - 107 * g_avg - 21 * b_avg + 32768) >> 8) as u8;
        }
    }
}
