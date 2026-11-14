package ru.plastinka.ui

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Job
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

    val loggedIn: Boolean get() = plastinka.tokenStore.load() != null

    fun setQuery(text: String) {
        _query.value = text
        searchJob?.cancel()
        if (text.isBlank()) {
            _search.value = emptyList()
            return
        }
        searchJob = viewModelScope.launch {
            delay(350)
            val token = plastinka.tokenStore.load() ?: return@launch
            _search.value = try {
                plastinka.api.searchTracks(token, text.trim())
            } catch (e: Exception) {
                emptyList()
            }
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

    /** Запустить «Мою волну»: сессия rotor + первая порция в очередь. */
    fun startWave(player: PlayerController?) {
        val token = plastinka.tokenStore.load() ?: return
        if (_waveStarting.value) return
        _waveStarting.value = true
        waveJob = viewModelScope.launch {
            try {
                val batch = plastinka.api.startWave(token)
                ru.plastinka.playback.PlaybackService.waveStarted(
                    batch.session, batch.batchId, batch.from,
                )
                if (batch.tracks.isEmpty()) throw IllegalStateException("волна прислала 0 треков")
                player?.playQueue(batch.tracks, 0, "Моя волна", isWave = true)
            } catch (e: Exception) {
                _library.value = _library.value.copy(error = "Волна не началась: ${e.message}")
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

    /** Скачать трек в кэш или убрать из кэша. */
    fun toggleCache(track: ru.plastinka.data.Track) {
        val token = plastinka.tokenStore.load() ?: return
        viewModelScope.launch {
            if (ru.plastinka.data.TrackCache.isCached(track.id)) {
                ru.plastinka.data.TrackCache.remove(track.id)
            } else {
                ru.plastinka.data.TrackCache.download(
                    plastinka.api, plastinka.fckCensor, token, track,
                )
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

    fun loadLibrary() {
        val token = plastinka.tokenStore.load() ?: return
        _library.value = _library.value.copy(loading = true, error = null)
        viewModelScope.launch {
            try {
                val status = plastinka.api.accountStatus(token)
                val ids = plastinka.api.likedIds(token, status.uid)
                val tracks = plastinka.api.tracks(token, ids)
                QueueHolder.setLikes(ids.toSet())
                _library.value = LibraryUi(
                    loading = false,
                    tracks = tracks,
                    account = status,
                )
            } catch (e: Exception) {
                _library.value = LibraryUi(loading = false, error = e.message ?: "ошибка загрузки")
            }
        }
        viewModelScope.launch { plastinka.fckCensor.refresh() }
    }

    fun logout() {
        plastinka.tokenStore.clear()
        QueueHolder.set(emptyList(), -1, "")
        _library.value = LibraryUi()
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
