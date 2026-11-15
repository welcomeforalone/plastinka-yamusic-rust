package ru.plastinka.data

import android.util.Log
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
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import okhttp3.OkHttpClient

/** Состояние списка FckCensor для страницы плагинов. */
sealed interface FckState {
    /** Плагин выключен: список не запрашивался. */
    data object Off : FckState
    data object Loading : FckState
    data class Ready(val count: Int) : FckState
    data class Failed(val message: String) : FckState
}

/**
 * FckCensor: список без цензуры Hazzz895/FckCensorData. Для нас главное —
 * хранилища звука (`tracksStorages`): прямые ссылки на файлы без цензуры,
 * `urlTemplate` с `%%`. Порядок тот же, что в десктопном клиенте.
 *
 * Плагин выключен по умолчанию: пока он выключен, список не запрашивается, а
 * [replacementUrl] всегда `null` — все места (поток, громкость, значок
 * качества, отчёты о прослушивании) смотрят замену только через него.
 * Список живёт в памяти, на диск не пишется.
 */
class FckCensor(private val http: OkHttpClient) {

    @Volatile private var urls: Map<String, String> = emptyMap()
    @Volatile private var fetchedAtMs: Long = 0
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val mutex = Mutex()

    private val _state = MutableStateFlow<FckState>(FckState.Off)
    val state: StateFlow<FckState> = _state

    /** Прямая ссылка на звук без цензуры, если плагин включён и трек есть в хранилище. */
    fun replacementUrl(trackId: String): String? = if (enabled) urls[trackId] else null

    /** Переключатель сменился (вызывает [Prefs]): включили — грузим список, выключили — забываем его. */
    fun onToggle() {
        if (enabled) {
            scope.launch { refresh() }
        } else {
            urls = emptyMap()
            fetchedAtMs = 0
            _state.value = FckState.Off
        }
    }

    /** «Повторить» на странице плагинов. */
    fun retry() {
        scope.launch { refresh(force = true) }
    }

    /** Подгрузить/обновить список (кэш 6 часов, как на десктопе). Выключенный плагин ничего не делает. */
    suspend fun refresh(force: Boolean = false) = withContext(Dispatchers.IO) {
        if (!enabled) return@withContext
        mutex.withLock {
            if (!enabled) return@withLock
            val fresh = System.currentTimeMillis() - fetchedAtMs < CACHE_MS
            if (!force && fresh && urls.isNotEmpty()) {
                _state.value = FckState.Ready(urls.size)
                return@withLock
            }
            if (urls.isEmpty()) _state.value = FckState.Loading
            try {
                val text = http.newCall(pluginRequest(LIST_URL)).execute().use { resp ->
                    if (!resp.isSuccessful) throw ListError("GitHub ответил кодом ${resp.code}")
                    resp.readLimited(LIST_LIMIT) ?: throw ListError("список слишком большой")
                }
                val parsed = try {
                    parseStorageUrls(text)
                } catch (e: kotlinx.coroutines.CancellationException) {
                    throw e
                } catch (_: Exception) {
                    throw ListError("список повреждён")
                }
                // Пока грузили, плагин могли выключить.
                if (!enabled) return@withLock
                urls = parsed
                fetchedAtMs = System.currentTimeMillis()
                _state.value = FckState.Ready(parsed.size)
                Log.i(TAG, "список загружен: ${parsed.size} замен")
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

    /** Ошибка: прежний список (если был) продолжает работать, страница показывает причину. */
    private fun fail(message: String) {
        Log.w(TAG, "список не загружен: $message")
        _state.value = if (urls.isEmpty()) FckState.Failed(message) else FckState.Ready(urls.size)
    }

    private class ListError(message: String) : Exception(message)

    private fun parseStorageUrls(text: String): Map<String, String> {
        val out = HashMap<String, String>()
        val root = Json.parseToJsonElement(text).jsonObject
        val sources = root["sources"]?.jsonArray ?: return out
        for (source in sources) {
            val storages = source.jsonObject["tracksStorages"]?.jsonArray ?: continue
            for (storage in storages) {
                val st = storage.jsonObject
                val template = (st["urlTemplate"] as? JsonPrimitive)?.content
                    ?.let(::normalizeTemplate) ?: continue
                // trackIds: файл — шаблон с номером самого трека.
                st["trackIds"]?.jsonArray?.forEach { item ->
                    val id = when (item) {
                        is JsonPrimitive -> item.content
                        else -> item.jsonObject["id"]?.let { (it as? JsonPrimitive)?.content }
                    }
                    if (id != null && plausibleId(id)) {
                        out[id] = template.replace("%%", id)
                    }
                }
                // tracks: номер трека → номер файла, ссылка или объект с url.
                (st["tracks"] as? kotlinx.serialization.json.JsonObject)?.forEach { (key, value) ->
                    val id = key.takeIf(::plausibleId) ?: return@forEach
                    val url = fileUrl(value, template) ?: return@forEach
                    out[id] = url
                }
            }
        }
        return out
    }

    private fun normalizeTemplate(raw: String): String? {
        if (raw.length > 1800 || !raw.contains("%%") || raw.any { it.isWhitespace() }) return null
        val url = if (raw.contains("://")) raw else "https://$raw"
        return url.takeIf(::plausibleUrl)
    }

    private fun fileUrl(value: kotlinx.serialization.json.JsonElement, template: String): String? {
        val raw: String = when (value) {
            is JsonPrimitive -> value.content
            else -> {
                val url = value.jsonObject["url"] ?: return null
                (url as? JsonPrimitive)?.content ?: return null
            }
        }
        if (raw.length > 1800 || raw.any { it.isWhitespace() }) return null
        val url = if (raw.contains("://")) raw else template.replace("%%", raw)
        return url.takeIf(::plausibleUrl)
    }

    private fun plausibleUrl(url: String): Boolean =
        url.startsWith("https://") && url.length <= 2000 && !url.any { it.isWhitespace() }

    private fun plausibleId(id: String): Boolean =
        id.length in 1..32 && id.all { it.isDigit() }

    companion object {
        private const val TAG = "FckCensor"
        const val LIST_URL = "https://raw.githubusercontent.com/Hazzz895/FckCensorData/main/list_v2.json"
        private const val LIST_LIMIT = 8 * 1024 * 1024
        private const val CACHE_MS = 6 * 3600 * 1000L

        /** Переключатель плагина: по умолчанию выключен, значение ставит [Prefs]. */
        @Volatile var enabled = false

        @Volatile private var instance: FckCensor? = null

        /** Созданный экземпляр (после старта приложения) или `null`. */
        val current: FckCensor? get() = instance

        fun get(http: OkHttpClient): FckCensor =
            instance ?: synchronized(this) {
                instance ?: FckCensor(http).also { instance = it }
            }
    }
}
