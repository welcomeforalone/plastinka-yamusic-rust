package ru.plastinka.ui

import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.annotation.DrawableRes
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.EnterExitState
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.core.tween
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.basicMarquee
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.SheetState
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
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
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.media3.common.Player
import coil.compose.AsyncImage
import kotlinx.coroutines.delay
import ru.plastinka.R
import ru.plastinka.data.FckCensor
import ru.plastinka.data.Prefs
import ru.plastinka.data.Track
import ru.plastinka.playback.QualityHolder
import ru.plastinka.playback.QueueHolder
import ru.plastinka.playback.currentTrack
import ru.plastinka.playback.mergeRecent

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

/**
 * Цвет VibeColorizer под играющий трек при любом источнике (волна, «Мне
 * нравится», альбом, плейлист, поиск, очередь). У трека волны цвет из ответа
 * сервиса, иначе средний цвет большой обложки; окраска выключена в настройках —
 * акцент темы. Красит фон, кольца и подпись экрана «Моя волна» и спектр под
 * плеером: кнопки, перемотка, сердце и выбранные значки всегда цвета акцента
 * ([Pl.accent]), как на десктопе. Смена цвета плавная, 300 мс.
 */
@Composable
fun rememberTint(queue: QueueHolder.State, track: Track?): Color {
    val accent = Pl.accent
    val enabled = Prefs.vibeColorizer && track != null
    // Цвет из ответа волны есть только у треков самой волны.
    val own = if (enabled && queue.isWave) track?.vibe else null
    // Обложку считаем, только если окраска включена и своего цвета у трека нет.
    val coverUrl = track?.coverUrl("600x600")?.takeIf { enabled && !Vibe.hasOwnColor(own) }
    val cover by rememberCoverColor(coverUrl)
    val target = if (enabled) Vibe.pick(own, cover, accent) else accent
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
            // Тень ползунка (макет: 0 2 6, чёрный 40 %) и сам ползунок 14 dp.
            drawCircle(Color.Black.copy(alpha = 0.25f), 8.5.dp.toPx(), Offset(size.width * shown, y + 2.dp.toPx()))
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
    val queue by QueueHolder.state.collectAsStateWithLifecycle()
    // VibeColorizer красит только спектр; полоса прогресса, сердце и кнопка — акцент.
    val tint = rememberTint(queue, track)
    val shape = RoundedCornerShape(18.dp)
    Box(
        Modifier
            .fillMaxWidth()
            .drawBehind {
                // Затухание списка под плеером (макет: 56 dp над карточкой и вся зона под ней).
                val top = -56.dp.toPx()
                drawRect(
                    Brush.verticalGradient(listOf(Color.Transparent, Color.Black), startY = top, endY = size.height),
                    topLeft = Offset(0f, top),
                    size = Size(size.width, size.height - top),
                )
            }
            .padding(start = 8.dp, end = 8.dp, bottom = 8.dp),
    ) {
        Box(
            Modifier
                .fillMaxWidth()
                .height(64.dp)
                .shadow(
                    12.dp,
                    shape,
                    clip = false,
                    ambientColor = Color.Transparent,
                    spotColor = Color.Black.copy(alpha = 0.5f),
                )
                .clip(shape)
                .background(Pl.surface2, shape)
                .border(1.dp, Color(0xFF262626), shape)
                .clickable(onClick = onOpen),
        ) {
            // Приглушённые столбики за содержимым (макет: нижние 34 dp карточки, цвет 18 %).
            SpectrumBars(
                tint.copy(alpha = 0.18f),
                Modifier.align(Alignment.BottomStart).fillMaxWidth().height(34.dp),
                barWidth = 4.dp,
                gap = 3.dp,
            )
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
                // Зона нажатия сердца 40 dp: по 3 dp слева и справа дают зазор 12 dp, как в макете.
                Spacer(Modifier.width(3.dp))
                HeartButton(liked, onLike, 22.dp)
                Spacer(Modifier.width(3.dp))
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
}

// ---------------------------------------------------- полноэкранный плеер

/** Высота места под визуализатор спектра внизу плеера (макет 08: 66 dp). */
private val VISUALIZER_HEIGHT = 66.dp

/** Доля высоты экрана, с которой начинается затемнение низа (макет: y 420 из 915). */
private const val BACKDROP_BOTTOM_FRACTION = (915f - 420f) / 915f

/**
 * Фон полноэкранного плеера и экрана текста (макеты 08 и 09): размытая обложка
 * на весь экран, затемнение `scrim` (0,45 у плеера, 0,6 у текста) и градиент
 * «прозрачный → чёрный 80 %» по нижним 54 % высоты. Настройка «Размытая
 * обложка» выключена — вместо картинки спокойный тёмный градиент, низ тот же.
 */
@Composable
internal fun CoverBackdrop(track: Track, scrim: Float) {
    Box(Modifier.fillMaxSize()) {
        if (!Prefs.blurCover) {
            Box(Modifier.fillMaxSize().background(Brush.verticalGradient(listOf(Pl.surface3, Pl.bg))))
        } else {
            val blurred = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
            Box(Modifier.fillMaxSize().background(coverPlaceholder(track.id)))
            AsyncImage(
                model = track.coverUrl("600x600"),
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxSize().then(if (blurred) Modifier.blur(48.dp) else Modifier),
            )
            // Затемнение плотнее там, где размытия нет.
            Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = if (blurred) scrim else scrim + 0.3f)))
        }
        Box(
            Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .fillMaxHeight(BACKDROP_BOTTOM_FRACTION)
                .background(Brush.verticalGradient(listOf(Color.Transparent, Color.Black.copy(alpha = 0.8f)))),
        )
    }
}

/**
 * Перемотка и время под ней. Дорожка занимает 16 dp в центре зоны нажатия 32 dp;
 * время поднято на 4 dp (отступ дорожка → время по макету), поэтому ниже
 * ставим отступ меньше на столько же.
 */
@Composable
internal fun SeekSection(progress: Progress, player: PlayerController?) {
    var preview by remember { mutableStateOf<Float?>(null) }
    SeekBar(
        fraction = progress.fraction,
        color = Pl.accent,
        onSeek = { f ->
            if (progress.durationMs > 0) player?.seekTo((f * progress.durationMs).toLong())
        },
        onPreview = { preview = it },
    )
    val shownPosition = preview?.let { (it * progress.durationMs).toLong() } ?: progress.positionMs
    TimeRow(
        shownPosition,
        progress.durationMs,
        Color.White.copy(alpha = 0.7f),
        Modifier.offset(y = (-4).dp),
    )
}

/**
 * Управление: перемешать, назад, пуск/пауза, вперёд, повтор. Значки стоят как
 * в макете (равные промежутки между значками, поля 28), но зоны нажатия не
 * меньше 44 dp, поэтому промежутки между зонами пересчитаны. Всё это — всегда
 * цвет акцента, VibeColorizer кнопки не красит. В «Моей волне» перемешивание
 * выключено, а повтор переключается «выкл ↔ трек».
 */
@Composable
internal fun PlayerControls(
    queue: QueueHolder.State,
    player: PlayerController?,
    playSize: Dp,
    skipSize: Dp,
) {
    val playing = player?.isPlaying?.collectAsStateWithLifecycle()?.value ?: false
    val shuffle = player?.shuffle?.collectAsStateWithLifecycle()?.value ?: false
    val repeat = player?.repeatMode?.collectAsStateWithLifecycle()?.value ?: Player.REPEAT_MODE_OFF
    // В волне перемешивания нет, а повтор переключается «выкл ↔ трек».
    val shuffleOff = queue.isWave
    val edge = 22.dp
    val touch = 44.dp
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        // Промежуток между значками по макету: пять значков, поля 28 по бокам.
        val gap = (maxWidth - 56.dp - edge * 2 - skipSize * 2 - playSize) / 4
        // Между зонами нажатия: крайняя зона шире значка на 11 dp с каждой стороны.
        val outer = (gap + skipSize / 2 - 33.dp).coerceAtLeast(0.dp)
        val inner = (gap + skipSize / 2 - 22.dp).coerceAtLeast(0.dp)
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 28.dp - (touch - edge) / 2),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            ControlIcon(
                R.drawable.ic_shuffle,
                edge,
                when {
                    shuffleOff -> Pl.textDisabled
                    shuffle -> Pl.accent
                    else -> Pl.text2
                },
                enabled = !shuffleOff,
                onClick = { player?.toggleShuffle() },
            )
            Spacer(Modifier.width(outer))
            ControlIcon(R.drawable.ic_previous_filled, skipSize, Pl.text, onClick = { player?.previous() })
            Spacer(Modifier.width(inner))
            PlayButton(
                playSize,
                playing,
                onClick = { player?.toggle() },
                modifier = Modifier.shadow(
                    16.dp,
                    CircleShape,
                    clip = false,
                    ambientColor = Color.Transparent,
                    spotColor = Pl.accent.copy(alpha = 0.6f),
                ),
                color = Pl.accent,
                onColor = Pl.onAccent,
            )
            Spacer(Modifier.width(inner))
            ControlIcon(R.drawable.ic_next_filled, skipSize, Pl.text, onClick = { player?.next() })
            Spacer(Modifier.width(outer))
            ControlIcon(
                if (repeat == Player.REPEAT_MODE_ONE) R.drawable.ic_repeat_one else R.drawable.ic_repeat,
                edge,
                if (repeat != Player.REPEAT_MODE_OFF) Pl.accent else Pl.text2,
                onClick = { if (queue.isWave) player?.toggleWaveRepeat() else player?.cycleRepeat() },
            )
        }
    }
}

/** «Таблетка» на стекле под плеером: «Текст», «Очередь» (макет: 44 dp, поля 14 и 16, значок 18). */
@Composable
private fun GlassPill(label: String, @DrawableRes icon: Int, onClick: () -> Unit) {
    Row(
        Modifier
            .height(44.dp)
            .clip(CircleShape)
            .background(Color.White.copy(alpha = 0.12f), CircleShape)
            .clickable(onClick = onClick)
            .padding(start = 14.dp, end = 16.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        PlIcon(icon, 18.dp, Pl.text)
        PlText(label, PlType.secondaryStrong, Pl.text)
    }
}

/** Круглая кнопка на стекле без подписи, 44 dp, как «таблетки» рядом. */
@Composable
private fun GlassIconButton(@DrawableRes icon: Int, onClick: () -> Unit) {
    Box(
        Modifier
            .size(44.dp)
            .clip(CircleShape)
            .background(Color.White.copy(alpha = 0.12f), CircleShape)
            .semantics { contentDescription = "Мини-плеер" }
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        PlIcon(icon, 20.dp, Pl.text)
    }
}

/** Значок качества потока, например «MP3 · 320»; нажатие открывает «Настройки звука». */
@Composable
private fun QualityBadge(label: String, onClick: () -> Unit) {
    val shape = RoundedCornerShape(6.dp)
    Box(
        Modifier
            .height(22.dp)
            .clip(shape)
            .background(Color.White.copy(alpha = 0.12f), shape)
            .clickable(onClick = onClick)
            .padding(horizontal = 8.dp),
        contentAlignment = Alignment.Center,
    ) {
        PlText(label, PlType.micro.copy(fontFeatureSettings = "tnum"), Pl.text, maxLines = 1)
    }
}

/**
 * Однострочный текст, который не помещается по ширине, уезжает бегущей строкой
 * (как в десктопном плеере: пауза 2 с, 40 dp/с), а не обрезается.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun MarqueeText(text: String, style: TextStyle, color: Color) {
    Text(
        text = text,
        style = style,
        color = color,
        maxLines = 1,
        softWrap = false,
        overflow = TextOverflow.Clip,
        modifier = Modifier.basicMarquee(
            iterations = Int.MAX_VALUE,
            repeatDelayMillis = 2000,
            initialDelayMillis = 2000,
            velocity = 40.dp,
        ),
    )
}

@Composable
fun FullPlayer(
    queue: QueueHolder.State,
    player: PlayerController?,
    model: AppViewModel,
    onClose: () -> Unit,
    // Выключается на время анимации закрытия: уходящий плеер не перехватывает «назад».
    backEnabled: Boolean = true,
    // Кнопка «Мини-плеер» (PiP); `null` — режим на устройстве не поддерживается.
    onMiniPlayer: (() -> Unit)? = null,
) {
    val track = queue.currentTrack() ?: return
    BackHandler(enabled = backEnabled, onBack = onClose)

    // VibeColorizer красит здесь только спектр; кнопки, перемотка и значки — акцент.
    val tint = rememberTint(queue, track)
    val liked by QueueHolder.likedIds.collectAsStateWithLifecycle()
    val qualities by QualityHolder.state.collectAsStateWithLifecycle()
    val progress by rememberProgress(player, track)
    var showQueue by remember { mutableStateOf(false) }
    var showLyrics by remember { mutableStateOf(false) }
    var showSound by remember { mutableStateOf(false) }
    // Плеер закрывается: листы и текст не должны пережить его, если его откроют снова.
    LaunchedEffect(backEnabled) {
        if (!backEnabled) {
            showQueue = false
            showLyrics = false
            showSound = false
        }
    }

    Box(
        Modifier
            .fillMaxSize()
            .background(Color.Black)
            // Плеер лежит поверх вкладок: нажатия мимо кнопок не должны
            // проваливаться в список и нижнее меню под ним.
            .pointerInput(Unit) { detectTapGestures { } },
    ) {
        CoverBackdrop(track, scrim = 0.45f)

        // Спектр внизу во всю ширину (макет: 66 dp, цвет акцента 40 %), над полосой жестов.
        SpectrumBars(
            tint.copy(alpha = 0.4f),
            Modifier
                .align(Alignment.BottomCenter)
                .navigationBarsPadding()
                .fillMaxWidth()
                .height(VISUALIZER_HEIGHT),
        )

        Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding()) {
            // Шапка 48 dp: стрелка слева, «ИГРАЕТ ИЗ» и источник по центру, «…» справа.
            // Кнопка «…» есть, только если у трека есть действия (включён плагин).
            Box(Modifier.fillMaxWidth().height(48.dp).padding(start = 10.dp, end = 12.dp)) {
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
                TrackMoreButton(
                    track,
                    Modifier.align(Alignment.CenterEnd),
                    iconSize = 24.dp,
                    tint = Pl.text,
                )
                Column(
                    Modifier.align(Alignment.Center).padding(horizontal = 56.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    PlText(
                        "ИГРАЕТ ИЗ",
                        PlType.micro.copy(letterSpacing = 0.08.em),
                        Color.White.copy(alpha = 0.6f),
                    )
                    Spacer(Modifier.height(1.dp))
                    PlText(queue.title, PlType.secondaryStrong, maxLines = 1)
                }
            }
            // Обложка: 24 dp под шапкой и 30 dp над названием, по ширине поля 32;
            // на низком экране ужимается.
            BoxWithConstraints(
                Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .padding(start = 32.dp, end = 32.dp, top = 24.dp, bottom = 30.dp),
                contentAlignment = Alignment.Center,
            ) {
                val side = minOf(maxWidth, maxHeight)
                Cover(
                    track.coverUrl("600x600"),
                    track.id,
                    side,
                    28.dp,
                    Modifier.shadow(
                        32.dp,
                        RoundedCornerShape(28.dp),
                        clip = false,
                        ambientColor = Color.Transparent,
                        spotColor = Color.Black.copy(alpha = 0.5f),
                    ),
                )
            }
            Column(Modifier.padding(horizontal = 28.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        MarqueeText(track.title, PlType.playerTitle, Pl.text)
                        MarqueeText(track.artistNames, PlType.headline, Color.White.copy(alpha = 0.7f))
                    }
                    Spacer(Modifier.width(4.dp))
                    // Зона нажатия 40 dp шире значка 28: сдвиг, чтобы значок встал у поля 28.
                    HeartButton(liked.contains(track.id), { model.toggleLike(track) }, 28.dp, Modifier.offset(x = 6.dp))
                }
                Spacer(Modifier.height(12.dp))
                // Место под значок держим всегда, чтобы вёрстка не прыгала, пока качество неизвестно.
                Row(
                    Modifier.height(22.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    qualities[track.id]?.label?.let { QualityBadge(it) { showSound = true } }
                    if (rememberIsAi(track)) AiTag(color = Pl.text)
                    if (FckCensor.hasReplacement(track.id)) ReplacedTag()
                }
                // Зона перемотки начинается на 8 dp выше дорожки: 18 по макету минус 8.
                Spacer(Modifier.height(10.dp))
                SeekSection(progress, player)
            }
            Spacer(Modifier.height(12.dp))
            PlayerControls(queue, player, playSize = 76.dp, skipSize = 34.dp)
            Spacer(Modifier.height(32.dp))
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(10.dp, Alignment.CenterHorizontally),
            ) {
                GlassPill("Текст", R.drawable.ic_lyrics) { showLyrics = true }
                GlassPill("Очередь", R.drawable.ic_queue) { showQueue = true }
                // Три таблетки с подписями не влезают на узкий экран: «Мини-плеер» — круглая кнопка.
                if (onMiniPlayer != null) GlassIconButton(R.drawable.ic_pip, onMiniPlayer)
            }
            // Под таблетками 25 dp и место под визуализатор спектра (рисуется выше, под колонкой).
            Spacer(Modifier.height(VISUALIZER_HEIGHT + 25.dp))
        }

        // Текст песни проявляется с лёгким сдвигом снизу, 280 мс.
        AnimatedVisibility(
            visible = showLyrics,
            enter = fadeIn(tween(280, easing = FastOutSlowInEasing)) +
                slideInVertically(tween(280, easing = FastOutSlowInEasing)) { it / 12 },
            exit = fadeOut(tween(280, easing = FastOutSlowInEasing)) +
                slideOutVertically(tween(280, easing = FastOutSlowInEasing)) { it / 12 },
        ) {
            LyricsScreen(
                queue = queue,
                track = track,
                player = player,
                liked = liked.contains(track.id),
                onLike = { model.toggleLike(track) },
                onClose = { showLyrics = false },
                backEnabled = transition.targetState == EnterExitState.Visible,
            )
        }
    }

    if (showQueue) {
        QueueSheet(queue, player, model, onDismiss = { showQueue = false })
    }
    if (showSound) {
        SoundSheet(onDismiss = { showSound = false })
    }
}

/** Значок управления: сам значок `size`, зона нажатия не меньше 44 dp. */
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

// ------------------------------------------------------------ нижние листы

/** Ручка листа: 40×4, surface4, как на макете 10 (отступ 10 сверху, 16 снизу до содержимого). */
@Composable
internal fun SheetHandle() {
    Box(
        Modifier
            .padding(top = 10.dp, bottom = 16.dp)
            .size(width = 40.dp, height = 4.dp)
            .background(Pl.surface4, RoundedCornerShape(2.dp)),
    )
}

/**
 * Общий нижний лист: фон `surface1`, затемнение 0,72, скругление 28 сверху,
 * ручка 40×4. Рамку 1 dp рисуем внутри листа, а не через `modifier` листа:
 * `modifier` у `ModalBottomSheet` стоит снаружи модификатора, который сдвигает
 * лист по вертикали, и рамка оставалась бы на месте, а не ехала с листом.
 * Отступ под полосу навигации содержимое ставит само (`navigationBarsPadding`).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun PlBottomSheet(
    onDismiss: () -> Unit,
    sheetState: SheetState = rememberModalBottomSheetState(),
    content: @Composable ColumnScope.() -> Unit,
) {
    val shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp)
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        containerColor = Pl.surface1,
        shape = shape,
        scrimColor = Color.Black.copy(alpha = 0.72f),
        dragHandle = null,
    ) {
        Column(
            Modifier.fillMaxWidth().drawWithContent {
                drawContent()
                // Контур без нижней стороны: содержимое кончается над полосой навигации,
                // и линия по его низу легла бы поперёк листа. Прямоугольник рисуем выше
                // листа на радиус, а всё, что ниже содержимого, обрезаем.
                val stroke = 1.dp.toPx()
                val radius = 28.dp.toPx()
                clipRect {
                    drawRoundRect(
                        color = Color(0xFF262626),
                        topLeft = Offset(stroke / 2, stroke / 2),
                        size = Size(size.width - stroke, size.height + radius),
                        cornerRadius = CornerRadius(radius - stroke / 2),
                        style = Stroke(stroke),
                    )
                }
            },
        ) {
            Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { SheetHandle() }
            content()
        }
    }
}

/**
 * «Настройки звука»: то же, что страница «Звук» в настройках (качество и
 * нормализация громкости), в нижнем листе. Смена качества перезапускает
 * играющий трек, лист остаётся открытым.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SoundSheet(onDismiss: () -> Unit) {
    PlBottomSheet(onDismiss) {
        Column(
            Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .navigationBarsPadding()
                .padding(bottom = 16.dp),
        ) {
            PlText(
                "Настройки звука",
                PlType.title,
                modifier = Modifier.padding(start = 20.dp, end = 20.dp, bottom = 14.dp),
            )
            SoundSettingsContent()
        }
    }
}

// ----------------------------------------------------------------- очередь

/** Метка раздела очереди: «СЕЙЧАС ИГРАЕТ» (микро, text3, трекинг 6 %). */
@Composable
private fun QueueLabel(text: String) {
    PlText(
        text,
        PlType.micro.copy(letterSpacing = 0.06.em),
        Pl.text3,
        Modifier.padding(horizontal = 20.dp),
    )
}

/** Чип «Перемешать» в шапке листа: 36 dp, рамка, значок 16; включённый — заливка. */
@Composable
private fun ShuffleChip(selected: Boolean, onClick: () -> Unit) {
    val color = if (selected) Pl.darkText else Pl.text
    Row(
        Modifier
            .height(36.dp)
            .clip(CircleShape)
            .then(
                if (selected) {
                    Modifier.background(Pl.text, CircleShape)
                } else {
                    Modifier.border(1.dp, Pl.outline, CircleShape)
                },
            )
            .clickable(onClick = onClick)
            .padding(start = 12.dp, end = 14.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        PlIcon(R.drawable.ic_shuffle, 16.dp, color)
        PlText("Перемешать", PlType.secondaryStrong, color, maxLines = 1)
    }
}

/**
 * Лист «Очередь» — как на десктопе: «Недавно играли» (до 20 треков, самые
 * свежие рядом с текущим, приглушены), «Сейчас играет» и один следующий трек
 * «Далее». При открытии лист прокручен к «Сейчас играет», прошлые треки выше.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun QueueSheet(
    queue: QueueHolder.State,
    player: PlayerController?,
    model: AppViewModel,
    onDismiss: () -> Unit,
) {
    val playing = player?.isPlaying?.collectAsStateWithLifecycle()?.value ?: false
    val shuffle = player?.shuffle?.collectAsStateWithLifecycle()?.value ?: false
    val repeat = player?.repeatMode?.collectAsStateWithLifecycle()?.value ?: Player.REPEAT_MODE_OFF
    val session by QueueHolder.sessionHistory.collectAsStateWithLifecycle()
    val server by model.history.collectAsStateWithLifecycle()
    // Серверная история подгружается в фоне; ошибка тихая — просто нет третьего источника.
    LaunchedEffect(Unit) { if (server.days == null && !server.loading) model.loadHistory() }

    val now = queue.currentTrack()
    val next = remember(queue.current, queue.tracks, shuffle, repeat) {
        player?.nextIndex()?.let { queue.tracks.getOrNull(it) }
    }
    val before = remember(queue.current, queue.tracks, shuffle) {
        player?.previousIndices(QueueHolder.RECENT_LIMIT).orEmpty().mapNotNull { queue.tracks.getOrNull(it) }
    }
    val serverTracks = remember(server.days) {
        server.days.orEmpty().flatMap { it.groups }.flatMap { it.tracks }
    }
    val recent = remember(now?.id, session, before, serverTracks) {
        mergeRecent(now?.id, session, before, serverTracks, QueueHolder.RECENT_LIMIT)
    }
    // Лист открывается на «Сейчас играет»: выше метка, строки «Недавно играли» и отступ.
    val listState = rememberLazyListState(
        initialFirstVisibleItemIndex = if (recent.isEmpty()) 0 else recent.size + 2,
    )
    val navBottom = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()

    PlBottomSheet(onDismiss, rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Box(Modifier.fillMaxWidth().fillMaxHeight(0.88f)) {
            Column(Modifier.fillMaxSize()) {
                // Шапка: заголовок с подписью и «Перемешать» (в волне перемешивания нет).
                Row(
                    Modifier.fillMaxWidth().padding(start = 20.dp, end = 16.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        PlText("Очередь", PlType.title)
                        PlText(
                            "${queue.title} · ${tracksCount(queue.tracks.size)}",
                            PlType.caption.copy(fontFeatureSettings = "tnum"),
                            Pl.text3,
                            maxLines = 1,
                        )
                    }
                    if (!queue.isWave) ShuffleChip(shuffle) { player?.toggleShuffle() }
                }
                if (now == null) {
                    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                        PlText("Очередь пуста", PlType.bodyStrong, Pl.text2)
                    }
                } else {
                    LazyColumn(
                        state = listState,
                        modifier = Modifier.fillMaxWidth().weight(1f),
                        contentPadding = PaddingValues(top = 18.dp, bottom = navBottom + 24.dp),
                    ) {
                        if (recent.isNotEmpty()) {
                            item(key = "recent-label") {
                                QueueLabel("НЕДАВНО ИГРАЛИ")
                                Spacer(Modifier.height(2.dp))
                            }
                            items(recent, key = { "recent:${it.id}" }) { item ->
                                QueueRow(
                                    item,
                                    current = false,
                                    soundOn = false,
                                    enabled = item.playable,
                                    onClick = {
                                        // Есть в текущей очереди — играем с него, иначе очередь из одного трека.
                                        val index = queue.tracks.indexOfFirst { it.id == item.id }
                                        if (index >= 0) {
                                            player?.seekTo(index, 0)
                                        } else {
                                            player?.playQueue(listOf(item), 0, "Недавно играли")
                                        }
                                    },
                                    modifier = Modifier.alpha(0.6f),
                                )
                            }
                            item(key = "recent-gap") { Spacer(Modifier.height(16.dp)) }
                        }
                        item(key = "now") {
                            QueueLabel("СЕЙЧАС ИГРАЕТ")
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
                        }
                        next?.let { upcoming ->
                            item(key = "next") {
                                Spacer(Modifier.height(16.dp))
                                QueueLabel("ДАЛЕЕ")
                                Spacer(Modifier.height(2.dp))
                                // Играем с этого трека, очередь не пересоздаётся.
                                QueueRow(upcoming, current = false, soundOn = false, onClick = {
                                    val index = queue.tracks.indexOfFirst { it.id == upcoming.id }
                                    if (index >= 0) player?.seekTo(index, 0)
                                })
                            }
                        }
                    }
                }
            }
            // Затухание списка у нижнего края листа: цвет листа, поэтому ступеньки нет.
            Box(
                Modifier
                    .align(Alignment.BottomCenter)
                    .fillMaxWidth()
                    .height(110.dp)
                    .background(Brush.verticalGradient(listOf(Pl.surface1.copy(alpha = 0f), Pl.surface1))),
            )
        }
    }
}

/**
 * Строка очереди: обложка 48 (радиус 8), название и исполнитель, длительность.
 * Играющий трек: заглушка 60 % и столбики на обложке, название акцентом.
 */
@Composable
private fun QueueRow(
    track: Track,
    current: Boolean,
    soundOn: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
) {
    Row(
        modifier
            .fillMaxWidth()
            .height(64.dp)
            .then(if (!current && enabled) Modifier.clickable(onClick = onClick) else Modifier)
            .padding(start = if (current) 8.dp else 16.dp, end = if (current) 12.dp else 16.dp),
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
                when {
                    current -> Pl.accent
                    enabled -> Pl.text
                    else -> Pl.textDisabled
                },
                maxLines = 1,
            )
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                if (track.explicit) ExplicitBadge()
                if (FckCensor.hasReplacement(track.id)) ReplacedTag()
                PlText(
                    track.artistNames,
                    PlType.secondary,
                    if (enabled) Pl.text2 else Pl.textDisabled,
                    maxLines = 1,
                    modifier = Modifier.weight(1f, fill = false),
                )
            }
        }
        track.durationMs?.let {
            Spacer(Modifier.width(12.dp))
            PlText(
                formatDuration(it),
                PlType.caption.copy(fontFeatureSettings = "tnum"),
                Pl.text3,
                textAlign = TextAlign.End,
            )
        }
    }
}
