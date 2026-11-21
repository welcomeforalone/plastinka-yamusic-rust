package ru.plastinka.ui

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import ru.plastinka.playback.QueueHolder
import ru.plastinka.playback.currentTrack
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import ru.plastinka.R
import ru.plastinka.data.FckCensor
import ru.plastinka.data.Relink
import ru.plastinka.data.Track

/** Длительность `м:сс`. */
fun formatDuration(ms: Long): String {
    val total = (ms / 1000).coerceAtLeast(0)
    return "${total / 60}:${(total % 60).toString().padStart(2, '0')}"
}

/** «1 трек / 2 трека / 5 треков». */
fun tracksCount(n: Int): String {
    val mod100 = n % 100
    val mod10 = n % 10
    val word = when {
        mod100 in 11..14 -> "треков"
        mod10 == 1 -> "трек"
        mod10 in 2..4 -> "трека"
        else -> "треков"
    }
    return "$n $word"
}

/** Три столбика «играет»: качаются, пока звук идёт, на паузе стоят. */
@Composable
fun EqBars(size: Dp, color: Color, animated: Boolean, modifier: Modifier = Modifier) {
    // Две функции: на паузе бесконечной анимации нет вовсе (лишние кадры не рисуются).
    if (animated) EqBarsMoving(size, color, modifier) else EqBarsStill(size, color, modifier)
}

@Composable
private fun EqBarsMoving(size: Dp, color: Color, modifier: Modifier) {
    val transition = rememberInfiniteTransition(label = "eq")
    val spec = { ms: Int ->
        infiniteRepeatable<Float>(tween(ms, easing = LinearEasing), RepeatMode.Reverse)
    }
    val a = transition.animateFloat(0.25f, 0.95f, spec(520), label = "eq-a")
    val b = transition.animateFloat(0.25f, 0.95f, spec(380), label = "eq-b")
    val c = transition.animateFloat(0.25f, 0.95f, spec(460), label = "eq-c")
    // Значения читаются только при отрисовке: перекомпозиции каждый кадр нет.
    Canvas(modifier.size(size)) { drawEqBars(color, a.value, b.value, c.value) }
}

@Composable
private fun EqBarsStill(size: Dp, color: Color, modifier: Modifier) {
    Canvas(modifier.size(size)) { drawEqBars(color, 0.55f, 0.9f, 0.4f) }
}

private fun androidx.compose.ui.graphics.drawscope.DrawScope.drawEqBars(color: Color, a: Float, b: Float, c: Float) {
    val s = this.size.minDimension
    val w = s / 6
    listOf(a, b, c).forEachIndexed { i, h ->
        val x = s * 0.14f + i * (w + s * 0.1f)
        val top = s * (0.84f - h * 0.68f)
        drawRect(
            color,
            topLeft = Offset(x, top),
            size = Size(w, s * 0.84f - top),
        )
    }
}

/**
 * Строка трека: высота 64, обложка 48. `current` — трек очереди, `soundOn` —
 * звук идёт (столбики качаются). Недоступный трек не нажимается.
 */
@Composable
fun TrackRow(
    track: Track,
    current: Boolean,
    soundOn: Boolean,
    liked: Boolean,
    onClick: () -> Unit,
    onLike: () -> Unit,
    modifier: Modifier = Modifier,
    /** Номер места слева от обложки (топ месяца); `null` — без номера. */
    number: Int? = null,
) {
    // Копия недоступного трека: видимые строки просят поиск (`want` сам отсекает лишнее).
    if (!track.available) LaunchedEffect(track.id) { Relink.current?.want(track) }
    val link = trackLink(track)
    val available = link != RowLink.Gone
    val ai = rememberIsAi(track)
    val links = rememberTrackLinks(track)
    Row(
        modifier
            .fillMaxWidth()
            .height(64.dp)
            .then(if (available) Modifier.clickable(onClick = onClick) else Modifier)
            .padding(start = 16.dp, end = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (number != null) {
            PlText(
                number.toString(),
                PlType.secondary.copy(fontFeatureSettings = "tnum"),
                Pl.text3,
                Modifier.width(28.dp),
                maxLines = 1,
            )
        }
        Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) {
            Cover(
                track.artUrl("100x100"),
                track.id,
                48.dp,
                8.dp,
                if (available) Modifier else Modifier.alpha(0.4f),
            )
            if (current && available) {
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
                track.displayTitle,
                if (current && available) PlType.bodyStrong else PlType.body,
                when {
                    !available -> Pl.textDisabled
                    current -> Pl.accent
                    else -> Pl.text
                },
                maxLines = 1,
            )
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                if (track.explicit) ExplicitBadge()
                if (ai) AiTag()
                if (FckCensor.hasReplacement(track.id)) ReplacedTag()
                LinkTag(link)
                PlText(
                    track.artistNames,
                    PlType.secondary,
                    if (available) Pl.text2 else Pl.textDisabled,
                    maxLines = 1,
                    modifier = Modifier.weight(1f, fill = false).then(links.artists),
                )
            }
        }
        // Зоны нажатия уже, чем в макете по высоте: значки встают с зазором 12 dp
        // (зона сердца 34 dp при значке 22, зона «…» 32 dp при значке 20).
        Spacer(Modifier.width(6.dp))
        HeartButton(
            liked,
            onLike,
            22.dp,
            Modifier.size(width = 34.dp, height = 48.dp),
            tint = if (available) null else Pl.textDisabled,
        )
        // «…» только если в меню есть действия (плагины включены).
        TrackMoreButton(
            track,
            Modifier.size(width = 32.dp, height = 48.dp),
            iconSize = 20.dp,
            tint = if (available) Pl.text2 else Pl.textDisabled,
        )
    }
}

/** Как строка трека показывает доступность (как `Link` в `src/ui/library/copies.rs` на десктопе). */
enum class RowLink {
    /** Доступен или звук даёт FckCensor. */
    Plain,

    /** Недоступен, играет копия из другого релиза. */
    Copy,

    /** Играть нечем: копии нет или поиск ещё не ответил. */
    Gone,
}

/** Состояние строки трека; чтение идёт через снимок Compose: строка перерисуется, когда копия найдётся. */
fun trackLink(track: Track): RowLink = when {
    track.available || FckCensor.hasSound(track.id) -> RowLink.Plain
    Relink.current?.playingCopy(track) != null -> RowLink.Copy
    else -> RowLink.Gone
}

/** Обложка трека, а если её нет — найденной копии (как `art_track`); копия известна целиком только в этом сеансе. */
fun Track.artUrl(size: String): String? =
    coverUrl(size) ?: Relink.current?.let { relink ->
        relink.copyOf(id)?.let { relink.copyTrack(it.id) }?.coverUrl(size)
    }

/** Метка «Копия» акцентом или серая «Недоступен»; у обычного трека её нет. Вид как у [ReplacedTag]. */
@Composable
fun LinkTag(link: RowLink) {
    when (link) {
        RowLink.Plain -> Unit
        RowLink.Copy -> RelinkTag("Копия", Pl.accent)
        RowLink.Gone -> RelinkTag("Недоступен", Pl.textDisabled)
    }
}

@Composable
fun RelinkTag(text: String, color: Color) {
    val shape = RoundedCornerShape(4.dp)
    Box(
        Modifier
            .height(16.dp)
            .border(1.dp, color.copy(alpha = 0.6f), shape)
            .padding(horizontal = 5.dp),
        contentAlignment = Alignment.Center,
    ) {
        PlText(text, PlType.micro, color, maxLines = 1)
    }
}

/** Значок «E» (explicit): 14 dp, плашка surface4 с радиусом 3 и буква в 9 sp. */
@Composable
fun ExplicitBadge() {
    Box(
        Modifier.size(14.dp).background(Pl.surface4, RoundedCornerShape(3.dp)),
        contentAlignment = Alignment.Center,
    ) {
        PlText("E", PlType.badge, Pl.text2, maxLines = 1)
    }
}

/** Сердце с зоной нажатия не меньше 40 dp. */
@Composable
fun HeartButton(
    liked: Boolean,
    onClick: () -> Unit,
    size: Dp,
    modifier: Modifier = Modifier,
    /** Свой цвет значка (недоступный трек); `null` — обычный: акцент или text2. */
    tint: Color? = null,
) {
    Box(
        modifier
            .size(maxOf(40.dp, size + 12.dp))
            .clip(RoundedCornerShape(20.dp))
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        PlIcon(
            if (liked) R.drawable.ic_heart_filled else R.drawable.ic_heart,
            size,
            tint ?: if (liked) Pl.accent else Pl.text2,
        )
    }
}

/** Состояние «что играет»: текущий трек очереди, идёт ли звук и множество лайков. */
class PlayingState(val currentId: String?, val soundOn: Boolean, val liked: Set<String>)

@Composable
fun rememberPlayingState(player: PlayerController?): PlayingState {
    val liked by QueueHolder.likedIds.collectAsStateWithLifecycle()
    val queue by QueueHolder.state.collectAsStateWithLifecycle()
    val soundOn = player?.isPlaying?.collectAsStateWithLifecycle()?.value ?: false
    return PlayingState(queue.currentTrack()?.id, soundOn, liked)
}

/**
 * Строки треков для `LazyColumn`. Недоступные показываются серыми и не
 * нажимаются; нажатие запускает очередь `tracks` с этого трека (недоступные
 * плеер из очереди отсеивает сам).
 */
fun LazyListScope.trackItems(
    tracks: List<Track>,
    state: PlayingState,
    player: PlayerController?,
    model: AppViewModel,
    queueTitle: String,
    source: ru.plastinka.data.HistoryContext? = null,
) {
    items(tracks.size, key = { index -> "$index:${tracks.getOrNull(index)?.id}" }) { index ->
        tracks.getOrNull(index)?.let { track ->
            TrackRow(
                track,
                current = track.id == state.currentId,
                soundOn = state.soundOn,
                liked = state.liked.contains(track.id),
                onClick = { player?.playQueue(tracks, index, queueTitle, source = source) },
                onLike = { model.toggleLike(track) },
            )
        }
    }
}
