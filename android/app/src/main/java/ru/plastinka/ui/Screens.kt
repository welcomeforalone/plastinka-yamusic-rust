package ru.plastinka.ui

import android.content.Intent
import android.net.Uri
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Favorite
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.DownloadDone
import androidx.compose.material.icons.filled.FavoriteBorder
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.SkipNext
import androidx.compose.material.icons.filled.SkipPrevious
import androidx.compose.material.icons.outlined.FavoriteBorder
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Tab
import androidx.compose.material3.TabRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import coil.compose.AsyncImage
import kotlinx.coroutines.delay
import ru.plastinka.data.Track
import ru.plastinka.playback.QueueHolder
import ru.plastinka.playback.currentTrack

/** Корневой экран: логин или библиотека + плеер. */
@Composable
fun RootScreen(
    loggedIn: Boolean,
    login: LoginUi,
    library: LibraryUi,
    model: AppViewModel,
) {
    val context = LocalContext.current
    var player by remember { mutableStateOf<PlayerController?>(null) }
    LaunchedEffect(Unit) {
        PlayerController.build(context) { player = it }
    }

    if (!loggedIn) {
        LoginScreen(login, onLogin = model::startLogin)
        return
    }

    val queue by QueueHolder.state.collectAsStateWithLifecycle()
    var showPlayer by remember { mutableStateOf(false) }

    Box(Modifier.fillMaxSize()) {
        LibraryScreen(library, model, player)
        // Мини-плеер над списком.
        queue.currentTrack()?.let { track ->
            AnimatedVisibility(
                visible = !showPlayer,
                modifier = Modifier.align(Alignment.BottomCenter),
            ) {
                MiniPlayer(
                    track = track,
                    player = player,
                    onOpen = { showPlayer = true },
                )
            }
        }
        if (showPlayer) {
            FullPlayer(
                queue = queue,
                player = player,
                model = model,
                onClose = { showPlayer = false },
            )
        }
    }
}

// ------------------------------------------------------------------ вход

@Composable
fun LoginScreen(login: LoginUi, onLogin: () -> Unit) {
    val context = LocalContext.current
    Column(
        Modifier
            .fillMaxSize()
            .padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Box(
            Modifier
                .size(96.dp)
                .clip(CircleShape)
                .background(MaterialTheme.colorScheme.primary),
            contentAlignment = Alignment.Center,
        ) {
            Text("♪", style = MaterialTheme.typography.displayMedium, color = Color.White)
        }
        Spacer(Modifier.height(24.dp))
        Text("Plastinka", style = MaterialTheme.typography.headlineMedium)
        Text(
            "Неофициальный клиент Яндекс Музыки. Нужна своя подписка Плюс.",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(32.dp))

        when (login.phase) {
            LoginPhase.Idle -> OutlinedButton(onClick = onLogin) { Text("Войти по коду устройства") }
            LoginPhase.Requesting -> CircularProgressIndicator()
            LoginPhase.Waiting, LoginPhase.Done -> login.code?.let { code ->
                Text("Откройте страницу и введите код:")
                Spacer(Modifier.height(12.dp))
                Text(
                    code.userCode,
                    style = MaterialTheme.typography.headlineLarge,
                    color = MaterialTheme.colorScheme.primary,
                )
                Spacer(Modifier.height(16.dp))
                OutlinedButton(onClick = {
                    context.startActivity(
                        Intent(Intent.ACTION_VIEW, Uri.parse(code.verificationUrl)),
                    )
                }) { Text("Открыть ${code.verificationUrl.removePrefix("https://")}") }
                login.error?.let {
                    Spacer(Modifier.height(12.dp))
                    Text(it, color = MaterialTheme.colorScheme.primary, textAlign = TextAlign.Center)
                    Spacer(Modifier.height(12.dp))
                    OutlinedButton(onClick = onLogin) { Text("Повторить вход") }
                }
            }
        }
    }
}

// -------------------------------------------------------------- библиотека

@Composable
fun LibraryScreen(
    library: LibraryUi,
    model: AppViewModel,
    player: PlayerController?,
) {
    var tab by remember { mutableStateOf(0) }
    val query by model.query.collectAsStateWithLifecycle()
    val search by model.search.collectAsStateWithLifecycle()
    val liked by QueueHolder.likedIds.collectAsStateWithLifecycle()

    Scaffold(
        topBar = {
            // Непрозрачный фон от самого верха (включая полосу статус-бара):
            // список листается под тулбаром, но не просвечивает.
            Surface(color = MaterialTheme.colorScheme.background) {
                Column(Modifier.statusBarsPadding()) {
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .padding(horizontal = 20.dp, vertical = 12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            "Plastinka",
                            style = MaterialTheme.typography.titleLarge,
                            modifier = Modifier.weight(1f),
                        )
                        library.account?.let { Text(it.displayName, style = MaterialTheme.typography.bodyMedium) }
                    }
                    TabRow(selectedTabIndex = tab) {
                        Tab(tab == 0, onClick = { tab = 0 }, text = { Text("Волна") })
                        Tab(tab == 1, onClick = { tab = 1 }, text = { Text("Мне нравится") })
                        Tab(tab == 2, onClick = { tab = 2 }, text = { Text("Поиск") })
                    }
                }
            }
        },
    ) { padding ->
        when (tab) {
            0 -> WaveTab(model, player, padding)
            1 -> LikesTab(library, liked, model, player, padding)
            else -> SearchTab(query, search, liked, model, player, padding)
        }
    }
}

@Composable
private fun WaveTab(
    model: AppViewModel,
    player: PlayerController?,
    padding: androidx.compose.foundation.layout.PaddingValues,
) {
    val queue by QueueHolder.state.collectAsStateWithLifecycle()
    val starting by model.waveStarting.collectAsStateWithLifecycle()
    val generated by model.generated.collectAsStateWithLifecycle()

    // VibeColorizer: цвет текущего трека волны, плавный переход 300 мс —
    // как на десктопе. Не в волне или без данных — акцент темы.
    val vibeTarget = remember(queue.current, queue.isWave) {
        if (queue.isWave) Vibe.pick(queue.currentTrack()?.vibe, Color.Unspecified)
        else Color.Unspecified
    }
    val themePrimary = MaterialTheme.colorScheme.primary
    val tint by animateColorAsState(
        targetValue = vibeTarget.takeIf { it != Color.Unspecified } ?: themePrimary,
        animationSpec = tween(durationMillis = 300),
        label = "vibe",
    )

    Box(
        Modifier
            .fillMaxSize()
            .padding(padding)
            .background(
                Brush.verticalGradient(
                    0f to tint.copy(alpha = 0.35f),
                    1f to Color.Transparent,
                ),
            ),
        contentAlignment = Alignment.Center,
    ) {
        if (queue.isWave) {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                queue.currentTrack()?.let { track ->
                    AsyncImage(
                        model = track.coverUrl("400x400"),
                        contentDescription = null,
                        modifier = Modifier
                            .size(200.dp)
                            .clip(RoundedCornerShape(16.dp))
                            .border(2.dp, tint.copy(alpha = 0.6f), RoundedCornerShape(16.dp))
                            .background(MaterialTheme.colorScheme.surfaceVariant),
                    )
                    Spacer(Modifier.height(16.dp))
                    Text(
                        track.title,
                        style = MaterialTheme.typography.titleLarge,
                        textAlign = TextAlign.Center,
                    )
                    Text(
                        track.artistNames,
                        style = MaterialTheme.typography.titleSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Spacer(Modifier.height(8.dp))
                }
                Text(
                    "Следующий трек подберётся сам: «Нравится» настраивает волну на ходу",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    textAlign = TextAlign.Center,
                    modifier = Modifier.padding(horizontal = 32.dp),
                )
            }
        } else if (starting) {
            CircularProgressIndicator()
        } else {
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp),
            ) {
                Spacer(Modifier.weight(0.6f))
                Box(
                    Modifier
                        .size(112.dp)
                        .clip(CircleShape)
                        .background(tint)
                        .clickable { model.startWave(player) },
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(
                        Icons.Filled.PlayArrow,
                        "Запустить волну",
                        tint = Color.White,
                        modifier = Modifier.size(56.dp),
                    )
                }
                Spacer(Modifier.height(20.dp))
                Text("Моя волна", style = MaterialTheme.typography.headlineSmall)
                Text(
                    "Бесконечный поток музыки под ваш вкус",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    textAlign = TextAlign.Center,
                )
                if (generated.isNotEmpty()) {
                    Spacer(Modifier.weight(0.4f))
                    Text(
                        "Собираем для вас",
                        style = MaterialTheme.typography.titleMedium,
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 8.dp),
                    )
                    Spacer(Modifier.height(12.dp))
                    LazyRow(
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                        contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 8.dp),
                    ) {
                        items(generated, key = { it.kind }) { playlist ->
                            PlaylistCard(playlist) { model.playPlaylist(player, playlist) }
                        }
                    }
                }
                Spacer(Modifier.weight(1f))
                Spacer(Modifier.height(24.dp))
            }
        }
    }
}

@Composable
private fun LikesTab(
    library: LibraryUi,
    liked: Set<String>,
    model: AppViewModel,
    player: PlayerController?,
    padding: androidx.compose.foundation.layout.PaddingValues,
) {
    if (library.loading) {
        Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
            CircularProgressIndicator()
        }
        return
    }
    library.error?.let { error ->
        Column(
            Modifier.fillMaxSize().padding(padding).padding(32.dp),
            verticalArrangement = Arrangement.Center,
        ) {
            Text(error, color = MaterialTheme.colorScheme.primary, textAlign = TextAlign.Center)
            Spacer(Modifier.height(16.dp))
            OutlinedButton(onClick = model::loadLibrary) { Text("Повторить") }
        }
        return
    }
    TrackList(
        tracks = library.tracks,
        liked = liked,
        onPlay = { index -> player?.playQueue(library.tracks, index, "Мне нравится") },
        onLike = model::toggleLike,
        padding = padding,
        emptyText = "В «Мне нравится» пока пусто",
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SearchTab(
    query: String,
    tracks: List<Track>,
    liked: Set<String>,
    model: AppViewModel,
    player: PlayerController?,
    padding: androidx.compose.foundation.layout.PaddingValues,
) {
    Column(Modifier.fillMaxSize().padding(padding)) {
        TextField(
            value = query,
            onValueChange = model::setQuery,
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 8.dp),
            placeholder = { Text("Трек или исполнитель") },
            leadingIcon = { Icon(Icons.Filled.Search, null) },
            singleLine = true,
            shape = RoundedCornerShape(14.dp),
            colors = TextFieldDefaults.colors(
                focusedContainerColor = MaterialTheme.colorScheme.surfaceVariant,
                unfocusedContainerColor = MaterialTheme.colorScheme.surfaceVariant,
                focusedIndicatorColor = Color.Transparent,
                unfocusedIndicatorColor = Color.Transparent,
            ),
        )
        TrackList(
            tracks = tracks,
            liked = liked,
            onPlay = { index -> player?.playQueue(tracks, index, "Поиск") },
            onLike = model::toggleLike,
            padding = androidx.compose.foundation.layout.PaddingValues(bottom = 96.dp),
            emptyText = if (query.isBlank()) "Введите запрос" else "Ничего не нашлось",
        )
    }
}

@Composable
private fun TrackList(
    tracks: List<Track>,
    liked: Set<String>,
    onPlay: (Int) -> Unit,
    onLike: (Track) -> Unit,
    padding: androidx.compose.foundation.layout.PaddingValues,
    emptyText: String,
) {
    if (tracks.isEmpty()) {
        Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
            Text(emptyText, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        return
    }
    val playing by QueueHolder.state.collectAsStateWithLifecycle()
    LazyColumn(Modifier.fillMaxSize(), contentPadding = padding) {
        itemsIndexed(tracks, key = { _, t -> t.id }) { index, track ->
            Row(
                Modifier
                    .fillMaxWidth()
                    .clickable { onPlay(index) }
                    .padding(horizontal = 16.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                AsyncImage(
                    model = track.coverUrl("100x100"),
                    contentDescription = null,
                    modifier = Modifier
                        .size(48.dp)
                        .clip(RoundedCornerShape(8.dp))
                        .background(MaterialTheme.colorScheme.surfaceVariant),
                )
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f)) {
                    Text(
                        track.title,
                        style = MaterialTheme.typography.bodyLarge,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        color = if (playing.current >= 0 && playing.tracks.getOrNull(playing.current)?.id == track.id)
                            MaterialTheme.colorScheme.primary
                        else MaterialTheme.colorScheme.onBackground,
                    )
                    Text(
                        track.artistNames,
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                if (!track.available) {
                    Text("недоступен", style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Spacer(Modifier.width(8.dp))
                }
                IconButton(onClick = { onLike(track) }) {
                    Icon(
                        if (liked.contains(track.id)) Icons.Filled.Favorite else Icons.Outlined.FavoriteBorder,
                        contentDescription = "Нравится",
                        tint = if (liked.contains(track.id)) MaterialTheme.colorScheme.primary
                        else MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
}

// ------------------------------------------------------------------ плеер

@Composable
fun MiniPlayer(track: Track, player: PlayerController?, onOpen: () -> Unit) {
    val playing = player?.isPlaying?.collectAsStateWithLifecycle()?.value ?: false
    var progress by remember { mutableStateOf(0f) }
    LaunchedEffect(player) {
        while (true) {
            player?.let {
                val duration = it.durationMs
                progress = if (duration > 0) it.positionMs.toFloat() / duration else 0f
            }
            delay(500)
        }
    }
    Surface(color = MaterialTheme.colorScheme.surface) {
        Column {
            LinearProgressIndicator(
                progress = { progress },
                modifier = Modifier.fillMaxWidth().height(2.dp),
                color = MaterialTheme.colorScheme.primary,
            )
            Row(
                Modifier
                    .fillMaxWidth()
                    .clickable { onOpen() }
                    .padding(horizontal = 12.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                AsyncImage(
                    model = track.coverUrl("100x100"),
                    contentDescription = null,
                    modifier = Modifier.size(42.dp).clip(RoundedCornerShape(6.dp)),
                )
                Spacer(Modifier.width(10.dp))
                Column(Modifier.weight(1f)) {
                    Text(track.title, maxLines = 1, overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.bodyMedium)
                    Text(track.artistNames, maxLines = 1, overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                IconButton(onClick = { player?.previous() }) {
                    Icon(Icons.Filled.SkipPrevious, "Назад")
                }
                IconButton(onClick = { player?.toggle() }) {
                    Icon(
                        if (playing) Icons.Filled.Pause else Icons.Filled.PlayArrow,
                        "Играть/пауза",
                        modifier = Modifier.size(36.dp),
                    )
                }
                IconButton(onClick = { player?.next() }) {
                    Icon(Icons.Filled.SkipNext, "Вперёд")
                }
            }
        }
    }
}

@Composable
fun FullPlayer(
    queue: QueueHolder.State,
    player: PlayerController?,
    model: AppViewModel,
    onClose: () -> Unit,
) {
    val track = queue.currentTrack() ?: return
    // VibeColorizer: в волне кнопка и слайдер — цветом трека.
    val playerTint by animateColorAsState(
        targetValue = if (queue.isWave) Vibe.pick(track.vibe, MaterialTheme.colorScheme.primary)
        else MaterialTheme.colorScheme.primary,
        animationSpec = tween(300),
        label = "vibe-player",
    )
    val playing = player?.isPlaying?.collectAsStateWithLifecycle()?.value ?: false
    var position by remember { mutableStateOf(0f) }
    var duration by remember { mutableStateOf(0f) }
    val liked by QueueHolder.likedIds.collectAsStateWithLifecycle()
    val cached by ru.plastinka.data.TrackCache.cached.collectAsStateWithLifecycle()
    val cacheProgress by ru.plastinka.data.TrackCache.progress.collectAsStateWithLifecycle()

    LaunchedEffect(player) {
        while (true) {
            player?.let {
                duration = it.durationMs.toFloat()
                position = if (duration > 0) it.positionMs.toFloat() / duration else 0f
            }
            delay(300)
        }
    }

    Surface(color = MaterialTheme.colorScheme.background, modifier = Modifier.fillMaxSize()) {
        Column(
            Modifier
                .fillMaxSize()
                .statusBarsPadding()
                .padding(24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = onClose) {
                    Icon(Icons.AutoMirrored.Filled.ArrowBack, "Свернуть")
                }
                Text(
                    queue.title,
                    style = MaterialTheme.typography.labelLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.weight(1f),
                    textAlign = TextAlign.Center,
                )
                val downloading = cacheProgress[track.id]
                if (downloading != null) {
                    CircularProgressIndicator(
                        progress = { if (downloading > 0) downloading / 100f else 0.1f },
                        modifier = Modifier.size(24.dp).padding(2.dp),
                        strokeWidth = 2.dp,
                    )
                } else {
                    IconButton(onClick = { model.toggleCache(track) }) {
                        Icon(
                            if (cached.contains(track.id)) Icons.Filled.DownloadDone
                            else Icons.Filled.Download,
                            if (cached.contains(track.id)) "Убрать из кэша" else "Скачать",
                            tint = if (cached.contains(track.id)) playerTint
                            else MaterialTheme.colorScheme.onSurface,
                        )
                    }
                }
                IconButton(onClick = { model.toggleLike(track) }) {
                    Icon(
                        if (liked.contains(track.id)) Icons.Filled.Favorite
                        else Icons.Outlined.FavoriteBorder,
                        "Нравится",
                        tint = if (liked.contains(track.id)) MaterialTheme.colorScheme.primary
                        else MaterialTheme.colorScheme.onSurface,
                    )
                }
            }
            Spacer(Modifier.height(16.dp))
            AsyncImage(
                model = track.coverUrl("600x600"),
                contentDescription = null,
                modifier = Modifier
                    .fillMaxWidth()
                    .aspectRatio(1f)
                    .clip(RoundedCornerShape(20.dp))
                    .background(MaterialTheme.colorScheme.surfaceVariant),
            )
            Spacer(Modifier.height(24.dp))
            Text(
                track.title,
                style = MaterialTheme.typography.headlineSmall,
                textAlign = TextAlign.Center,
                maxLines = 2, overflow = TextOverflow.Ellipsis,
            )
            Text(
                track.artistNames,
                style = MaterialTheme.typography.titleMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(20.dp))
            Slider(
                value = position.coerceIn(0f, 1f),
                onValueChange = { value ->
                    position = value
                    if (duration > 0) player?.seekTo((value * duration).toLong())
                },
                modifier = Modifier.fillMaxWidth(),
            )
            Row(verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = { player?.previous() }, modifier = Modifier.size(56.dp)) {
                    Icon(Icons.Filled.SkipPrevious, "Назад", modifier = Modifier.size(40.dp))
                }
                Spacer(Modifier.width(24.dp))
                Box(
                    Modifier
                        .size(76.dp)
                        .clip(CircleShape)
                        .background(playerTint)
                        .clickable { player?.toggle() },
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(
                        if (playing) Icons.Filled.Pause else Icons.Filled.PlayArrow,
                        "Играть/пауза",
                        tint = Color.White,
                        modifier = Modifier.size(40.dp),
                    )
                }
                Spacer(Modifier.width(24.dp))
                IconButton(onClick = { player?.next() }, modifier = Modifier.size(56.dp)) {
                    Icon(Icons.Filled.SkipNext, "Вперёд", modifier = Modifier.size(40.dp))
                }
            }
            Spacer(Modifier.height(16.dp))
            // Очередь
            LazyColumn(Modifier.weight(1f)) {
                itemsIndexed(queue.tracks, key = { _, t -> t.id }) { index, item ->
                    Text(
                        "${if (index == queue.current) "▶ " else ""}${item.title} — ${item.artistNames}",
                        style = MaterialTheme.typography.bodyMedium,
                        color = if (index == queue.current) MaterialTheme.colorScheme.primary
                        else MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1, overflow = TextOverflow.Ellipsis,
                        modifier = Modifier
                            .fillMaxWidth()
                            .clickable { player?.seekTo(index, 0) }
                            .padding(vertical = 6.dp),
                    )
                }
            }
        }
    }
}



/** Карточка персональной подборки («Плейлист дня» и т.п.). */
@Composable
fun PlaylistCard(
    playlist: ru.plastinka.data.GeneratedPlaylist,
    onPlay: () -> Unit,
) {
    Column(
        Modifier
            .width(150.dp)
            .clip(RoundedCornerShape(14.dp))
            .background(MaterialTheme.colorScheme.surface)
            .clickable { onPlay() }
            .padding(10.dp),
    ) {
        AsyncImage(
            model = playlist.coverUrl(),
            contentDescription = null,
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(1f)
                .clip(RoundedCornerShape(10.dp))
                .background(MaterialTheme.colorScheme.surfaceVariant),
        )
        Spacer(Modifier.height(8.dp))
        Text(
            playlist.title,
            style = MaterialTheme.typography.bodyMedium,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(
            "${playlist.trackCount} треков",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}
