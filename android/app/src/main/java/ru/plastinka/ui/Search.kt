package ru.plastinka.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.flow.filter
import ru.plastinka.R
import ru.plastinka.data.AlbumCard
import ru.plastinka.data.ArtistCard
import ru.plastinka.data.PlaylistCard
import ru.plastinka.data.SearchBest
import ru.plastinka.data.SearchKind
import ru.plastinka.data.SearchResult

/** Сколько треков показывает вкладка «Всё». */
private const val ALL_TRACKS_SHOWN = 3

/** Ширина плитки в ленте: квадрат 132 dp, как на макете. */
private val TILE_WIDTH = 132.dp

private val TABS = listOf(
    SearchKind.All to "Всё",
    SearchKind.Track to "Треки",
    SearchKind.Album to "Альбомы",
    SearchKind.Artist to "Исполнители",
    SearchKind.Playlist to "Плейлисты",
)

/**
 * Вкладка «Поиск»: поле, вкладки-«таблетки» и результаты. Альбом, исполнитель и
 * плейлист открываются внутри вкладки (стек как в «Коллекции»), «назад»
 * возвращает к результатам с прежней прокруткой.
 */
@Composable
fun SearchTab(model: AppViewModel, player: PlayerController?) {
    var stack by rememberSaveable(stateSaver = StackSaver) {
        mutableStateOf<List<CollectionRoute>>(listOf(CollectionRoute.Hub))
    }
    val holder = rememberSaveableStateHolder()
    val push = { route: CollectionRoute -> stack = stack + route }
    val pop = {
        if (stack.size > 1) {
            stack.lastOrNull()?.let { holder.removeState(stateKey(stack.lastIndex, it)) }
            stack = stack.dropLast(1)
        }
    }
    BackHandler(enabled = stack.size > 1) { pop() }

    val top = stack.lastOrNull() ?: CollectionRoute.Hub
    holder.SaveableStateProvider(stateKey(stack.lastIndex, top)) {
        if (top == CollectionRoute.Hub) {
            SearchMain(model, player, onOpen = push)
        } else {
            DetailRouteScreen(top, model, player, onBack = pop, push = push)
        }
    }
}

@Composable
private fun SearchMain(model: AppViewModel, player: PlayerController?, onOpen: (CollectionRoute) -> Unit) {
    val query by model.query.collectAsStateWithLifecycle()
    val tab by model.searchTab.collectAsStateWithLifecycle()
    val slots by model.searchSlots.collectAsStateWithLifecycle()
    val playing = rememberPlayingState(player)
    val slot = slots[tab]
    val result = slot?.result

    Column(Modifier.fillMaxSize().padding(top = 12.dp)) {
        PlText("Поиск", PlType.display, modifier = Modifier.padding(horizontal = 20.dp))
        Spacer(Modifier.height(14.dp))
        SearchField(query, model::setQuery)
        Spacer(Modifier.height(12.dp))
        // Лента вкладок прокручивается по горизонтали; выбор переживает смену запроса.
        LazyRow(
            contentPadding = PaddingValues(horizontal = 16.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            items(TABS, key = { it.first }) { (kind, label) ->
                Chip(label, selected = kind == tab, onClick = { model.selectSearchTab(kind) })
            }
        }
        Spacer(Modifier.height(8.dp))
        when {
            query.isBlank() -> Message("Введите запрос", null)
            slot == null || slot.loading -> LoadingBox()
            slot.error != null -> ErrorState(slot.error) { model.retrySearch(tab) }
            result == null || (result.isEmpty && result.best == null) ->
                Message("Ничего не нашлось", "Проверьте запрос или попробуйте другое слово")
            tab == SearchKind.All -> AllResults(result, playing, player, model, onOpen)
            else -> TypedResults(tab, slot, playing, player, model, onOpen)
        }
    }
}

private fun openAlbum(card: AlbumCard) = CollectionRoute.Album(card.id, card.displayTitle, card.coverUri)
private fun openArtist(card: ArtistCard) = CollectionRoute.Artist(card.id, card.name, card.coverUri)
private fun openPlaylist(card: PlaylistCard) =
    CollectionRoute.Playlist(card.uid, card.kind, card.title, card.coverUri)

/** Вкладка «Всё»: лучший результат и секции с ссылкой «Все». */
@Composable
private fun AllResults(
    result: SearchResult,
    playing: PlayingState,
    player: PlayerController?,
    model: AppViewModel,
    onOpen: (CollectionRoute) -> Unit,
) {
    val best = result.best
    val tracks = result.tracks
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 16.dp)) {
        if (best != null) {
            item(key = "best") {
                BestResult(
                    best,
                    onOpen = {
                        when (best) {
                            is SearchBest.TrackHit -> player?.playQueue(listOf(best.track), 0, "Поиск")
                            is SearchBest.AlbumHit -> onOpen(openAlbum(best.card))
                            is SearchBest.ArtistHit -> onOpen(openArtist(best.card))
                            is SearchBest.PlaylistHit -> onOpen(openPlaylist(best.card))
                        }
                    },
                    onPlay = { model.playSearchBest(player, best) },
                )
            }
        }
        if (tracks.isNotEmpty()) {
            item(key = "h-tracks") { SectionHeader("Треки") { model.selectSearchTab(SearchKind.Track) } }
            val shown = tracks.take(ALL_TRACKS_SHOWN)
            items(shown.size, key = { "t$it:${shown.getOrNull(it)?.id}" }) { index ->
                shown.getOrNull(index)?.let { track ->
                    TrackRow(
                        track,
                        current = track.id == playing.currentId,
                        soundOn = playing.soundOn,
                        liked = playing.liked.contains(track.id),
                        onClick = { player?.playQueue(tracks, index, "Поиск") },
                        onLike = { model.toggleLike(track) },
                    )
                }
            }
        }
        if (result.albums.isNotEmpty()) {
            item(key = "h-albums") { SectionHeader("Альбомы", top = 10.dp, bottom = 8.dp) { model.selectSearchTab(SearchKind.Album) } }
            item(key = "albums") {
                TileRow {
                    items(result.albums, key = { it.id }) { card ->
                        MediaTile(
                            card.coverUrl("200x200"), card.id, card.displayTitle, albumSubtitle(card),
                            Modifier.width(TILE_WIDTH),
                            onClick = { onOpen(openAlbum(card)) },
                        )
                    }
                }
            }
        }
        if (result.artists.isNotEmpty()) {
            item(key = "h-artists") { SectionHeader("Исполнители", top = 10.dp, bottom = 8.dp) { model.selectSearchTab(SearchKind.Artist) } }
            item(key = "artists") {
                TileRow {
                    items(result.artists, key = { it.id }) { card ->
                        ArtistTile(card, onClick = { onOpen(openArtist(card)) })
                    }
                }
            }
        }
        if (result.playlists.isNotEmpty()) {
            item(key = "h-playlists") { SectionHeader("Плейлисты", top = 10.dp, bottom = 8.dp) { model.selectSearchTab(SearchKind.Playlist) } }
            item(key = "playlists") {
                TileRow {
                    items(result.playlists, key = { "${it.uid}:${it.kind}" }) { card ->
                        MediaTile(
                            card.coverUrl("200x200"), "${card.uid}:${card.kind}", card.title,
                            tracksCount(card.trackCount),
                            Modifier.width(TILE_WIDTH),
                            onClick = { onOpen(openPlaylist(card)) },
                        )
                    }
                }
            }
        }
    }
}

/** Вкладка одного типа: полный список с подгрузкой страниц. */
@Composable
private fun TypedResults(
    kind: SearchKind,
    slot: SearchSlot,
    playing: PlayingState,
    player: PlayerController?,
    model: AppViewModel,
    onOpen: (CollectionRoute) -> Unit,
) {
    val result = slot.result
    val listState = rememberLazyListState()
    // Нет следующей страницы или прошлая подгрузка не удалась — API не дёргаем.
    if (!slot.moreFailed && result.hasNext) {
        LoadMoreEffect(listState, slot.nextPage) { model.loadMoreSearch(kind) }
    }
    LazyColumn(Modifier.fillMaxSize(), state = listState, contentPadding = PaddingValues(bottom = 16.dp)) {
        when (kind) {
            SearchKind.Track -> trackItems(result.tracks, playing, player, model, "Поиск")
            SearchKind.Album -> items(result.albums, key = { "a${it.id}" }) { card ->
                MediaRow(
                    card.coverUrl("100x100"), card.id, 8.dp, card.displayTitle, albumSubtitle(card),
                    onClick = { onOpen(openAlbum(card)) },
                )
            }
            SearchKind.Artist -> items(result.artists, key = { "r${it.id}" }) { card ->
                MediaRow(
                    card.coverUrl("100x100"), card.id, 24.dp, card.name, null,
                    onClick = { onOpen(openArtist(card)) },
                )
            }
            else -> items(result.playlists, key = { "p${it.uid}:${it.kind}" }) { card ->
                MediaRow(
                    card.coverUrl("100x100"), "${card.uid}:${card.kind}", 8.dp, card.title,
                    tracksCount(card.trackCount),
                    onClick = { onOpen(openPlaylist(card)) },
                )
            }
        }
        if (slot.loadingMore) item(key = "more") { LoadingBox() }
        if (slot.moreFailed) {
            item(key = "more-failed") {
                ErrorState("Не удалось загрузить ещё. Проверьте соединение.") { model.loadMoreSearch(kind) }
            }
        }
    }
}

/**
 * Когда виден хвост списка, просит следующую страницу. Ключ — номер следующей
 * страницы: он меняется после каждой подгрузки, даже если она не добавила
 * новых элементов (все оказались повторами).
 */
@Composable
private fun LoadMoreEffect(state: LazyListState, page: Int, onMore: () -> Unit) {
    val callback by rememberUpdatedState(onMore)
    LaunchedEffect(state, page) {
        snapshotFlow {
            val info = state.layoutInfo
            (info.visibleItemsInfo.lastOrNull()?.index ?: -1) >= info.totalItemsCount - 4
        }.filter { it }.collect { callback() }
    }
}

/**
 * Заголовок секции и ссылка «Все» акцентным цветом справа (макет 07). Зона
 * нажатия 36 dp: заголовок 28 dp стоит в ней по центру, поэтому отступы
 * `top`/`bottom` — это зазоры макета минус 4 dp.
 */
@Composable
private fun SectionHeader(title: String, top: Dp = 18.dp, bottom: Dp = 0.dp, onAll: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(start = 20.dp, end = 8.dp, top = top, bottom = bottom),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        PlText(title, PlType.title, modifier = Modifier.weight(1f))
        Box(
            Modifier.height(36.dp).clip(CircleShape).clickable(onClick = onAll).padding(horizontal = 12.dp),
            contentAlignment = Alignment.Center,
        ) {
            PlText("Все", PlType.secondaryStrong, Pl.accent)
        }
    }
}

@Composable
private fun TileRow(content: LazyListScope.() -> Unit) {
    LazyRow(
        contentPadding = PaddingValues(horizontal = 20.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        content = content,
    )
}

/** Плитка исполнителя: круглая обложка и имя по центру. */
@Composable
private fun ArtistTile(card: ArtistCard, onClick: () -> Unit) {
    Column(
        Modifier.width(TILE_WIDTH).clip(RoundedCornerShape(16.dp)).clickable(onClick = onClick),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Cover(card.coverUrl("200x200"), card.id, TILE_WIDTH, TILE_WIDTH / 2)
        PlText(card.name, PlType.secondaryStrong, maxLines = 1, textAlign = TextAlign.Center)
    }
}

/** Блок «ЛУЧШИЙ РЕЗУЛЬТАТ»: подпись и карточка (обложка 72, у исполнителя круглая; Play 44). */
@Composable
private fun BestResult(best: SearchBest, onOpen: () -> Unit, onPlay: () -> Unit) {
    val cover: String?
    val title: String
    val kind: String
    val seed: String
    val radius: androidx.compose.ui.unit.Dp
    when (best) {
        is SearchBest.TrackHit -> {
            cover = best.track.coverUrl("200x200"); title = best.track.title; kind = "Трек"
            seed = best.track.id; radius = 12.dp
        }
        is SearchBest.AlbumHit -> {
            cover = best.card.coverUrl("200x200"); title = best.card.displayTitle; kind = "Альбом"
            seed = best.card.id; radius = 14.dp
        }
        is SearchBest.ArtistHit -> {
            cover = best.card.coverUrl("200x200"); title = best.card.name; kind = "Исполнитель"
            seed = best.card.id; radius = 36.dp
        }
        is SearchBest.PlaylistHit -> {
            cover = best.card.coverUrl("200x200"); title = best.card.title; kind = "Плейлист"
            seed = "${best.card.uid}:${best.card.kind}"; radius = 14.dp
        }
    }
    val shape = RoundedCornerShape(20.dp)
    PlText(
        "ЛУЧШИЙ РЕЗУЛЬТАТ",
        PlType.micro.copy(letterSpacing = 0.06.em),
        Pl.text3,
        modifier = Modifier.padding(start = 16.dp, top = 12.dp, bottom = 10.dp),
    )
    Box(Modifier.padding(horizontal = 16.dp)) {
        Row(
            Modifier
                .fillMaxWidth()
                .clip(shape)
                .background(Pl.surface1, shape)
                .border(1.dp, Pl.border, shape)
                .clickable(onClick = onOpen)
                .padding(start = 12.dp, top = 12.dp, end = 16.dp, bottom = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Cover(cover, seed, 72.dp, radius)
            Spacer(Modifier.width(14.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                PlText(title, PlType.headline, maxLines = 1)
                PlText(kind, PlType.secondary, Pl.text2, maxLines = 1)
            }
            Spacer(Modifier.width(14.dp))
            PlayButton(44.dp, playing = false, onClick = onPlay)
        }
    }
}

@Composable
private fun Message(title: String, hint: String?) {
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 32.dp, vertical = 40.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        PlText(title, PlType.bodyStrong, Pl.text, textAlign = TextAlign.Center)
        hint?.let { PlText(it, PlType.secondary, Pl.text2, textAlign = TextAlign.Center) }
    }
}

@Composable
private fun SearchField(query: String, onChange: (String) -> Unit) {
    val focus = LocalFocusManager.current
    Row(
        Modifier
            .padding(horizontal = 16.dp)
            .fillMaxWidth()
            .height(52.dp)
            .clip(CircleShape)
            .background(Pl.surface2, CircleShape)
            .border(1.dp, Pl.outline, CircleShape)
            .padding(start = 18.dp, end = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        PlIcon(R.drawable.ic_search, 20.dp, Pl.text3)
        Spacer(Modifier.width(12.dp))
        Box(Modifier.weight(1f)) {
            BasicTextField(
                value = query,
                onValueChange = onChange,
                singleLine = true,
                textStyle = PlType.body.copy(color = Pl.text),
                cursorBrush = SolidColor(Pl.accent),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                keyboardActions = KeyboardActions(onSearch = { focus.clearFocus() }),
                modifier = Modifier.fillMaxWidth(),
                decorationBox = { inner ->
                    Box(contentAlignment = Alignment.CenterStart) {
                        if (query.isEmpty()) PlText("Трек или исполнитель", PlType.body, Pl.text3)
                        inner()
                    }
                },
            )
        }
        if (query.isNotEmpty()) {
            Box(
                Modifier.size(36.dp).clip(CircleShape).clickable { onChange("") },
                contentAlignment = Alignment.Center,
            ) {
                PlIcon(R.drawable.ic_close, 18.dp, Pl.text2)
            }
        }
    }
}
