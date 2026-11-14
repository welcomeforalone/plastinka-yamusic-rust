package ru.plastinka.data

import android.content.Context
import android.util.Log
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.withContext
import java.io.File
import java.io.IOException

/**
 * Кэш треков: скачанный звук лежит в filesDir/tracks/<id>.bin и играется
 * офлайн. Ссылка та же, что у плеера: замена FckCensor → поток Яндекса.
 */
object TrackCache {

    private const val TAG = "TrackCache"

    private lateinit var dir: File
    private var http: okhttp3.OkHttpClient? = null

    /** Скачанные треки (для кнопок UI). */
    private val _cached = MutableStateFlow<Set<String>>(emptySet())
    val cached: StateFlow<Set<String>> = _cached

    /** Идёт загрузка: id → прогресс 0…100, -1 — неизвестен. */
    private val _progress = MutableStateFlow<Map<String, Int>>(emptyMap())
    val progress: StateFlow<Map<String, Int>> = _progress

    fun init(context: Context, client: okhttp3.OkHttpClient) {
        if (::dir.isInitialized) return
        dir = File(context.filesDir, "tracks").apply { mkdirs() }
        http = client
        _cached.value = dir.listFiles()?.mapNotNull { f ->
            f.extension.takeIf { it == "bin" }?.let { f.nameWithoutExtension }
        }?.toSet() ?: emptySet()
    }

    fun isCached(trackId: String): Boolean = trackId in _cached.value

    /** Локальный адрес для плеера: `file:///…`, если трек скачан. */
    fun cachedUri(trackId: String): String? {
        if (!::dir.isInitialized) return null
        val file = File(dir, "$trackId.bin")
        return if (file.isFile) "file://" + file.absolutePath else null
    }

    fun remove(trackId: String) {
        if (!::dir.isInitialized) return
        File(dir, "$trackId.bin").delete()
        _cached.value = _cached.value - trackId
    }

    /** Скачать трек: FckCensor-замена или поток Яндекса, с прогрессом. */
    suspend fun download(
        api: YandexApi,
        fck: FckCensor,
        token: String,
        track: Track,
    ) = withContext(Dispatchers.IO) {
        if (track.id in _cached.value || track.id in _progress.value) return@withContext
        _progress.value = _progress.value + (track.id to -1)
        val tmp = File(dir, "${track.id}.tmp")
        try {
            val url = fck.replacementUrl(track.id)
                ?: when (val result = api.streamUrl(token, track.id)) {
                    is StreamResult.Url -> result.url
                    else -> throw IOException("ссылка не получена: $result")
                }
            val request = okhttp3.Request.Builder()
                .url(url)
                .header("User-Agent", "Plastinka/0.1 Android")
                .build()
            http!!.newCall(request).execute().use { response ->
                if (!response.isSuccessful) throw IOException("сервер ответил ${response.code}")
                val body = response.body ?: throw IOException("пустой ответ")
                val total = body.contentLength()
                var copied = 0L
                body.byteStream().use { input ->
                    tmp.outputStream().use { output ->
                        val buffer = ByteArray(64 * 1024)
                        while (true) {
                            val read = input.read(buffer)
                            if (read < 0) break
                            output.write(buffer, 0, read)
                            copied += read
                            if (total > 0) {
                                val percent = (copied * 100 / total).toInt()
                                _progress.value = _progress.value + (track.id to percent)
                            }
                        }
                    }
                }
            }
            if (!tmp.renameTo(File(dir, "${track.id}.bin"))) throw IOException("не сохранить файл")
            _cached.value = _cached.value + track.id
            Log.i(TAG, "скачан трек ${track.id} (${tmp.length() / 1024} КБ)")
        } catch (e: Exception) {
            tmp.delete()
            Log.w(TAG, "загрузка трека ${track.id} не удалась: ${e.message}")
        } finally {
            _progress.value = _progress.value - track.id
        }
    }
}
