package ru.plastinka.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import ru.plastinka.data.AlbumCard
import ru.plastinka.data.ArtistCard

/** Сколько популярных треков показываем. */
private const val POPULAR_SHOWN = 10

/** «1 альбом / 2 альбома / 5 альбомов». */
private fun albumsCount(n: Int): String {
    val word = when {
        n % 100 in 11..14 -> "альбомов"
        n % 10 == 1 -> "альбом"
        n % 10 in 2..4 -> "альбома"
        else -> "альбомов"
    }
    return "$n $word"
}

/** Экран исполнителя: шапка, популярные треки и лента альбомов. */
@Composable
fun ArtistScreen(
    id: String,
    name: String,
    coverUri: String?,
    model: AppViewModel,
    player: PlayerController?,
    onBack: () -> Unit,
    onOpenAlbum: (AlbumCard) -> Unit,
    onOpenAllTracks: () -> Unit,
) {
    val loaded by model.artist.state.collectAsStateWithLifecycle()
    val playing = rememberPlayingState(player)
    LaunchedEffect(id) { model.loadArtist(id) }
    LaunchedEffect(id) { model.ensureLikedArtists() }
    val likedArtists by model.collection.collectAsStateWithLifecycle()

    val state = loaded.takeIf { it.key == id }
    val details = state?.data
    val shownName = details?.card?.name ?: name
    val source = ru.plastinka.data.HistoryContext.Artist(id, shownName)
    val popular = details?.popular.orEmpty().take(POPULAR_SHOWN)
    val albums = details?.albums.orEmpty()
    val coverUrl = (details?.card?.coverUri ?: coverUri)?.takeIf { it.contains("%%") }
        ?.let { "https://${it.replace("%%", "400x400")}" }
    val error = state?.error
    Box(Modifier.fillMaxSize()) {
        DetailGlow()
        Column(Modifier.fillMaxSize()) {
            BackBar(onBack, DETAIL_BAR_HEIGHT)
            LazyColumn(Modifier.fillMaxSize()) {
                item {
                    TracksHeader(
                        cover = { Cover(coverUrl, id, 116.dp, 58.dp, Modifier.coverShadow(58.dp)) },
                        title = shownName,
                        totalMs = 0L,
                        count = popular.size,
                        onPlay = { player?.playQueue(popular, 0, shownName, source = source) },
                        onShuffle = { player?.playQueue(popular.shuffled(), 0, shownName, source = source) },
                        subtitle = albums.size.takeIf { it > 0 }?.let { albumsCount(it) },
                        // Пока список понравившихся исполнителей неизвестен, сердце не показываем.
                        titleAction = likedArtists.artists?.let { list ->
                            {
                                HeartButton(
                                    liked = list.any { it.id == id },
                                    onClick = {
                                        model.toggleArtistLike(
                                            details?.card ?: ArtistCard(id, shownName, coverUri, albums.size),
                                        )
                                    },
                                    size = 28.dp,
                                )
                            }
                        },
                    )
                }
                when {
                    error != null -> item { ErrorState(error) { model.loadArtist(id, force = true) } }
                    details == null -> item { LoadingBox() }
                    else -> {
                        if (popular.isEmpty() && albums.isEmpty()) {
                            item {
                                Centered {
                                    PlText("Пока ничего нет", PlType.bodyStrong, textAlign = TextAlign.Center)
                                }
                            }
                        }
                        if (popular.isNotEmpty()) {
                            item {
                                SectionHeader("Популярные треки", bottom = 4.dp, link = "Все треки", onAll = onOpenAllTracks)
                            }
                            trackItems(popular, playing, player, model, shownName, source)
                        }
                        if (albums.isNotEmpty()) {
                            item { SectionHeader("Альбомы") }
                            item {
                                LazyRow(
                                    contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 20.dp),
                                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                                ) {
                                    items(albums, key = { it.id }) { card ->
                                        MediaTile(
                                            card.coverUrl("200x200"),
                                            card.id,
                                            card.displayTitle,
                                            card.year?.toString(),
                                            Modifier.width(140.dp),
                                            onClick = { onOpenAlbum(card) },
                                        )
                                    }
                                }
                                Spacer(Modifier.height(16.dp))
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun SectionHeader(text: String) {
    PlText(text, PlType.title, modifier = Modifier.padding(start = 20.dp, end = 20.dp, top = 18.dp, bottom = 8.dp))
}
