package ru.plastinka.data

import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put
import okhttp3.OkHttpClient
import java.io.File
import java.util.concurrent.ConcurrentHashMap

/** Метка трека или исполнителя. */
enum class SlopLabel { Pending, Human, Ai }

/** Что показать на странице плагина: сколько исполнителей проверено и последняя ошибка. */
data class SlopStatus(val known: Int = 0, val error: String? = null)

/**
 * Slopless: метки ИИ-исполнителей и ИИ-треков по публичному API slopless.art.
 * На исполнителя — один запрос `GET /api/artist/{id}` (доля ИИ-треков и их
 * список). Токен Яндекса сюда не уходит: запрос собирается заново, без
 * заголовка `Authorization`. Подробности — `docs/api/android-plugins.md`.
 *
 * Выключенный плагин ничего не спрашивает и ничего не помечает ([isAi] всегда
 * `false`). Ответы лежат в памяти и в кэше (`cacheDir/plugins/slopless.json`,
 * только метаданные, живут 7 дней, как на десктопе).
 */
class Slopless(private val http: OkHttpClient, private val cacheDir: File) {

    private class Verdict(val ai: Boolean, val tracks: Set<String>, val unknown: Boolean, val atSec: Long)

    private sealed interface Fetched {
        class Ok(val verdict: Verdict) : Fetched
        class Err(val message: String) : Fetched
    }

    private val cache = ConcurrentHashMap<String, Verdict>()
    private val lock = Any()
    private val saveLock = Any()
    private val inFlight = HashSet<String>()
    private val queue = ArrayDeque<String>()
    private val failedAtMs = HashMap<String, Long>()
    private var dirty = false
    @Volatile private var diskLoaded = false
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    /** Треки, которым уже ставили «Не нравится» в этом сеансе. */
    private val disliked = HashSet<String>()

    private val _version = MutableStateFlow(0)

    /** Растёт с каждым новым ответом: подписчики перерисовывают метки и перепроверяют очередь. */
    val version: StateFlow<Int> = _version

    private val _status = MutableStateFlow(SlopStatus())
    val status: StateFlow<SlopStatus> = _status

    /** Исполнитель — ИИ. */
    fun artistLabel(id: String): SlopLabel {
        val verdict = cache[id] ?: return SlopLabel.Pending
        return if (verdict.ai) SlopLabel.Ai else SlopLabel.Human
    }

    /**
     * Трек — ИИ: он в списке ИИ-треков исполнителя или исполнитель ИИ. Пока
     * хоть об одном исполнителе ничего не знаем (и ИИ не найден) — `Pending`.
     */
    fun trackLabel(track: Track): SlopLabel {
        var pending = false
        // Исполнители и настоящие, и показанные (FckCensor): в базе slopless.art лежат настоящие номера.
        for (artist in track.sourceArtists + track.artists) {
            val id = artist.id ?: continue
            val verdict = cache[id]
            when {
                verdict == null -> pending = true
                verdict.ai || verdict.tracks.contains(track.id) -> return SlopLabel.Ai
            }
        }
        return if (pending) SlopLabel.Pending else SlopLabel.Human
    }

    /** Метка для интерфейса и автопропуска: `true` только при включённом плагине. */
    fun isAi(track: Track): Boolean = Prefs.slopless && trackLabel(track) == SlopLabel.Ai

    /** Попросить данные об исполнителях треков (видимые строки, плеер, следующий трек). */
    fun wantTracks(tracks: Iterable<Track>) {
        wantArtists(tracks.flatMap { track -> (track.sourceArtists + track.artists).mapNotNull { it.id } })
    }

    /** Попросить данные об исполнителях: неизвестных ставим в очередь, не больше [PARALLEL] запросов сразу. */
    fun wantArtists(ids: Collection<String>) {
        if (!Prefs.slopless) return
        val wanted = ids.filter(::plausibleNumericId).distinct()
        if (wanted.isEmpty()) return
        if (diskLoaded && wanted.all { cache.containsKey(it) }) return
        scope.launch {
            ensureDiskLoaded()
            val now = System.currentTimeMillis()
            synchronized(lock) {
                for (id in wanted) {
                    val known = cache.containsKey(id) || inFlight.contains(id) || queue.contains(id) ||
                        failedAtMs[id]?.let { now - it < RETRY_AFTER_MS } == true
                    // Новые — вперёд: они на экране сейчас.
                    if (!known) queue.addFirst(id)
                }
                while (queue.size > QUEUE_LIMIT) queue.removeLast()
            }
            pump()
        }
    }

    /**
     * Можно ли поставить «Не нравится» этому треку: один раз за сеанс. Само
     * разрешение (плагин, переключатель, подтверждённая подписка) проверяет
     * вызывающий.
     */
    fun markDisliked(trackId: String): Boolean = synchronized(lock) { disliked.add(trackId) }

    // ------------------------------------------------------------ запросы

    private fun pump() {
        val batch = ArrayList<String>()
        synchronized(lock) {
            if (!Prefs.slopless) {
                queue.clear()
                return
            }
            while (inFlight.size < PARALLEL) {
                val id = queue.removeFirstOrNull() ?: break
                inFlight.add(id)
                batch.add(id)
            }
        }
        for (id in batch) {
            scope.launch {
                val result = try {
                    fetch(id)
                } catch (e: kotlinx.coroutines.CancellationException) {
                    throw e
                } catch (_: Exception) {
                    Fetched.Err("не удалось получить ответ slopless.art")
                }
                finished(id, result)
                pump()
            }
        }
    }

    private fun finished(id: String, result: Fetched) {
        var save = false
        synchronized(lock) {
            inFlight.remove(id)
            when (result) {
                is Fetched.Ok -> {
                    failedAtMs.remove(id)
                    cache[id] = result.verdict
                    dirty = true
                }
                is Fetched.Err -> failedAtMs[id] = System.currentTimeMillis()
            }
            if (dirty && inFlight.isEmpty() && queue.isEmpty()) {
                dirty = false
                save = true
            }
        }
        when (result) {
            is Fetched.Ok -> {
                _status.value = SlopStatus(cache.size, null)
                _version.value += 1
            }
            is Fetched.Err -> {
                Log.w(TAG, "исполнитель $id: ${result.message}")
                _status.value = _status.value.copy(error = result.message)
            }
        }
        if (save) saveDisk()
    }

    /** Блокирующий запрос об исполнителе; нет в базе (404) — «не знаем», не «человек». */
    private fun fetch(artist: String): Fetched {
        return try {
            http.newCall(pluginRequest("$API_BASE/artist/$artist")).execute().use { resp ->
                when {
                    resp.code == 404 -> Fetched.Ok(unknown())
                    !resp.isSuccessful -> Fetched.Err("slopless.art ответил кодом ${resp.code}")
                    else -> {
                        val text = resp.readLimited(RESPONSE_LIMIT)
                            ?: return@use Fetched.Err("slopless.art прислал слишком большой ответ")
                        parseArtist(text)?.let { Fetched.Ok(it) }
                            ?: Fetched.Err("slopless.art прислал непонятный ответ")
                    }
                }
            }
        } catch (_: java.io.IOException) {
            Fetched.Err("нет соединения с slopless.art")
        }
    }

    private fun unknown() = Verdict(ai = false, tracks = emptySet(), unknown = true, atSec = nowSec())

    /** Разбор ответа `GET /api/artist/{id}`; `null` — ответ не тот. */
    private fun parseArtist(text: String): Verdict? {
        val root = try {
            Json.parseToJsonElement(text) as? JsonObject
        } catch (_: Exception) {
            null
        } ?: return null
        val aiCount = number(root["aiTracks"]) ?: return null
        val total = number(root["totalTracks"]) ?: return null
        fun listed(key: String, scored: Boolean): List<String> =
            (root[key] as? JsonArray).orEmpty().mapNotNull { item ->
                val obj = item as? JsonObject ?: return@mapNotNull null
                if (scored) {
                    val score = number(obj["score"])
                    if (score != null && score < TRACK_THRESHOLD) return@mapNotNull null
                }
                (obj["id"] as? JsonPrimitive)?.content?.trim()?.takeIf(::plausibleNumericId)
            }
        val tracks = (listed("aiTrackList", true) + listed("yandexAiTrackList", false))
            .take(MAX_AI_TRACKS).toSet()
        return Verdict(
            ai = aiCount > 0.0 && aiCount * 2.0 >= total,
            tracks = tracks,
            unknown = false,
            atSec = nowSec(),
        )
    }

    /** Число из ответа: схема разрешает и число, и строку. */
    private fun number(element: JsonElement?): Double? =
        (element as? JsonPrimitive)?.content?.trim()?.toDoubleOrNull()

    // ------------------------------------------------------------ кэш на диске

    private fun cacheFile() = File(File(cacheDir, "plugins"), "slopless.json")

    /** Читает кэш один раз за запуск; битый или просроченный не страшен. */
    private fun ensureDiskLoaded() {
        if (diskLoaded) return
        synchronized(lock) {
            if (diskLoaded) return
            try {
                val file = cacheFile()
                if (file.isFile && file.length() <= RESPONSE_LIMIT) {
                    val root = Json.parseToJsonElement(file.readText()).jsonObject
                    val artists = root["artists"]?.jsonObject.orEmpty()
                    val now = nowSec()
                    for ((id, value) in artists) {
                        if (!plausibleNumericId(id)) continue
                        val obj = value as? JsonObject ?: continue
                        val at = (obj["at"] as? JsonPrimitive)?.longOrNull ?: continue
                        if (now - at > FRESH_SEC) continue
                        val tracks = (obj["tracks"] as? JsonArray).orEmpty()
                            .mapNotNull { (it as? JsonPrimitive)?.content }
                            .filter(::plausibleNumericId).take(MAX_AI_TRACKS).toSet()
                        cache.putIfAbsent(
                            id,
                            Verdict(
                                ai = (obj["ai"] as? JsonPrimitive)?.booleanOrNull ?: false,
                                tracks = tracks,
                                unknown = (obj["unknown"] as? JsonPrimitive)?.booleanOrNull ?: false,
                                atSec = at,
                            ),
                        )
                    }
                }
            } catch (_: Exception) {
                // Кэша нет или он битый: спросим заново.
            }
            diskLoaded = true
        }
        _status.value = SlopStatus(cache.size, _status.value.error)
        _version.value += 1
    }

    /** Запись кэша: временный файл и переименование. Сбой записи не мешает работе. */
    private fun saveDisk() = synchronized(saveLock) {
        val json = buildJsonObject {
            put("artists", buildJsonObject {
                for ((id, v) in cache) {
                    put(id, buildJsonObject {
                        put("ai", v.ai)
                        put("unknown", v.unknown)
                        put("at", v.atSec)
                        put("tracks", buildJsonArray { v.tracks.forEach { add(JsonPrimitive(it)) } })
                    })
                }
            })
        }
        try {
            val file = cacheFile()
            file.parentFile?.mkdirs()
            val tmp = File(file.parentFile, "slopless.tmp")
            tmp.writeText(json.toString())
            if (!tmp.renameTo(file)) {
                file.delete()
                tmp.renameTo(file)
            }
        } catch (e: Exception) {
            Log.w(TAG, "кэш не записан: ${e.javaClass.simpleName}")
        }
    }

    private fun nowSec() = System.currentTimeMillis() / 1000

    companion object {
        private const val TAG = "Slopless"
        private const val API_BASE = "https://slopless.art/api"
        const val SITE_URL = "https://slopless.art"

        /** У самых больших ИИ-исполнителей ответ около 1 МБ. */
        private const val RESPONSE_LIMIT = 8 * 1024 * 1024
        private const val MAX_AI_TRACKS = 20_000
        private const val FRESH_SEC = 7L * 24 * 3600
        private const val RETRY_AFTER_MS = 60_000L
        private const val QUEUE_LIMIT = 200
        private const val PARALLEL = 4

        /** Порог из описания API: трек считается ИИ при оценке от 0,3. */
        private const val TRACK_THRESHOLD = 0.3

        @Volatile private var instance: Slopless? = null

        /** Созданный экземпляр (после старта приложения) или `null`. */
        val current: Slopless? get() = instance

        fun get(http: OkHttpClient, cacheDir: File): Slopless =
            instance ?: synchronized(this) {
                instance ?: Slopless(http, cacheDir).also { instance = it }
            }
    }
}
