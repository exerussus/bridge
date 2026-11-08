package com.exerussus.most.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

// Цвета панели ПК (3.21): тёмный фон, бирюзовый акцент; красный — только выключенный звук/микрофон и ошибки.
object MostColors {
    val bg = Color(0xFF101418)
    val card = Color(0xFF182027)
    val accent = Color(0xFF38E1F0)
    val ok = Color(0xFF3DDC84)
    val warn = Color(0xFFFFB547)
    val bad = Color(0xFFFF4F64)
    val text = Color(0xFFE8EEF2)
    val quiet = Color(0xFF8B9AA6)
}

@Composable
fun MostTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = darkColorScheme(
            primary = MostColors.accent,
            onPrimary = MostColors.bg,
            background = MostColors.bg,
            surface = MostColors.card,
            onSurface = MostColors.text,
            onBackground = MostColors.text,
            error = MostColors.bad,
        ),
        content = content,
    )
}
