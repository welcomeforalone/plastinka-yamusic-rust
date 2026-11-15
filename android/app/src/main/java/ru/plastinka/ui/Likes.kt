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
import ru.plastinka.playback.QueueHolder
import ru.plastinka.playback.currentTrack

/** Вкладка «Коллекция»: «Мне нравится». */
@Composable
fun LikesTab(library: LibraryUi, model: AppViewModel, player: PlayerController?) {
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
            item { LikesHeader(tracks.sumOf { it.durationMs ?: 0L }, tracks.size, player, tracks) }
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
                            onClick = { player?.playQueue(tracks, index, "Мне нравится") },
                            onLike = { model.toggleLike(track) },
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun LikesHeader(
    totalMs: Long,
    count: Int,
    player: PlayerController?,
    tracks: List<ru.plastinka.data.Track>,
) {
    val accent = Pl.accent
    Column(Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, top = 24.dp, bottom = 14.dp)) {
        Box(
            Modifier
                .size(116.dp)
                .clip(RoundedCornerShape(22.dp))
                .background(
                    Brush.linearGradient(
                        listOf(accent, lerp(accent, Color(0xFF1A1033), 0.7f)),
                        start = Offset.Zero,
                        end = Offset.Infinite,
                    ),
                ),
            contentAlignment = Alignment.Center,
        ) {
            PlIcon(R.drawable.ic_heart_filled, 52.dp, Pl.onAccent)
        }
        Spacer(Modifier.height(18.dp))
        PlText("Мне нравится", PlType.hero)
        Spacer(Modifier.height(4.dp))
        if (count > 0) {
            val minutesTotal = totalMs / 60_000
            val hours = minutesTotal / 60
            val minutes = minutesTotal % 60
            val length = if (hours > 0) "$hours ч $minutes мин" else "$minutes мин"
            PlText(
                "${tracksCount(count)} · $length",
                PlType.secondary.copy(fontFeatureSettings = "tnum"),
                Pl.text2,
            )
            Spacer(Modifier.height(18.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Pill(
                    "Слушать",
                    Modifier.weight(1f),
                    fillWidth = true,
                    icon = R.drawable.ic_play_filled,
                    onClick = { player?.playQueue(tracks, 0, "Мне нравится") },
                )
                Pill(
                    "Перемешать",
                    Modifier.weight(1f),
                    kind = PillKind.Outline,
                    fillWidth = true,
                    icon = R.drawable.ic_shuffle,
                    onClick = { player?.playQueue(tracks.shuffled(), 0, "Мне нравится") },
                )
            }
        }
    }
}
