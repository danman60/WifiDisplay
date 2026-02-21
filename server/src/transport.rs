/// UDP transport for H.264 NAL units.
///
/// Phase 2: Chunk NAL units into <1400 byte packets
/// Header: [seq:4][flags:1][reserved:3] = 8 bytes

use std::net::SocketAddr;

pub const MAX_PACKET_SIZE: usize = 1400;
pub const HEADER_SIZE: usize = 8;
pub const MAX_PAYLOAD: usize = MAX_PACKET_SIZE - HEADER_SIZE;

pub struct UdpTransport {
    // TODO: tokio UDP socket
}

impl UdpTransport {
    pub async fn new(_port: u16, _client: Option<SocketAddr>) -> anyhow::Result<Self> {
        anyhow::bail!("UDP transport not yet implemented (Phase 2)")
    }
}
