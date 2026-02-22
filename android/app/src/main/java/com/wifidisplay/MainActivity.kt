package com.wifidisplay

import android.os.Bundle
import android.view.View
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.runtime.*
import com.wifidisplay.ui.ConnectionScreen
import com.wifidisplay.ui.DisplayScreen

class MainActivity : ComponentActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Keep screen on while streaming
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)

        // Immersive fullscreen
        window.decorView.systemUiVisibility = (
            View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
            or View.SYSTEM_UI_FLAG_FULLSCREEN
            or View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
            or View.SYSTEM_UI_FLAG_LAYOUT_STABLE
            or View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
            or View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
        )

        setContent {
            var serverIp by remember { mutableStateOf("") }
            var isConnected by remember { mutableStateOf(false) }

            if (!isConnected) {
                ConnectionScreen(
                    onConnect = { ip ->
                        serverIp = ip
                        isConnected = true
                    }
                )
            } else {
                DisplayScreen(
                    serverIp = serverIp,
                    onDisconnect = { isConnected = false }
                )
            }
        }
    }
}
