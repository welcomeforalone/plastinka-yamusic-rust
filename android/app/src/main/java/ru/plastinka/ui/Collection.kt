package ru.plastinka.ui

import androidx.activity.compose.BackHandler
import androidx.annotation.DrawableRes
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import ru.plastinka.R
import ru.plastinka.data.AlbumCard
import ru.plastinka.data.ArtistCard
import ru.plastinka.data.HistoryContext
import ru.plastinka.data.RecentItem
import ru.plastinka.data.mergeHistory
import ru.plastinka.data.recentItems
import ru.plastinka.data.PlaylistCard

/**
 * Маршруты внутри вкладки «Коллекция». Хаб — корень стека. Данные, нужные для
 * первого кадра (название, обложка), едут в самом маршруте.
 */
sealed interface CollectionRoute {
    data object Hub : CollectionRoute
    data object Liked : CollectionRoute
    data object Playlists : CollectionRoute
    data object Albums : CollectionRoute
    data object Artists : CollectionRoute
    data object History : CollectionRoute
    data object TopMonth : CollectionRoute
    data class Playlist(val uid: Long, val kind: Long, val title: String, val cover: String?) : CollectionRoute
    data class Album(val id: String, val title: String, val cover: String?) : CollectionRoute
    data class Artist(val id: String, val name: String, val cover: String?) : CollectionRoute
}

private const val SEP = "\u0001"

private fun CollectionRoute.encode(): String = when (this) {
    CollectionRoute.Hub -> "H"
    CollectionRoute.Liked -> "L"
    CollectionRoute.Playlists -> "P"
    CollectionRoute.Albums -> "A"
    CollectionRoute.Artists -> "R"
    CollectionRoute.History -> "S"
    CollectionRoute.TopMonth -> "T"
    is CollectionRoute.Playlist -> listOf("p", uid, kind, title, cover.orEmpty()).joinToString(SEP)
    is CollectionRoute.Album -> listOf("a", id, title, cover.orEmpty()).joinToString(SEP)
    is CollectionRoute.Artist -> listOf("r", id, name, cover.orEmpty()).joinToString(SEP)
}

private fun decodeRoute(text: String): CollectionRoute? {
    val p = text.split(SEP)
    return when (p.firstOrNull()) {
        "H" -> CollectionRoute.Hub
        "L" -> CollectionRoute.Liked
        "P" -> CollectionRoute.Playlists
        "A" -> CollectionRoute.Albums
        "R" -> CollectionRoute.Artists
        "S" -> CollectionRoute.History
        "T" -> CollectionRoute.TopMonth
        "p" -> CollectionRoute.Playlist(
            p.getOrNull(1)?.toLongOrNull() ?: return null,
            p.getOrNull(2)?.toLongOrNull() ?: return null,
            p.getOrNull(3) ?: return null,
            p.getOrNull(4)?.takeIf { it.isNotEmpty() },
        )
        "a" -> CollectionRoute.Album(
            p.getOrNull(1) ?: return null,
            p.getOrNull(2) ?: return null,
            p.getOrNull(3)?.takeIf { it.isNotEmpty() },
        )
        "r" -> CollectionRoute.Artist(
            p.getOrNull(1) ?: return null,
            p.getOrNull(2) ?: return null,
            p.getOrNull(3)?.takeIf { it.isNotEmpty() },
        )
        else -> null
    }
}

/** Ключ сохранённого состояния экрана: маршрут и его место в стеке (один маршрут может встретиться дважды). */
internal fun stateKey(position: Int, route: CollectionRoute): String = "$position:${route.encode()}"

/** Стек маршрутов переживает пересоздание активности: хранится списком строк. */
internal val StackSaver = Saver<List<CollectionRoute>, List<String>>(
    save = { stack -> stack.map { it.encode() } },
    restore = { saved ->
        saved.mapNotNull(::decodeRoute).ifEmpty { listOf(CollectionRoute.Hub) }
    },
)

/**
 * Вкладка «Коллекция»: хаб и вложенные экраны. Системная «назад» снимает
 * верхний маршрут; переключение вкладок стек не сбрасывает. Состояние
 * прокрутки каждого экрана хранится, пока маршрут в стеке.
 */
@Composable
fun CollectionTab(
    library: LibraryUi,
    model: AppViewModel,
    player: PlayerController?,
    onOpenSettings: () -> Unit,
) {
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
        when (top) {
            CollectionRoute.Hub -> CollectionHub(
                library = library,
                model = model,
                player = player,
                onOpenSettings = onOpenSettings,
                onOpen = push,
            )
            CollectionRoute.Liked -> LikesTab(library, model, player, onBack = pop)
            CollectionRoute.Playlists -> PlaylistsScreen(
                model = model,
                onBack = pop,
                onOpen = { card -> push(CollectionRoute.Playlist(card.uid, card.kind, card.title, card.coverUri)) },
            )
            is CollectionRoute.Playlist, is CollectionRoute.Album, is CollectionRoute.Artist ->
                DetailRouteScreen(top, model, player, onBack = pop, push = push)
            CollectionRoute.Albums -> AlbumsScreen(
                model = model,
                onBack = pop,
                onOpen = { card -> push(CollectionRoute.Album(card.id, card.displayTitle, card.coverUri)) },
            )
            CollectionRoute.History -> HistoryScreen(model, player, onBack = pop)
            CollectionRoute.TopMonth -> TopMonthScreen(
                model = model,
                player = player,
                onBack = pop,
                onOpenArtist = { top -> push(CollectionRoute.Artist(top.card.id, top.card.name, top.card.coverUri)) },
            )
            CollectionRoute.Artists -> ArtistsScreen(
                model = model,
                onBack = pop,
                onAllTop = { push(CollectionRoute.TopMonth) },
                onOpen = { card -> push(CollectionRoute.Artist(card.id, card.name, card.coverUri)) },
            )
        }
    }
}

/** Экраны плейлиста, альбома и исполнителя: общие для «Коллекции» и «Поиска». */
@Composable
internal fun DetailRouteScreen(
    route: CollectionRoute,
    model: AppViewModel,
    player: PlayerController?,
    onBack: () -> Unit,
    push: (CollectionRoute) -> Unit,
) {
    when (route) {
        is CollectionRoute.Playlist -> PlaylistScreen(
            uid = route.uid,
            kind = route.kind,
            title = route.title,
            coverUri = route.cover,
            model = model,
            player = player,
            onBack = onBack,
        )
        is CollectionRoute.Album -> AlbumScreen(
            id = route.id,
            title = route.title,
            coverUri = route.cover,
            model = model,
            player = player,
            onBack = onBack,
            onOpenArtist = { artist ->
                artist.id?.takeIf { ru.plastinka.data.YandexApi.isNumericId(it) }
                    ?.let { push(CollectionRoute.Artist(it, artist.name, null)) }
            },
        )
        is CollectionRoute.Artist -> ArtistScreen(
            id = route.id,
            name = route.name,
            coverUri = route.cover,
            model = model,
            player = player,
            onBack = onBack,
            onOpenAlbum = { card -> push(CollectionRoute.Album(card.id, card.displayTitle, card.coverUri)) },
        )
        else -> Unit
    }
}

@Composable
private fun CollectionHub(
    library: LibraryUi,
    model: AppViewModel,
    player: PlayerController?,
    onOpenSettings: () -> Unit,
    onOpen: (CollectionRoute) -> Unit,
) {
    val collection by model.collection.collectAsStateWithLifecycle()
    val history by model.history.collectAsStateWithLifecycle()
    val local by ru.plastinka.data.PlayLog.plays.collectAsStateWithLifecycle()
    // Данные обновляются при каждом входе на хаб; история — отдельным запросом.
    LaunchedEffect(Unit) {
        model.loadCollection()
        model.loadHistory()
    }
    val recent = remember(history.days, local, collection.playlists) {
        recentItems(mergeHistory(history.days.orEmpty(), local), collection.playlists.orEmpty())
    }

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(top = 12.dp, bottom = 16.dp)) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 20.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            PlText("Коллекция", PlType.display, modifier = Modifier.weight(1f))
            Avatar(36.dp, library.account?.avatarUrl, Modifier.clickable(onClick = onOpenSettings))
        }
        Spacer(Modifier.height(16.dp))
        LikedCard(
            count = library.tracks.size,
            onOpen = { onOpen(CollectionRoute.Liked) },
            onPlay = { player?.playQueue(library.tracks, 0, "Мне нравится", source = HistoryContext.Liked) },
        )
        Spacer(Modifier.height(8.dp))
        HubRow(R.drawable.ic_playlist, "Плейлисты", collection.playlists?.size) {
            onOpen(CollectionRoute.Playlists)
        }
        HubRow(R.drawable.ic_album, "Альбомы", collection.albums?.size) { onOpen(CollectionRoute.Albums) }
        HubRow(R.drawable.ic_artist, "Исполнители", collection.artists?.size) {
            onOpen(CollectionRoute.Artists)
        }
        HubRow(R.drawable.ic_history, "История", null) { onOpen(CollectionRoute.History) }
        val topArtists = collection.top.orEmpty()
        if (topArtists.isNotEmpty()) {
            Spacer(Modifier.height(12.dp))
            TopMonthShelf(
                topArtists,
                onAll = { onOpen(CollectionRoute.TopMonth) },
                onOpenArtist = { top -> onOpen(CollectionRoute.Artist(top.card.id, top.card.name, top.card.coverUri)) },
            )
        }
        if (recent.isNotEmpty()) {
            Spacer(Modifier.height(12.dp))
            PlText("Недавнее", PlType.title, modifier = Modifier.padding(horizontal = 20.dp))
            Spacer(Modifier.height(12.dp))
            RecentGrid(recent, onOpen)
        }
    }
}

@Composable
private fun LikedCard(count: Int, onOpen: () -> Unit, onPlay: () -> Unit) {
    val shape = RoundedCornerShape(20.dp)
    Box(Modifier.padding(horizontal = 16.dp)) {
        Row(
            Modifier
                .fillMaxWidth()
                .clip(shape)
                .background(Pl.surface1, shape)
                .border(1.dp, Pl.border, shape)
                .clickable(onClick = onOpen)
                .padding(start = 12.dp, end = 16.dp, top = 12.dp, bottom = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            GradientHeart(72.dp, 14.dp, 32.dp)
            Spacer(Modifier.width(14.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                PlText("Мне нравится", PlType.headline, maxLines = 1)
                PlText(tracksCount(count), PlType.secondary.copy(fontFeatureSettings = "tnum"), Pl.text2)
            }
            Spacer(Modifier.width(14.dp))
            PlayButton(44.dp, playing = false, onClick = onPlay)
        }
    }
}

/** Строка раздела: иконка, название, число справа (если есть) и шеврон. */
@Composable
private fun HubRow(@DrawableRes icon: Int, label: String, count: Int?, onClick: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .height(56.dp)
            .clickable(onClick = onClick)
            .padding(horizontal = 20.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        PlIcon(icon, 22.dp, Pl.text2)
        Spacer(Modifier.width(14.dp))
        PlText(label, PlType.body, modifier = Modifier.weight(1f), maxLines = 1)
        if (count != null) {
            PlText(count.toString(), PlType.secondary.copy(fontFeatureSettings = "tnum"), Pl.text3)
            Spacer(Modifier.width(14.dp))
        }
        Chevron()
    }
}

/** Блок «Недавнее»: плитки в две колонки (поля 16, зазор 12). */
@Composable
private fun RecentGrid(items: List<RecentItem>, onOpen: (CollectionRoute) -> Unit) {
    Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
        for (pair in items.chunked(2)) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                for (item in pair) {
                    val ctx = item.context
                    val (title, seed, route) = when (ctx) {
                        is HistoryContext.Album ->
                            Triple(ctx.title, ctx.id, CollectionRoute.Album(ctx.id, ctx.title, item.coverUri))
                        is HistoryContext.Playlist -> {
                            val uid = ctx.uid.toLongOrNull()
                            val kind = ctx.kind.toLongOrNull()
                            Triple(
                                ctx.title, "${ctx.uid}:${ctx.kind}",
                                if (uid != null && kind != null) {
                                    CollectionRoute.Playlist(uid, kind, ctx.title, item.coverUri)
                                } else {
                                    null
                                },
                            )
                        }
                        else -> continue
                    }
                    MediaTile(
                        item.coverUri?.takeIf { it.contains("%%") }?.let { "https://${it.replace("%%", "300x300")}" },
                        seed, title, item.subtitle,
                        Modifier.weight(1f),
                        onClick = { route?.let(onOpen) },
                    )
                }
                // Нечётная плитка занимает одну колонку, вторая остаётся пустой.
                if (pair.size == 1) Spacer(Modifier.weight(1f))
            }
        }
    }
}

/** Содержимое раздела: загрузка, ошибка с «Повторить», пусто или сам список. */
@Composable
private fun <T> SectionBody(
    items: List<T>?,
    error: String?,
    emptyTitle: String,
    emptyHint: String,
    onRetry: () -> Unit,
    content: @Composable (List<T>) -> Unit,
) {
    when {
        items == null && error != null -> ErrorState(error, onRetry)
        items == null -> LoadingBox()
        items.isEmpty() -> Centered {
            PlText(emptyTitle, PlType.bodyStrong, textAlign = TextAlign.Center)
            PlText(emptyHint, PlType.secondary, Pl.text2, textAlign = TextAlign.Center)
        }
        else -> content(items)
    }
}

@Composable
private fun PlaylistsScreen(model: AppViewModel, onBack: () -> Unit, onOpen: (PlaylistCard) -> Unit) {
    val collection by model.collection.collectAsStateWithLifecycle()
    LaunchedEffect(Unit) { if (collection.playlists == null && !collection.loading) model.loadCollection() }

    Column(Modifier.fillMaxSize()) {
        BackBar(onBack)
        PlText("Плейлисты", PlType.display, modifier = Modifier.padding(horizontal = 20.dp))
        Spacer(Modifier.height(18.dp))
        SectionBody(
            items = collection.playlists,
            error = collection.playlistsError,
            emptyTitle = "Плейлистов пока нет",
            emptyHint = "Создайте плейлист или отметьте чужой — он появится здесь",
            onRetry = model::loadCollection,
        ) { playlists ->
            LazyColumn(Modifier.fillMaxSize()) {
                items(playlists.size, key = { index ->
                    playlists.getOrNull(index)?.let { "${it.uid}:${it.kind}" } ?: index.toString()
                }) { index ->
                    playlists.getOrNull(index)?.let { card ->
                        MediaRow(
                            card.coverUrl("100x100"), "${card.uid}:${card.kind}", 8.dp,
                            card.title, tracksCount(card.trackCount),
                            onClick = { onOpen(card) },
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun AlbumsScreen(model: AppViewModel, onBack: () -> Unit, onOpen: (AlbumCard) -> Unit) {
    val collection by model.collection.collectAsStateWithLifecycle()
    LaunchedEffect(Unit) { if (collection.albums == null && !collection.loading) model.loadCollection() }

    Column(Modifier.fillMaxSize()) {
        BackBar(onBack)
        PlText("Альбомы", PlType.display, modifier = Modifier.padding(horizontal = 20.dp))
        Spacer(Modifier.height(18.dp))
        SectionBody(
            items = collection.albums,
            error = collection.albumsError,
            emptyTitle = "Альбомов пока нет",
            emptyHint = "Отмечайте альбомы сердцем — они появятся здесь",
            onRetry = model::loadCollection,
        ) { albums ->
            // Две колонки, как плитки «Недавнее» в макете: поля 16, зазор 12.
            LazyVerticalGrid(
                columns = GridCells.Fixed(2),
                modifier = Modifier.fillMaxSize(),
                contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 16.dp),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                items(albums, key = { it.id }) { card ->
                    MediaTile(
                        card.coverUrl("300x300"), card.id, card.displayTitle, albumSubtitle(card),
                        onClick = { onOpen(card) },
                    )
                }
            }
        }
    }
}

/** Подпись плитки альбома: «Исполнитель · год». */
internal fun albumSubtitle(card: AlbumCard): String =
    listOfNotNull(card.artistNames.takeIf { it.isNotEmpty() }, card.year?.toString()).joinToString(" · ")

@Composable
private fun ArtistsScreen(
    model: AppViewModel,
    onBack: () -> Unit,
    onAllTop: () -> Unit,
    onOpen: (ArtistCard) -> Unit,
) {
    val collection by model.collection.collectAsStateWithLifecycle()
    LaunchedEffect(Unit) { if (collection.artists == null && !collection.loading) model.loadCollection() }

    Column(Modifier.fillMaxSize()) {
        BackBar(onBack)
        PlText("Исполнители", PlType.display, modifier = Modifier.padding(horizontal = 20.dp))
        Spacer(Modifier.height(18.dp))
        val topArtists = collection.top.orEmpty()
        val liked = collection.artists
        // Полка топа — первый элемент того же списка; понравившихся нет, а топ есть — полка видна.
        if (liked != null && liked.isEmpty() && topArtists.isNotEmpty()) {
            LazyColumn(Modifier.fillMaxSize()) {
                item {
                    TopMonthShelf(topArtists, onAll = onAllTop, onOpenArtist = { top ->
                        onOpen(top.card)
                    })
                }
                item {
                    Centered {
                        PlText("Исполнителей пока нет", PlType.bodyStrong, textAlign = TextAlign.Center)
                        PlText(
                            "Отмечайте исполнителей сердцем — они появятся здесь",
                            PlType.secondary,
                            Pl.text2,
                            textAlign = TextAlign.Center,
                        )
                    }
                }
            }
            return@Column
        }
        SectionBody(
            items = collection.artists,
            error = collection.artistsError,
            emptyTitle = "Исполнителей пока нет",
            emptyHint = "Отмечайте исполнителей сердцем — они появятся здесь",
            onRetry = model::loadCollection,
        ) { artists ->
            LazyColumn(Modifier.fillMaxSize()) {
                if (topArtists.isNotEmpty()) {
                    item {
                        TopMonthShelf(topArtists, onAll = onAllTop, onOpenArtist = { top -> onOpen(top.card) })
                        Spacer(Modifier.height(12.dp))
                    }
                }
                items(artists.size, key = { index -> artists.getOrNull(index)?.id ?: index.toString() }) { index ->
                    artists.getOrNull(index)?.let { card ->
                        MediaRow(
                            card.coverUrl("100x100"), card.id, 24.dp, card.name, null,
                            onClick = { onOpen(card) },
                        )
                    }
                }
            }
        }
    }
}
