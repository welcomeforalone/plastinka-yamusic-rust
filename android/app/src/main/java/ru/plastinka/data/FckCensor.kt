package ru.plastinka.data

import android.content.ContentResolver
import android.content.Intent
import android.net.Uri
import android.util.Log
import androidx.compose.runtime.mutableIntStateOf
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put
import okhttp3.OkHttpClient
import java.io.File

/** Состояние списка FckCensor для страницы плагинов. */
sealed interface FckState {
    /** Плагин выключен: список не запрашивался. */
    data object Off : FckState
    data object Loading : FckState
    data class Ready(val count: Int) : FckState
    data class Failed(val message: String) : FckState
}

/** Откуда берётся звук трека. */
sealed interface FckSound {
    /** Прямая ссылка на файл без цензуры из хранилища списка. */
    data class Direct(val url: String) : FckSound

    /** Поток трека Яндекса: самого трека или другого (замена, найденная explicit-версия). */
    data class Yandex(val id: String) : FckSound

    /** Свой файл пользователя (замена): адрес `content://` с постоянным разрешением на чтение. */
    data class File(val uri: String) : FckSound

    /** Explicit-версию ещё ищем: что заиграет, пока неизвестно. */
    data object Pending : FckSound
}

/** Итог действия с заменой: что показать пользователю; [ok] — действие выполнено. */
data class ReplaceOutcome(val message: String, val ok: Boolean = true)

/**
 * Что реально играет вместо трека очереди. [loudness] — громкость трека Яндекса, у которого
 * берётся звук, если это не сам трек (для нормализации); только в памяти.
 */
data class FckPlaying(val sound: FckSound, val loudness: Loudness?)

/**
 * FckCensor: список без цензуры Hazzz895/FckCensorData (`list_v2.json`). Из него берём
 * названия, исполнителей и обложки без цензуры ([TrackPatch], [AlbumPatch]) и хранилища
 * звука (`tracksStorages`): прямые ссылки на файлы без цензуры, `urlTemplate` с `%%`.
 *
 * Подмены применяет сама модель: [Track.title], [Track.artists], [Track.coverUri] и такие же
 * поля [AlbumCard] читают [trackPatch] и [albumPatch]. Поэтому все экраны, плеер и шторка
 * показывают одно и то же без правок в каждом месте. Чтение идёт через снимок состояния
 * Compose: включили или выключили плагин, пришёл новый список — экраны перерисовываются
 * сразу. Идентификатор трека всегда настоящий.
 *
 * Плагин выключен по умолчанию: пока он выключен, список не запрашивается, а [trackPatch],
 * [albumPatch] всегда `null`, звук не меняется и explicit-версии не ищутся. Список живёт
 * в памяти и в кэше на диске (`cacheDir/plugins/fckcensor-list.json`, только метаданные): раз в
 * 6 часов обновляется, без сети работает по кэшу. Звук на диск не пишется.
 *
 * Откуда брать звук трека, решает одно место — [resolveSound], по порядку десктопа:
 * 1. ручная замена ([Replacements]): свой файл или другой трек Яндекса, выше списка;
 * 2. `audioSourceId` из списка — другой трек Яндекса;
 * 3. прямая ссылка из хранилища списка (`tracksStorages`);
 * 4. explicit-версия того же трека на Яндексе (автопоиск, если включён), для треков с
 *    цензурной версией; найденное запоминает [ExplicitCache] (только номера);
 * 5. сам трек.
 * Что решено, запоминается в памяти ([playingOf]): по нему значок качества, нормализация и
 * `play-audio` работают с тем, что реально играет. Ручные замены лежат в `filesDir` (номера и
 * адреса файлов, не звук), действуют и в очереди, «Сбросить всё» их не трогает. Свой файл
 * только читается плеером по `content://`: он нигде не копируется и не пишется.
 */
class FckCensor(
    private val http: OkHttpClient,
    private val cacheDir: File,
    filesDir: File,
    private val resolver: ContentResolver,
) {

    private val explicit = ExplicitCache(File(File(cacheDir, "plugins"), EXPLICIT_FILE))
    private val replacements = Replacements(File(filesDir, Replacements.FILE_NAME))

    @Volatile private var fetchedAtMs: Long = 0
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val mutex = Mutex()

    init {
        // Замены читаются в фоне; экраны перерисуются, когда файл прочитан.
        scope.launch { replacements.ensureLoaded() }
    }

    private val _state = MutableStateFlow<FckState>(FckState.Off)
    val state: StateFlow<FckState> = _state

    // ------------------------------------------------------------ решение о звуке

    /** Что играть по списку и кэшу, без сети. `null` — нужно искать explicit-версию. */
    private fun decideQuick(id: String, track: Track?): FckSound? {
        if (!enabled) return FckSound.Yandex(id)
        // Ручная замена выше списка (как `Censor::sound` на десктопе); свой файл выше трека.
        when (val manual = replacements.get(id)) {
            is Replacement.LocalFile -> return FckSound.File(manual.uri)
            is Replacement.YandexTrack -> return FckSound.Yandex(manual.id)
            null -> Unit
        }
        val list = active
        list.tracks[id]?.audioSourceId?.let { return FckSound.Yandex(it) }
        list.urls[id]?.let { return FckSound.Direct(it) }
        // Недоступному треку без замены искать нечего; неизвестный трек (нет в очереди) играет как есть.
        if (track == null || !track.available) return FckSound.Yandex(id)
        if (!explicitSearch || !list.censored(track)) return FckSound.Yandex(id)
        return when (val hit = explicit.get(id)) {
            ExplicitLookup.Missing -> null
            ExplicitLookup.NotFound -> FckSound.Yandex(id)
            is ExplicitLookup.Found -> FckSound.Yandex(hit.id)
        }
    }

    /**
     * Звук трека [id]; [track] — он же из очереди (`null`, если его там нет). Вызывается при
     * открытии потока, на потоке загрузчика плеера: поиск explicit-версии блокирует его не
     * дольше [SEARCH_TIMEOUT_MS], ответ из кэша мгновенный. Найти ничего не удалось (нет сети,
     * долго) — играет сам трек, без сообщения и без записи в кэш. Результат запоминается для
     * [playingOf]. Следующий трек плеер открывает заранее, поэтому поиск идёт до его начала.
     */
    fun resolveSound(api: YandexApi, token: String, id: String, track: Track?): FckSound {
        explicit.ensureLoaded()
        replacements.ensureLoaded()
        val sound = decideQuick(id, track)
            ?: track?.let { findExplicit(api, token, it) }
            ?: FckSound.Yandex(id)
        val other = (sound as? FckSound.Yandex)?.id?.takeIf { it != id }
        remember(id, FckPlaying(sound, other?.let { loudnessOfTrack(api, token, it) }))
        return sound
    }

    /**
     * Поиск `GET /search?type=track` по «исполнитель название» и подбор explicit-версии
     * ([ExplicitMatch.pick]). Ответ (даже «нет такой») пишется в кэш; ошибка и таймаут — нет.
     */
    private fun findExplicit(api: YandexApi, token: String, track: Track): FckSound {
        val own = FckSound.Yandex(track.id)
        if (track.sourceTitle.isBlank()) {
            explicit.put(track.id, null)
            return own
        }
        val artist = track.sourceArtists.firstOrNull()?.name.orEmpty()
        val query = "$artist ${track.sourceTitle}".trim()
        val result = try {
            runBlocking { withTimeoutOrNull(SEARCH_TIMEOUT_MS) { api.search(token, SearchKind.Track, query) } }
        } catch (e: InterruptedException) {
            // Загрузку отменили: возвращаем флаг и играем как есть.
            Thread.currentThread().interrupt()
            return own
        } catch (e: Exception) {
            Log.w(TAG, "поиск explicit-версии не удался (трек ${track.id}): ${e.javaClass.simpleName}")
            return own
        }
        if (result == null) {
            Log.w(TAG, "поиск explicit-версии слишком долгий (трек ${track.id})")
            return own
        }
        val pick = ExplicitMatch.pick(track, result.tracks)
        pick?.loudness?.let { loudnessMemo[pick.id] = it }
        explicit.put(track.id, pick?.id)
        return FckSound.Yandex(pick?.id ?: track.id)
    }

    /** Громкость трека Яндекса для нормализации замены: из памяти или одним запросом метаданных. */
    private fun loudnessOfTrack(api: YandexApi, token: String, id: String): Loudness? {
        loudnessMemo[id]?.let { return it }
        val loud = try {
            runBlocking { api.tracks(token, listOf(id)) }.firstOrNull { it.id == id }?.loudness
        } catch (e: InterruptedException) {
            Thread.currentThread().interrupt()
            null
        } catch (e: Exception) {
            null
        }
        if (loud != null) {
            if (loudnessMemo.size > LOUDNESS_MEMO_MAX) loudnessMemo.clear()
            loudnessMemo[id] = loud
        }
        return loud
    }

    /**
     * Что играет вместо [track] сейчас: решение, принятое при открытии потока, а пока его нет —
     * решение по списку и кэшу (`null` — плагин ничего не меняет). Безопасно на любом потоке.
     */
    fun playingOf(track: Track): FckPlaying? {
        synchronized(playing) { playing[track.id] }?.let { return it }
        if (!enabled) return null
        val sound = decideQuick(track.id, track) ?: return FckPlaying(FckSound.Pending, null)
        val other = (sound as? FckSound.Yandex)?.id?.takeIf { it != track.id }
        return FckPlaying(sound, other?.let { loudnessMemo[it] })
    }

    /** Громкость того, что играет, для нормализации: у чужого звука — его громкость, неизвестна — `null`. */
    fun loudnessOf(track: Track): Loudness? {
        val now = playingOf(track) ?: return track.loudness
        return when (val sound = now.sound) {
            is FckSound.Direct, is FckSound.File, FckSound.Pending -> null
            is FckSound.Yandex -> if (sound.id == track.id) track.loudness else now.loudness
        }
    }

    /** Номер трека Яндекса для `play-audio`. `null` — звук не с Яндекса (или ещё не ясно): не сообщаем. */
    fun reportedId(track: Track): String? = when (val sound = playingOf(track)?.sound) {
        null -> track.id
        is FckSound.Yandex -> sound.id
        is FckSound.Direct, is FckSound.File, FckSound.Pending -> null
    }

    /**
     * Трек играет не потоком Яндекса (прямая ссылка из списка или свой файл): смена качества
     * его не касается.
     */
    fun playsOutsideYandex(track: Track): Boolean =
        playingOf(track)?.sound.let { it is FckSound.Direct || it is FckSound.File }

    // ------------------------------------------------------------ ручные замены

    /** Заменить звук трека [trackId] треком Яндекса [chosen]. */
    suspend fun replaceWithTrack(trackId: String, chosen: Track): ReplaceOutcome = withContext(Dispatchers.IO) {
        if (!plausibleTrackId(chosen.id) || chosen.id == trackId) {
            return@withContext ReplaceOutcome("Этот трек не подходит для замены", ok = false)
        }
        chosen.loudness?.let { loudnessMemo[chosen.id] = it }
        val title = cleanLabel("${chosen.artistNames} — ${chosen.title}")
        commit(trackId, Replacement.YandexTrack(chosen.id, title), "Звук заменён: $title")
    }

    /**
     * Заменить звук трека [trackId] своим файлом [uri] (результат системного выбора файла).
     * Берёт постоянное разрешение на чтение; файл не копируется и не читается до воспроизведения.
     */
    suspend fun replaceWithFile(trackId: String, uri: Uri): ReplaceOutcome = withContext(Dispatchers.IO) {
        val name = cleanLabel(displayNameOf(uri) ?: uri.lastPathSegment.orEmpty())
        val audio = isAudioName(name) || resolver.getType(uri)?.startsWith("audio/") == true
        if (!audio) {
            return@withContext ReplaceOutcome("Этот файл не подходит: нужен MP3, FLAC, OGG, WAV, M4A или AAC", ok = false)
        }
        if (!Replacements.isContentUri(uri.toString())) return@withContext ReplaceOutcome("Этот файл не подходит", ok = false)
        try {
            resolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
        } catch (e: Exception) {
            Log.w(TAG, "постоянный доступ к файлу не получен: ${e.javaClass.simpleName}")
            return@withContext ReplaceOutcome("Не удалось получить постоянный доступ к файлу. Выберите другой файл", ok = false)
        }
        commit(trackId, Replacement.LocalFile(uri.toString(), name), "Звук заменён файлом $name")
    }

    /**
     * Выполнить действие с заменой вне жизни экрана (строка или лист могут уйти из композиции) и
     * показать итог [onDone] на главном потоке.
     */
    fun replaceAsync(action: suspend () -> ReplaceOutcome, onDone: (ReplaceOutcome) -> Unit) {
        scope.launch {
            val outcome = action()
            withContext(Dispatchers.Main) { onDone(outcome) }
        }
    }

    /** «Убрать замену»: снимает и постоянное разрешение на файл, если он больше нигде не нужен. */
    suspend fun clearReplacement(trackId: String): ReplaceOutcome =
        withContext(Dispatchers.IO) { commit(trackId, null, "Замена убрана") }

    /** Имя файла из системной выборки (`DISPLAY_NAME`); `null`, если провайдер его не сообщил. */
    private fun displayNameOf(uri: Uri): String? = try {
        resolver.query(uri, arrayOf(android.provider.OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
            if (c.moveToFirst()) c.getString(0) else null
        }
    } catch (e: Exception) {
        null
    }

    /** Подпись замены трека для меню и строки; `null` — замены нет или плагин выключен. */
    fun replacementLabel(trackId: String): String? {
        replacements.observe()
        return if (enabled) replacements.get(trackId)?.label else null
    }

    /** Сколько ручных замен сохранено. */
    fun replacementCount(): Int {
        replacements.observe()
        return replacements.count()
    }

    /** Адрес из сохранённых замен: только его плеер имеет право читать как локальный файл. */
    fun isReplacementFile(uri: String): Boolean = enabled && replacements.usesUri(uri)

    /**
     * Файл замены можно открыть? `null` — да, иначе сообщение на русском (файл удалён, доступ
     * потерян). Только открывает и закрывает дескриптор, ничего не читает и не пишет.
     */
    fun checkFile(uri: String): String? {
        if (!isReplacementFile(uri)) return "Файл замены не найден в списке замен"
        return try {
            val fd = resolver.openAssetFileDescriptor(Uri.parse(uri), "r")
            if (fd == null) {
                "Файл замены недоступен"
            } else {
                fd.close()
                null
            }
        } catch (e: java.io.FileNotFoundException) {
            "Файл замены не найден: возможно, его удалили или переместили"
        } catch (e: SecurityException) {
            "Доступ к файлу замены потерян: выберите файл заново"
        } catch (e: Exception) {
            "Файл замены не читается"
        }
    }

    /** Применить новую замену (`null` — убрать), снять ненужное разрешение, забыть решение о звуке, записать файл. */
    private fun commit(trackId: String, replacement: Replacement?, done: String): ReplaceOutcome {
        replacements.ensureLoaded()
        val old = if (replacement == null) replacements.remove(trackId) else replacements.put(trackId, replacement)
        if (old is Replacement.LocalFile && !replacements.usesUri(old.uri)) releasePermission(old.uri)
        // Решение о звуке этого трека принято по прежней замене.
        synchronized(playing) { playing.remove(trackId) }
        _playingVersion.value += 1
        val saved = replacements.save()
        _replaced.tryEmit(trackId)
        return if (saved) ReplaceOutcome(done) else ReplaceOutcome("Не удалось сохранить замену: она действует, пока приложение не закрыто", ok = false)
    }

    private fun releasePermission(uri: String) {
        try {
            resolver.releasePersistableUriPermission(Uri.parse(uri), Intent.FLAG_GRANT_READ_URI_PERMISSION)
        } catch (e: Exception) {
            Log.w(TAG, "разрешение на файл не снято: ${e.javaClass.simpleName}")
        }
    }

    /** Подпись без управляющих знаков и знаков смены направления, не длиннее 300 знаков. */
    private fun cleanLabel(text: String): String = text.map { ch ->
        if (ch.isISOControl() || ch in '\u2028'..'\u2029' || ch in '\u202a'..'\u202e' || ch in '\u2066'..'\u2069') ' ' else ch
    }.joinToString("").trim().take(300)

    private fun remember(id: String, now: FckPlaying) {
        synchronized(playing) {
            playing.remove(id)
            playing[id] = now
            while (playing.size > PLAYING_MAX) {
                val oldest = playing.keys.firstOrNull() ?: break
                playing.remove(oldest)
            }
        }
        _playingVersion.value += 1
    }

    // ------------------------------------------------------------ состав альбома

    /**
     * Альбом по составу из списка (`volumes`): вместо треков Яндекса — треки из списка, по
     * порядку списка; при `replaceArtistsInAlbumVolumes` их исполнители — исполнители альбома.
     * Выключенный плагин, альбом не из списка, нет сети — состав Яндекса без изменений. Пока
     * список ещё не пришёл, ждём его не дольше [LIST_WAIT_MS]. Состав берётся при открытии
     * альбома, как на десктопе: он не меняется, пока альбом открыт.
     */
    suspend fun withAlbumVolumes(api: YandexApi, token: String, details: AlbumDetails): AlbumDetails {
        if (!enabled) return details
        if (loaded == null) withTimeoutOrNull(LIST_WAIT_MS) { refresh() }
        val patch = albumPatch(details.card.id)?.takeIf { it.volumes.isNotEmpty() } ?: return details
        val fetched = try {
            api.tracks(token, patch.volumes)
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            Log.w(TAG, "состав альбома ${details.card.id} не загружен: ${e.javaClass.simpleName}")
            return details
        }
        val byId = fetched.associateBy { it.id }
        val artists = patch.artists.takeIf { patch.replaceArtists }
        val tracks = patch.volumes.distinct().mapNotNull { byId[it] }
            .map { if (artists != null) it.copy(sourceArtists = artists) else it }
        return if (tracks.isEmpty()) details else details.copy(tracks = tracks)
    }

    /** Переключатель сменился (вызывает [Prefs]): включили — грузим список, выключили — забываем его. */
    fun onToggle() {
        // Принятые решения о звуке относились к прежнему состоянию плагина.
        synchronized(playing) { playing.clear() }
        _playingVersion.value += 1
        if (enabled) {
            // Экраны сразу видят «включено»: подмены появятся, как только список прочитается.
            publish(active)
            scope.launch { refresh() }
        } else {
            fetchedAtMs = 0
            publish(CensorList.EMPTY)
            _state.value = FckState.Off
        }
    }

    /** «Повторить» на странице плагинов. */
    fun retry() {
        scope.launch { refresh(force = true) }
    }

    /**
     * Подгрузить/обновить список. Сначала кэш на диске (работает без сети), потом, если он
     * старше 6 часов, GitHub. Выключенный плагин ничего не делает.
     */
    suspend fun refresh(force: Boolean = false) = withContext(Dispatchers.IO) {
        if (!enabled) return@withContext
        mutex.withLock {
            if (!enabled) return@withLock
            if (loaded == null) {
                readCache()?.let { (list, at) ->
                    if (!enabled) return@withLock
                    fetchedAtMs = at
                    publish(list)
                }
            }
            val age = System.currentTimeMillis() - fetchedAtMs
            if (!force && loaded != null && age in 0 until CACHE_MS) {
                _state.value = FckState.Ready(active.count)
                return@withLock
            }
            if (loaded == null) _state.value = FckState.Loading
            try {
                val text = http.newCall(pluginRequest(LIST_URL)).execute().use { resp ->
                    if (!resp.isSuccessful) throw ListError("GitHub ответил кодом ${resp.code}")
                    resp.readLimited(LIST_LIMIT) ?: throw ListError("список слишком большой")
                }
                val parsed = try {
                    CensorList.parse(text)
                } catch (e: kotlinx.coroutines.CancellationException) {
                    throw e
                } catch (_: Exception) {
                    throw ListError("список повреждён")
                }
                // Пока грузили, плагин могли выключить.
                if (!enabled) return@withLock
                fetchedAtMs = System.currentTimeMillis()
                publish(parsed)
                writeCache(text, fetchedAtMs)
                _state.value = FckState.Ready(parsed.count)
                Log.i(TAG, "список загружен: ${parsed.count} замен")
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: ListError) {
                fail(e.message ?: "ошибка загрузки")
            } catch (e: java.io.IOException) {
                fail("нет соединения с GitHub")
            } catch (e: Exception) {
                fail("не удалось загрузить список")
            }
        }
    }

    /** Ошибка: прежний список (из памяти или кэша), если был, продолжает работать, страница показывает причину. */
    private fun fail(message: String) {
        Log.w(TAG, "список не загружен: $message")
        _state.value = if (loaded == null) FckState.Failed(message) else FckState.Ready(active.count)
    }

    private class ListError(message: String) : Exception(message)

    // ------------------------------------------------------------ кэш на диске

    private fun cacheFile() = File(File(cacheDir, "plugins"), CACHE_FILE)

    /** Список из кэша и когда он получен. Нет файла, он битый или слишком большой — `null`, без ошибки. */
    private fun readCache(): Pair<CensorList, Long>? = try {
        val file = cacheFile()
        if (!file.isFile || file.length() > CACHE_FILE_LIMIT) {
            null
        } else {
            val root = Json.parseToJsonElement(file.readText()) as? JsonObject
            val at = (root?.get("fetched") as? JsonPrimitive)?.longOrNull
            val body = (root?.get("body") as? JsonPrimitive)?.takeIf { it.isString }?.content
            if (at == null || body == null) null else CensorList.parse(body) to at
        }
    } catch (e: kotlinx.coroutines.CancellationException) {
        throw e
    } catch (_: Exception) {
        null
    }

    /** Запись кэша: временный файл и переименование. Сбой записи не мешает работе. */
    private fun writeCache(body: String, fetchedMs: Long) {
        try {
            val file = cacheFile()
            file.parentFile?.mkdirs()
            val tmp = File(file.parentFile, "$CACHE_FILE.tmp")
            tmp.writeText(
                buildJsonObject {
                    put("fetched", fetchedMs)
                    put("body", body)
                }.toString(),
            )
            if (!tmp.renameTo(file)) {
                file.delete()
                tmp.renameTo(file)
            }
        } catch (e: Exception) {
            Log.w(TAG, "кэш списка не записан: ${e.javaClass.simpleName}")
        }
    }

    companion object {
        private const val TAG = "FckCensor"
        const val LIST_URL = "https://raw.githubusercontent.com/Hazzz895/FckCensorData/main/list_v2.json"
        private const val LIST_LIMIT = 8 * 1024 * 1024
        private const val CACHE_FILE_LIMIT = 24L * 1024 * 1024
        private const val CACHE_FILE = "fckcensor-list.json"
        private const val CACHE_MS = 6 * 3600 * 1000L
        private const val EXPLICIT_FILE = "fckcensor-explicit.json"

        /** Сколько ждать ответ поиска explicit-версии перед запуском трека (как на десктопе). */
        const val SEARCH_TIMEOUT_MS = 10_000L

        /** Сколько страница альбома ждёт список, если он ещё не пришёл. */
        private const val LIST_WAIT_MS = 3_000L
        private const val PLAYING_MAX = 40
        private const val LOUDNESS_MEMO_MAX = 200

        /** Переключатель плагина: по умолчанию выключен, значение ставит [Prefs]. */
        @Volatile var enabled = false

        /** Искать explicit-версию на Яндексе (вложенный переключатель, по умолчанию включён). */
        @Volatile var explicitSearch = true

        /** Что реально играет вместо трека: по номеру трека очереди, последние [PLAYING_MAX]. Только память. */
        private val playing = LinkedHashMap<String, FckPlaying>()

        /** Громкость треков Яндекса, чей звук подставлен вместо цензурных. Только память. */
        private val loudnessMemo = java.util.concurrent.ConcurrentHashMap<String, Loudness>()

        private val _playingVersion = MutableStateFlow(0)

        /** Растёт, когда решение о звуке принято или сброшено: сервис пересчитывает нормализацию. */
        val playingVersion: StateFlow<Int> = _playingVersion

        /** Список, по которому сейчас работаем (пустой, пока плагин выключен или список не получен). */
        @Volatile private var active: CensorList = CensorList.EMPTY

        /** Список уже есть (из кэша или с GitHub). */
        private val loaded: CensorList? get() = active.takeIf { it !== CensorList.EMPTY }

        private val revisionState = mutableIntStateOf(0)
        private val _revision = MutableStateFlow(0)

        /** Растёт, когда подмены или переключатель изменились: сервис воспроизведения обновляет шторку. */
        val revision: StateFlow<Int> = _revision

        private fun publish(list: CensorList) = synchronized(this) {
            active = list
            revisionState.intValue += 1
            _revision.value += 1
        }

        /**
         * Подмена трека для показа или `null` (плагин выключен, списка нет, трека в нём нет).
         * Вызывается из свойств [Track]; чтение счётчика подписывает экран на изменения.
         */
        fun trackPatch(id: String): TrackPatch? {
            // Чтение идёт всегда: так Compose запоминает, что экран зависит от списка.
            if (revisionState.intValue < 0 || !enabled) return null
            return active.tracks[id]
        }

        /** У трека есть звук из списка (другой трек Яндекса или ссылка): его можно включить, даже если Яндекс его скрыл. */
        fun hasSound(id: String): Boolean {
            instance?.replacements?.observe()
            if (revisionState.intValue < 0 || !enabled) return false
            val list = active
            return instance?.replacements?.has(id) == true ||
                list.tracks[id]?.audioSourceId != null || list.urls.containsKey(id)
        }

        /** У трека ручная замена (плагин включён): строка показывает метку «Замена». */
        fun hasReplacement(id: String): Boolean {
            instance?.replacements?.observe()
            return enabled && instance?.replacements?.has(id) == true
        }

        private val _replaced = MutableSharedFlow<String>(extraBufferCapacity = 16)

        /** Номер трека, у которого замену поставили или убрали: сервис перезапускает его, если он играет или следующий. */
        val replaced: SharedFlow<String> = _replaced

        /** Подмена альбома для показа или `null`. См. [trackPatch]. */
        fun albumPatch(id: String): AlbumPatch? {
            if (revisionState.intValue < 0 || !enabled) return null
            return active.albums[id]
        }

        @Volatile private var instance: FckCensor? = null

        /** Созданный экземпляр (после старта приложения) или `null`. */
        val current: FckCensor? get() = instance

        fun get(http: OkHttpClient, cacheDir: File, filesDir: File, resolver: ContentResolver): FckCensor =
            instance ?: synchronized(this) {
                instance ?: FckCensor(http, cacheDir, filesDir, resolver).also { instance = it }
            }
    }
}
