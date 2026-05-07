use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "wifi-display-server")]
#[command(about = "Stream a virtual monitor over WiFi to an Android tablet")]
pub struct Config {
    /// Monitor index to capture (0 = primary, 1 = second, etc.)
    #[arg(long, default_value_t = 0)]
    pub monitor_index: usize,

    /// UDP port for video stream
    #[arg(long, default_value_t = 5000)]
    pub video_port: u16,

    /// UDP port for touch input
    #[arg(long, default_value_t = 5001)]
    pub touch_port: u16,

    /// Video bitrate in kbps
    #[arg(long, default_value_t = 3000)]
    pub bitrate: u32,

    /// Target frames per second
    #[arg(long, default_value_t = 30)]
    pub fps: u32,

    /// Target client IP address (e.g. 192.168.1.50). If omitted, sends to broadcast.
    #[arg(long)]
    pub client: Option<String>,

    /// Encoder backend. `openh264` (default) keeps the existing software path.
    /// `hevc-nvenc` opts into GPU-offloaded HEVC via a bundled ffmpeg.exe
    /// child process (requires --ffmpeg-path).
    #[arg(long, default_value = "openh264")]
    pub encoder: String,

    /// Absolute path to ffmpeg.exe. Only consulted when --encoder is
    /// hevc-nvenc. The CSE host should pass the path under
    /// resources/ffmpeg/ffmpeg.exe.
    #[arg(long)]
    pub ffmpeg_path: Option<String>,
}
