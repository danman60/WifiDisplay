use crate::encoder::EncodedNal;
use anyhow::Context;
use tokio::net::UdpSocket;
use std::net::SocketAddr;

/// Max UDP payload to stay under typical MTU (1500 - IP header 20 - UDP header 8)
const MAX_UDP_PAYLOAD: usize = 1400;

/// Packet header: 12 bytes
/// [0..4]  seq: u32 LE - global sequence number
/// [4]     flags: u8 - bit 0: keyframe, bit 1: last fragment of NAL
/// [5..7]  fragment_index: u16 LE - fragment number within this NAL (0-based)
/// [7..11] nal_size: u32 LE - total NAL size (for reassembly)
/// [11]    reserved: u8
const HEADER_SIZE: usize = 12;
const MAX_FRAGMENT_PAYLOAD: usize = MAX_UDP_PAYLOAD - HEADER_SIZE;

pub struct UdpTransport {
    socket: UdpSocket,
    target: SocketAddr,
    seq: std::sync::atomic::AtomicU32,
}

impl UdpTransport {
    pub async fn new(port: u16, client_ip: Option<&str>) -> anyhow::Result<Self> {
        let bind_addr: SocketAddr = format!("0.0.0.0:{port}").parse()?;
        let socket = UdpSocket::bind(bind_addr)
            .await
            .context(format!("Failed to bind UDP socket on port {port}"))?;

        // Enable broadcast if no specific client
        socket.set_broadcast(true)?;

        let target: SocketAddr = match client_ip {
            Some(ip) => format!("{ip}:{port}").parse()?,
            None => format!("255.255.255.255:{port}").parse()?,
        };

        tracing::info!("UDP transport: sending to {target}");

        Ok(Self {
            socket,
            target,
            seq: std::sync::atomic::AtomicU32::new(0),
        })
    }

    /// Send a list of NAL units, fragmenting as needed.
    /// Returns total bytes sent.
    pub async fn send_nals(&self, nals: &[EncodedNal]) -> anyhow::Result<usize> {
        let mut total_sent = 0;

        for nal in nals {
            total_sent += self.send_nal(nal).await?;
        }

        Ok(total_sent)
    }

    /// Send a single NAL unit, fragmenting if larger than MAX_FRAGMENT_PAYLOAD.
    async fn send_nal(&self, nal: &EncodedNal) -> anyhow::Result<usize> {
        let data = &nal.data;
        let nal_size = data.len();
        let mut total_sent = 0;

        if nal_size == 0 {
            return Ok(0);
        }

        let num_fragments = (nal_size + MAX_FRAGMENT_PAYLOAD - 1) / MAX_FRAGMENT_PAYLOAD;

        if num_fragments > u16::MAX as usize {
            tracing::warn!("NAL too large to fragment ({nal_size} bytes, {num_fragments} fragments), skipping");
            return Ok(0);
        }

        for i in 0..num_fragments {
            let offset = i * MAX_FRAGMENT_PAYLOAD;
            let end = (offset + MAX_FRAGMENT_PAYLOAD).min(nal_size);
            let fragment = &data[offset..end];
            let is_last = i == num_fragments - 1;

            let seq = self
                .seq
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            let mut packet = Vec::with_capacity(HEADER_SIZE + fragment.len());

            // Header (12 bytes)
            packet.extend_from_slice(&seq.to_le_bytes());           // [0..4] seq
            let flags = (nal.is_keyframe as u8) | ((is_last as u8) << 1);
            packet.push(flags);                                      // [4] flags
            packet.extend_from_slice(&(i as u16).to_le_bytes());    // [5..7] fragment_index
            packet.extend_from_slice(&(nal_size as u32).to_le_bytes()); // [7..11] nal_size
            packet.push(0);                                          // [11] reserved

            // Payload
            packet.extend_from_slice(fragment);

            match self.socket.send_to(&packet, self.target).await {
                Ok(n) => total_sent += n,
                Err(e) => {
                    // UDP send errors are usually transient
                    tracing::trace!("UDP send error: {e}");
                }
            }
        }

        Ok(total_sent)
    }
}
