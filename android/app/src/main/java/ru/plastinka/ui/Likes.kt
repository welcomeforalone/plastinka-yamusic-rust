package ru.plastinka.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import ru.plastinka.R
import ru.plastinka.data.HistoryContext
import ru.plastinka.playback.QueueHolder
import ru.plastinka.playback.currentTrack

/** Вложенный экран коллекции: «Мне нравится». */
@Composable
fun LikesTab(library: LibraryUi, model: AppViewModel, player: PlayerController?, onBack: () -> Unit) {
    val liked by QueueHolder.likedIds.collectAsStateWithLifecycle()
    val queue by QueueHolder.state.collectAsStateWithLifecycle()
    val soundOn = player?.isPlaying?.collectAsStateWithLifecycle()?.value ?: false
    val currentId = queue.currentTrack()?.id
    val tracks = library.tracks

    Box(Modifier.fillMaxSize()) {
        Glow(
            380.dp,
            Modifier.align(Alignment.TopStart).offset(x = (-120).dp, y = (-130).dp),
            alpha = 0.5f,
        )
        LazyColumn(Modifier.fillMaxSize()) {
            item { BackBar(onBack) }
            item {
                TracksHeader(
                    cover = {
                        GradientHeart(116.dp, 22.dp, 52.dp)
                    },
                    title = "Мне нравится",
                    totalMs = tracks.sumOf { it.durationMs ?: 0L },
                    count = tracks.size,
                    onPlay = { player?.playQueue(tracks, 0, "Мне нравится", source = HistoryContext.Liked) },
                    onShuffle = { player?.playQueue(tracks.shuffled(), 0, "Мне нравится", source = HistoryContext.Liked) },
                )
            }
            if (tracks.isEmpty()) {
                item {
                    Column(
                        Modifier.fillMaxWidth().padding(horizontal = 32.dp, vertical = 48.dp),
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        if (library.loading) {
                            CircularProgressIndicator(Modifier.size(28.dp), color = Pl.accent, strokeWidth = 2.dp)
                        } else {
                            PlText("В «Мне нравится» пока пусто", PlType.bodyStrong, textAlign = TextAlign.Center)
                            PlText(
                                "Отмечайте треки сердцем — они появятся здесь",
                                PlType.secondary,
                                Pl.text2,
                                textAlign = TextAlign.Center,
                            )
                        }
                    }
                }
            } else {
                items(tracks.size, key = { tracks.getOrNull(it)?.id ?: it.toString() }) { index ->
                    tracks.getOrNull(index)?.let { track ->
                        TrackRow(
                            track,
                            current = track.id == currentId,
                            soundOn = soundOn,
                            liked = liked.contains(track.id),
                            onClick = { player?.playQueue(tracks, index, "Мне нравится", source = HistoryContext.Liked) },
                            onLike = { model.toggleLike(track) },
                        )
                    }
                }
            }
        }
    }
}

/** Плитка с сердцем на градиенте акцента. */
@Composable
fun GradientHeart(size: androidx.compose.ui.unit.Dp, radius: androidx.compose.ui.unit.Dp, icon: androidx.compose.ui.unit.Dp) {
    val accent = Pl.accent
    Box(
        Modifier
            .size(size)
            .clip(RoundedCornerShape(radius))
            .background(
                Brush.linearGradient(
                    listOf(accent, lerp(accent, Color(0xFF1A1033), 0.7f)),
                    start = Offset.Zero,
                    end = Offset.Infinite,
                ),
            ),
        contentAlignment = Alignment.Center,
    ) {
        PlIcon(R.drawable.ic_heart_filled, icon, Pl.onAccent)
    }
}

/** «15 ч 42 мин» / «51 мин». */
fun durationText(totalMs: Long): String {
    val minutesTotal = totalMs / 60_000
    val hours = minutesTotal / 60
    val minutes = minutesTotal % 60
    return if (hours > 0) "$hours ч $minutes мин" else "$minutes мин"
}

/**
 * Общая шапка списка треков («Мне нравится», плейлист): обложка (любая),
 * заголовок, «N треков · длительность», кнопки «Слушать» и «Перемешать».
 */
@Composable
fun TracksHeader(
    cover: @Composable () -> Unit,
    title: String,
    totalMs: Long,
    count: Int,
    onPlay: () -> Unit,
    onShuffle: () -> Unit,
    /** Своя подпись вместо «N треков · длительность» (например, для исполнителя). */
    subtitle: String? = null,
    /** Своя подпись с нажимаемыми частями; перекрывает `subtitle`. */
    subtitleContent: (@Composable () -> Unit)? = null,
) {
    Column(Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, top = 8.dp, bottom = 14.dp)) {
        cover()
        Spacer(Modifier.height(18.dp))
        PlText(title, PlType.hero)
        Spacer(Modifier.height(4.dp))
        if (subtitleContent != null) {
            subtitleContent()
        } else if (subtitle != null) {
            PlText(subtitle, PlType.secondary.copy(fontFeatureSettings = "tnum"), Pl.text2)
        } else if (count > 0) {
            PlText(
                "${tracksCount(count)} · ${durationText(totalMs)}",
                PlType.secondary.copy(fontFeatureSettings = "tnum"),
                Pl.text2,
            )
        }
        if (count > 0) {
            Spacer(Modifier.height(18.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Pill(
                    "Слушать",
                    Modifier.weight(1f),
                    fillWidth = true,
                    icon = R.drawable.ic_play_filled,
                    onClick = onPlay,
                )
                Pill(
                    "Перемешать",
                    Modifier.weight(1f),
                    kind = PillKind.Outline,
                    fillWidth = true,
                    icon = R.drawable.ic_shuffle,
                    onClick = onShuffle,
                )
            }
        }
    }
}
