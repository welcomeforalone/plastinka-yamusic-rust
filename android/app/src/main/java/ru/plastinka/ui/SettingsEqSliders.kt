package ru.plastinka.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import kotlin.math.abs
import kotlin.math.roundToInt
import ru.plastinka.playback.EqSpec

// Свои ползунки эквалайзера на pointerInput: касание сразу ставит значение,
// жест целиком забирают себе (consume), поэтому вертикальная прокрутка страницы
// не перехватывает его. Область касания — вся колонка полосы, шире дорожки.

/** Отступ дорожки сверху и снизу области: ручка не выходит за неё. */
private val PLOT_PAD = 12.dp
private val PLOT_HEIGHT = 200.dp

// ------------------------------------------------------- чистая геометрия

/**
 * Значение полосы (десятые доли дБ) по координате: [top] — +12 дБ, [bottom] —
 * −12 дБ. Шаг 0,5 дБ, в пределах ±0,5 дБ от нуля — 0.
 */
internal fun bandTenthsAt(y: Float, top: Float, bottom: Float): Int {
    val half = (bottom - top) / 2f
    if (!y.isFinite() || half <= 0f) return 0
    val limit = EqSpec.GAIN_LIMIT.toFloat()
    val step = EqSpec.STEP.toFloat()
    val raw = (((top + bottom) / 2f - y) / half * limit).coerceIn(-limit, limit)
    if (abs(raw) < step) return 0
    return (raw / step).roundToInt() * EqSpec.STEP
}

/** Значение предусилителя по координате x: шаг 0,5 дБ, [left] — −12 дБ, [right] — +12 дБ. */
internal fun preampTenthsAt(x: Float, left: Float, right: Float): Int {
    val width = right - left
    if (!x.isFinite() || width <= 0f) return 0
    val limit = EqSpec.GAIN_LIMIT.toFloat()
    val raw = ((x - left) / width * 2f * limit - limit).coerceIn(-limit, limit)
    return EqSpec.clampGain((raw / EqSpec.STEP).roundToInt() * EqSpec.STEP)
}

// ----------------------------------------------------------------- подписи

/** Подпись полосы: `+4,5`, `0`, `−2` (настоящий минус). */
internal fun formatEqValue(tenths: Int): String {
    if (tenths == 0) return "0"
    val sign = if (tenths > 0) '+' else '−'
    val abs = abs(tenths)
    val (whole, frac) = abs / 10 to abs % 10
    return if (frac == 0) "$sign$whole" else "$sign$whole,$frac"
}

/** Значение с единицей: `−3,0 дБ`, `0,0 дБ`, `+12,0 дБ`. */
internal fun formatEqDb(tenths: Int): String {
    val sign = when {
        tenths == 0 -> ""
        tenths > 0 -> "+"
        else -> "−"
    }
    val abs = abs(tenths)
    return "$sign${abs / 10},${abs % 10} дБ"
}

/** Частота полосы: `32`, `1k`, `16k`. */
internal fun formatEqFreq(hz: Int): String {
    if (hz < 1000) return hz.toString()
    val (kilo, rest) = hz / 1000 to hz % 1000 / 100
    return if (rest == 0) "${kilo}k" else "$kilo,${rest}k"
}

// ------------------------------------------------------------------ виджеты

/**
 * Десять вертикальных ползунков с подписями (значение и частота). Колонки
 * делят ширину поровну, поэтому помещаются и на экране 360 dp. [dimmed] —
 * эквалайзер выключен: цвета приглушены, но двигать можно. [onBand] зовётся
 * при каждом изменении, [onCommit] — когда палец отпущен после изменения.
 */
@Composable
fun EqBands(
    gains: List<Int>,
    dimmed: Boolean,
    onBand: (index: Int, tenths: Int) -> Unit,
    onCommit: () -> Unit,
) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp)) {
        for (index in 0 until EqSpec.BANDS) {
            val gain = gains.getOrElse(index) { 0 }
            Column(Modifier.weight(1f), horizontalAlignment = Alignment.CenterHorizontally) {
                BandSlider(index, gain, dimmed, onBand, onCommit)
                BandLabel(formatEqValue(gain), if (dimmed) Pl.text3 else Pl.text2)
                BandLabel(formatEqFreq(EqSpec.FREQS.getOrElse(index) { 0 }), Pl.text3)
            }
        }
    }
}

/** Подпись под полосой в одну строку; шире колонки не переносится. */
@Composable
private fun BandLabel(text: String, color: androidx.compose.ui.graphics.Color) {
    PlText(
        text,
        PlType.caption,
        color,
        Modifier.wrapContentWidth(unbounded = true).padding(top = 2.dp),
        textAlign = TextAlign.Center,
        maxLines = 1,
    )
}

@Composable
private fun BandSlider(
    index: Int,
    gain: Int,
    dimmed: Boolean,
    onBand: (Int, Int) -> Unit,
    onCommit: () -> Unit,
) {
    val currentGain by rememberUpdatedState(gain)
    val currentBand by rememberUpdatedState(onBand)
    val currentCommit by rememberUpdatedState(onCommit)
    var pressed by remember { mutableStateOf(false) }
    val accent = if (dimmed) Pl.textDisabled else Pl.accent
    val handle = if (dimmed) Pl.textDisabled else Pl.text
    val description = "${formatEqFreq(EqSpec.FREQS.getOrElse(index) { 0 })} Гц: ${formatEqDb(gain)}"
    Canvas(
        Modifier
            .fillMaxWidth()
            .height(PLOT_HEIGHT)
            .semantics { contentDescription = description }
            .pointerInput(index) {
                val pad = PLOT_PAD.toPx()
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    down.consume()
                    pressed = true
                    var last = currentGain
                    var changed = false
                    fun pick(y: Float) {
                        val value = bandTenthsAt(y, pad, size.height - pad)
                        if (value != last) {
                            last = value
                            changed = true
                            currentBand(index, value)
                        }
                    }
                    pick(down.position.y)
                    while (true) {
                        val event = awaitPointerEvent()
                        val change = event.changes.firstOrNull { it.id == down.id } ?: break
                        change.consume()
                        if (!change.pressed) break
                        pick(change.position.y)
                    }
                    pressed = false
                    // Сохраняем на диск один раз, когда палец отпущен.
                    if (changed) currentCommit()
                }
            },
    ) {
        val pad = PLOT_PAD.toPx()
        val top = pad
        val bottom = size.height - pad
        val mid = (top + bottom) / 2f
        val centerX = size.width / 2f
        val line = 1.dp.toPx()
        // Сетка: +12, 0, −12 дБ; нулевая линия ярче.
        drawLine(Pl.surface3, Offset(0f, top), Offset(size.width, top), line)
        drawLine(Pl.surface3, Offset(0f, bottom), Offset(size.width, bottom), line)
        drawLine(Pl.outline, Offset(0f, mid), Offset(size.width, mid), line)
        // Дорожка и заполнение от нуля до ручки.
        val track = 4.dp.toPx()
        drawLine(Pl.surface4, Offset(centerX, top), Offset(centerX, bottom), track, StrokeCap.Round)
        val clamped = gain.coerceIn(-EqSpec.GAIN_LIMIT, EqSpec.GAIN_LIMIT)
        val y = mid - clamped / EqSpec.GAIN_LIMIT.toFloat() * (bottom - top) / 2f
        drawLine(accent, Offset(centerX, mid), Offset(centerX, y), track, StrokeCap.Round)
        drawCircle(handle, (if (pressed) 10.dp else 8.dp).toPx(), Offset(centerX, y))
    }
}

/** Горизонтальный ползунок предусилителя от −12 до +12 дБ с шагом 0,5 дБ; заполнение от нуля. */
@Composable
fun EqPreampSlider(
    tenths: Int,
    dimmed: Boolean,
    onChange: (Int) -> Unit,
    onCommit: () -> Unit,
) {
    val currentValue by rememberUpdatedState(tenths)
    val currentChange by rememberUpdatedState(onChange)
    val currentCommit by rememberUpdatedState(onCommit)
    var pressed by remember { mutableStateOf(false) }
    val accent = if (dimmed) Pl.textDisabled else Pl.accent
    val handle = if (dimmed) Pl.textDisabled else Pl.text
    Canvas(
        Modifier
            .fillMaxWidth()
            .height(44.dp)
            .semantics { contentDescription = "Предусилитель: ${formatEqDb(tenths)}" }
            .pointerInput(Unit) {
                val pad = PLOT_PAD.toPx()
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    down.consume()
                    pressed = true
                    var last = currentValue
                    var changed = false
                    fun pick(x: Float) {
                        val value = preampTenthsAt(x, pad, size.width - pad)
                        if (value != last) {
                            last = value
                            changed = true
                            currentChange(value)
                        }
                    }
                    pick(down.position.x)
                    while (true) {
                        val event = awaitPointerEvent()
                        val change = event.changes.firstOrNull { it.id == down.id } ?: break
                        change.consume()
                        if (!change.pressed) break
                        pick(change.position.x)
                    }
                    pressed = false
                    if (changed) currentCommit()
                }
            },
    ) {
        val pad = PLOT_PAD.toPx()
        val left = pad
        val right = size.width - pad
        val mid = (left + right) / 2f
        val centerY = size.height / 2f
        val track = 4.dp.toPx()
        drawLine(Pl.surface4, Offset(left, centerY), Offset(right, centerY), track, StrokeCap.Round)
        drawLine(Pl.outline, Offset(mid, centerY - 6.dp.toPx()), Offset(mid, centerY + 6.dp.toPx()), 1.dp.toPx())
        val clamped = tenths.coerceIn(-EqSpec.GAIN_LIMIT, EqSpec.GAIN_LIMIT)
        val x = mid + clamped / EqSpec.GAIN_LIMIT.toFloat() * (right - left) / 2f
        drawLine(accent, Offset(mid, centerY), Offset(x, centerY), track, StrokeCap.Round)
        drawCircle(handle, (if (pressed) 10.dp else 8.dp).toPx(), Offset(x, centerY))
    }
}
