package ru.plastinka.ui

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import ru.plastinka.PlastinkaApp
import ru.plastinka.data.AccountInfo
import ru.plastinka.data.DeviceCode
import ru.plastinka.data.Prefs
import ru.plastinka.data.WaveActivity
import ru.plastinka.data.WaveSettings
import ru.plastinka.data.YandexApi
import ru.plastinka.playback.AuthEvents
import ru.plastinka.data.SearchKind
import ru.plastinka.data.Track
import ru.plastinka.playback.QueueHolder
import ru.plastinka.playback.currentTrack

data class LoginUi(
    val phase: LoginPhase = LoginPhase.Idle,
    val code: DeviceCode? = null,
    val error: String? = null,
    /** Пометка над кодом («Вход больше не действует…»). */
    val notice: String? = null,
    /** Токен получен, но не сохранён: экран предлагает «Повторить» сохранение. */
    val retrySave: Boolean = false,
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
    /** Идёт загрузка «Мне нравится» (отдельно от проверки аккаунта). */
    val likesLoading: Boolean = false,
    /** Список лайков загружен: только тогда его правят по изменениям сердец. */
    val likesLoaded: Boolean = false,
    /** Готовый текст ошибки «Мне нравится», только если показать нечего. */
    val likesError: String? = null,
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
    /** Исполнители топа месяца для полки; `null` — не загружены или сбой (полки нет). */
    val top: List<ru.plastinka.data.TopArtist>? = null,
    /** Полки «Встречали в Моей волне», «Новые релизы», «Собираем для вас»: `null` — нет (полку не показываем). */
    val met: List<ru.plastinka.data.ArtistCard>? = null,
    val releases: List<ru.plastinka.data.AlbumCard>? = null,
    val personal: List<ru.plastinka.data.PlaylistCard>? = null,
    val playlistsError: String? = null,
    val albumsError: String? = null,
    val artistsError: String? = null,
)

/**
 * «Все треки» исполнителя: загруженные страницы по 100. `key` — исполнитель, чьи это
 * треки (чужой ключ экран не показывает). Ошибка первой страницы — `error`, ошибка
 * следующей — `moreFailed`: загруженное остаётся.
 */
data class ArtistTracksUi(
    val key: String? = null,
    val loading: Boolean = false,
    val loadingMore: Boolean = false,
    val loaded: Boolean = false,
    val items: List<ru.plastinka.data.Track> = emptyList(),
    val total: Int = 0,
    val nextPage: Int = 0,
    /** Сервис прислал пустую страницу: дальше грузить нечего. */
    val exhausted: Boolean = false,
    val error: String? = null,
    val moreFailed: Boolean = false,
) {
    val hasNext: Boolean get() = loaded && !exhausted && items.size < total
}

/** Страница «Топ месяца». `data == null` — ещё не получена; ошибка хранится, только если показать нечего. */
data class MonthTopUi(
    val loading: Boolean = false,
    val data: ru.plastinka.data.MonthTop? = null,
    val error: String? = null,
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

/** Хранилище ключей Android не приняло токен (как `token_saved` на десктопе). */
private const val KEYSTORE_UNAVAILABLE =
    "Хранилище ключей Android недоступно. Без него Plastinka не запомнит вход. Перезапустите телефон и повторите."

/** Сообщение об ошибке поиска. */
private const val SEARCH_FAILED = "Не удалось выполнить поиск. Проверьте соединение."

/** Состояние одной вкладки поиска. */
data class SearchSlot(
    /** Идёт запрос первой страницы (или ждёт паузу ввода). */
    val loading: Boolean = false,
    val loadingMore: Boolean = false,
    /** Ошибка первой страницы. */
    val error: String? = null,
    /** Ошибка следующей страницы: список остаётся, внизу «Повторить». */
    val moreFailed: Boolean = false,
    val loaded: Boolean = false,
    val result: ru.plastinka.data.SearchResult = ru.plastinka.data.SearchResult(),
    val nextPage: Int = 0,
)

/** Следующая страница дописывается к списку без повторов (ключи элементов LazyColumn уникальны). */
private fun ru.plastinka.data.SearchResult.append(next: ru.plastinka.data.SearchResult) = copy(
    tracks = tracks + next.tracks,
    albums = (albums + next.albums).distinctBy { it.id },
    artists = (artists + next.artists).distinctBy { it.id },
    playlists = (playlists + next.playlists).distinctBy { it.uid to it.kind },
    hasNext = next.hasNext,
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

    /** Пометка экрана входа («Вход больше не действует…»): живёт, пока не войдём или не выйдем. */
    private val _loginNotice = MutableStateFlow<String?>(null)
    val login: StateFlow<LoginUi> = combine(_login, _loginNotice) { ui, notice -> ui.copy(notice = notice) }
        .stateIn(viewModelScope, SharingStarted.Eagerly, LoginUi())

    /** Полученный токен, который не удалось сохранить: только в памяти, до успеха или нового входа. */
    private var unsavedToken: String? = null

    private val _library = MutableStateFlow(LibraryUi())
    val library: StateFlow<LibraryUi> = _library

    /** Адрес аватара из Яндекс ID: только в памяти, на диск не пишется. `null` — аватара нет. */
    private val _avatarUrl = MutableStateFlow<String?>(null)
    val avatarUrl: StateFlow<String?> = _avatarUrl
    private var avatarJob: Job? = null

    private val _query = MutableStateFlow("")
    val query: StateFlow<String> = _query

    private var pollJob: Job? = null

    // Состояние, а не геттер: экран перерисовывается и при входе, и при выходе.
    private val _loggedIn = MutableStateFlow(plastinka.tokenStore.load() != null)
    val loggedInState: StateFlow<Boolean> = _loggedIn

    val loggedIn: Boolean get() = _loggedIn.value

    /** Выбранная вкладка поиска: сохраняется при смене запроса. */
    private val _searchTab = MutableStateFlow(SearchKind.All)
    val searchTab: StateFlow<SearchKind> = _searchTab

    /** Состояние каждой вкладки для текущего запроса; нет записи — вкладку ещё не открывали. */
    private val _searchSlots = MutableStateFlow<Map<SearchKind, SearchSlot>>(emptyMap())
    val searchSlots: StateFlow<Map<SearchKind, SearchSlot>> = _searchSlots

    private val searchJobs = mutableMapOf<SearchKind, Job>()

    /** Номер запроса: ответ с прежним номером (запрос уже сменился) отбрасывается. */
    private var searchGen = 0

    fun setQuery(text: String) {
        _query.value = text
        searchGen++
        searchJobs.values.forEach { it.cancel() }
        searchJobs.clear()
        _searchSlots.value = emptyMap()
        if (text.isBlank()) return
        runSearch(_searchTab.value, page = 0, debounce = true)
    }

    fun selectSearchTab(kind: SearchKind) {
        _searchTab.value = kind
        if (_query.value.isBlank()) return
        // Вкладку ещё не открывали или прошлый запрос не удался — запрашиваем (заново).
        val slot = _searchSlots.value[kind]
        if (slot == null || slot.error != null) runSearch(kind, page = 0, debounce = false)
    }

    /** «Повторить» после ошибки первой страницы. */
    fun retrySearch(kind: SearchKind) {
        if (_query.value.isBlank() || _searchSlots.value[kind]?.loading == true) return
        runSearch(kind, page = 0, debounce = false)
    }

    /** Следующая страница вкладки по типу: вызывается при прокрутке к концу списка. */
    fun loadMoreSearch(kind: SearchKind) {
        val slot = _searchSlots.value[kind] ?: return
        if (kind == SearchKind.All || !slot.loaded || slot.loading || slot.loadingMore) return
        if (!slot.result.hasNext) return
        runSearch(kind, page = slot.nextPage, debounce = false)
    }

    private fun updateSlot(kind: SearchKind, change: (SearchSlot) -> SearchSlot) {
        _searchSlots.value = _searchSlots.value + (kind to change(_searchSlots.value[kind] ?: SearchSlot()))
    }

    private fun runSearch(kind: SearchKind, page: Int, debounce: Boolean) {
        val text = _query.value.trim()
        if (text.isEmpty()) return
        val gen = searchGen
        searchJobs[kind]?.cancel()
        updateSlot(kind) {
            if (page == 0) SearchSlot(loading = true) else it.copy(loadingMore = true, moreFailed = false)
        }
        searchJobs[kind] = viewModelScope.launch {
            if (debounce) delay(350)
            val token = plastinka.tokenStore.load()
            if (token == null) {
                updateSlot(kind) { SearchSlot(error = SEARCH_FAILED) }
                return@launch
            }
            try {
                val fresh = plastinka.api.search(token, kind, text, page)
                if (gen != searchGen) return@launch
                updateSlot(kind) { old ->
                    val merged = if (page == 0) fresh else old.result.append(fresh)
                    SearchSlot(loaded = true, result = merged, nextPage = page + 1)
                }
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                if (gen != searchGen) return@launch
                updateSlot(kind) { old ->
                    if (page == 0) SearchSlot(error = SEARCH_FAILED) else old.copy(loadingMore = false, moreFailed = true)
                }
            }
        }
    }

    /** Вход по коду устройства: запросить код и ждать подтверждения. */
    fun startLogin() {
        if (_login.value.phase == LoginPhase.Requesting) return
        pollJob?.cancel()
        unsavedToken = null
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
            var token: String? = null
            try {
                token = plastinka.api.pollToken(code)
                if (token == null) failures = 0
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
            // Сохранение вне try: его сбой не сетевой, опрос кода повторять незачем.
            if (token != null) {
                finishLogin(token)
                return
            }
        }
        _login.value = LoginUi(error = "код истёк, начните заново")
    }

    /** Токен получен: сохраняем в хранилище ключей; не вышло — экран с «Повторить», токен остаётся в памяти. */
    private suspend fun finishLogin(token: String) {
        val saved = kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) {
            plastinka.tokenStore.save(token)
        }
        if (!saved) {
            unsavedToken = token
            _login.value = LoginUi(error = KEYSTORE_UNAVAILABLE, retrySave = true)
            return
        }
        unsavedToken = null
        _loginNotice.value = null
        _loggedIn.value = true
        _login.value = LoginUi(phase = LoginPhase.Done)
        loadLibrary()
    }

    /** «Повторить» на экране «Не удалось сохранить вход»: то же сохранение того же токена. */
    fun retrySaveToken() {
        val token = unsavedToken ?: return
        viewModelScope.launch { finishLogin(token) }
    }

    private var waveJob: Job? = null
    private val _waveStarting = MutableStateFlow(false)
    val waveStarting: StateFlow<Boolean> = _waveStarting

    private val _waveError = MutableStateFlow<String?>(null)
    val waveError: StateFlow<String?> = _waveError

    init {
        // Сервис воспроизведения получил 401 от потока: токен не принят.
        viewModelScope.launch { AuthEvents.relogin.collect { relogin() } }
        viewModelScope.launch { QueueHolder.likedIds.collect(::syncLikedList) }
        // Ошибка запуска волны устаревает, когда пользователь включил другой список:
        // иначе вкладка «Волна» так и показывала бы её вместо играющего трека.
        viewModelScope.launch {
            var seen: List<Track>? = null
            ru.plastinka.playback.QueueHolder.state.collect { queue ->
                val fresh = seen != null && seen !== queue.tracks
                seen = queue.tracks
                if (fresh && !queue.isWave) _waveError.value = null
            }
        }
    }

    /** Номер запуска волны: результат прежнего запуска (его отменили сменой настроек) не применяется. */
    private var waveGen = 0

    /**
     * Запустить «Мою волну»: сессия rotor с выбранными настройками + первая порция в
     * очередь. `restart` — смена настроек: прежний запуск (если ещё идёт) отменяется.
     */
    fun startWave(player: PlayerController?, restart: Boolean = false) {
        val token = plastinka.tokenStore.load() ?: return
        if (_waveStarting.value && !restart) return
        waveJob?.cancel()
        val gen = ++waveGen
        _waveStarting.value = true
        _waveError.value = null
        val seeds = Prefs.waveSettings.seeds()
        waveJob = viewModelScope.launch {
            try {
                val batch = plastinka.api.startWave(token, seeds)
                ru.plastinka.playback.PlaybackService.waveStarted(
                    batch.session, batch.batchId, batch.from,
                )
                if (batch.tracks.isEmpty()) throw IllegalStateException("волна прислала 0 треков")
                player?.playQueue(batch.tracks, 0, "Моя волна", isWave = true)
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                _waveError.value = "Волна не началась: ${e.message ?: "ошибка сети"}"
            } finally {
                if (gen == waveGen) _waveStarting.value = false
            }
        }
    }

    /**
     * Выбор в настройках волны (как на десктопе): сохраняется на устройстве, в аккаунт ничего
     * не пишется; любая смена сразу запускает новую волну. Тот же выбор — ничего не меняет.
     */
    fun changeWaveSettings(player: PlayerController?, next: WaveSettings) {
        if (next == Prefs.waveSettings) return
        Prefs.changeWaveSettings(next)
        startWave(player, restart = true)
    }

    /** Занятия для настроек волны: не загрузились — раздел «Занятие» скрыт, остальное работает. */
    sealed interface WaveStations {
        data object Idle : WaveStations
        data object Loading : WaveStations
        data class Ready(val list: List<WaveActivity>) : WaveStations
        data object Failed : WaveStations
    }

    private val _waveStations = MutableStateFlow<WaveStations>(WaveStations.Idle)
    val waveStations: StateFlow<WaveStations> = _waveStations

    /** Список занятий: при показе экрана волны и снова, если прошлый запрос не удался. */
    fun loadWaveStations() {
        val token = plastinka.tokenStore.load() ?: return
        if (_waveStations.value is WaveStations.Loading || _waveStations.value is WaveStations.Ready) return
        _waveStations.value = WaveStations.Loading
        viewModelScope.launch {
            _waveStations.value = try {
                WaveStations.Ready(plastinka.api.waveActivities(token))
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (_: Exception) {
                WaveStations.Failed
            }
        }
    }

    /**
     * «Не нравится» играющему треку волны: логика общая с кнопкой в системном плеере
     * ([TrackActions.dislikeCurrent]). Только при подтверждённой подписке.
     */
    fun dislikeWaveTrack(player: PlayerController?) {
        if (!QueueHolder.state.value.isWave) return
        ru.plastinka.playback.TrackActions.dislikeCurrent(plastinka, viewModelScope, player?.controller)
    }

    /** Кнопка Play у лучшего результата: трек — сам трек, остальное — его треки (у исполнителя популярные). */
    fun playSearchBest(player: PlayerController?, best: ru.plastinka.data.SearchBest) {
        val token = plastinka.tokenStore.load() ?: return
        viewModelScope.launch {
            try {
                val api = plastinka.api
                when (best) {
                    is ru.plastinka.data.SearchBest.TrackHit -> player?.playQueue(listOf(best.track), 0, "Поиск")
                    is ru.plastinka.data.SearchBest.AlbumHit -> {
                        val tracks = albumDetails(token, best.card.id).tracks
                        val ctx = ru.plastinka.data.HistoryContext.Album(best.card.id, best.card.displayTitle)
                        player?.playQueue(tracks, 0, best.card.displayTitle, source = ctx)
                    }
                    is ru.plastinka.data.SearchBest.PlaylistHit -> {
                        val tracks = api.playlistTracks(token, best.card.uid, best.card.kind)
                        val ctx = ru.plastinka.data.HistoryContext.Playlist(
                            best.card.uid.toString(), best.card.kind.toString(), best.card.title,
                        )
                        player?.playQueue(tracks, 0, best.card.title, source = ctx)
                    }
                    is ru.plastinka.data.SearchBest.ArtistHit -> {
                        val tracks = api.artistBrief(token, best.card.id).popular
                        val ctx = ru.plastinka.data.HistoryContext.Artist(best.card.id, best.card.name)
                        player?.playQueue(tracks, 0, best.card.name, source = ctx)
                    }
                }
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (_: Exception) {
                // Сбой сети: очередь остаётся прежней, экран не падает.
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
            // Полка топа необязательна: сбой её не показывает, остальное работает.
            val top = async { attempt { api.topArtists(token) } }
            // Остальные полки тоже необязательны.
            val met = async { attempt { api.metArtists(token) } }
            val releases = async { attempt { api.newReleases(token) } }
            val personal = async { attempt { api.personalShelf(token) } }
            val (pl, al, ar) = Triple(playlists.await(), albums.await(), artists.await())
            val tp = top.await()
            val (mt, rl, ps) = Triple(met.await(), releases.await(), personal.await())
            val old = _collection.value
            _collection.value = CollectionUi(
                loading = false,
                playlists = pl ?: old.playlists,
                albums = al ?: old.albums,
                artists = ar ?: old.artists,
                top = tp ?: old.top,
                met = mt ?: old.met,
                releases = rl ?: old.releases,
                personal = ps ?: old.personal,
                playlistsError = if (pl == null && old.playlists == null) "Не удалось загрузить плейлисты. Проверьте соединение." else null,
                albumsError = if (al == null && old.albums == null) "Не удалось загрузить альбомы. Проверьте соединение." else null,
                artistsError = if (ar == null && old.artists == null) "Не удалось загрузить исполнителей. Проверьте соединение." else null,
            )
        }
    }

    private val _monthTop = MutableStateFlow(MonthTopUi())
    val monthTop: StateFlow<MonthTopUi> = _monthTop
    private var monthTopJob: Job? = null

    /**
     * Страница «Топ месяца». Без `force` не спрашивает заново, пока данные есть
     * или запрос идёт; «Повторить» идёт с `force`.
     */
    fun loadMonthTop(force: Boolean = false) {
        val token = plastinka.tokenStore.load() ?: return
        if (!force && (_monthTop.value.data != null || monthTopJob?.isActive == true)) return
        monthTopJob?.cancel()
        _monthTop.value = _monthTop.value.copy(loading = true)
        monthTopJob = viewModelScope.launch {
            val top = attempt { plastinka.api.monthTop(token) }
            val old = _monthTop.value
            _monthTop.value = MonthTopUi(
                loading = false,
                data = top ?: old.data,
                error = if (top == null && old.data == null) "Не удалось собрать топ. Проверьте соединение." else null,
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
        album.load(id, force) { albumDetails(token, id) }
    }

    /** Альбом Яндекса; если FckCensor знает его состав (`volumes`), то с составом из списка. */
    private suspend fun albumDetails(token: String, id: String): ru.plastinka.data.AlbumDetails =
        plastinka.fckCensor.withAlbumVolumes(plastinka.api, token, plastinka.api.albumWithTracks(token, id))

    fun loadArtist(id: String, force: Boolean = false) {
        val token = plastinka.tokenStore.load() ?: return
        artist.load(id, force) { plastinka.api.artistBrief(token, id) }
    }

    private val _artistTracks = MutableStateFlow(ArtistTracksUi())
    val artistTracks: StateFlow<ArtistTracksUi> = _artistTracks
    private var artistTracksJob: Job? = null

    /** Номер загрузки списка: ответ с прежним номером (исполнитель сменился) отбрасывается. */
    private var artistTracksGen = 0

    /** Первая страница «Всех треков». Тот же исполнитель, уже загруженный или грузящийся, не перезапрашивается. */
    fun loadArtistTracks(id: String, force: Boolean = false) {
        val token = plastinka.tokenStore.load() ?: return
        val cur = _artistTracks.value
        if (!force && cur.key == id && (cur.loaded || cur.loading)) return
        artistTracksJob?.cancel()
        val gen = ++artistTracksGen
        _artistTracks.value = ArtistTracksUi(key = id, loading = true)
        artistTracksJob = viewModelScope.launch {
            try {
                val page = plastinka.api.artistTracks(token, id, 0)
                if (gen != artistTracksGen) return@launch
                _artistTracks.value = ArtistTracksUi(
                    key = id,
                    loaded = true,
                    items = page.tracks,
                    total = page.total,
                    nextPage = 1,
                    exhausted = page.fetched == 0,
                )
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                if (gen != artistTracksGen) return@launch
                _artistTracks.value = ArtistTracksUi(
                    key = id,
                    error = if (e is ru.plastinka.data.NotFoundException) {
                        "Не найдено"
                    } else {
                        "Не удалось загрузить треки. Проверьте соединение."
                    },
                )
            }
        }
    }

    /**
     * Следующая страница: вызывается при прокрутке к концу и по «Повторить». Страница уже
     * грузится, список другого исполнителя или конец — ничего не делает.
     */
    fun loadMoreArtistTracks(id: String) {
        val token = plastinka.tokenStore.load() ?: return
        val cur = _artistTracks.value
        if (cur.key != id || !cur.hasNext || cur.loading || cur.loadingMore) return
        val gen = artistTracksGen
        val page = cur.nextPage
        _artistTracks.value = cur.copy(loadingMore = true, moreFailed = false)
        artistTracksJob = viewModelScope.launch {
            try {
                val next = plastinka.api.artistTracks(token, id, page)
                if (gen != artistTracksGen) return@launch
                val old = _artistTracks.value
                val known = old.items.mapTo(HashSet()) { it.id }
                _artistTracks.value = old.copy(
                    loadingMore = false,
                    items = old.items + next.tracks.filter { known.add(it.id) },
                    total = maxOf(old.total, next.total),
                    nextPage = page + 1,
                    exhausted = next.fetched == 0,
                )
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (_: Exception) {
                if (gen != artistTracksGen) return@launch
                _artistTracks.value = _artistTracks.value.copy(loadingMore = false, moreFailed = true)
            }
        }
    }

    private var libraryJob: Job? = null

    /** Загружает библиотеку, только если её ещё нет и загрузка не идёт (например, не после поворота экрана). */
    fun ensureLibrary() {
        if (!loggedIn) return
        if (_library.value.account != null || libraryJob?.isActive == true) return
        loadLibrary()
    }

    /**
     * Сначала статус аккаунта: Плюс есть — главный экран открывается сразу, не дожидаясь
     * лайков. Ошибка статуса не считается подпиской (правило 2). Уже подтверждённый
     * аккаунт сбой сети не выбрасывает: `AccountHolder` очищается только при выходе,
     * отсутствии Плюса и повторном входе.
     */
    fun loadLibrary() {
        val token = plastinka.tokenStore.load() ?: return
        _library.value = _library.value.copy(loading = true, error = null)
        libraryJob?.cancel()
        libraryJob = viewModelScope.launch {
            val known = _library.value.account != null
            val status = try {
                plastinka.api.accountStatus(token)
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                if (e is YandexApi.UnauthorizedException) {
                    relogin()
                } else if (known) {
                    _library.value = _library.value.copy(loading = false)
                    toast("Не удалось обновить данные аккаунта. Проверьте соединение")
                } else {
                    _library.value = LibraryUi(loading = false, error = e.message ?: "ошибка загрузки")
                }
                return@launch
            }
            if (!status.hasPlus) {
                ru.plastinka.playback.AccountHolder.clear()
                _library.value = LibraryUi(account = status)
                return@launch
            }
            // Сообщения о прослушивании разрешены только при подтверждённом Плюсе.
            ru.plastinka.playback.AccountHolder.allow(status.uid)
            loadAvatar(token)
            _library.value = _library.value.copy(
                loading = false,
                error = null,
                account = status,
                likesLoading = true,
                likesError = null,
            )
            fetchLikes(token, status.uid)
        }
        viewModelScope.launch { plastinka.fckCensor.refresh() }
    }

    /** «Повторить» в «Мне нравится»: перезагружаются только лайки. */
    fun retryLikes() {
        val token = plastinka.tokenStore.load() ?: return
        val uid = _library.value.account?.uid ?: return
        if (libraryJob?.isActive == true) return
        _library.value = _library.value.copy(likesLoading = true, likesError = null)
        libraryJob = viewModelScope.launch { fetchLikes(token, uid) }
    }

    private suspend fun fetchLikes(token: String, uid: Long) {
        try {
            val ids = plastinka.api.likedIds(token, uid)
            val tracks = plastinka.api.tracks(token, ids)
            // Пока набор сердец заменяется, список не сверяем с ним (likesLoaded = false).
            _library.value = _library.value.copy(likesLoaded = false)
            QueueHolder.setLikes(ids.toSet())
            _library.value = _library.value.copy(
                tracks = tracks,
                likesLoaded = true,
                likesLoading = false,
                likesError = null,
            )
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            if (e is YandexApi.UnauthorizedException) {
                relogin()
                return
            }
            // Список уже был — остаётся, ошибка только тостом; иначе экран покажет блок с «Повторить».
            val had = _library.value.tracks.isNotEmpty()
            _library.value = _library.value.copy(
                likesLoading = false,
                likesError = if (had) null else likesErrorText(e),
            )
            if (had) toast("Не удалось обновить «Мне нравится»")
        }
    }

    private fun likesErrorText(e: Exception): String =
        if (e is java.io.IOException) {
            "Нет соединения с сервером. Проверьте интернет и попробуйте ещё раз."
        } else {
            e.message?.takeIf { it.isNotBlank() } ?: "Неизвестная ошибка"
        }

    private fun toast(text: String) {
        android.widget.Toast.makeText(getApplication<PlastinkaApp>(), text, android.widget.Toast.LENGTH_SHORT).show()
    }

    /** Один запрос после подтверждения входа и Плюса; уже полученный адрес не перезапрашивается. */
    private fun loadAvatar(token: String) {
        if (_avatarUrl.value != null || avatarJob?.isActive == true) return
        avatarJob = viewModelScope.launch { _avatarUrl.value = plastinka.api.avatarUrl(token) }
    }

    fun logout() {
        plastinka.tokenStore.clear()
        resetSession(clearHistory = true)
        _loginNotice.value = null
    }

    /**
     * Токен не принят (401): как на десктопе, экран входа с пометкой и сразу запрос кода.
     * Токен из хранилища не удаляется: новый вход его перезапишет, а при перезапуске
     * приложение проверит его ещё раз. Не в аккаунте — ничего не делает.
     */
    fun relogin() {
        if (!_loggedIn.value) return
        resetSession(clearHistory = false)
        _loginNotice.value = "Вход больше не действует. Войдите заново."
        startLogin()
    }

    /** Общий сброс состояния аккаунта для выхода и повторного входа (очередь останавливает уход с главного экрана). */
    private fun resetSession(clearHistory: Boolean) {
        avatarJob?.cancel()
        _avatarUrl.value = null
        ru.plastinka.playback.AccountHolder.clear() // отложенные play-audio отменяются
        _loggedIn.value = false
        unsavedToken = null
        knownTracks.clear()
        removedLikes.clear()
        libraryJob?.cancel() // запоздавший ответ не должен вернуть аккаунт после выхода
        QueueHolder.set(emptyList(), -1, "")
        _library.value = LibraryUi()
        collectionJob?.cancel()
        historyJob?.cancel()
        monthTopJob?.cancel()
        _monthTop.value = MonthTopUi()
        _history.value = HistoryUi()
        if (clearHistory) ru.plastinka.data.PlayLog.clear()
        playlist.reset()
        album.reset()
        artist.reset()
        artistTracksJob?.cancel()
        artistTracksGen++
        _artistTracks.value = ArtistTracksUi()
        _collection.value = CollectionUi()
        _waveStations.value = WaveStations.Idle
        _login.value = LoginUi()
    }

    fun toggleLike(track: Track) {
        knownTracks[track.id] = track
        ru.plastinka.playback.TrackActions.toggleLike(plastinka, viewModelScope, track.id)
    }

    /** Треки, которые нажимали в приложении: из них строка встаёт в «Мне нравится», когда сердце поставлено. */
    private val knownTracks = HashMap<String, Track>()

    /** Строки, ушедшие из списка при снятии сердца: при откате возвращаются на то же место. */
    private val removedLikes = HashMap<String, Pair<Int, Track>>()

    /**
     * «Мне нравится» меняется сразу, как `edit_likes` на десктопе. Источник правды —
     * [QueueHolder.likedIds]: его меняют и приложение, и шторка. Ушёл id — строка исчезает,
     * добавился — трек встаёт в начало (откат ошибки — на прежнее место), если он известен
     * (из нажатия в приложении или из очереди); неизвестный появится при следующей загрузке.
     */
    private fun syncLikedList(ids: Set<String>) {
        val lib = _library.value
        if (!lib.likesLoaded) return
        var list = lib.tracks
        val gone = list.withIndex().filter { it.value.id !in ids }
        var changed = gone.isNotEmpty()
        if (changed) {
            gone.forEach { removedLikes[it.value.id] = it.index to it.value }
            list = list.filter { it.id in ids }
        }
        val have = list.mapTo(HashSet()) { it.id }
        for (id in ids) {
            if (id in have) continue
            val rolledBack = QueueHolder.consumeRollback(id)
            val old = removedLikes.remove(id)
            if (rolledBack && old != null) {
                list = list.toMutableList().also { it.add(old.first.coerceIn(0, it.size), old.second) }
                have.add(id)
                changed = true
                continue
            }
            val track = knownTracks[id] ?: old?.second
                ?: QueueHolder.state.value.tracks.firstOrNull { it.id == id }
                ?: continue
            list = listOf(track) + list
            have.add(id)
            changed = true
        }
        if (changed) _library.value = _library.value.copy(tracks = list)
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
