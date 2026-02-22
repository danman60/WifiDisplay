package com.wifidisplay.network

import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Sends touch events to the server as 9-byte UDP packets.
 *
 * Packet format:
 * [0]     type: u8   — 0=MOVE, 1=DOWN, 2=UP
 * [1..5]  x: f32 LE  — normalized 0.0-1.0
 * [5..9]  y: f32 LE  — normalized 0.0-1.0
 */
class TouchSender(private val serverIp: String, private val port: Int = 5001) {

    companion object {
        const val TOUCH_MOVE: Byte = 0
        const val TOUCH_DOWN: Byte = 1
        const val TOUCH_UP: Byte = 2
        private const val PACKET_SIZE = 9
    }

    private var socket: DatagramSocket? = null
    private var serverAddress: InetAddress? = null

    fun start() {
        socket = DatagramSocket()
        serverAddress = InetAddress.getByName(serverIp)
    }

    /**
     * Build and send a 9-byte touch packet.
     * Call from IO dispatcher — this does blocking network I/O.
     */
    fun sendTouch(type: Byte, normX: Float, normY: Float) {
        val addr = serverAddress ?: return
        val sock = socket ?: return

        val buf = ByteBuffer.allocate(PACKET_SIZE).order(ByteOrder.LITTLE_ENDIAN)
        buf.put(type)
        buf.putFloat(normX)
        buf.putFloat(normY)

        val data = buf.array()
        val packet = DatagramPacket(data, data.size, addr, port)
        sock.send(packet)
    }

    fun stop() {
        socket?.close()
        socket = null
        serverAddress = null
    }
}
