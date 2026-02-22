package com.wifidisplay.network

import java.net.DatagramPacket
import java.net.DatagramSocket
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Receives UDP packets from the streaming server and reassembles
 * fragmented NAL units.
 *
 * Packet header (12 bytes):
 * [0..4]  seq: u32 LE - global sequence number
 * [4]     flags: u8 - bit 0: keyframe, bit 1: last fragment
 * [5..7]  fragment_index: u16 LE - fragment number within NAL (0-based)
 * [7..11] nal_size: u32 LE - total NAL size
 * [11]    reserved: u8
 */
class UdpReceiver(private val port: Int) {

    private var socket: DatagramSocket? = null
    @Volatile
    private var running = false

    companion object {
        private const val HEADER_SIZE = 12
        private const val MAX_PACKET_SIZE = 1500
    }

    /**
     * Start receiving packets. Calls [onNal] for each complete NAL unit.
     * Blocks the calling thread.
     */
    fun receive(onNal: (ByteArray) -> Unit) {
        socket = DatagramSocket(port).apply {
            receiveBufferSize = 512 * 1024  // 512KB receive buffer
            soTimeout = 5000  // 5s timeout for disconnect detection
        }
        running = true

        val buffer = ByteArray(MAX_PACKET_SIZE)
        val packet = DatagramPacket(buffer, buffer.size)

        // Reassembly state
        var currentNal: ByteArray? = null
        var currentNalOffset = 0
        var expectedFragmentIndex = 0

        while (running) {
            try {
                socket?.receive(packet)

                if (packet.length < HEADER_SIZE) continue

                val data = packet.data

                // Parse header
                val headerBuf = ByteBuffer.wrap(data, 0, HEADER_SIZE).order(ByteOrder.LITTLE_ENDIAN)
                val seq = headerBuf.int                    // [0..4] seq
                val flags = data[4].toInt()                // [4] flags
                val fragmentIndex = (ByteBuffer.wrap(data, 5, 2)
                    .order(ByteOrder.LITTLE_ENDIAN).short.toInt() and 0xFFFF)  // [5..7] fragment_index
                val nalSize = (ByteBuffer.wrap(data, 7, 4)
                    .order(ByteOrder.LITTLE_ENDIAN).int.toLong() and 0xFFFFFFFFL)  // [7..11] nal_size
                // [11] reserved — skip

                val isKeyframe = (flags and 0x01) != 0
                val isLastFragment = (flags and 0x02) != 0

                val payloadOffset = HEADER_SIZE
                val payloadLength = packet.length - HEADER_SIZE

                // Sanity check: reject absurdly large NALs (>16MB)
                if (nalSize > 16 * 1024 * 1024) {
                    currentNal = null
                    continue
                }

                if (fragmentIndex == 0) {
                    // Start of a new NAL unit
                    currentNal = ByteArray(nalSize.toInt())
                    currentNalOffset = 0
                    expectedFragmentIndex = 0
                }

                // Validate fragment ordering
                if (fragmentIndex != expectedFragmentIndex || currentNal == null) {
                    // Out of order or missing fragment — reset
                    currentNal = null
                    currentNalOffset = 0
                    expectedFragmentIndex = 0
                    continue
                }

                // Copy payload into reassembly buffer
                val copyLen = minOf(payloadLength, currentNal.size - currentNalOffset)
                System.arraycopy(data, payloadOffset, currentNal, currentNalOffset, copyLen)
                currentNalOffset += copyLen
                expectedFragmentIndex++

                if (isLastFragment && currentNalOffset > 0) {
                    // Complete NAL unit — deliver it
                    onNal(currentNal.copyOf(currentNalOffset))
                    currentNal = null
                    currentNalOffset = 0
                    expectedFragmentIndex = 0
                }

            } catch (e: java.net.SocketTimeoutException) {
                // No data for 5s — still waiting
                continue
            } catch (e: Exception) {
                if (running) {
                    throw e
                }
            }
        }
    }

    fun stop() {
        running = false
        socket?.close()
        socket = null
    }
}
