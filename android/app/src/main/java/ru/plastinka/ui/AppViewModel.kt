package ru.plastinka.ui

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import ru.plastinka.PlastinkaApp
import ru.plastinka.data.AccountInfo
import ru.plastinka.data.DeviceCode
import ru.plastinka.data.Track
import ru.plastinka.playback.QueueHolder

data class LoginUi(
    val phase: LoginPhase = LoginPhase.Idle,
    val code: DeviceCode? = null,
    val error: String? = null,
)

sealed interface LoginPhase {
    data object Idle : LoginPhase
    data object Requesting : LoginPhase
    data object Waiting : LoginPhase
    data object Done : LoginPhase
}

data class LibraryUi(
    val loading: Boolean = false,
    val tracks: List<Track> = emptyList(),
    val error: String? = null,
    val account: AccountInfo? = null,
)

/**
 * Разделы коллекции. `null` в списке — ещё не получен (строка хаба без числа).
 * Ошибка раздела хранится, только если показать нечего: прежний список не теряется.
 */
data class CollectionUi(
    val loading: Boolean = false,
    val playlists: List<ru.plastinka.data.PlaylistCard>? = null,
    val albums: List<ru.plastinka.data.AlbumCard>? = null,
    val artists: List<ru.plastinka.data.ArtistCard>? = null,
    val playlistsError: String? = null,
    val albumsError: String? = null,
    val artistsError: String? = null,
)

/** Серверная история. `days == null` — ещё не получена; ошибка хранится, только если показать нечего. */
data class HistoryUi(
    val loading: Boolean = false,
    val days: List<ru.plastinka.data.HistoryDay>? = null,
    val error: String? = null,
)

/** Загруженное для ключа (плейлист, альбом, исполнитель): экран берёт данные, только если ключ его. */
data class Keyed<T>(
    val key: String? = null,
    val loading: Boolean = false,
    val data: T? = null,
    val error: String? = null,
)

/**
 * Загрузчик одного экрана с ключом. Уже загруженный ключ не перезапрашивается
 * (кроме `force`), смена ключа отменяет прежний запрос.
 */
class KeyedLoader<T>(
    private val scope: kotlinx.coroutines.CoroutineScope,
    private val failure: String,
) {
    private val _state = MutableStateFlow(Keyed<T>())
    val state: StateFlow<Keyed<T>> = _state
    private var job: Job? = null

    fun load(key: String, force: Boolean = false, block: suspend () -> T) {
        val cur = _state.value
        if (!force && cur.key == key && (cur.data != null || cur.loading)) return
        job?.cancel()
        _state.value = Keyed(key = key, loading = true, data = cur.data.takeIf { cur.key == key })
        job = scope.launch {
            _state.value = try {
                Keyed(key = key, data = block())
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                // Для несуществующего или недоступного объекта сервис отвечает ошибкой, не данными.
                Keyed(key = key, error = if (e is ru.plastinka.data.NotFoundException) "Не найдено" else failure)
            }
        }
    }

    fun reset() {
        job?.cancel()
        _state.value = Keyed()
    }
}

class AppViewModel(app: Application) : AndroidViewModel(app) {

    private val plastinka = app as PlastinkaApp

    private val _login = MutableStateFlow(LoginUi())
    val login: StateFlow<LoginUi> = _login

    private val _library = MutableStateFlow(LibraryUi())
    val library: StateFlow<LibraryUi> = _library

    private val _query = MutableStateFlow("")
    val query: StateFlow<String> = _query

    private val _search = MutableStateFlow<List<Track>>(emptyList())
    val search: StateFlow<List<Track>> = _search

    private var pollJob: Job? = null
    private var searchJob: Job? = null

    // Состояние, а не геттер: экран перерисовывается и при входе, и при выходе.
    private val _loggedIn = MutableStateFlow(plastinka.tokenStore.load() != null)
    val loggedInState: StateFlow<Boolean> = _loggedIn

    val loggedIn: Boolean get() = _loggedIn.value

    private val _searchError = MutableStateFlow<String?>(null)
    val searchError: StateFlow<String?> = _searchError

    /** Запрос отправлен, ответа ещё нет: экран показывает индикатор, а не «ничего не нашлось». */
    private val _searching = MutableStateFlow(false)
    val searching: StateFlow<Boolean> = _searching

    fun setQuery(text: String) {
        _query.value = text
        searchJob?.cancel()
        _searchError.value = null
        if (text.isBlank()) {
            _search.value = emptyList()
            _searching.value = false
            return
        }
        _searching.value = true
        searchJob = viewModelScope.launch {
            delay(350)
            val token = plastinka.tokenStore.load()
            if (token == null) {
                _searching.value = false
                return@launch
            }
            try {
                _search.value = plastinka.api.searchTracks(token, text.trim())
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                _search.value = emptyList()
                _searchError.value = "Не удалось выполнить поиск. Проверьте соединение."
            }
            // Отменённый запрос сюда не доходит: флаг держит новый запрос.
            _searching.value = false
        }
    }

    /** Вход по коду устройства: запросить код и ждать подтверждения. */
    fun startLogin() {
        if (_login.value.phase == LoginPhase.Requesting) return
        pollJob?.cancel()
        _login.value = LoginUi(phase = LoginPhase.Requesting)
        viewModelScope.launch {
            try {
                val deviceId = deviceId()
                val code = plastinka.api.requestDeviceCode(deviceId)
                _login.value = LoginUi(phase = LoginPhase.Waiting, code = code)
                poll(code)
            } catch (e: Exception) {
                _login.value = LoginUi(error = e.message ?: "не удалось начать вход")
            }
        }
    }

    private suspend fun poll(code: DeviceCode) {
        val deadline = System.currentTimeMillis() + code.expiresInSeconds * 1000
        // Сетевые сбои — временны: приложение часто в фоне, пока пользователь
        // вводит код в браузере, и телефон может рвать сеть. Повторяем, как в
        // десктопном клиенте (RFC 8628). Отказ сервера — конец сразу.
        var failures = 0
        while (System.currentTimeMillis() < deadline) {
            delay(code.intervalSec * 1000)
            try {
                val token = plastinka.api.pollToken(code)
                if (token != null) {
                    plastinka.tokenStore.save(token)
                    _loggedIn.value = true
                    _login.value = LoginUi(phase = LoginPhase.Done)
                    loadLibrary()
                    return
                }
                failures = 0
            } catch (e: IllegalStateException) {
                _login.value = LoginUi(
                    phase = LoginPhase.Waiting,
                    code = code,
                    error = e.message,
                )
                return
            } catch (e: Exception) {
                failures++
                if (failures >= 6) {
                    _login.value = LoginUi(
                        phase = LoginPhase.Waiting,
                        code = code,
                        error = "нет соединения с сервером Яндекса. Проверьте сеть и повторите",
                    )
                    return
                }
            }
        }
        _login.value = LoginUi(error = "код истёк, начните заново")
    }

    private var waveJob: Job? = null
    private val _waveStarting = MutableStateFlow(false)
    val waveStarting: StateFlow<Boolean> = _waveStarting

    private val _waveError = MutableStateFlow<String?>(null)
    val waveError: StateFlow<String?> = _waveError

    /** Запустить «Мою волну»: сессия rotor + первая порция в очередь. */
    fun startWave(player: PlayerController?) {
        val token = plastinka.tokenStore.load() ?: return
        if (_waveStarting.value) return
        _waveStarting.value = true
        _waveError.value = null
        waveJob = viewModelScope.launch {
            try {
                val batch = plastinka.api.startWave(token)
                ru.plastinka.playback.PlaybackService.waveStarted(
                    batch.session, batch.batchId, batch.from,
                )
                if (batch.tracks.isEmpty()) throw IllegalStateException("волна прислала 0 треков")
                player?.playQueue(batch.tracks, 0, "Моя волна", isWave = true)
            } catch (e: Exception) {
                _waveError.value = "Волна не началась: ${e.message ?: "ошибка сети"}"
            } finally {
                _waveStarting.value = false
            }
        }
    }

    private val _generated = MutableStateFlow<List<ru.plastinka.data.GeneratedPlaylist>>(emptyList())
    val generated: StateFlow<List<ru.plastinka.data.GeneratedPlaylist>> = _generated

    /** Подборки лендинга: «Плейлист дня» и прочие. */
    private fun loadGenerated() {
        val token = plastinka.tokenStore.load() ?: return
        viewModelScope.launch {
            _generated.value = try {
                plastinka.api.personalPlaylists(token)
            } catch (_: Exception) {
                emptyList()
            }
        }
    }

    /** Запустить подборку как очередь. */
    fun playPlaylist(player: PlayerController?, playlist: ru.plastinka.data.GeneratedPlaylist) {
        val token = plastinka.tokenStore.load() ?: return
        viewModelScope.launch {
            try {
                val tracks = plastinka.api.playlistTracks(token, playlist.uid, playlist.kind)
                if (tracks.isNotEmpty()) player?.playQueue(tracks, 0, playlist.title)
            } catch (_: Exception) {
            }
        }
    }

    private val _collection = MutableStateFlow(CollectionUi())
    val collection: StateFlow<CollectionUi> = _collection
    private var collectionJob: Job? = null

    /**
     * Плейлисты, альбомы и исполнители для хаба: три независимых запроса. Сбой
     * одного не трогает остальные и не стирает уже показанный список.
     */
    fun loadCollection() {
        val token = plastinka.tokenStore.load() ?: return
        val uid = _library.value.account?.uid ?: return
        collectionJob?.cancel()
        _collection.value = _collection.value.copy(loading = true)
        collectionJob = viewModelScope.launch {
            val api = plastinka.api
            val playlists = async { attempt { api.userPlaylists(token, uid) } }
            val albums = async { attempt { api.likedAlbums(token, uid) } }
            val artists = async { attempt { api.likedArtists(token, uid) } }
            val (pl, al, ar) = Triple(playlists.await(), albums.await(), artists.await())
            val old = _collection.value
            _collection.value = CollectionUi(
                loading = false,
                playlists = pl ?: old.playlists,
                albums = al ?: old.albums,
                artists = ar ?: old.artists,
                playlistsError = if (pl == null && old.playlists == null) "Не удалось загрузить плейлисты. Проверьте соединение." else null,
                albumsError = if (al == null && old.albums == null) "Не удалось загрузить альбомы. Проверьте соединение." else null,
                artistsError = if (ar == null && old.artists == null) "Не удалось загрузить исполнителей. Проверьте соединение." else null,
            )
        }
    }

    private val _history = MutableStateFlow(HistoryUi())
    val history: StateFlow<HistoryUi> = _history
    private var historyJob: Job? = null

    /** Серверная история: независимый запрос, её сбой не трогает плейлисты, альбомы и исполнителей. */
    fun loadHistory() {
        val token = plastinka.tokenStore.load() ?: return
        historyJob?.cancel()
        _history.value = _history.value.copy(loading = true)
        historyJob = viewModelScope.launch {
            val days = attempt { plastinka.api.history(token) }
            val old = _history.value
            _history.value = HistoryUi(
                loading = false,
                days = days ?: old.days,
                error = if (days == null && old.days == null) {
                    "Не удалось загрузить историю. Проверьте соединение."
                } else {
                    null
                },
            )
        }
    }

    /** Результат или `null` при ошибке сети и API (отмена пробрасывается). */
    private suspend fun <T> attempt(block: suspend () -> T): T? = try {
        block()
    } catch (e: kotlinx.coroutines.CancellationException) {
        throw e
    } catch (_: Exception) {
        null
    }

    /** Треки плейлиста, альбом и исполнитель: у каждого свой загрузчик с ключом. */
    val playlist = KeyedLoader<List<Track>>(
        viewModelScope, "Не удалось загрузить плейлист. Проверьте соединение.",
    )
    val album = KeyedLoader<ru.plastinka.data.AlbumDetails>(
        viewModelScope, "Не удалось загрузить альбом. Проверьте соединение.",
    )
    val artist = KeyedLoader<ru.plastinka.data.ArtistDetails>(
        viewModelScope, "Не удалось загрузить исполнителя. Проверьте соединение.",
    )

    fun loadPlaylist(uid: Long, kind: Long, force: Boolean = false) {
        val token = plastinka.tokenStore.load() ?: return
        playlist.load("$uid:$kind", force) { plastinka.api.playlistTracks(token, uid, kind) }
    }

    fun loadAlbum(id: String, force: Boolean = false) {
        val token = plastinka.tokenStore.load() ?: return
        album.load(id, force) { plastinka.api.albumWithTracks(token, id) }
    }

    fun loadArtist(id: String, force: Boolean = false) {
        val token = plastinka.tokenStore.load() ?: return
        artist.load(id, force) { plastinka.api.artistBrief(token, id) }
    }

    private var libraryJob: Job? = null

    fun loadLibrary() {
        val token = plastinka.tokenStore.load() ?: return
        _library.value = _library.value.copy(loading = true, error = null)
        libraryJob?.cancel()
        libraryJob = viewModelScope.launch {
            try {
                val status = plastinka.api.accountStatus(token)
                // Сообщения о прослушивании разрешены только при подтверждённом Плюсе.
                if (status.hasPlus) {
                    ru.plastinka.playback.AccountHolder.allow(status.uid)
                } else {
                    ru.plastinka.playback.AccountHolder.clear()
                }
                val ids = plastinka.api.likedIds(token, status.uid)
                val tracks = plastinka.api.tracks(token, ids)
                QueueHolder.setLikes(ids.toSet())
                _library.value = LibraryUi(
                    loading = false,
                    tracks = tracks,
                    account = status,
                )
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                ru.plastinka.playback.AccountHolder.clear()
                _library.value = LibraryUi(loading = false, error = e.message ?: "ошибка загрузки")
            }
        }
        viewModelScope.launch { plastinka.fckCensor.refresh() }
    }

    fun logout() {
        plastinka.tokenStore.clear()
        ru.plastinka.playback.AccountHolder.clear() // отложенные play-audio отменяются
        _loggedIn.value = false
        libraryJob?.cancel() // запоздавший ответ не должен вернуть аккаунт после выхода
        QueueHolder.set(emptyList(), -1, "")
        _library.value = LibraryUi()
        collectionJob?.cancel()
        historyJob?.cancel()
        _history.value = HistoryUi()
        ru.plastinka.data.PlayLog.clear()
        playlist.reset()
        album.reset()
        artist.reset()
        _collection.value = CollectionUi()
        _login.value = LoginUi()
    }

    fun toggleLike(track: Track) {
        val token = plastinka.tokenStore.load() ?: return
        val uid = _library.value.account?.uid ?: return
        val nowLiked = QueueHolder.toggleLike(track.id)
        viewModelScope.launch {
            try {
                plastinka.api.setLike(token, uid, track.id, nowLiked)
            } catch (_: Exception) {
                QueueHolder.toggleLike(track.id) // откат
            }
        }
    }

    private fun deviceId(): String {
        val prefs = getApplication<PlastinkaApp>()
            .getSharedPreferences("plastinka", android.content.Context.MODE_PRIVATE)
        var id = prefs.getString("device_id", null)
        if (id == null) {
            id = java.util.UUID.randomUUID().toString()
            prefs.edit().putString("device_id", id).apply()
        }
        return id
    }
}
