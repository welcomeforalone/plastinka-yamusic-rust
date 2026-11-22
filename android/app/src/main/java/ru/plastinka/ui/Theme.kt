package ru.plastinka.ui

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.graphics.drawable.ColorDrawable
import android.view.Window
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import ru.plastinka.R
import ru.plastinka.data.Prefs

/**
 * Готовые акценты интерфейса: порядок, ключи и цвета те же, что на десктопе
 * (src/ui/theme.rs). [rgb] — исходный цвет `0xRRGGBB`; в светлом тоне интерфейс
 * показывает затемнённый (см. [ThemeMath.legibleOn]), образцы в настройках — исходный.
 */
enum class Accent(val key: String, val title: String, val rgb: Int) {
    Red("red", "Красный", 0xFA2D48),
    Orange("orange", "Оранжевый", 0xFF8A3D),
    Yellow("yellow", "Жёлтый", 0xFFC53D),
    Green("green", "Зелёный", 0x3CCB7F),
    Teal("teal", "Бирюзовый", 0x3DD6C3),
    Blue("blue", "Синий", 0x3D8BFF),
    Violet("violet", "Фиолетовый", 0x9D7BFF),
    Pink("pink", "Розовый", 0xFF4F9A),
    ;

    val color: Color get() = Color(0xFF000000.toInt() or rgb)

    companion object {
        /** Непонятное или пустое значение — красный по умолчанию. */
        fun fromKey(key: String?): Accent {
            val clean = key?.trim().orEmpty()
            return entries.firstOrNull { it.key.equals(clean, ignoreCase = true) } ?: Red
        }
    }
}

/**
 * Выбранный акцент: готовый или свой цвет. Состояние Compose, смена сразу
 * перекрашивает интерфейс. В настройках одна строка: ключ готового акцента
 * (`red`) или `#RRGGBB`; непонятное значение — красный.
 */
object AccentState {
    /** Готовый акцент; когда выбран свой цвет, не используется. */
    var preset by mutableStateOf(Accent.Red)
        private set

    /** Свой цвет `0xRRGGBB`, как его выбрал пользователь, или `null`. */
    var custom by mutableStateOf<Int?>(null)
        private set

    /** Исходный цвет выбора `0xRRGGBB`: для образцов и окна выбора цвета. */
    val rgb: Int get() = custom ?: preset.rgb

    /** Исходный цвет выбора, без подгонки под тон. */
    val color: Color get() = Color(0xFF000000.toInt() or rgb)

    /** Название для подписи: «Красный» или «Свой цвет». */
    val title: String get() = if (custom != null) "Свой цвет" else preset.title

    /**
     * Акцент в тоне [tone] на фоне [bg]. Тёмный: слишком тёмный свой цвет осветляется
     * ([ColorMath.readable]). Светлый: слишком светлый затемняется до контраста 3:1.
     */
    fun resolve(bg: Int, tone: Tone): Int = resolveFor(rgb, bg, tone)

    /** После [Prefs.load]. */
    fun load() {
        val key = Prefs.accentKey()
        val parsed = key?.let(ColorMath::parseHex)
        if (parsed != null) {
            custom = parsed
            preset = Accent.Red
        } else {
            custom = null
            preset = Accent.fromKey(key)
        }
    }

    fun selectPreset(accent: Accent) {
        custom = null
        preset = accent
        Prefs.saveAccentKey(accent.key)
    }

    /** Свой цвет сохраняется как выбран; под тон его подгоняет [resolve]. */
    fun selectCustom(rgb: Int) {
        val value = rgb and 0xFFFFFF
        custom = value
        Prefs.saveAccentKey(ColorMath.formatHex(value))
    }
}

/** Акцент [rgb] в тоне [tone] на фоне [bg]: тёмный осветляется ([ColorMath.readable]), светлый затемняется. */
fun resolveFor(rgb: Int, bg: Int, tone: Tone): Int {
    if (tone == Tone.Dark) return ThemeMath.lightenOn(ColorMath.readable(rgb), bg)
    return ThemeMath.legibleOn(rgb, bg, tone)
}

/**
 * Выбранная тема: семейство и режим тона. Состояние Compose; ключи в настройках
 * `theme` и `theme_mode` те же, что в `config.toml` десктопа.
 */
object ThemeState {
    var family by mutableStateOf(Family.Classic)
        private set

    var mode by mutableStateOf(ThemeMode.Dark)
        private set

    /** После [Prefs.load]. */
    fun load() {
        family = Family.fromKey(Prefs.themeKey())
        mode = ThemeMode.fromKey(Prefs.themeModeKey())
    }

    fun selectFamily(value: Family) {
        family = value
        Prefs.saveThemeKey(value.key)
    }

    fun selectMode(value: ThemeMode) {
        mode = value
        Prefs.saveThemeModeKey(value.key)
    }

    /** «Сбросить всё»: «Классика» и тёмная. */
    fun reset() {
        selectFamily(Family.Classic)
        selectMode(ThemeMode.Dark)
    }

    /** Тон окна: по режиму, «Как в системе» — по [systemDark]. */
    fun tone(systemDark: Boolean): Tone = mode.tone(systemDark)
}

/** Цвет значка или текста на произвольном фоне: по яркости, как на акценте. */
fun onColorFor(background: Color): Color {
    val luma = 0.299f * background.red + 0.587f * background.green + 0.114f * background.blue
    return if (luma > 0.6f) Pl.darkText else Color.White
}

private fun rgbColor(rgb: Int) = Color(0xFF000000.toInt() or (rgb and 0xFFFFFF))

/** Цвета одного тона одной палитры плюс акцент, уже подогнанный под фон. */
@Immutable
class PlScheme(val tone: Tone, colors: PlColors, accentRgb: Int) {
    /** Фон как `0xRRGGBB`: для подгонки цветов под него. */
    val bgRgb = colors.bg and 0xFFFFFF
    val bg = rgbColor(colors.bg)
    val surface1 = rgbColor(colors.surface1)
    val surface2 = rgbColor(colors.surface2)
    val surface3 = rgbColor(colors.surface3)
    val surface4 = rgbColor(colors.surface4)
    val border = rgbColor(colors.border)
    val outline = rgbColor(colors.outline)
    val text = rgbColor(colors.textPrimary)
    val text2 = rgbColor(colors.textSecondary)
    val text3 = rgbColor(colors.textTertiary)
    val textDisabled = rgbColor(colors.textDisabled)
    val error = if (tone == Tone.Dark) Color(0xFFFF6B6B) else Color(0xFFD23F3F)
    val errorSoft = error.copy(alpha = 0.14f)
    val growth = if (tone == Tone.Dark) Color(0xFF4CD07D) else Color(0xFF1C9A50)
    val accent = rgbColor(accentRgb)
    val accentSoft = accent.copy(alpha = 0.14f)
    val onAccent = onColorFor(accent)

    /** Рамка карточки и листа: середина между `border` и `outline` (в «Классике» тёмной 0x262626). */
    val edge = lerp(border, outline, 0.5f)

    /** Затемнение под шторками и диалогами: как `scrim()` на десктопе. */
    val scrim = if (tone == Tone.Dark) Color(0xB8000000) else Color(0x660F0F14)

    /** Тень слабее в светлом тоне: вдвое, как на десктопе. */
    fun shadowAlpha(dark: Float): Float = if (tone == Tone.Dark) dark else dark * 0.5f

    companion object {
        fun build(family: Family, tone: Tone): PlScheme {
            val colors = family.colors(tone)
            return PlScheme(tone, colors, AccentState.resolve(colors.bg, tone))
        }
    }
}

/** Material-схема по цветам [s]: тот же тон, те же поверхности. */
private fun materialScheme(s: PlScheme): ColorScheme = if (s.tone == Tone.Dark) {
    darkColorScheme(
        primary = s.accent,
        onPrimary = s.onAccent,
        secondary = s.accent,
        onSecondary = s.onAccent,
        tertiary = s.accent,
        background = s.bg,
        onBackground = s.text,
        surface = s.surface1,
        onSurface = s.text,
        surfaceVariant = s.surface3,
        onSurfaceVariant = s.text2,
        surfaceContainer = s.surface2,
        surfaceContainerHigh = s.surface2,
        outline = s.outline,
        outlineVariant = s.border,
        error = s.error,
    )
} else {
    lightColorScheme(
        primary = s.accent,
        onPrimary = s.onAccent,
        secondary = s.accent,
        onSecondary = s.onAccent,
        tertiary = s.accent,
        background = s.bg,
        onBackground = s.text,
        surface = s.surface1,
        onSurface = s.text,
        surfaceVariant = s.surface3,
        onSurfaceVariant = s.text2,
        surfaceContainer = s.surface2,
        surfaceContainerHigh = s.surface2,
        surfaceContainerHighest = s.surface3,
        surfaceContainerLow = s.surface1,
        surfaceContainerLowest = s.bg,
        outline = s.outline,
        outlineVariant = s.border,
        error = s.error,
    )
}

/** Цвета текущей области; по умолчанию «Классика», тёмная. */
val LocalPl = staticCompositionLocalOf {
    PlScheme(Tone.Dark, Family.Classic.colors(Tone.Dark), Accent.Red.rgb)
}

/**
 * Токены цвета макета. Читаются только в composable-области: цвета зависят от выбранной
 * темы и от области (см. [PlDarkScope]). В лямбдах `drawBehind` и обработчиках цвет
 * поднимают в локальную `val` снаружи.
 */
object Pl {
    val bg: Color @Composable @ReadOnlyComposable get() = LocalPl.current.bg
    val surface1: Color @Composable @ReadOnlyComposable get() = LocalPl.current.surface1
    val surface2: Color @Composable @ReadOnlyComposable get() = LocalPl.current.surface2
    val surface3: Color @Composable @ReadOnlyComposable get() = LocalPl.current.surface3
    val surface4: Color @Composable @ReadOnlyComposable get() = LocalPl.current.surface4
    val border: Color @Composable @ReadOnlyComposable get() = LocalPl.current.border
    val outline: Color @Composable @ReadOnlyComposable get() = LocalPl.current.outline
    val text: Color @Composable @ReadOnlyComposable get() = LocalPl.current.text
    val text2: Color @Composable @ReadOnlyComposable get() = LocalPl.current.text2
    val text3: Color @Composable @ReadOnlyComposable get() = LocalPl.current.text3
    val textDisabled: Color @Composable @ReadOnlyComposable get() = LocalPl.current.textDisabled
    val error: Color @Composable @ReadOnlyComposable get() = LocalPl.current.error
    val errorSoft: Color @Composable @ReadOnlyComposable get() = LocalPl.current.errorSoft
    val growth: Color @Composable @ReadOnlyComposable get() = LocalPl.current.growth
    val darkText = Color(0xFF0A0A0A)

    /** Акцент в текущем тоне: в светлом при необходимости затемнён до контраста 3:1 с фоном. */
    val accent: Color @Composable @ReadOnlyComposable get() = LocalPl.current.accent
    val accentSoft: Color @Composable @ReadOnlyComposable get() = LocalPl.current.accentSoft

    /** Цвет текста на акценте: на светлом — тёмный, на тёмном — белый. */
    val onAccent: Color @Composable @ReadOnlyComposable get() = LocalPl.current.onAccent

    val edge: Color @Composable @ReadOnlyComposable get() = LocalPl.current.edge
    val scrim: Color @Composable @ReadOnlyComposable get() = LocalPl.current.scrim
    val tone: Tone @Composable @ReadOnlyComposable get() = LocalPl.current.tone
    val isLight: Boolean @Composable @ReadOnlyComposable get() = LocalPl.current.tone == Tone.Light

    /** Альфа тени: в светлом тоне вдвое меньше. */
    @Composable
    @ReadOnlyComposable
    fun shadowAlpha(dark: Float): Float = LocalPl.current.shadowAlpha(dark)
}

/** Сколько тёмных областей на экране сейчас: пока хоть одна есть, значки системных панелей светлые. */
object DarkAreas {
    var count by mutableIntStateOf(0)
        private set

    fun enter() {
        count += 1
    }

    fun leave() {
        count = maxOf(0, count - 1)
    }
}

/**
 * Тёмная область: полноэкранный плеер, мини-плеер и другие слои на затемнённой обложке
 * всегда тёмные, как на десктопе (`dark_scope`): тёмный тон текущего семейства
 * и тёмная Material-схема.
 */
@Composable
fun PlDarkScope(content: @Composable () -> Unit) {
    val family = ThemeState.family
    val scheme = remember(family, AccentState.preset, AccentState.custom) { PlScheme.build(family, Tone.Dark) }
    DisposableEffect(Unit) {
        DarkAreas.enter()
        onDispose { DarkAreas.leave() }
    }
    CompositionLocalProvider(LocalPl provides scheme) {
        MaterialTheme(colorScheme = materialScheme(scheme), typography = MaterialTheme.typography, content = content)
    }
}

val Inter = FontFamily(
    Font(R.font.inter_regular, FontWeight.Normal),
    Font(R.font.inter_semibold, FontWeight.SemiBold),
    Font(R.font.inter_bold, FontWeight.Bold),
    Font(R.font.inter_extrabold, FontWeight.ExtraBold),
)

private const val TNUM = "tnum"

private fun style(
    weight: FontWeight,
    size: Int,
    line: Int,
    tracking: Double = 0.0,
    tnum: Boolean = false,
) = TextStyle(
    fontFamily = Inter,
    fontWeight = weight,
    fontSize = size.sp,
    lineHeight = line.sp,
    letterSpacing = tracking.em,
    fontFeatureSettings = if (tnum) TNUM else null,
)

/** Шкала текста макета: [начертание, размер, межстрочный, трекинг]. */
object PlType {
    val hero = style(FontWeight.ExtraBold, 40, 44, -0.02)
    val display = style(FontWeight.ExtraBold, 32, 38, -0.015)
    val playerTitle = style(FontWeight.ExtraBold, 28, 34, -0.01)
    val lyrics = style(FontWeight.Bold, 24, 32, -0.005)
    val title = style(FontWeight.Bold, 22, 28, -0.005)
    val headline = style(FontWeight.Bold, 17, 22)
    val body = style(FontWeight.Normal, 16, 22)
    val bodyStrong = style(FontWeight.SemiBold, 16, 22)
    val secondary = style(FontWeight.Normal, 14, 18)
    val secondaryStrong = style(FontWeight.SemiBold, 14, 18)
    val caption = style(FontWeight.Normal, 12, 16)
    val captionStrong = style(FontWeight.SemiBold, 12, 16)
    val micro = style(FontWeight.SemiBold, 11, 14, 0.02)
    val badge = style(FontWeight.Bold, 9, 12)
    val code = style(FontWeight.Bold, 40, 48, 0.08, tnum = true)
}

/** Material-типографика на Inter, чтобы старые экраны не выбивались из шрифта. */
private fun interTypography(): Typography {
    val base = Typography()
    return Typography(
        displayLarge = base.displayLarge.copy(fontFamily = Inter),
        displayMedium = base.displayMedium.copy(fontFamily = Inter),
        displaySmall = base.displaySmall.copy(fontFamily = Inter),
        headlineLarge = base.headlineLarge.copy(fontFamily = Inter),
        headlineMedium = base.headlineMedium.copy(fontFamily = Inter),
        headlineSmall = base.headlineSmall.copy(fontFamily = Inter),
        titleLarge = base.titleLarge.copy(fontFamily = Inter),
        titleMedium = base.titleMedium.copy(fontFamily = Inter),
        titleSmall = base.titleSmall.copy(fontFamily = Inter),
        bodyLarge = base.bodyLarge.copy(fontFamily = Inter),
        bodyMedium = base.bodyMedium.copy(fontFamily = Inter),
        bodySmall = base.bodySmall.copy(fontFamily = Inter),
        labelLarge = base.labelLarge.copy(fontFamily = Inter),
        labelMedium = base.labelMedium.copy(fontFamily = Inter),
        labelSmall = base.labelSmall.copy(fontFamily = Inter),
    )
}

private fun Context.findActivityWindow(): Window? {
    var current: Context? = this
    while (current is ContextWrapper) {
        if (current is Activity) return current.window
        current = current.baseContext
    }
    return null
}

/** Тон окна по настройкам и теме системы (для «Как в системе»). */
@Composable
fun rememberWindowTone(): Tone = ThemeState.tone(isSystemInDarkTheme())

@Composable
fun PlastinkaTheme(content: @Composable () -> Unit) {
    // Тема и акцент читаются из настроек в PlastinkaApp до показа окна.
    val tone = rememberWindowTone()
    val family = ThemeState.family
    val scheme = remember(family, tone, AccentState.preset, AccentState.custom) {
        PlScheme.build(family, tone)
    }
    val typography = remember { interTypography() }
    // Фон окна под Compose (виден при повороте и перерисовке) — цвет темы, а не чёрный из манифеста.
    val window = LocalContext.current.findActivityWindow()
    val windowBg = scheme.bg.toArgb()
    SideEffect { window?.setBackgroundDrawable(ColorDrawable(windowBg)) }
    CompositionLocalProvider(LocalPl provides scheme) {
        MaterialTheme(colorScheme = materialScheme(scheme), typography = typography, content = content)
    }
}
