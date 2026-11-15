package ru.plastinka.ui

import androidx.compose.ui.graphics.Color

/** Цвет в HSV: тон 0–360°, насыщенность и яркость 0–1. */
data class Hsv(val h: Float, val s: Float, val v: Float) {

    /** Цвет как `0xRRGGBB`. */
    fun toRgb(): Int {
        val hue = h.coerceIn(0f, 360f).let { if (it >= 360f) 0f else it }
        val sat = s.coerceIn(0f, 1f)
        val value = v.coerceIn(0f, 1f)
        val chroma = value * sat
        val sector = hue / 60f
        val x = chroma * (1f - kotlin.math.abs(sector % 2f - 1f))
        val (r, g, b) = when {
            sector < 1f -> Triple(chroma, x, 0f)
            sector < 2f -> Triple(x, chroma, 0f)
            sector < 3f -> Triple(0f, chroma, x)
            sector < 4f -> Triple(0f, x, chroma)
            sector < 5f -> Triple(x, 0f, chroma)
            else -> Triple(chroma, 0f, x)
        }
        val m = value - chroma
        return (byteOf(r + m) shl 16) or (byteOf(g + m) shl 8) or byteOf(b + m)
    }

    /** Чистый тон: насыщенность и яркость полные (для фона квадрата и полосы). */
    fun pure(): Color = Color(0xFF000000.toInt() or Hsv(h, 1f, 1f).toRgb())

    companion object {
        /** У серого и чёрного тона нет: берётся [hue], чтобы он не терялся при перетаскивании. */
        fun fromRgb(rgb: Int, hue: Float): Hsv {
            val r = (rgb shr 16 and 0xFF) / 255f
            val g = (rgb shr 8 and 0xFF) / 255f
            val b = (rgb and 0xFF) / 255f
            val max = maxOf(r, g, b)
            val min = minOf(r, g, b)
            val delta = max - min
            val h = when {
                delta <= 0f -> hue.coerceIn(0f, 360f)
                max == r -> 60f * ((g - b) / delta).mod(6f)
                max == g -> 60f * ((b - r) / delta + 2f)
                else -> 60f * ((r - g) / delta + 4f)
            }
            val s = if (max <= 0f) 0f else delta / max
            return Hsv(h, s, max)
        }

        private fun byteOf(channel: Float): Int =
            Math.round(channel.coerceIn(0f, 1f) * 255f)
    }
}

/** Работа со своим акцентом: запись `#RRGGBB` и защита от слишком тёмного цвета. */
object ColorMath {

    /**
     * Нижняя граница яркости (по формуле luma, как в `onColorFor`) для своего
     * акцента. Готовые акценты — от 0,43. На десктопе своего цвета никто не
     * ограничивает, но на телефоне акцент ещё и цвет текста выбранного трека,
     * поэтому тёмный цвет осветляем к белому до этой границы.
     */
    private const val MIN_LUMA = 0.35f

    /** Разбор `#RRGGBB` (регистр и пробелы по краям не важны, `#` обязателен). */
    fun parseHex(text: String): Int? {
        val trimmed = text.trim()
        if (!trimmed.startsWith("#")) return null
        val digits = trimmed.substring(1)
        if (digits.length != 6 || digits.any { Character.digit(it, 16) < 0 }) return null
        return digits.toIntOrNull(16)
    }

    /** Запись цвета `#RRGGBB` прописными. */
    fun formatHex(rgb: Int): String = "#%06X".format(rgb and 0xFFFFFF)

    private fun luma(rgb: Int): Float =
        0.299f * (rgb shr 16 and 0xFF) / 255f +
            0.587f * (rgb shr 8 and 0xFF) / 255f +
            0.114f * (rgb and 0xFF) / 255f

    /** Цвет, видимый на чёрном: слишком тёмный осветляется (оттенок сохраняется), остальные без изменений. */
    fun readable(rgb: Int): Int {
        val clean = rgb and 0xFFFFFF
        val luma = luma(clean)
        if (luma >= MIN_LUMA) return clean
        val t = (MIN_LUMA - luma) / (1f - luma)
        fun mix(shift: Int): Int {
            val c = (clean shr shift and 0xFF)
            return (c + Math.round((255 - c) * t)).coerceIn(0, 255)
        }
        return (mix(16) shl 16) or (mix(8) shl 8) or mix(0)
    }
}
