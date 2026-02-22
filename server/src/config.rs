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
}
