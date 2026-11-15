package ru.plastinka.ui

import androidx.compose.foundation.Canvas
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.awaitCancellation
import ru.plastinka.playback.SpectrumHub

/**
 * Столбики спектра играющего звука. Пока экран на виду (STARTED), держим
 * подписку на [SpectrumHub]: без зрителей анализ не работает. Значения
 * читаются в фазе рисования, поэтому на кадре перерисовывается только Canvas,
 * а не весь плеер.
 */
@Composable
fun SpectrumBars(
    color: Color,
    modifier: Modifier = Modifier,
    barWidth: Dp = 5.dp,
    gap: Dp = 4.dp,
    /** Пока звука нет, рисовать неподвижный образец (предпросмотр в настройках). */
    idleDemo: Boolean = false,
) {
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(lifecycle) {
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            SpectrumHub.acquire()
            try {
                awaitCancellation()
            } finally {
                SpectrumHub.release()
            }
        }
    }
    Canvas(modifier) {
        // Чтение состояния внутри рисования: меняется только эта фаза.
        SpectrumHub.tick.longValue
        val live = SpectrumHub.levels
        val count = live.size
        if (count == 0 || size.width <= 0f) return@Canvas
        // Визуализатор выключен в настройках: ничего не рисуем, даже столбики покоя.
        if (!SpectrumHub.enabled) return@Canvas
        val levels = if (idleDemo && live.all { it <= 0.001f }) demoLevels(count) else live
        // Столбики делят ширину поровну; доля промежутка — как в макете.
        val gapShare = gap.toPx() / (barWidth.toPx() + gap.toPx())
        val pitch = size.width / count
        val bar = pitch * (1f - gapShare)
        val radius = CornerRadius(bar / 2f, bar / 2f)
        for (i in 0 until count) {
            val level = levels[i]
            // Минимум — столбик-«точка» шириной в столбик: в покое они видны, как на макете.
            val h = (level * size.height).coerceAtLeast(bar)
            drawRoundRect(
                color,
                Offset(i * pitch + (pitch - bar) / 2f, size.height - h),
                Size(bar, h),
                radius,
            )
        }
    }
}

/** Неподвижный образец столбиков: плавные холмы без случайности, как на макете. */
private fun demoLevels(count: Int): FloatArray =
    FloatArray(count) { i ->
        val x = i.toFloat()
        val wave = 0.5f + 0.25f * kotlin.math.sin(x * 0.55f) + 0.2f * kotlin.math.sin(x * 1.7f + 1f)
        val tilt = 1f - 0.5f * kotlin.math.abs(x / count - 0.35f)
        (wave * tilt).coerceIn(0.08f, 0.95f)
    }
