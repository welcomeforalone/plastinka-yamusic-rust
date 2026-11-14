package ru.plastinka.data

import android.util.Log
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import okhttp3.OkHttpClient
import okhttp3.Request

/**
 * FckCensor: список без цензуры Hazzz895/FckCensorData. Для нас главное —
 * хранилища звука (`tracksStorages`): прямые ссылки на файлы без цензуры,
 * `urlTemplate` с `%%`. Порядок тот же, что в десктопном клиенте.
 */
class FckCensor(private val http: OkHttpClient) {

    @Volatile private var urls: Map<String, String> = emptyMap()
    @Volatile private var fetchedAtMs: Long = 0

    /** Прямая ссылка на звук без цензуры, если трек есть в хранилище. */
    fun replacementUrl(trackId: String): String? = urls[trackId]

    val hasReplacements: Boolean get() = urls.isNotEmpty()

    /** Подгрузить/обновить список (кэш 6 часов, как на десктопе). */
    suspend fun refresh(force: Boolean = false) = withContext(Dispatchers.IO) {
        val fresh = System.currentTimeMillis() - fetchedAtMs < CACHE_MS
        if (!force && (fresh || urls.isNotEmpty())) return@withContext
        try {
            val request = Request.Builder()
                .url(LIST_URL)
                .header("User-Agent", "Plastinka/0.1 Android")
                .build()
            val text = http.newCall(request).execute().use { resp ->
                if (!resp.isSuccessful) return@withContext
                resp.body?.string()
            } ?: return@withContext
            if (text.length > LIST_LIMIT) return@withContext
            urls = parseStorageUrls(text)
            fetchedAtMs = System.currentTimeMillis()
            Log.i(TAG, "список загружен: ${urls.size} замен")
        } catch (e: Exception) {
            Log.w(TAG, "список не загружен: ${e.message}")
        }
    }

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

        @Volatile private var instance: FckCensor? = null
        fun get(http: OkHttpClient): FckCensor =
            instance ?: synchronized(this) {
                instance ?: FckCensor(http).also { instance = it }
            }
    }
}
