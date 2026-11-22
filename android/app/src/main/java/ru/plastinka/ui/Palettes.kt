package ru.plastinka.ui

import kotlin.math.pow
import kotlin.math.roundToInt

// Модель тем: тон, режим, семейства и 24 палитры. Значения те же, что на десктопе
// (src/ui/theme.rs), цветовая математика — чистые функции на `Int` RGB без Compose,
// чтобы гонять их в JVM-тестах.

/** Тон схемы: тёмный или светлый. */
enum class Tone { Dark, Light }

/** Режим тона в настройках: свой выбор или как в системе. Ключи как в `config.toml` десктопа. */
enum class ThemeMode(val key: String, val title: String) {
    Dark("dark", "Тёмная"),
    Light("light", "Светлая"),
    System("system", "Как в системе"),
    ;

    /** Итоговый тон; [systemDark] — сообщила ли система тёмную тему. */
    fun tone(systemDark: Boolean): Tone = when (this) {
        Dark -> Tone.Dark
        Light -> Tone.Light
        System -> if (systemDark) Tone.Dark else Tone.Light
    }

    companion object {
        /** Пробелы и регистр не важны; непонятное значение — тёмная. */
        fun fromKey(key: String?): ThemeMode {
            val clean = key?.trim().orEmpty()
            return entries.firstOrNull { it.key.equals(clean, ignoreCase = true) } ?: Dark
        }
    }
}

/** Цвета окна одной палитры, `0xRRGGBB`: фон, четыре поверхности, рамка, контур и четыре цвета текста. */
class PlColors(hex: IntArray) {
    val bg = hex[0]
    val surface1 = hex[1]
    val surface2 = hex[2]
    val surface3 = hex[3]
    val surface4 = hex[4]
    val border = hex[5]
    val outline = hex[6]
    val textPrimary = hex[7]
    val textSecondary = hex[8]
    val textTertiary = hex[9]
    val textDisabled = hex[10]

    /** Все цвета по порядку, как в таблице [Palettes]. */
    fun toList(): List<Int> = listOf(
        bg, surface1, surface2, surface3, surface4, border, outline,
        textPrimary, textSecondary, textTertiary, textDisabled,
    )
}

/** Цветовое семейство: у каждого есть тёмный и светлый тон. Порядок — как в настройках. */
enum class Family(val key: String, val title: String) {
    Classic("classic", "Классика"),
    Graphite("graphite", "Графит"),
    Coffee("coffee", "Кофе"),
    Slate("slate", "Сланец"),
    Midnight("midnight", "Полночь"),
    Ocean("ocean", "Океан"),
    Forest("forest", "Лес"),
    Matcha("matcha", "Матча"),
    Amber("amber", "Янтарь"),
    Dusk("dusk", "Закат"),
    Sakura("sakura", "Сакура"),
    Velvet("velvet", "Бархат"),
    ;

    /** Цвета семейства в заданном тоне. */
    fun colors(tone: Tone): PlColors = Palettes.get(this, tone)

    companion object {
        /** Пробелы и регистр не важны; неизвестное семейство — «Классика». */
        fun fromKey(key: String?): Family {
            val clean = key?.trim().orEmpty()
            return entries.firstOrNull { it.key.equals(clean, ignoreCase = true) } ?: Classic
        }
    }
}

/** Палитры: для каждого семейства по порядку [Family] тёмная, затем светлая. */
object Palettes {
    /** Порядок цветов: фон, четыре поверхности, рамка, контур, четыре цвета текста. */
    internal val table: List<IntArray> = listOf(
    // classic, dark
    intArrayOf(0x000000, 0x0B0B0B, 0x141414, 0x1E1E1E, 0x2A2A2A, 0x1A1A1A, 0x333333, 0xF2F2F2, 0xA8A8A8, 0x8A8A8A, 0x5A5A5A),
    // classic, light
    intArrayOf(0xFFFFFF, 0xF4F4F4, 0xEBEBEB, 0xE1E1E1, 0xD6D6D6, 0xE5E5E5, 0xCDCDCD, 0x111111, 0x545454, 0x696969, 0xA5A5A5),
    // graphite, dark
    intArrayOf(0x131417, 0x1C1D20, 0x242528, 0x2C2D31, 0x36383B, 0x292A2D, 0x3D3F43, 0xEEF0F3, 0xABADB0, 0x909194, 0x646669),
    // graphite, light
    intArrayOf(0xF2F3F5, 0xE9EAEC, 0xE1E3E5, 0xD9DBDE, 0xCFD1D4, 0xDDDEE1, 0xC8CACD, 0x16181C, 0x545559, 0x67696C, 0x9EA0A3),
    // coffee, dark
    intArrayOf(0x13100E, 0x1C1816, 0x241F1C, 0x2C2723, 0x36302C, 0x292421, 0x3D3732, 0xF4EFEA, 0xAFABA7, 0x938F8B, 0x676360),
    // coffee, light
    intArrayOf(0xF8F4EF, 0xF0ECE6, 0xEAE5DF, 0xE2DDD8, 0xDAD4CE, 0xE5E0DB, 0xD3CEC7, 0x1F1A15, 0x5C5752, 0x6F6B66, 0xA6A19C),
    // slate, dark
    intArrayOf(0x0E1418, 0x151C20, 0x1B2227, 0x222A2F, 0x2A3238, 0x1F272C, 0x30393F, 0xEBF1F5, 0xA7ADB1, 0x8C9296, 0x60666A),
    // slate, light
    intArrayOf(0xEDF2F6, 0xE4EAEF, 0xDDE4E9, 0xD6DDE3, 0xCCD5DB, 0xD9E0E5, 0xC5CED6, 0x141C22, 0x51585D, 0x646B70, 0x9BA1A5),
    // midnight, dark
    intArrayOf(0x090E1A, 0x101624, 0x161C2C, 0x1C2335, 0x242C3F, 0x1A2031, 0x293247, 0xEAF0FF, 0xA5ABB9, 0x898F9D, 0x5D626F),
    // midnight, light
    intArrayOf(0xEEF3FB, 0xE5EBF5, 0xDDE4EF, 0xD5DDEA, 0xCBD4E3, 0xD8E0EC, 0xC3CDDD, 0x0F1A2E, 0x4D5767, 0x626A7A, 0x99A1AD),
    // ocean, dark
    intArrayOf(0x03181D, 0x092026, 0x0E272D, 0x142F35, 0x1B383F, 0x122C32, 0x203F46, 0xE4F4F7, 0x9FB1B4, 0x839599, 0x576A6E),
    // ocean, light
    intArrayOf(0xE5F4F7, 0xDBEDF0, 0xD3E7EB, 0xCAE1E5, 0xC0D9DE, 0xCEE3E7, 0xB8D3D8, 0x082830, 0x466168, 0x5A737A, 0x91A6AB),
    // forest, dark
    intArrayOf(0x07140D, 0x0D1D14, 0x12241A, 0x182C20, 0x1F3528, 0x16281E, 0x243C2E, 0xE6F3EA, 0xA2AFA6, 0x86938B, 0x5A675F),
    // forest, light
    intArrayOf(0xE9F5EC, 0xE0EEE4, 0xD8E8DD, 0xD0E2D5, 0xC5DACC, 0xD3E4D8, 0xBED4C5, 0x0E2416, 0x4B5F52, 0x5F7165, 0x96A69B),
    // matcha, dark
    intArrayOf(0x111407, 0x191D0E, 0x202413, 0x272C19, 0x303520, 0x242916, 0x373D25, 0xEEF2E2, 0xAAAE9F, 0x8F9384, 0x636758),
    // matcha, light
    intArrayOf(0xF2F5E1, 0xEBEED8, 0xE5E8D0, 0xDEE2C8, 0xD6DABD, 0xE0E5CB, 0xD0D5B6, 0x1D220D, 0x595D48, 0x6C705B, 0xA1A590),
    // amber, dark
    intArrayOf(0x161007, 0x1F180D, 0x271F12, 0x2F2617, 0x3A2F1E, 0x2C2315, 0x413623, 0xF6EFE3, 0xB2ABA0, 0x968F84, 0x696359),
    // amber, light
    intArrayOf(0xFBF4E5, 0xF5ECDC, 0xEFE6D4, 0xEADFCB, 0xE3D7C1, 0xECE2CF, 0xDDD1B9, 0x2A1F0C, 0x655B49, 0x776E5C, 0xACA393),
    // dusk, dark
    intArrayOf(0x190C07, 0x23130D, 0x2B1912, 0x332018, 0x3E281F, 0x301D16, 0x462E24, 0xFAEDE6, 0xB5A8A2, 0x998C86, 0x6D605A),
    // dusk, light
    intArrayOf(0xFDEEE5, 0xF7E6DC, 0xF2DFD4, 0xEDD8CC, 0xE6CFC2, 0xEFDBCF, 0xE1C8BB, 0x2E1810, 0x68544C, 0x7B675F, 0xAE9D94),
    // sakura, dark
    intArrayOf(0x190A11, 0x231119, 0x2B1720, 0x331D27, 0x3E2530, 0x301B24, 0x462B37, 0xFAECF1, 0xB5A7AD, 0x998B91, 0x6D5E64),
    // sakura, light
    intArrayOf(0xFDEEF4, 0xF7E5ED, 0xF2DEE6, 0xEDD6E0, 0xE7CDD8, 0xEFD9E2, 0xE2C6D1, 0x2D1620, 0x67525B, 0x7A666E, 0xAE9CA3),
    // velvet, dark
    intArrayOf(0x120C21, 0x1A132A, 0x201932, 0x261F3B, 0x2F2745, 0x241D37, 0x352C4D, 0xF0EDFB, 0xACA8B8, 0x918C9D, 0x656072),
    // velvet, light
    intArrayOf(0xF2EEFD, 0xEAE6F8, 0xE4DFF3, 0xDDD8EF, 0xD5CFE9, 0xE0DBF1, 0xCFC8E5, 0x1F1A33, 0x5A556C, 0x6D687E, 0xA29DB0),
    )

    private val colors: List<PlColors> = table.map(::PlColors)

    fun get(family: Family, tone: Tone): PlColors {
        val index = family.ordinal * 2 + if (tone == Tone.Light) 1 else 0
        return colors.getOrNull(index) ?: colors[0]
    }
}

/** Цветовая математика на `Int` RGB (`0xRRGGBB`, старшие биты игнорируются). */
object ThemeMath {
    /** Контраст акцента с фоном, ниже которого его затемняют в светлом тоне. */
    const val ACCENT_MIN_CONTRAST = 3.0

    private fun channel(rgb: Int, shift: Int): Double = (rgb shr shift and 0xFF) / 255.0

    /** Относительная яркость по WCAG. */
    fun luminance(rgb: Int): Double {
        fun lin(c: Double) = if (c <= 0.04045) c / 12.92 else ((c + 0.055) / 1.055).pow(2.4)
        return 0.2126 * lin(channel(rgb, 16)) + 0.7152 * lin(channel(rgb, 8)) + 0.0722 * lin(channel(rgb, 0))
    }

    /** Контраст двух цветов по WCAG (от 1 до 21). */
    fun contrast(a: Int, b: Int): Double {
        val la = luminance(a)
        val lb = luminance(b)
        return (maxOf(la, lb) + 0.05) / (minOf(la, lb) + 0.05)
    }

    /** Смесь цветов: [t] = 0 — [from], 1 — [to]. */
    fun mix(from: Int, to: Int, t: Double): Int {
        val k = if (t.isFinite()) t.coerceIn(0.0, 1.0) else 0.0
        fun part(shift: Int): Int {
            val a = from shr shift and 0xFF
            val b = to shr shift and 0xFF
            return (a + (b - a) * k).roundToInt().coerceIn(0, 255)
        }
        return (part(16) shl 16) or (part(8) shl 8) or part(0)
    }

    /**
     * [color] на фоне [bg]: в светлом тоне подмешивает чёрный шагами по 5 % (не больше 50 %),
     * пока контраст не достигнет 3:1; в тёмном цвет не меняется.
     */
    fun legibleOn(color: Int, bg: Int, tone: Tone): Int {
        val clean = color and 0xFFFFFF
        if (tone == Tone.Dark) return clean
        for (step in 0..10) {
            val candidate = mix(clean, 0x000000, step * 0.05)
            if (contrast(candidate, bg) >= ACCENT_MIN_CONTRAST) return candidate
        }
        return mix(clean, 0x000000, 0.5)
    }

    /**
     * [color] на тёмном фоне [bg]: подмешивает белый шагами по 5 %, пока контраст
     * не достигнет 3:1. Десктоп свой цвет не ограничивает, но на телефоне акцент ещё и
     * цвет текста выбранного трека, поэтому чёрный не должен пропадать на тёмном фоне.
     */
    fun lightenOn(color: Int, bg: Int): Int {
        val clean = color and 0xFFFFFF
        for (step in 0..20) {
            val candidate = mix(clean, 0xFFFFFF, step * 0.05)
            if (contrast(candidate, bg) >= ACCENT_MIN_CONTRAST) return candidate
        }
        return 0xFFFFFF
    }
}
