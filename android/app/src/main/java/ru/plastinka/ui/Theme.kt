package ru.plastinka.ui

import android.content.Context
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import ru.plastinka.R

/** Акценты интерфейса: ключи и цвета те же, что на десктопе (src/ui/theme.rs). */
enum class Accent(val key: String, val title: String, val color: Color) {
    Red("red", "Красный", Color(0xFFFA2D48)),
    Teal("teal", "Бирюзовый", Color(0xFF3DD6C3)),
    Orange("orange", "Оранжевый", Color(0xFFFF8A3D)),
    Violet("violet", "Фиолетовый", Color(0xFF9D7BFF)),
    Blue("blue", "Синий", Color(0xFF3D8BFF)),
    ;

    companion object {
        /** Непонятное или пустое значение — красный по умолчанию. */
        fun fromKey(key: String?): Accent = entries.firstOrNull { it.key == key } ?: Red
    }
}

/** Выбранный акцент: состояние Compose, смена сразу перекрашивает интерфейс. */
object AccentState {
    private const val PREFS = "plastinka"
    private const val KEY = "accent"

    var current by mutableStateOf(Accent.Red)
        private set

    fun load(context: Context) {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        current = Accent.fromKey(prefs.getString(KEY, null))
    }

    fun select(context: Context, accent: Accent) {
        current = accent
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit().putString(KEY, accent.key).apply()
    }
}

/** Цвет значка или текста на произвольном фоне: по яркости, как на акценте. */
fun onColorFor(background: Color): Color {
    val luma = 0.299f * background.red + 0.587f * background.green + 0.114f * background.blue
    return if (luma > 0.6f) Pl.darkText else Color.White
}

/** Токены цвета макета. Цвета акцента читают состояние, поэтому следят за сменой. */
object Pl {
    val bg = Color(0xFF000000)
    val surface1 = Color(0xFF0B0B0B)
    val surface2 = Color(0xFF141414)
    val surface3 = Color(0xFF1E1E1E)
    val surface4 = Color(0xFF2A2A2A)
    val border = Color(0xFF1A1A1A)
    val outline = Color(0xFF333333)
    val text = Color(0xFFF2F2F2)
    val text2 = Color(0xFFA8A8A8)
    val text3 = Color(0xFF8A8A8A)
    val textDisabled = Color(0xFF5A5A5A)
    val error = Color(0xFFFF6B6B)
    val errorSoft = error.copy(alpha = 0.14f)
    val growth = Color(0xFF4CD07D)
    val darkText = Color(0xFF0A0A0A)

    val accent: Color get() = AccentState.current.color
    val accentSoft: Color get() = accent.copy(alpha = 0.14f)

    /** Цвет текста на акценте: на светлом — тёмный, на тёмном — белый. */
    val onAccent: Color get() = onColorFor(accent)
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
    val title = style(FontWeight.Bold, 22, 28, -0.005)
    val headline = style(FontWeight.Bold, 17, 22)
    val body = style(FontWeight.Normal, 16, 22)
    val bodyStrong = style(FontWeight.SemiBold, 16, 22)
    val secondary = style(FontWeight.Normal, 14, 18)
    val secondaryStrong = style(FontWeight.SemiBold, 14, 18)
    val caption = style(FontWeight.Normal, 12, 16)
    val captionStrong = style(FontWeight.SemiBold, 12, 16)
    val micro = style(FontWeight.SemiBold, 11, 14, 0.02)
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

@Composable
fun PlastinkaTheme(content: @Composable () -> Unit) {
    // Акцент читает из настроек MainActivity до показа; клиент тёмный всегда.
    val accent = AccentState.current.color
    val scheme = darkColorScheme(
        primary = accent,
        onPrimary = Pl.onAccent,
        secondary = accent,
        onSecondary = Pl.onAccent,
        tertiary = accent,
        background = Pl.bg,
        onBackground = Pl.text,
        surface = Pl.surface1,
        onSurface = Pl.text,
        surfaceVariant = Pl.surface3,
        onSurfaceVariant = Pl.text2,
        surfaceContainer = Pl.surface2,
        surfaceContainerHigh = Pl.surface2,
        outline = Pl.outline,
        outlineVariant = Pl.border,
        error = Pl.error,
    )
    MaterialTheme(colorScheme = scheme, typography = interTypography(), content = content)
}
