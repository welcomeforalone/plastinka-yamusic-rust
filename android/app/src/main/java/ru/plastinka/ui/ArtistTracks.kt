package ru.plastinka.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import ru.plastinka.R
import ru.plastinka.data.HistoryContext

/**
 * «Все треки» исполнителя: отдельный экран в стеке вкладки. Страницы по 100
 * догружаются при прокрутке к концу; нажатие на трек строит очередь из уже
 * загруженных треков списка, источник в истории — исполнитель (как у популярных).
 */
@Composable
fun ArtistTracksScreen(
    id: String,
    name: String,
    model: AppViewModel,
    player: PlayerController?,
    onBack: () -> Unit,
) {
    val loaded by model.artistTracks.collectAsStateWithLifecycle()
    val playing = rememberPlayingState(player)
    LaunchedEffect(id) { model.loadArtistTracks(id) }

    // Список другого исполнителя (запоздавший ответ, прежний экран) не показываем.
    val state = loaded.takeIf { it.key == id }
    val tracks = state?.items.orEmpty()
    val source = HistoryContext.Artist(id, name)
    val listState = rememberLazyListState()
    // Страницы кончились или прошлая догрузка не удалась — сервис не дёргаем (там «Повторить»).
    if (state != null && state.hasNext && !state.moreFailed) {
        LoadMoreEffect(listState, state.nextPage) { model.loadMoreArtistTracks(id) }
    }

    Box(Modifier.fillMaxSize()) {
        DetailGlow()
        Column(Modifier.fillMaxSize()) {
            BackBar(onBack, DETAIL_BAR_HEIGHT)
            LazyColumn(Modifier.fillMaxSize(), state = listState, contentPadding = PaddingValues(bottom = 16.dp)) {
                item(key = "head") {
                    Column(Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 14.dp)) {
                        PlText(name, PlType.body, Pl.text2, maxLines = 1)
                        PlText("Все треки", PlType.hero)
                        if (state?.loaded == true) {
                            Spacer(Modifier.height(4.dp))
                            PlText(
                                tracksCount(state.total),
                                PlType.secondary.copy(fontFeatureSettings = "tnum"),
                                Pl.text2,
                            )
                        }
                        if (tracks.isNotEmpty()) {
                            Spacer(Modifier.height(18.dp))
                            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                                Pill(
                                    "Слушать",
                                    Modifier.weight(1f),
                                    fillWidth = true,
                                    icon = R.drawable.ic_play_filled,
                                    onClick = { player?.playQueue(tracks, 0, name, source = source) },
                                )
                                Pill(
                                    "Перемешать",
                                    Modifier.weight(1f),
                                    kind = PillKind.Outline,
                                    fillWidth = true,
                                    icon = R.drawable.ic_shuffle,
                                    onClick = { player?.playQueue(tracks.shuffled(), 0, name, source = source) },
                                )
                            }
                        }
                    }
                }
                when {
                    state?.error != null ->
                        item(key = "error") { ErrorState(state.error) { model.loadArtistTracks(id, force = true) } }
                    state == null || !state.loaded -> item(key = "loading") { LoadingBox() }
                    tracks.isEmpty() -> item(key = "empty") {
                        Centered { PlText("Треков нет", PlType.bodyStrong, textAlign = TextAlign.Center) }
                    }
                    else -> {
                        trackItems(tracks, playing, player, model, name, source)
                        if (state.loadingMore) item(key = "more") { LoadingBox() }
                        if (state.moreFailed) {
                            item(key = "more-failed") {
                                ErrorState("Не удалось загрузить ещё. Проверьте соединение.") {
                                    model.loadMoreArtistTracks(id)
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
