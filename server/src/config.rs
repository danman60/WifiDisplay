use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "wifi-display-server")]
#[command(about = "Stream a virtual monitor over WiFi to an Android tablet")]
pub struct Config {
    /// Name or index of the monitor to capture
    #[arg(long, default_value = "WiFi Display")]
    pub monitor: String,

    /// UDP port for video stream
    #[arg(long, default_value_t = 5000)]
    pub video_port: u16,

    /// UDP port for touch input
    #[arg(long, default_value_t = 5001)]
    pub touch_port: u16,

    /// Video bitrate in kbps
    #[arg(long, default_value_t = 5000)]
    pub bitrate: u32,

    /// Target client IP address
    #[arg(long)]
    pub client: Option<String>,
}
