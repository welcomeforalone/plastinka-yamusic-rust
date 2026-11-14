package ru.plastinka.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

private val Red = Color(0xFFFF3333)
private val Dark = darkColorScheme(
    primary = Red,
    onPrimary = Color.White,
    background = Color(0xFF0B0B0D),
    onBackground = Color(0xFFEDEDF0),
    surface = Color(0xFF141417),
    onSurface = Color(0xFFEDEDF0),
    surfaceVariant = Color(0xFF1E1E23),
    onSurfaceVariant = Color(0xFFB9B9C0),
)

@Composable
fun PlastinkaTheme(content: @Composable () -> Unit) {
    // Клиент тёмный по дизайну; системная светлая тема не используется.
    isSystemInDarkTheme()
    MaterialTheme(colorScheme = Dark, content = content)
}
