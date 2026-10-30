package io.github.win81134679.meetscribe.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp

/**
 * 色票照桌面版 theme.rs（取自 Filee 的 Soft 主題）：暖灰頁面、白卡片、紫色主色，跟隨系統深淺色。
 * 畫面裡的顏色一律從 [Pal] 或 MaterialTheme 取，不要寫死。
 */
@Immutable
data class Palette(
    val page: Color,
    val surface: Color,
    val surfaceAlt: Color,
    val input: Color,
    val border: Color,
    val text: Color,
    val textMuted: Color,
    val danger: Color,
    val success: Color,
    val warning: Color,
    val accent: Color,
    val accentSoft: Color,
    val accentFaint: Color,
    val accentStrong: Color,
)

private val Light = Palette(
    page = Color(0xFFF6F5F1), surface = Color(0xFFFFFFFF), surfaceAlt = Color(0xFFEFEDE6), input = Color(0xFFFBFAF8),
    border = Color(0x16000000), text = Color(0xFF1F1E1D), textMuted = Color(0xFF6B6A66),
    danger = Color(0xFFC73A39), success = Color(0xFF0F7A5A), warning = Color(0xFF9A5B0C),
    accent = Color(0xFF7F77DD), accentSoft = Color(0x2E7F77DD), accentFaint = Color(0x177F77DD), accentStrong = Color(0xFF5F59A6),
)

private val Dark = Palette(
    page = Color(0xFF1B1B19), surface = Color(0xFF262624), surfaceAlt = Color(0xFF31312E), input = Color(0xFF1F1F1D),
    border = Color(0x1CFFFFFF), text = Color(0xFFF1EFE8), textMuted = Color(0xFFAEACA4),
    danger = Color(0xFFF09595), success = Color(0xFF5DCAA5), warning = Color(0xFFEF9F27),
    accent = Color(0xFF7F77DD), accentSoft = Color(0x2E7F77DD), accentFaint = Color(0x177F77DD), accentStrong = Color(0xFFBCAEE5),
)

/** 說話者顏色（依 Speaker.color 循環取用）。 */
val SpeakerColors = listOf(
    Color(0xFF7F77DD), Color(0xFF1D9E75), Color(0xFFD85A30), Color(0xFF378ADD),
    Color(0xFFD4537E), Color(0xFF639922), Color(0xFFBA7517), Color(0xFF888780),
)

fun speakerColor(i: Int): Color = SpeakerColors[((i % SpeakerColors.size) + SpeakerColors.size) % SpeakerColors.size]

val LocalPalette = staticCompositionLocalOf { Light }

object Pal {
    val current: Palette @Composable get() = LocalPalette.current
}

@Composable
fun MeetScribeTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val p = if (dark) Dark else Light
    val scheme = if (dark) {
        darkColorScheme(
            primary = p.accent, onPrimary = Color.White, primaryContainer = p.accentSoft, onPrimaryContainer = p.accentStrong,
            secondary = p.accentStrong, background = p.page, onBackground = p.text, surface = p.surface, onSurface = p.text,
            surfaceVariant = p.surfaceAlt, onSurfaceVariant = p.textMuted, surfaceContainer = p.surface,
            surfaceContainerLow = p.surface, surfaceContainerHigh = p.surfaceAlt, surfaceContainerHighest = p.surfaceAlt,
            outline = p.textMuted, outlineVariant = p.border, error = p.danger, secondaryContainer = p.accentSoft,
            onSecondaryContainer = p.text,
        )
    } else {
        lightColorScheme(
            primary = p.accent, onPrimary = Color.White, primaryContainer = p.accentSoft, onPrimaryContainer = p.accentStrong,
            secondary = p.accentStrong, background = p.page, onBackground = p.text, surface = p.surface, onSurface = p.text,
            surfaceVariant = p.surfaceAlt, onSurfaceVariant = p.textMuted, surfaceContainer = p.surface,
            surfaceContainerLow = p.surface, surfaceContainerHigh = p.surfaceAlt, surfaceContainerHighest = p.surfaceAlt,
            outline = p.textMuted, outlineVariant = Color(0x29000000), error = p.danger, secondaryContainer = p.accentSoft,
            onSecondaryContainer = p.text,
        )
    }
    CompositionLocalProvider(LocalPalette provides p) {
        MaterialTheme(
            colorScheme = scheme,
            shapes = Shapes(
                small = RoundedCornerShape(10.dp),
                medium = RoundedCornerShape(12.dp),
                large = RoundedCornerShape(16.dp),
            ),
            content = content,
        )
    }
}
