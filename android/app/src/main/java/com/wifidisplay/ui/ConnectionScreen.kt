package com.wifidisplay.ui

import android.content.Context
import android.content.SharedPreferences
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

@Composable
fun ConnectionScreen(onConnect: (String) -> Unit) {
    val context = LocalContext.current
    val prefs = remember { context.getSharedPreferences("wifi_display", Context.MODE_PRIVATE) }
    var ip by remember { mutableStateOf(prefs.getString("last_ip", "") ?: "") }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(Color.Black),
        contentAlignment = Alignment.Center
    ) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(24.dp)
        ) {
            Text(
                text = "WiFi Display",
                color = Color.White,
                fontSize = 32.sp
            )

            Text(
                text = "Enter server IP address",
                color = Color.Gray,
                fontSize = 16.sp
            )

            OutlinedTextField(
                value = ip,
                onValueChange = { ip = it },
                label = { Text("Server IP") },
                placeholder = { Text("192.168.1.100") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(
                    keyboardType = KeyboardType.Number,
                    imeAction = ImeAction.Go
                ),
                keyboardActions = KeyboardActions(
                    onGo = {
                        if (ip.isNotBlank()) {
                            prefs.edit().putString("last_ip", ip).apply()
                            onConnect(ip.trim())
                        }
                    }
                ),
                colors = OutlinedTextFieldDefaults.colors(
                    focusedTextColor = Color.White,
                    unfocusedTextColor = Color.White,
                    focusedBorderColor = Color(0xFF6200EE),
                    unfocusedBorderColor = Color.Gray,
                    focusedLabelColor = Color(0xFF6200EE),
                    unfocusedLabelColor = Color.Gray,
                    cursorColor = Color.White
                ),
                modifier = Modifier.width(280.dp)
            )

            Button(
                onClick = {
                    if (ip.isNotBlank()) {
                        prefs.edit().putString("last_ip", ip).apply()
                        onConnect(ip.trim())
                    }
                },
                colors = ButtonDefaults.buttonColors(
                    containerColor = Color(0xFF6200EE)
                ),
                modifier = Modifier.width(280.dp)
            ) {
                Text("Connect", fontSize = 18.sp)
            }
        }
    }
}
