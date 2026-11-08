package com.exerussus.most.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.exerussus.most.NodeHost
import com.exerussus.most.NodeService
import com.exerussus.most.NodeState

@Composable
fun MostRoot() {
    MostTheme {
        Box(Modifier.fillMaxSize().background(MostColors.bg).safeDrawingPadding()) {
            val ctx = LocalContext.current
            val state by NodeHost.state.collectAsStateWithLifecycle()
            var setupDone by remember { mutableStateOf(NodeHost.configured(ctx)) }
            // Приглашение на сервер, введённое при первом запуске: отправим, как только узел поднимется.
            var pendingInvite by remember { mutableStateOf("") }
            when {
                !setupDone -> SetupScreen { name, invite ->
                    pendingInvite = invite
                    setupDone = true
                    NodeService.start(ctx, name)
                }
                state is NodeState.Running -> HomeScreen(pendingInvite) { pendingInvite = "" }
                state is NodeState.Failed -> Column(Modifier.padding(24.dp)) {
                    Text("Мост не запустился", color = MostColors.bad)
                    Spacer(Modifier.height(8.dp))
                    Text((state as NodeState.Failed).message, color = MostColors.text)
                    Spacer(Modifier.height(16.dp))
                    Button(onClick = { NodeService.start(ctx) }) { Text("Повторить") }
                }
                else -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(color = MostColors.accent)
                }
            }
        }
    }
}
