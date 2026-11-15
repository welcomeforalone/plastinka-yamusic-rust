package ru.plastinka.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import ru.plastinka.data.ArtistRef

/** Экран альбома: шапка с исполнителем, «Слушать»/«Перемешать» и треки по порядку. */
@Composable
fun AlbumScreen(
    id: String,
    title: String,
    coverUri: String?,
    model: AppViewModel,
    player: PlayerController?,
    onBack: () -> Unit,
    onOpenArtist: (ArtistRef) -> Unit,
) {
    val loaded by model.album.state.collectAsStateWithLifecycle()
    val playing = rememberPlayingState(player)
    LaunchedEffect(id) { model.loadAlbum(id) }

    val state = loaded.takeIf { it.key == id }
    val details = state?.data
    val tracks = details?.tracks.orEmpty()
    val card = details?.card
    val shownTitle = card?.displayTitle ?: title
    val source = ru.plastinka.data.HistoryContext.Album(id, shownTitle)
    val coverUrl = (card?.coverUri ?: coverUri)?.takeIf { it.contains("%%") }
        ?.let { "https://${it.replace("%%", "400x400")}" }
    val error = state?.error
    Column(Modifier.fillMaxSize()) {
        BackBar(onBack)
        LazyColumn(Modifier.fillMaxSize()) {
            item {
                TracksHeader(
                    cover = { Cover(coverUrl, id, 116.dp, 22.dp) },
                    title = shownTitle,
                    totalMs = tracks.sumOf { it.durationMs ?: 0L },
                    count = tracks.size,
                    onPlay = { player?.playQueue(tracks, 0, shownTitle, source = source) },
                    onShuffle = { player?.playQueue(tracks.shuffled(), 0, shownTitle, source = source) },
                    subtitleContent = card?.let { c ->
                        { AlbumSubtitle(c.artists, c.year, tracks.size, tracks.sumOf { it.durationMs ?: 0L }, onOpenArtist) }
                    },
                )
            }
            when {
                error != null -> item { ErrorState(error) { model.loadAlbum(id, force = true) } }
                details == null -> item { LoadingBox() }
                tracks.isEmpty() -> item {
                    Centered { PlText("В альбоме нет треков", PlType.bodyStrong, textAlign = TextAlign.Center) }
                }
                else -> trackItems(tracks, playing, player, model, shownTitle, source)
            }
        }
    }
}

/** «Исполнитель · год · N треков · длительность»; имя исполнителя нажимается, если есть id. */
@Composable
private fun AlbumSubtitle(
    artists: List<ArtistRef>,
    year: Int?,
    count: Int,
    totalMs: Long,
    onOpenArtist: (ArtistRef) -> Unit,
) {
    val first = artists.firstOrNull()
    val tail = listOfNotNull(
        year?.toString(),
        tracksCount(count).takeIf { count > 0 },
        durationText(totalMs).takeIf { count > 0 },
    ).joinToString(" · ")
    val style = PlType.secondary.copy(fontFeatureSettings = "tnum")
    Row {
        if (first != null) {
            val clickable = if (first.id != null) Modifier.clickable { onOpenArtist(first) } else Modifier
            PlText(
                artists.joinToString(", ") { it.name },
                PlType.secondaryStrong,
                if (first.id != null) Pl.text else Pl.text2,
                clickable.weight(1f, fill = false),
                maxLines = 1,
            )
            if (tail.isNotEmpty()) PlText(" · ", style, Pl.text2)
        }
        if (tail.isNotEmpty()) PlText(tail, style, Pl.text2, maxLines = 1)
    }
}
