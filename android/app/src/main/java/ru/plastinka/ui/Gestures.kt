package ru.plastinka.ui

import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.spring
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Velocity
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch

// Общие жесты: элемент идёт за пальцем, на отпускании решает расстояние или бросок,
// не дотянул — пружиной возвращается на место.

/** Доля высоты (ширины) элемента, после которой отпускание закрывает его / листает трек. */
internal const val SWIPE_DISTANCE_FRACTION = 0.25f

/** Скорость броска, с которой жест засчитывается и на малом расстоянии, dp/с. */
internal val SWIPE_FLING_VELOCITY = 1000.dp

/** Сопротивление, когда тянут туда, куда двигаться нельзя (нет следующего трека): палец идёт втрое медленнее. */
private const val BLOCKED_RESISTANCE = 0.35f

/** Предел, на который элемент уходит в сторону, куда двигаться нельзя, dp. */
private val BLOCKED_LIMIT = 56.dp

/** Минимальный путь мини-плеера вверх, после которого он открывает плеер, dp. */
internal val MINI_OPEN_DISTANCE = 40.dp

/** Скорость броска мини-плеера вверх, при которой он открывает плеер, dp/с. */
internal val MINI_OPEN_VELOCITY = 700.dp

/**
 * Вкладка сейчас на экране. Соседнюю пейджер держит (и может заранее собрать) во время
 * свайпа; её `BackHandler` не должен перехватывать «назад» у видимой вкладки.
 */
val LocalTabActive = androidx.compose.runtime.compositionLocalOf { true }

/** Смещение элемента вдоль одной оси: следует за пальцем и пружиной возвращается в 0. */
@Stable
class SwipeOffset internal constructor(private val scope: CoroutineScope) {
    var value by mutableFloatStateOf(0f)
        private set

    /** Размер элемента вдоль оси жеста; обновляется при измерении. */
    var extent by mutableFloatStateOf(1f)
        internal set

    private var job: Job? = null

    /** Сдвинуть за пальцем; результат в пределах [lo], [hi]. */
    fun dragBy(delta: Float, lo: Float, hi: Float) {
        job?.cancel()
        job = null
        value = (value + delta).coerceIn(lo, hi)
    }

    /** Вернуть на место (или в [to]) пружиной, продолжая скорость пальца. */
    fun settle(velocity: Float = 0f, to: Float = 0f) {
        job?.cancel()
        job = scope.launch {
            animate(
                initialValue = value,
                targetValue = to,
                initialVelocity = velocity,
                animationSpec = spring(dampingRatio = Spring.DampingRatioLowBouncy, stiffness = Spring.StiffnessMedium),
            ) { v, _ -> value = v }
        }
    }
}

@Composable
internal fun rememberSwipeOffset(): SwipeOffset {
    val scope = rememberCoroutineScope()
    return remember(scope) { SwipeOffset(scope) }
}

/** Жест вниз закрывает элемент: дотянули до четверти высоты или бросили быстро. */
internal fun shouldDismissDown(offset: Float, velocity: Float, extent: Float, flingPx: Float): Boolean =
    offset > 0f && (offset > extent * SWIPE_DISTANCE_FRACTION || velocity > flingPx)

/** Направление листания трека: -1 — следующий (палец влево), 1 — предыдущий, 0 — не засчитано. */
internal fun trackSwipeDirection(offset: Float, velocity: Float, extent: Float, flingPx: Float): Int = when {
    offset < 0f && (offset < -extent * SWIPE_DISTANCE_FRACTION || velocity < -flingPx) -> -1
    offset > 0f && (offset > extent * SWIPE_DISTANCE_FRACTION || velocity > flingPx) -> 1
    else -> 0
}

/**
 * Элемент едет вниз за пальцем; отпускание за порогом вызывает [onDismiss] (сдвиг
 * остаётся, закрывающая анимация продолжает движение вниз), иначе — пружина назад.
 * Вверх не двигается. Ставится первым в цепочке, до фона.
 */
@Composable
internal fun Modifier.dismissDownDraggable(state: SwipeOffset, onDismiss: () -> Unit): Modifier {
    val fling = with(LocalDensity.current) { SWIPE_FLING_VELOCITY.toPx() }
    val dragState = rememberDraggableState { delta -> state.dragBy(delta, 0f, Float.MAX_VALUE) }
    return this
        .onSizeChanged { state.extent = it.height.toFloat().coerceAtLeast(1f) }
        .graphicsLayer { translationY = state.value }
        .draggable(
            state = dragState,
            orientation = Orientation.Vertical,
            onDragStopped = { velocity ->
                if (shouldDismissDown(state.value, velocity, state.extent, fling)) {
                    onDismiss()
                } else {
                    state.settle(velocity)
                }
            },
        )
}

/**
 * Для прокручиваемого содержимого внутри [dismissDownDraggable]: остаток прокрутки
 * вниз в начале списка двигает весь экран, а пока список можно листать вверх, он листается сам.
 */
internal fun dismissDownNestedScroll(state: SwipeOffset, flingPx: Float, onDismiss: () -> Unit) =
    object : NestedScrollConnection {
        override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset {
            // Экран уже сдвинут, а палец идёт обратно вверх: сначала возвращаем экран.
            if (source == NestedScrollSource.UserInput && state.value > 0f && available.y < 0f) {
                val back = maxOf(available.y, -state.value)
                state.dragBy(back, 0f, Float.MAX_VALUE)
                return Offset(0f, back)
            }
            return Offset.Zero
        }

        override fun onPostScroll(consumed: Offset, available: Offset, source: NestedScrollSource): Offset {
            if (source == NestedScrollSource.UserInput && available.y > 0f) {
                state.dragBy(available.y, 0f, Float.MAX_VALUE)
                return Offset(0f, available.y)
            }
            return Offset.Zero
        }

        override suspend fun onPreFling(available: Velocity): Velocity {
            if (state.value <= 0f) return Velocity.Zero
            if (shouldDismissDown(state.value, available.y, state.extent, flingPx)) {
                onDismiss()
            } else {
                state.settle(available.y)
            }
            return available
        }
    }

/**
 * Горизонтальный свайп по элементу: влево — следующий трек, вправо — предыдущий.
 * Элемент едет за пальцем; куда нельзя ([canNext], [canPrevious]) — тянется туго и
 * пружинит назад. Нажатия на элементе не затрагиваются.
 */
@Composable
internal fun Modifier.trackSwipe(
    state: SwipeOffset,
    canPrevious: Boolean,
    canNext: Boolean,
    onPrevious: () -> Unit,
    onNext: () -> Unit,
): Modifier {
    val density = LocalDensity.current
    val fling = with(density) { SWIPE_FLING_VELOCITY.toPx() }
    val blocked = with(density) { BLOCKED_LIMIT.toPx() }
    val dragState = rememberDraggableState { delta ->
        val toNext = state.value + delta < 0f
        val allowed = if (toNext) canNext else canPrevious
        val step = if (allowed) delta else delta * BLOCKED_RESISTANCE
        val lo = if (canNext) -state.extent else -blocked
        val hi = if (canPrevious) state.extent else blocked
        state.dragBy(step, lo, hi)
    }
    return this
        .onSizeChanged { state.extent = it.width.toFloat().coerceAtLeast(1f) }
        .graphicsLayer { translationX = state.value }
        .draggable(
            state = dragState,
            orientation = Orientation.Horizontal,
            onDragStopped = { velocity ->
                when (trackSwipeDirection(state.value, velocity, state.extent, fling)) {
                    -1 -> if (canNext) onNext()
                    1 -> if (canPrevious) onPrevious()
                }
                state.settle(velocity)
            },
        )
}

/**
 * Мини-плеер: свайп вверх открывает полноэкранный плеер. Сам элемент не двигается,
 * жест засчитывается по пути [MINI_OPEN_DISTANCE] или броску [MINI_OPEN_VELOCITY].
 */
@Composable
internal fun Modifier.swipeUpToOpen(onOpen: () -> Unit): Modifier {
    val density = LocalDensity.current
    val distance = with(density) { MINI_OPEN_DISTANCE.toPx() }
    val fling = with(density) { MINI_OPEN_VELOCITY.toPx() }
    var total by remember { mutableFloatStateOf(0f) }
    val dragState = rememberDraggableState { delta -> total += delta }
    return draggable(
        state = dragState,
        orientation = Orientation.Vertical,
        onDragStarted = { total = 0f },
        onDragStopped = { velocity ->
            if (total < -distance || velocity < -fling) onOpen()
            total = 0f
        },
    )
}
