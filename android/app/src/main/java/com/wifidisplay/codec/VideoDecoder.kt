package com.wifidisplay.codec

import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Build
import android.view.Surface

/**
 * Hardware H.264 decoder using Android MediaCodec.
 * Renders decoded frames directly to a Surface.
 */
class VideoDecoder(private val surface: Surface) {

    private var codec: MediaCodec? = null
    @Volatile
    private var started = false
    private val lock = Any()

    companion object {
        private const val MIME_TYPE = "video/avc"
        // Initial dimensions — will be updated by SPS/PPS from the stream
        private const val DEFAULT_WIDTH = 1920
        private const val DEFAULT_HEIGHT = 1080
        private const val TIMEOUT_US = 10_000L  // 10ms timeout for dequeue
    }

    /**
     * Initialize and start the decoder.
     */
    fun start() {
        synchronized(lock) {
            val format = MediaFormat.createVideoFormat(MIME_TYPE, DEFAULT_WIDTH, DEFAULT_HEIGHT).apply {
                // Low latency mode (Android 11+)
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
                }
                // Realtime priority
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                    setInteger(MediaFormat.KEY_PRIORITY, 0)
                    setInteger(MediaFormat.KEY_OPERATING_RATE, Short.MAX_VALUE.toInt())
                }
            }

            codec = MediaCodec.createDecoderByType(MIME_TYPE).apply {
                configure(format, surface, null, 0)
                start()
            }
            started = true
        }
    }

    /**
     * Submit a complete NAL unit (or group of NALs) for decoding.
     * This will also drain any available output buffers to the surface.
     */
    fun submitNal(nalData: ByteArray) {
        synchronized(lock) {
            val codec = this.codec ?: return
            if (!started) return

            // Submit input
            val inputIndex = codec.dequeueInputBuffer(TIMEOUT_US)
            if (inputIndex >= 0) {
                val inputBuffer = codec.getInputBuffer(inputIndex) ?: return
                inputBuffer.clear()
                inputBuffer.put(nalData, 0, minOf(nalData.size, inputBuffer.capacity()))
                codec.queueInputBuffer(
                    inputIndex,
                    0,
                    minOf(nalData.size, inputBuffer.capacity()),
                    System.nanoTime() / 1000,  // timestamp in microseconds
                    0
                )
            }

            // Drain output — render to surface
            drainOutput(codec)
        }
    }

    /**
     * Drain decoded frames and render them to the surface.
     */
    private fun drainOutput(codec: MediaCodec) {
        val bufferInfo = MediaCodec.BufferInfo()

        while (true) {
            val outputIndex = codec.dequeueOutputBuffer(bufferInfo, 0)  // non-blocking
            when {
                outputIndex >= 0 -> {
                    // Render to surface immediately (true = render)
                    codec.releaseOutputBuffer(outputIndex, true)
                }
                outputIndex == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                    // Format changed — codec auto-adjusts, nothing to do
                }
                else -> break  // No more output available
            }
        }
    }

    /**
     * Stop and release the decoder.
     */
    fun stop() {
        synchronized(lock) {
            started = false
            try {
                codec?.stop()
                codec?.release()
            } catch (e: Exception) {
                // Ignore errors during cleanup
            }
            codec = null
        }
    }
}
