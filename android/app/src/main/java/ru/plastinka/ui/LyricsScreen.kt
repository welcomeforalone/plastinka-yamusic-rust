package ru.plastinka.ui

import android.os.SystemClock
import androidx.activity.compose.BackHandler
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.animateScrollBy
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import ru.plastinka.PlastinkaApp
import ru.plastinka.R
import ru.plastinka.data.LrcParser
import ru.plastinka.data.LyricLine
import ru.plastinka.data.Lyrics
import ru.plastinka.data.LyricsException
import ru.plastinka.data.LyricsResult
import ru.plastinka.data.Track
import ru.plastinka.playback.QueueHolder

/** Сколько ждать после того, как пользователь отпустил список, прежде чем вернуть автопрокрутку. */
private const val RESUME_AFTER_TOUCH_MS = 3000L

/** Что показывает экран текста. */
private sealed interface LyricsUi {
    data object Loading : LyricsUi
    data class Loaded(val result: LyricsResult) : LyricsUi
    data class Failed(val message: String) : LyricsUi
}

/**
 * Экран текста песни поверх плеера (макет 09). Системное «назад» и стрелка
 * вниз закрывают его. Текст грузится для текущего трека; при смене трека
 * загрузка прежнего отменяется, его ответ не показывается.
 */
@Composable
internal fun LyricsScreen(
    queue: QueueHolder.State,
    track: Track,
    player: PlayerController?,
    liked: Boolean,
    onLike: () -> Unit,
    onClose: () -> Unit,
    // Выключается на время анимации закрытия: уходящий экран не перехватывает «назад».
    backEnabled: Boolean = true,
) {
    BackHandler(enabled = backEnabled, onBack = onClose)
    val links = rememberTrackLinks(track)
    val context = LocalContext.current
    val repo = (context.applicationContext as PlastinkaApp).lyrics
    var attempt by remember { mutableIntStateOf(0) }
    // Состояние привязано к треку и попытке: смена трека сбрасывает его сразу,
    // без кадра со старым текстом.
    var state by remember(track.id, attempt) {
        mutableStateOf<LyricsUi>(repo.cached(track.id)?.let { LyricsUi.Loaded(it) } ?: LyricsUi.Loading)
    }
    LaunchedEffect(track.id, attempt) {
        if (state !is LyricsUi.Loading) return@LaunchedEffect
        state = try {
            // У недоступного трека с копией текст Яндекса берётся у копии (как `lyrics_source` на десктопе).
            val copyId = (context.applicationContext as PlastinkaApp).relink.playingCopy(track)?.id
            LyricsUi.Loaded(repo.load(track, copyId ?: track.id))
        } catch (e: CancellationException) {
            throw e
        } catch (e: LyricsException) {
            LyricsUi.Failed(e.message ?: "Не удалось загрузить текст")
        }
    }
    val progress by rememberProgress(player, track)
    // Свайп вниз закрывает текст (не весь плеер): за пальцем едет сам экран текста.
    val dismiss = rememberSwipeOffset()
    val fling = with(LocalDensity.current) { SWIPE_FLING_VELOCITY.toPx() }
    val dismissScroll = remember(dismiss, fling) { dismissDownNestedScroll(dismiss, fling, onClose) }

    Box(
        Modifier
            .dismissDownDraggable(dismiss, onClose)
            .nestedScroll(dismissScroll)
            .fillMaxSize()
            .background(Color.Black)
            .pointerInput(Unit) { detectTapGestures { } },
    ) {
        // Единый фон под текстом и под кнопками (макет 09): затемнение 60 % и общий низ.
        CoverBackdrop(track, scrim = 0.6f)
        Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding()) {
            // Шапка (макет: отступ 8 сверху, стрелка 28 на x 18, обложка 44, сердце 24 на краю
            // поля 20): стрелка, мини-обложка, название и исполнитель, сердце.
            Row(
                Modifier.fillMaxWidth().padding(top = 8.dp).height(44.dp).padding(start = 10.dp, end = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(
                    Modifier.size(44.dp).clip(CircleShape).clickable(onClick = onClose),
                    contentAlignment = Alignment.Center,
                ) {
                    PlIcon(R.drawable.ic_chevron_down, 28.dp, Pl.text)
                }
                Spacer(Modifier.width(4.dp))
                Cover(track.coverUrl("100x100"), track.id, 44.dp, 10.dp)
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
                    PlText(track.displayTitle, PlType.bodyStrong, modifier = links.title, maxLines = 1)
                    PlText(track.artistNames, PlType.secondary, Color.White.copy(alpha = 0.7f), links.artists, maxLines = 1)
                }
                HeartButton(liked, onLike, 24.dp)
            }

            // Строки текста; у нижнего края они растворяются маской прозрачности самих
            // строк, а не плашкой поверх фона: фон под текстом и кнопками един.
            Box(Modifier.weight(1f).fillMaxWidth().fadeBottom(LYRICS_FADE)) {
                when (val ui = state) {
                    LyricsUi.Loading -> Box(Modifier.align(Alignment.Center)) { LoadingBox() }
                    is LyricsUi.Failed -> Box(Modifier.align(Alignment.Center)) {
                        ErrorState(ui.message) { attempt++ }
                    }
                    is LyricsUi.Loaded -> when (val result = ui.result) {
                        is LyricsResult.Found -> key(track.id) {
                            val lyrics = result.lyrics
                            val synced = lyrics.synced
                            if (synced != null) {
                                SyncedLyrics(synced, progress.positionMs) { ms -> player?.seekTo(ms) }
                            } else {
                                PlainLyrics(lyrics.plain.orEmpty())
                            }
                        }
                        LyricsResult.Instrumental -> Message("Без слов")
                        LyricsResult.Missing -> Message("Текста нет")
                    }
                }
            }

            val found = ((state as? LyricsUi.Loaded)?.result as? LyricsResult.Found)?.lyrics
            Column(Modifier.padding(horizontal = 28.dp)) {
                // Строка подписи держит высоту всегда: вёрстка не прыгает при загрузке.
                // 24 dp до зоны перемотки: дорожка ниже подписи на 32 по макету.
                Box(Modifier.height(24.dp), contentAlignment = Alignment.TopStart) {
                    found?.let { PlText(sourceLabel(it), PlType.caption, Color.White.copy(alpha = 0.5f)) }
                }
                SeekSection(progress, player)
            }
            Spacer(Modifier.height(12.dp))
            // Кнопки всегда цвета акцента: VibeColorizer их не красит.
            PlayerControls(queue, player, playSize = 60.dp, skipSize = 28.dp)
            Spacer(Modifier.height(35.dp))
        }
    }
}

/** Высота растворения строк у нижнего края (макет: затухание 200 dp до подписи источника). */
private val LYRICS_FADE = 160.dp

/** Сколько места между шапкой и первой строкой (макет: шапка до y 92, строки с y 150). */
private val LYRICS_TOP = 58.dp

/**
 * Нижний край содержимого плавно уходит в прозрачность (до 10 %): слой
 * рисуется отдельно, поверх него маска `DstIn`. Фон за ним не затемняется.
 */
private fun Modifier.fadeBottom(height: Dp): Modifier = this
    .graphicsLayer { compositingStrategy = CompositingStrategy.Offscreen }
    .drawWithContent {
        drawContent()
        val fade = height.toPx().coerceAtMost(size.height)
        drawRect(
            brush = Brush.verticalGradient(
                colors = listOf(Color.Black, Color.Black.copy(alpha = 0.1f)),
                startY = size.height - fade,
                endY = size.height,
            ),
            topLeft = Offset(0f, size.height - fade),
            size = Size(size.width, fade),
            blendMode = BlendMode.DstIn,
        )
    }

/** «Текст: Яндекс · синхронный». */
private fun sourceLabel(lyrics: Lyrics): String =
    "Текст: ${lyrics.source.title} · " + if (lyrics.synced != null) "синхронный" else "без синхронизации"

/** Пустое состояние по центру. */
@Composable
private fun Message(text: String) {
    Box(Modifier.fillMaxSize().padding(horizontal = 32.dp), contentAlignment = Alignment.Center) {
        PlText(text, PlType.headline, Color.White.copy(alpha = 0.6f), textAlign = TextAlign.Center)
    }
}

/** Несинхронный текст: прокручиваемый, без подсветки. */
@Composable
private fun PlainLyrics(text: String) {
    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(start = 28.dp, end = 28.dp, top = LYRICS_TOP, bottom = 48.dp),
    ) {
        PlText(text, PlType.lyrics, Color.White.copy(alpha = 0.85f))
    }
}

/**
 * Синхронный текст: текущая строка яркая, остальные приглушены; список плавно
 * подводит её к верхней трети экрана, нажатие на строку перематывает.
 *
 * Пока пользователь держит или листает список, автопрокрутка молчит; через
 * [RESUME_AFTER_TOUCH_MS] после того, как он убрал палец (и затих инерционный
 * скролл), текущая строка возвращается на место. Смена строки в это время не
 * прерывает листание, но возврат уже идёт к новой строке.
 */
@Composable
private fun SyncedLyrics(lines: List<LyricLine>, positionMs: Long, onSeek: (Long) -> Unit) {
    val listState = rememberLazyListState()
    var viewport by remember { mutableIntStateOf(0) }
    var touching by remember { mutableStateOf(false) }
    var touchEnd by remember { mutableLongStateOf(0L) }
    val active = LrcParser.activeLine(lines, positionMs) ?: -1

    LaunchedEffect(active, touching, touchEnd, viewport) {
        if (viewport <= 0 || active < 0 || touching) return@LaunchedEffect
        val wait = touchEnd + RESUME_AFTER_TOUCH_MS - SystemClock.uptimeMillis()
        if (wait > 0) delay(wait)
        while (listState.isScrollInProgress) delay(100)
        listState.scrollLineTo(active, viewport / 3)
    }

    val bottomPadding = with(LocalDensity.current) { (viewport * 0.6f).toDp() }
    LazyColumn(
        state = listState,
        modifier = Modifier
            .fillMaxSize()
            .onSizeChanged { viewport = it.height }
            // Следим за пальцем, ничего не перехватывая.
            .pointerInput(Unit) {
                awaitPointerEventScope {
                    while (true) {
                        val pressed = awaitPointerEvent(PointerEventPass.Initial).changes.any { it.pressed }
                        if (pressed != touching) {
                            touching = pressed
                            if (!pressed) touchEnd = SystemClock.uptimeMillis()
                        }
                    }
                }
            },
        contentPadding = androidx.compose.foundation.layout.PaddingValues(
            start = 28.dp,
            end = 28.dp,
            top = LYRICS_TOP,
            bottom = bottomPadding,
        ),
        verticalArrangement = Arrangement.spacedBy(18.dp),
    ) {
        itemsIndexed(lines) { index, line ->
            if (line.text.isEmpty()) {
                // Пауза: просто зазор.
                Spacer(Modifier.height(6.dp))
            } else {
                val target = when {
                    index == active -> Color.White
                    index < active -> Color.White.copy(alpha = 0.35f)
                    else -> Color.White.copy(alpha = 0.55f)
                }
                val color by animateColorAsState(target, tween(250), label = "lyric-line")
                PlText(
                    line.text,
                    PlType.lyrics,
                    color,
                    Modifier
                        .fillMaxWidth()
                        .clickable(
                            interactionSource = remember { MutableInteractionSource() },
                            indication = null,
                        ) {
                            // Явный выбор строки: автопрокрутка не ждёт паузы после касания.
                            touchEnd = 0L
                            onSeek(line.timeMs)
                        },
                )
            }
        }
    }
}

/** Подвести строку `index` к высоте `targetPx` от верха списка. */
private suspend fun LazyListState.scrollLineTo(index: Int, targetPx: Int) {
    var info = layoutInfo.visibleItemsInfo.firstOrNull { it.index == index }
    if (info == null) {
        // Строки нет на экране: сначала доводим её в видимую область.
        animateScrollToItem(index)
        info = layoutInfo.visibleItemsInfo.firstOrNull { it.index == index } ?: return
    }
    animateScrollBy((info.offset - targetPx).toFloat(), tween(450))
}
