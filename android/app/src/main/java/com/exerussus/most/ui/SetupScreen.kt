package com.exerussus.most.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.exerussus.most.NodeHost

/** Первый запуск: имя телефона в сети мостов и (по желанию сразу) приглашение на свой сервер. */
@Composable
fun SetupScreen(onDone: (name: String, invite: String) -> Unit) {
    var name by remember { mutableStateOf("") }
    var invite by remember { mutableStateOf("") }
    val nameOk = NodeHost.validName(name)
    val inviteOk = invite.isBlank() || invite.trim().startsWith("MRI-") || invite.trim().startsWith("MRM-") || invite.contains(' ')
    Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(24.dp)) {
        Text("Мост", color = MostColors.accent, fontSize = 32.sp)
        Spacer(Modifier.height(4.dp))
        Text("версия ${NodeHost.version}", color = MostColors.quiet)
        Spacer(Modifier.height(24.dp))
        Text("Телефон станет таким же участником, как ПК: свой ключ, свои соседи, связь через ваш сервер.", color = MostColors.text)
        Spacer(Modifier.height(24.dp))
        OutlinedTextField(
            value = name,
            onValueChange = { name = it.trim() },
            label = { Text("Имя телефона") },
            supportingText = { Text("латиницей, например taranis-phone", color = if (name.isEmpty() || nameOk) MostColors.quiet else MostColors.bad) },
            singleLine = true,
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false),
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(12.dp))
        OutlinedTextField(
            value = invite,
            onValueChange = { invite = it },
            label = { Text("Приглашение на сервер") },
            supportingText = { Text("строка из приложения «Сервер» на ПК; можно позже", color = if (inviteOk) MostColors.quiet else MostColors.bad) },
            minLines = 2,
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(24.dp))
        Button(onClick = { onDone(name, invite.trim()) }, enabled = nameOk && inviteOk, modifier = Modifier.fillMaxWidth()) {
            Text("Начать")
        }
    }
}
