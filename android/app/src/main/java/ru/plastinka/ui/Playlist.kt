package ru.plastinka.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle

/** Экран плейлиста: шапка, «Слушать»/«Перемешать» и список треков. */
@Composable
fun PlaylistScreen(
    uid: Long,
    kind: Long,
    title: String,
    coverUri: String?,
    model: AppViewModel,
    player: PlayerController?,
    onBack: () -> Unit,
) {
    val key = "$uid:$kind"
    val source = ru.plastinka.data.HistoryContext.Playlist(uid.toString(), kind.toString(), title)
    val loaded by model.playlist.state.collectAsStateWithLifecycle()
    val playing = rememberPlayingState(player)
    LaunchedEffect(key) { model.loadPlaylist(uid, kind) }

    // Чужие данные (прежний плейлист) не показываем: берём их, только если ключ совпал.
    val state = loaded.takeIf { it.key == key }
    val tracks = state?.data.orEmpty()
    val error = state?.error
    val coverUrl = coverUri?.takeIf { it.contains("%%") }?.let { "https://${it.replace("%%", "400x400")}" }
    Box(Modifier.fillMaxSize()) {
        DetailGlow()
        Column(Modifier.fillMaxSize()) {
            BackBar(onBack, DETAIL_BAR_HEIGHT)
            LazyColumn(Modifier.fillMaxSize()) {
                item {
                    TracksHeader(
                        cover = { Cover(coverUrl, key, 116.dp, 22.dp, Modifier.coverShadow(22.dp)) },
                        title = title,
                        totalMs = tracks.sumOf { it.durationMs ?: 0L },
                        count = tracks.size,
                        onPlay = { player?.playQueue(tracks, 0, title, source = source) },
                        onShuffle = { player?.playQueue(tracks.shuffled(), 0, title, source = source) },
                    )
                }
                when {
                    error != null -> item { ErrorState(error) { model.loadPlaylist(uid, kind, force = true) } }
                    state == null || state.data == null -> item { LoadingBox() }
                    tracks.isEmpty() -> item {
                        Centered {
                            PlText("В плейлисте пока нет треков", PlType.bodyStrong, textAlign = TextAlign.Center)
                        }
                    }
                    else -> trackItems(tracks, playing, player, model, title, source)
                }
            }
        }
    }
}
