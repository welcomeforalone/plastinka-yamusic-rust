package ru.plastinka.ui

import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.annotation.DrawableRes
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.media3.common.Player
import coil.compose.AsyncImage
import kotlinx.coroutines.delay
import ru.plastinka.R
import ru.plastinka.data.Track
import ru.plastinka.playback.QueueHolder
import ru.plastinka.playback.currentTrack

/** Позиция и длительность в миллисекундах. */
class Progress(val positionMs: Long, val durationMs: Long) {
    val fraction: Float
        get() = if (durationMs > 0) (positionMs.toFloat() / durationMs).coerceIn(0f, 1f) else 0f
}

/** Опрос позиции плеера; длительность трека — запасной вариант, пока плеер её не знает. */
@Composable
fun rememberProgress(player: PlayerController?, track: Track?): State<Progress> =
    produceState(Progress(0, 0), player, track?.id) {
        while (true) {
            val duration = player?.durationMs?.takeIf { it > 0 } ?: track?.durationMs ?: 0L
            value = Progress(player?.positionMs ?: 0L, duration)
            delay(300)
        }
    }

/** Цвет «Моей волны» под трек; не в волне — акцент темы. */
@Composable
fun rememberTint(queue: QueueHolder.State, track: Track?): Color {
    val accent = Pl.accent
    val target = if (queue.isWave && track != null) Vibe.pick(track.vibe, accent) else accent
    val tint by animateColorAsState(target, tween(300), label = "tint")
    return tint
}

/**
 * Перемотка: дорожка 4 dp, ручка 14 dp. Во время перетаскивания двигается
 * только ползунок, `onSeek` вызывается один раз при отпускании.
 */
@Composable
fun SeekBar(
    fraction: Float,
    color: Color,
    onSeek: (Float) -> Unit,
    modifier: Modifier = Modifier,
    onPreview: (Float?) -> Unit = {},
) {
    // Жесты живут дольше одной композиции: берём свежие обработчики.
    val seek by rememberUpdatedState(onSeek)
    val previewCb by rememberUpdatedState(onPreview)
    var drag by remember { mutableStateOf<Float?>(null) }
    var widthPx by remember { mutableStateOf(1f) }
    val shown = (drag ?: fraction).coerceIn(0f, 1f)
    Box(
        modifier
            .fillMaxWidth()
            .height(32.dp)
            .pointerInput(Unit) {
                detectTapGestures { offset ->
                    seek((offset.x / widthPx).coerceIn(0f, 1f))
                }
            }
            .pointerInput(Unit) {
                detectHorizontalDragGestures(
                    onDragStart = { offset ->
                        val f = (offset.x / widthPx).coerceIn(0f, 1f)
                        drag = f
                        previewCb(f)
                    },
                    onDragEnd = {
                        drag?.let(seek)
                        drag = null
                        previewCb(null)
                    },
                    onDragCancel = {
                        drag = null
                        previewCb(null)
                    },
                ) { change, amount ->
                    change.consume()
                    val next = ((drag ?: fraction) + amount / widthPx).coerceIn(0f, 1f)
                    drag = next
                    previewCb(next)
                }
            },
        contentAlignment = Alignment.CenterStart,
    ) {
        androidx.compose.foundation.Canvas(Modifier.fillMaxWidth().height(16.dp)) {
            widthPx = size.width.coerceAtLeast(1f)
            val h = 4.dp.toPx()
            val y = size.height / 2
            drawLine(Color.White.copy(alpha = 0.18f), Offset(0f, y), Offset(size.width, y), h, StrokeCap.Round)
            drawLine(color, Offset(0f, y), Offset(size.width * shown, y), h, StrokeCap.Round)
            drawCircle(Color.White, 7.dp.toPx(), Offset(size.width * shown, y))
        }
    }
}

/** Время `м:сс` слева и справа от перемотки. */
@Composable
fun TimeRow(position: Long, duration: Long, color: Color, modifier: Modifier = Modifier) {
    val style = PlType.caption.copy(fontFeatureSettings = "tnum")
    Row(modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
        PlText(formatDuration(position), style, color)
        PlText(formatDuration(duration), style, color)
    }
}

// ------------------------------------------------------------- мини-плеер

@Composable
fun MiniPlayer(
    track: Track,
    player: PlayerController?,
    liked: Boolean,
    onLike: () -> Unit,
    onOpen: () -> Unit,
) {
    val playing = player?.isPlaying?.collectAsStateWithLifecycle()?.value ?: false
    val progress by rememberProgress(player, track)
    val shape = RoundedCornerShape(18.dp)
    Box(
        Modifier
            .padding(start = 8.dp, end = 8.dp, bottom = 8.dp)
            .fillMaxWidth()
            .height(64.dp)
            .clip(shape)
            .background(Pl.surface2, shape)
            .border(1.dp, Color(0xFF262626), shape)
            .clickable(onClick = onOpen),
    ) {
        Row(
            Modifier.fillMaxSize().padding(start = 10.dp, end = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Cover(track.coverUrl("100x100"), track.id, 44.dp, 10.dp)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
                PlText(track.title, PlType.secondaryStrong, maxLines = 1)
                PlText(track.artistNames, PlType.caption, Pl.text2, maxLines = 1)
            }
            HeartButton(liked, onLike, 22.dp)
            Spacer(Modifier.width(4.dp))
            PlayButton(40.dp, playing, onClick = { player?.toggle() })
        }
        Box(
            Modifier
                .align(Alignment.BottomStart)
                .fillMaxWidth()
                .height(2.dp)
                .background(Color.White.copy(alpha = 0.08f)),
        ) {
            Box(Modifier.fillMaxWidth(progress.fraction).fillMaxHeight().background(Pl.accent))
        }
    }
}

// ---------------------------------------------------- полноэкранный плеер

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FullPlayer(
    queue: QueueHolder.State,
    player: PlayerController?,
    model: AppViewModel,
    onClose: () -> Unit,
) {
    val track = queue.currentTrack() ?: return
    BackHandler(onBack = onClose)

    val tint = rememberTint(queue, track)
    val playing = player?.isPlaying?.collectAsStateWithLifecycle()?.value ?: false
    val shuffle = player?.shuffle?.collectAsStateWithLifecycle()?.value ?: false
    val repeat = player?.repeatMode?.collectAsStateWithLifecycle()?.value ?: Player.REPEAT_MODE_OFF
    val liked by QueueHolder.likedIds.collectAsStateWithLifecycle()
    val progress by rememberProgress(player, track)
    var preview by remember { mutableStateOf<Float?>(null) }
    var showQueue by remember { mutableStateOf(false) }

    val blurred = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
    Box(
        Modifier
            .fillMaxSize()
            .background(coverPlaceholder(track.id))
            // Плеер лежит поверх вкладок: нажатия мимо кнопок не должны
            // проваливаться в список и нижнее меню под ним.
            .pointerInput(Unit) { detectTapGestures { } },
    ) {
        AsyncImage(
            model = track.coverUrl("600x600"),
            contentDescription = null,
            contentScale = ContentScale.Crop,
            modifier = Modifier.fillMaxSize().then(if (blurred) Modifier.blur(48.dp) else Modifier),
        )
        // Затемнение плотнее там, где размытия нет.
        Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = if (blurred) 0.45f else 0.75f)))
        Box(
            Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .fillMaxHeight(0.5f)
                .background(Brush.verticalGradient(listOf(Color.Transparent, Color.Black.copy(alpha = 0.8f)))),
        )

        Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding()) {
            // Шапка.
            Box(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 8.dp)) {
                Box(
                    Modifier
                        .align(Alignment.CenterStart)
                        .size(44.dp)
                        .clip(CircleShape)
                        .clickable(onClick = onClose),
                    contentAlignment = Alignment.Center,
                ) {
                    PlIcon(R.drawable.ic_chevron_down, 28.dp, Pl.text)
                }
                Column(
                    Modifier.align(Alignment.Center).padding(horizontal = 56.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    PlText(
                        "ИГРАЕТ ИЗ",
                        PlType.micro.copy(letterSpacing = 0.08.em),
                        Color.White.copy(alpha = 0.6f),
                    )
                    PlText(queue.title, PlType.secondaryStrong, maxLines = 1)
                }
            }
            // Обложка занимает оставшееся место и ужимается.
            BoxWithConstraints(
                Modifier.weight(1f).fillMaxWidth().padding(horizontal = 32.dp, vertical = 8.dp),
                contentAlignment = Alignment.Center,
            ) {
                val side = minOf(maxWidth, maxHeight, 348.dp)
                Cover(track.coverUrl("600x600"), track.id, side, 28.dp)
            }
            Column(Modifier.padding(horizontal = 28.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        PlText(track.title, PlType.playerTitle, maxLines = 1)
                        PlText(track.artistNames, PlType.headline, Color.White.copy(alpha = 0.7f), maxLines = 1)
                    }
                    HeartButton(liked.contains(track.id), { model.toggleLike(track) }, 28.dp)
                }
                Spacer(Modifier.height(8.dp))
                SeekBar(
                    fraction = progress.fraction,
                    color = tint,
                    onSeek = { f ->
                        if (progress.durationMs > 0) player?.seekTo((f * progress.durationMs).toLong())
                    },
                    onPreview = { preview = it },
                )
                val shownPosition = preview?.let { (it * progress.durationMs).toLong() } ?: progress.positionMs
                TimeRow(shownPosition, progress.durationMs, Color.White.copy(alpha = 0.7f))
            }
            Spacer(Modifier.height(12.dp))
            // Управление: равные промежутки, поля 28.
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 28.dp),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                val modesOff = queue.isWave
                ControlIcon(
                    R.drawable.ic_shuffle,
                    22.dp,
                    when {
                        modesOff -> Pl.textDisabled
                        shuffle -> Pl.accent
                        else -> Pl.text2
                    },
                    enabled = !modesOff,
                    onClick = { player?.toggleShuffle() },
                )
                ControlIcon(R.drawable.ic_previous_filled, 34.dp, Pl.text, onClick = { player?.previous() })
                PlayButton(
                    76.dp,
                    playing,
                    onClick = { player?.toggle() },
                    color = tint,
                    onColor = onColorFor(tint),
                )
                ControlIcon(R.drawable.ic_next_filled, 34.dp, Pl.text, onClick = { player?.next() })
                ControlIcon(
                    if (repeat == Player.REPEAT_MODE_ONE) R.drawable.ic_repeat_one else R.drawable.ic_repeat,
                    22.dp,
                    when {
                        modesOff -> Pl.textDisabled
                        repeat != Player.REPEAT_MODE_OFF -> Pl.accent
                        else -> Pl.text2
                    },
                    enabled = !modesOff,
                    onClick = { player?.cycleRepeat() },
                )
            }
            Spacer(Modifier.height(16.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center) {
                Row(
                    Modifier
                        .height(44.dp)
                        .clip(CircleShape)
                        .background(Color.White.copy(alpha = 0.12f), CircleShape)
                        .clickable { showQueue = true }
                        .padding(horizontal = 18.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    PlIcon(R.drawable.ic_queue, 18.dp, Pl.text)
                    PlText("Очередь", PlType.secondaryStrong, Pl.text)
                }
            }
            Spacer(Modifier.height(16.dp))
        }
    }

    if (showQueue) {
        QueueSheet(queue, player, onDismiss = { showQueue = false })
    }
}

/** Значок управления с зоной нажатия не меньше 44 dp. */
@Composable
private fun ControlIcon(
    @DrawableRes icon: Int,
    size: Dp,
    tint: Color,
    onClick: () -> Unit,
    enabled: Boolean = true,
) {
    Box(
        Modifier
            .size(maxOf(44.dp, size))
            .clip(CircleShape)
            .then(if (enabled) Modifier.clickable(onClick = onClick) else Modifier),
        contentAlignment = Alignment.Center,
    ) {
        PlIcon(icon, size, tint)
    }
}

// ----------------------------------------------------------------- очередь

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun QueueSheet(queue: QueueHolder.State, player: PlayerController?, onDismiss: () -> Unit) {
    val playing = player?.isPlaying?.collectAsStateWithLifecycle()?.value ?: false
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        containerColor = Pl.surface1,
        shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
        dragHandle = {
            Box(
                Modifier
                    .padding(top = 10.dp, bottom = 8.dp)
                    .size(width = 40.dp, height = 4.dp)
                    .background(Pl.surface4, RoundedCornerShape(2.dp)),
            )
        },
    ) {
        val current = queue.current
        val nextFrom = (current + 1).coerceAtLeast(0)
        val upcoming = if (nextFrom < queue.tracks.size) nextFrom until queue.tracks.size else IntRange.EMPTY
        LazyColumn(Modifier.fillMaxWidth().navigationBarsPadding()) {
            item {
                Column(Modifier.padding(start = 20.dp, end = 20.dp, bottom = 18.dp)) {
                    PlText("Очередь", PlType.title)
                    PlText(
                        "${queue.title} · ${tracksCount(queue.tracks.size)}",
                        PlType.caption.copy(fontFeatureSettings = "tnum"),
                        Pl.text3,
                    )
                }
            }
            queue.currentTrack()?.let { now ->
                item {
                    PlText("СЕЙЧАС ИГРАЕТ", PlType.micro, Pl.text3, Modifier.padding(horizontal = 20.dp))
                    Spacer(Modifier.height(6.dp))
                    QueueRow(
                        now,
                        current = true,
                        soundOn = playing,
                        onClick = {},
                        modifier = Modifier
                            .padding(horizontal = 8.dp)
                            .clip(RoundedCornerShape(14.dp))
                            .background(Pl.surface3),
                    )
                    Spacer(Modifier.height(16.dp))
                }
            }
            if (!upcoming.isEmpty()) {
                item {
                    PlText("ДАЛЕЕ", PlType.micro, Pl.text3, Modifier.padding(horizontal = 20.dp))
                    Spacer(Modifier.height(2.dp))
                }
                items(upcoming.toList(), key = { it }) { index ->
                    queue.tracks.getOrNull(index)?.let { item ->
                        QueueRow(item, current = false, soundOn = false, onClick = {
                            player?.seekTo(index, 0)
                        })
                    }
                }
            }
        }
    }
}

@Composable
private fun QueueRow(
    track: Track,
    current: Boolean,
    soundOn: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier
            .fillMaxWidth()
            .height(64.dp)
            .then(if (!current) Modifier.clickable(onClick = onClick) else Modifier)
            .padding(horizontal = if (current) 8.dp else 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) {
            Cover(track.coverUrl("100x100"), track.id, 48.dp, 8.dp)
            if (current) {
                Box(
                    Modifier
                        .size(48.dp)
                        .clip(RoundedCornerShape(8.dp))
                        .background(Color.Black.copy(alpha = 0.6f)),
                )
                EqBars(24.dp, Pl.accent, soundOn)
            }
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            PlText(
                track.title,
                if (current) PlType.bodyStrong else PlType.body,
                if (current) Pl.accent else Pl.text,
                maxLines = 1,
            )
            PlText(track.artistNames, PlType.secondary, Pl.text2, maxLines = 1)
        }
        track.durationMs?.let {
            Spacer(Modifier.width(8.dp))
            PlText(
                formatDuration(it),
                PlType.caption.copy(fontFeatureSettings = "tnum"),
                Pl.text3,
                textAlign = TextAlign.End,
            )
        }
    }
}
