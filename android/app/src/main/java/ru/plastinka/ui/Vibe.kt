package ru.plastinka.ui

import androidx.compose.ui.graphics.Color
import ru.plastinka.data.VibeColors

/**
 * VibeColorizer: цвет «Моей волны» под трек. Портировано из десктопного
 * клиента (src/plugins/vibe.rs): порядок источников `derivedColors.accent`
 * → `derivedColors.average` → оттенок `hue`; любой цвет приводится к
 * читаемому на чёрном ([lift]).
 */
object Vibe {

    private const val HUE_SATURATION = 0.55f
    private const val HUE_LIGHTNESS = 0.5f
    private const val MIN_LIGHTNESS = 0.42f
    private const val MAX_LIGHTNESS = 0.68f
    private const val MIN_SATURATION = 0.35f
    private const val GRAY_BELOW = 0.08f

    /** Цвет трека волны; `fallback` — акцент темы, если данных нет. */
    fun pick(vibe: VibeColors?, fallback: Color): Color {
        val color = vibe?.accent?.let(::parseHex)
            ?: vibe?.average?.let(::parseHex)
            ?: vibe?.hue?.let(::fromHue)
            ?: return fallback
        return lift(color)
    }

    /** `#RRGGBB` или `#RRGGBBAA`. */
    fun parseHex(text: String): Color? {
        val digits = text.removePrefix("#")
        if (digits.length != 6 && digits.length != 8) return null
        if (digits.any { Character.digit(it, 16) < 0 }) return null
        val channel = { at: Int -> digits.substring(at, at + 2).toInt(16) }
        val alpha = if (digits.length == 8) channel(4) else 255
        return Color(channel(0), channel(2), channel(4), alpha)
    }

    /** Оттенок в градусах (0–360, по кругу). */
    fun fromHue(hue: Double): Color? {
        if (!hue.isFinite()) return null
        val degrees = (((hue % 360.0) + 360.0) % 360.0).toFloat()
        return fromHsl(degrees, HUE_SATURATION, HUE_LIGHTNESS)
    }

    /** Светлота в пределах, у цветных — минимальная насыщенность; серый остаётся серым. */
    fun lift(color: Color): Color {
        val (hue, saturation, lightness) = toHsl(color)
        val nextSaturation =
            if (saturation < GRAY_BELOW) saturation else maxOf(saturation, MIN_SATURATION)
        val nextLightness = lightness.coerceIn(MIN_LIGHTNESS, MAX_LIGHTNESS)
        return fromHsl(hue, nextSaturation, nextLightness).copy(alpha = color.alpha)
    }

    /** (оттенок в градусах, насыщенность, светлота). */
    private fun toHsl(color: Color): Triple<Float, Float, Float> {
        val (r, g, b) = Triple(color.red, color.green, color.blue)
        val max = maxOf(r, g, b)
        val min = minOf(r, g, b)
        val lightness = (max + min) / 2f
        val delta = max - min
        if (delta < 1e-6f) return Triple(0f, 0f, lightness)
        val saturation = delta / (1f - kotlin.math.abs(2f * lightness - 1f)).coerceAtLeast(1e-6f)
        val hue = when {
            kotlin.math.abs(max - r) < 1e-6f -> ((g - b) / delta).mod(6f)
            kotlin.math.abs(max - g) < 1e-6f -> (b - r) / delta + 2f
            else -> (r - g) / delta + 4f
        }
        return Triple(hue * 60f, saturation.coerceIn(0f, 1f), lightness)
    }

    private fun fromHsl(hue: Float, saturation: Float, lightness: Float): Color {
        val chroma = (1f - kotlin.math.abs(2f * lightness - 1f)) * saturation
        val sector = hue.mod(360f) / 60f
        val x = chroma * (1f - kotlin.math.abs(sector.mod(2f) - 1f))
        val (r, g, b) = when {
            sector < 1f -> Triple(chroma, x, 0f)
            sector < 2f -> Triple(x, chroma, 0f)
            sector < 3f -> Triple(0f, chroma, x)
            sector < 4f -> Triple(0f, x, chroma)
            sector < 5f -> Triple(x, 0f, chroma)
            else -> Triple(chroma, 0f, x)
        }
        val m = lightness - chroma / 2f
        return Color(r + m, g + m, b + m)
    }
}
