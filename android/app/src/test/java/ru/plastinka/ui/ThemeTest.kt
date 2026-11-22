package ru.plastinka.ui

import androidx.compose.ui.graphics.Color
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotEquals
import kotlin.test.assertTrue

/** Темы и акценты: палитры совпадают с десктопными (src/ui/theme.rs), цветовая математика, ключи. */
class ThemeTest {
    /** Таблица значений из PALETTES десктопа: «семейство тон: 11 цветов». */
    private val desktop = listOf(
        "classic dark: 000000 0B0B0B 141414 1E1E1E 2A2A2A 1A1A1A 333333 F2F2F2 A8A8A8 8A8A8A 5A5A5A",
        "classic light: FFFFFF F4F4F4 EBEBEB E1E1E1 D6D6D6 E5E5E5 CDCDCD 111111 545454 696969 A5A5A5",
        "graphite dark: 131417 1C1D20 242528 2C2D31 36383B 292A2D 3D3F43 EEF0F3 ABADB0 909194 646669",
        "graphite light: F2F3F5 E9EAEC E1E3E5 D9DBDE CFD1D4 DDDEE1 C8CACD 16181C 545559 67696C 9EA0A3",
        "coffee dark: 13100E 1C1816 241F1C 2C2723 36302C 292421 3D3732 F4EFEA AFABA7 938F8B 676360",
        "coffee light: F8F4EF F0ECE6 EAE5DF E2DDD8 DAD4CE E5E0DB D3CEC7 1F1A15 5C5752 6F6B66 A6A19C",
        "slate dark: 0E1418 151C20 1B2227 222A2F 2A3238 1F272C 30393F EBF1F5 A7ADB1 8C9296 60666A",
        "slate light: EDF2F6 E4EAEF DDE4E9 D6DDE3 CCD5DB D9E0E5 C5CED6 141C22 51585D 646B70 9BA1A5",
        "midnight dark: 090E1A 101624 161C2C 1C2335 242C3F 1A2031 293247 EAF0FF A5ABB9 898F9D 5D626F",
        "midnight light: EEF3FB E5EBF5 DDE4EF D5DDEA CBD4E3 D8E0EC C3CDDD 0F1A2E 4D5767 626A7A 99A1AD",
        "ocean dark: 03181D 092026 0E272D 142F35 1B383F 122C32 203F46 E4F4F7 9FB1B4 839599 576A6E",
        "ocean light: E5F4F7 DBEDF0 D3E7EB CAE1E5 C0D9DE CEE3E7 B8D3D8 082830 466168 5A737A 91A6AB",
        "forest dark: 07140D 0D1D14 12241A 182C20 1F3528 16281E 243C2E E6F3EA A2AFA6 86938B 5A675F",
        "forest light: E9F5EC E0EEE4 D8E8DD D0E2D5 C5DACC D3E4D8 BED4C5 0E2416 4B5F52 5F7165 96A69B",
        "matcha dark: 111407 191D0E 202413 272C19 303520 242916 373D25 EEF2E2 AAAE9F 8F9384 636758",
        "matcha light: F2F5E1 EBEED8 E5E8D0 DEE2C8 D6DABD E0E5CB D0D5B6 1D220D 595D48 6C705B A1A590",
        "amber dark: 161007 1F180D 271F12 2F2617 3A2F1E 2C2315 413623 F6EFE3 B2ABA0 968F84 696359",
        "amber light: FBF4E5 F5ECDC EFE6D4 EADFCB E3D7C1 ECE2CF DDD1B9 2A1F0C 655B49 776E5C ACA393",
        "dusk dark: 190C07 23130D 2B1912 332018 3E281F 301D16 462E24 FAEDE6 B5A8A2 998C86 6D605A",
        "dusk light: FDEEE5 F7E6DC F2DFD4 EDD8CC E6CFC2 EFDBCF E1C8BB 2E1810 68544C 7B675F AE9D94",
        "sakura dark: 190A11 231119 2B1720 331D27 3E2530 301B24 462B37 FAECF1 B5A7AD 998B91 6D5E64",
        "sakura light: FDEEF4 F7E5ED F2DEE6 EDD6E0 E7CDD8 EFD9E2 E2C6D1 2D1620 67525B 7A666E AE9CA3",
        "velvet dark: 120C21 1A132A 201932 261F3B 2F2745 241D37 352C4D F0EDFB ACA8B8 918C9D 656072",
        "velvet light: F2EEFD EAE6F8 E4DFF3 DDD8EF D5CFE9 E0DBF1 CFC8E5 1F1A33 5A556C 6D687E A29DB0",
    )

    private val tones = listOf(Tone.Dark, Tone.Light)

    @Test
    fun palettesMatchDesktop() {
        assertEquals(24, Palettes.table.size)
        assertEquals(24, desktop.size)
        assertEquals(12, Family.entries.size)
        for (family in Family.entries) {
            for (tone in tones) {
                val label = "${family.key} ${tone.name.lowercase()}"
                val line = desktop.first { it.startsWith("$label:") }
                val expected = line.substringAfter(": ").split(" ").map { it.toInt(16) }
                assertEquals(11, expected.size, label)
                assertEquals(expected, family.colors(tone).toList(), label)
            }
        }
    }

    @Test
    fun classicDarkKeepsOldConstants() {
        val c = Family.Classic.colors(Tone.Dark)
        assertEquals(0x000000, c.bg)
        assertEquals(0x0B0B0B, c.surface1)
        assertEquals(0x141414, c.surface2)
        assertEquals(0x1E1E1E, c.surface3)
        assertEquals(0x2A2A2A, c.surface4)
        assertEquals(0x1A1A1A, c.border)
        assertEquals(0x333333, c.outline)
        assertEquals(0xF2F2F2, c.textPrimary)
        assertEquals(0xA8A8A8, c.textSecondary)
        assertEquals(0x8A8A8A, c.textTertiary)
        assertEquals(0x5A5A5A, c.textDisabled)
    }

    @Test
    fun familyKeysAndTitles() {
        val titles = listOf(
            "Классика", "Графит", "Кофе", "Сланец", "Полночь", "Океан",
            "Лес", "Матча", "Янтарь", "Закат", "Сакура", "Бархат",
        )
        assertEquals(titles, Family.entries.map { it.title })
        for (family in Family.entries) assertEquals(family, Family.fromKey(family.key))
        assertEquals(Family.Midnight, Family.fromKey("  MIDNIGHT "))
        // Неизвестная и пустая тема — «Классика».
        assertEquals(Family.Classic, Family.fromKey("полночь"))
        assertEquals(Family.Classic, Family.fromKey(""))
        assertEquals(Family.Classic, Family.fromKey(null))
        assertEquals(Family.Classic, Family.fromKey("mid night"))
    }

    @Test
    fun themeModeKeysAndTone() {
        assertEquals(listOf("dark", "light", "system"), ThemeMode.entries.map { it.key })
        assertEquals(listOf("Тёмная", "Светлая", "Как в системе"), ThemeMode.entries.map { it.title })
        for (mode in ThemeMode.entries) assertEquals(mode, ThemeMode.fromKey(mode.key))
        assertEquals(ThemeMode.System, ThemeMode.fromKey(" System "))
        // Непонятное значение — тёмная.
        assertEquals(ThemeMode.Dark, ThemeMode.fromKey("5"))
        assertEquals(ThemeMode.Dark, ThemeMode.fromKey(null))
        assertEquals(Tone.Dark, ThemeMode.Dark.tone(systemDark = false))
        assertEquals(Tone.Light, ThemeMode.Light.tone(systemDark = true))
        assertEquals(Tone.Light, ThemeMode.System.tone(systemDark = false))
        assertEquals(Tone.Dark, ThemeMode.System.tone(systemDark = true))
    }

    @Test
    fun palettesAreReadable() {
        for (family in Family.entries) {
            for (tone in tones) {
                val c = family.colors(tone)
                val name = "${family.key}/$tone"
                assertTrue(ThemeMath.contrast(c.textPrimary, c.bg) >= 7.0, "$name: primary")
                assertTrue(ThemeMath.contrast(c.textSecondary, c.surface2) >= 4.5, "$name: secondary")
                assertTrue(ThemeMath.contrast(c.textTertiary, c.bg) >= 4.0, "$name: tertiary")
                val bg = ThemeMath.luminance(c.bg)
                val text = ThemeMath.luminance(c.textPrimary)
                if (tone == Tone.Light) assertTrue(bg > text, "$name: светлый тон") else assertTrue(bg < text, "$name: тёмный тон")
            }
        }
    }

    @Test
    fun contrastMath() {
        assertEquals(21.0, ThemeMath.contrast(0x000000, 0xFFFFFF), 1e-9)
        assertEquals(1.0, ThemeMath.contrast(0x123456, 0x123456), 1e-9)
        assertEquals(0x808080, ThemeMath.mix(0x000000, 0xFFFFFF, 0.5))
        assertEquals(0xFFFFFF, ThemeMath.mix(0x000000, 0xFFFFFF, 2.0))
        assertEquals(0x000000, ThemeMath.mix(0x000000, 0xFFFFFF, -1.0))
        assertEquals(0x000000, ThemeMath.mix(0x000000, 0xFFFFFF, Double.NaN))
    }

    @Test
    fun accentKeysAndColors() {
        assertEquals(
            listOf("red", "orange", "yellow", "green", "teal", "blue", "violet", "pink"),
            Accent.entries.map { it.key },
        )
        assertEquals(0xFFC53D, Accent.Yellow.rgb)
        assertEquals(0x3CCB7F, Accent.Green.rgb)
        assertEquals(0xFF4F9A, Accent.Pink.rgb)
        assertEquals("Жёлтый", Accent.Yellow.title)
        assertEquals("Зелёный", Accent.Green.title)
        assertEquals("Розовый", Accent.Pink.title)
        for (accent in Accent.entries) assertEquals(accent, Accent.fromKey(accent.key))
        assertEquals(Accent.Red, Accent.fromKey("фиолетовый"))
        assertEquals(Accent.Red, Accent.fromKey(null))
        assertEquals(Accent.Red, Accent.fromKey(" RED "))
    }

    @Test
    fun accentsAreLegibleOnLightPalettes() {
        for (family in Family.entries) {
            val bg = family.colors(Tone.Light).bg
            for (accent in Accent.entries) {
                val shown = ThemeMath.legibleOn(accent.rgb, bg, Tone.Light)
                assertTrue(ThemeMath.contrast(shown, bg) >= 3.0, "${family.key}/${accent.key}")
            }
        }
        // В тёмном тоне цвет не меняется.
        assertEquals(Accent.Red.rgb, ThemeMath.legibleOn(Accent.Red.rgb, 0x000000, Tone.Dark))
        // Подмешивается не больше половины чёрного: когда контраста не добиться, остаётся серый 0x808080.
        assertEquals(0x808080, ThemeMath.legibleOn(0xFFFFFF, 0xBBBBBB, Tone.Light))
        val shown = ThemeMath.legibleOn(0xFFFFFF, 0xFFFFFF, Tone.Light)
        assertTrue((shown shr 16 and 0xFF) >= 0x7F && ThemeMath.contrast(shown, 0xFFFFFF) >= 3.0)
    }

    @Test
    fun presetsStayAsIsOnDarkPalettes() {
        // Как на десктопе: в тёмном тоне готовые акценты не меняются.
        for (family in Family.entries) {
            val bg = family.colors(Tone.Dark).bg
            for (accent in Accent.entries) {
                assertEquals(accent.rgb, resolveFor(accent.rgb, bg, Tone.Dark), "${family.key}/${accent.key}")
            }
        }
    }

    @Test
    fun customColorsDoNotGetLost() {
        for (family in Family.entries) {
            for (tone in tones) {
                val bg = family.colors(tone).bg
                // Свой тёмный цвет не пропадает ни на тёмном, ни на светлом фоне; светлый — тоже.
                for (rgb in listOf(0x000000, 0x101010, 0x1A1033, 0x808080, 0xFFFFFF, 0xFFFF00)) {
                    val shown = resolveFor(rgb, bg, tone)
                    assertTrue(ThemeMath.contrast(shown, bg) >= 3.0, "${family.key}/$tone/${rgb.toString(16)}")
                }
            }
        }
        // На светлом фоне уже читаемый цвет остаётся как есть, на тёмном — тоже.
        assertEquals(0x1A1033, resolveFor(0x1A1033, 0xFFFFFF, Tone.Light))
        assertEquals(Accent.Blue.rgb, resolveFor(Accent.Blue.rgb, 0x000000, Tone.Dark))
        assertNotEquals(0x000000, resolveFor(0x000000, 0x000000, Tone.Dark))
    }

    @Test
    fun schemeRolesFollowTone() {
        val dark = PlScheme(Tone.Dark, Family.Classic.colors(Tone.Dark), Accent.Red.rgb)
        assertEquals(Color(0xFFFF6B6B), dark.error)
        assertEquals(Color(0xFFFF6B6B).copy(alpha = 0.14f), dark.errorSoft)
        assertEquals(Color(0xFF4CD07D), dark.growth)
        assertEquals(Color(0xB8000000), dark.scrim)
        assertEquals(0.5f, dark.shadowAlpha(0.5f))
        val light = PlScheme(Tone.Light, Family.Classic.colors(Tone.Light), Accent.Red.rgb)
        assertEquals(Color(0xFFD23F3F), light.error)
        assertEquals(Color(0xFFD23F3F).copy(alpha = 0.14f), light.errorSoft)
        assertEquals(Color(0xFF1C9A50), light.growth)
        assertEquals(0.25f, light.shadowAlpha(0.5f))
        assertNotEquals(dark.bg, light.bg)
        // Рамка карточки в «Классике» тёмной — прежний 0x262626.
        assertEquals(0x26, Math.round(dark.edge.red * 255f))
    }
}
