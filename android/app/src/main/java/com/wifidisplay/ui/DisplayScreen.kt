package com.wifidisplay.ui

import android.annotation.SuppressLint
import android.view.MotionEvent
import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import com.wifidisplay.codec.VideoDecoder
import com.wifidisplay.network.TouchSender
import com.wifidisplay.network.UdpReceiver
import kotlinx.coroutines.*

@Composable
fun DisplayScreen(serverIp: String, onDisconnect: () -> Unit) {
    var status by remember { mutableStateOf("Connecting to $serverIp...") }
    var framesDecoded by remember { mutableLongStateOf(0L) }
    val scope = rememberCoroutineScope()

    var decoder by remember { mutableStateOf<VideoDecoder?>(null) }
    var receiver by remember { mutableStateOf<UdpReceiver?>(null) }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(Color.Black)
    ) {
        // SurfaceView for video rendering
        AndroidView(
            factory = { context ->
                @SuppressLint("ClickableViewAccessibility")
                val view = SurfaceView(context).apply {
                    var touchSender: TouchSender? = null
                    var lastMoveTime = 0L

                    holder.addCallback(object : SurfaceHolder.Callback {
                        override fun surfaceCreated(holder: SurfaceHolder) {
                            val dec = VideoDecoder(holder.surface)
                            decoder = dec

                            val recv = UdpReceiver(5000)
                            receiver = recv

                            val sender = TouchSender(serverIp)
                            touchSender = sender

                            scope.launch(Dispatchers.IO) {
                                try {
                                    sender.start()
                                    dec.start()
                                    withContext(Dispatchers.Main) {
                                        status = "Connected"
                                    }

                                    recv.receive { nalData ->
                                        dec.submitNal(nalData)
                                        framesDecoded++
                                    }
                                } catch (e: Exception) {
                                    withContext(Dispatchers.Main) {
                                        status = "Error: ${e.message}"
                                    }
                                } finally {
                                    // Ensure cleanup even on error/cancellation
                                    sender.stop()
                                    recv.stop()
                                    dec.stop()
                                }
                            }
                        }

                        override fun surfaceChanged(
                            holder: SurfaceHolder,
                            format: Int,
                            width: Int,
                            height: Int
                        ) {}

                        override fun surfaceDestroyed(holder: SurfaceHolder) {
                            touchSender?.stop()
                            touchSender = null
                            receiver?.stop()
                            decoder?.stop()
                        }
                    })

                    setOnTouchListener { v, event ->
                        val sender = touchSender ?: return@setOnTouchListener false
                        val normX = event.x / v.width
                        val normY = event.y / v.height

                        when (event.action) {
                            MotionEvent.ACTION_DOWN -> {
                                scope.launch(Dispatchers.IO) {
                                    sender.sendTouch(TouchSender.TOUCH_DOWN, normX, normY)
                                }
                                true
                            }
                            MotionEvent.ACTION_MOVE -> {
                                val now = System.currentTimeMillis()
                                if (now - lastMoveTime >= 16) {
                                    lastMoveTime = now
                                    scope.launch(Dispatchers.IO) {
                                        sender.sendTouch(TouchSender.TOUCH_MOVE, normX, normY)
                                    }
                                }
                                true
                            }
                            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                                scope.launch(Dispatchers.IO) {
                                    sender.sendTouch(TouchSender.TOUCH_UP, normX, normY)
                                }
                                true
                            }
                            else -> false
                        }
                    }
                }
                view
            },
            modifier = Modifier.fillMaxSize()
        )

        // Status overlay (top-left)
        Column(
            modifier = Modifier
                .align(Alignment.TopStart)
                .padding(16.dp)
        ) {
            Text(
                text = status,
                color = Color.Green,
                fontSize = 14.sp
            )
            Text(
                text = "Frames: $framesDecoded",
                color = Color.Green,
                fontSize = 12.sp
            )
        }
    }
}
