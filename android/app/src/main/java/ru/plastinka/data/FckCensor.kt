package ru.plastinka.data

import android.util.Log
import androidx.compose.runtime.mutableIntStateOf
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
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
 * [albumPatch] и [replacementUrl] всегда `null`. Список живёт в памяти и в кэше на диске
 * (`cacheDir/plugins/fckcensor-list.json`, только метаданные): раз в 6 часов обновляется,
 * без сети работает по кэшу. Звук на диск не пишется.
 */
class FckCensor(private val http: OkHttpClient, private val cacheDir: File) {

    @Volatile private var fetchedAtMs: Long = 0
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val mutex = Mutex()

    private val _state = MutableStateFlow<FckState>(FckState.Off)
    val state: StateFlow<FckState> = _state

    /** Прямая ссылка на звук без цензуры, если плагин включён и трек есть в хранилище. */
    fun replacementUrl(trackId: String): String? = if (enabled) active.urls[trackId] else null

    /** Переключатель сменился (вызывает [Prefs]): включили — грузим список, выключили — забываем его. */
    fun onToggle() {
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

        /** Переключатель плагина: по умолчанию выключен, значение ставит [Prefs]. */
        @Volatile var enabled = false

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

        /** Подмена альбома для показа или `null`. См. [trackPatch]. */
        fun albumPatch(id: String): AlbumPatch? {
            if (revisionState.intValue < 0 || !enabled) return null
            return active.albums[id]
        }

        @Volatile private var instance: FckCensor? = null

        /** Созданный экземпляр (после старта приложения) или `null`. */
        val current: FckCensor? get() = instance

        fun get(http: OkHttpClient, cacheDir: File): FckCensor =
            instance ?: synchronized(this) {
                instance ?: FckCensor(http, cacheDir).also { instance = it }
            }
    }
}
