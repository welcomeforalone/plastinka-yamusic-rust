package ru.plastinka.data

import android.os.SystemClock
import android.util.Log
import androidx.compose.runtime.mutableIntStateOf
import java.io.File
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put
import ru.plastinka.playback.AccountHolder

/** Номера копии: то, что хранится на диске (звука и названий там нет). */
data class CopyRef(val id: String, val albumId: String?)

/** Итог одного поиска копии (как `Lookup` в `src/relink.rs`). */
sealed interface Lookup {
    /** Копия найдена. [track] есть, только если поиск шёл в этом сеансе (после перезапуска из кэша известны лишь номера). */
    data class Found(val copy: CopyRef, val track: Track?) : Lookup

    /** Поиск удался, копии нет. */
    data object Missing : Lookup

    /** Сеть, код ответа, тайм-аут, битый ответ: не кэшируем. */
    data object Failed : Lookup
}

/** Что известно о копии недоступного трека. */
enum class RelinkState { Found, Missing, Unknown }

/**
 * Копия недоступного трека: тот же трек в другом релизе (сборник, переиздание, сингл), найденный
 * поиском по Яндексу. Поведение как у `src/relink.rs` на десктопе (спека `docs/api/relink.md`).
 *
 * Работает всегда, не плагин. Лайк, история и очередь остаются за оригиналом; копия нужна только
 * звуку, `play-audio`, громкости и тексту песни. На диск (`cacheDir/relink.json`) пишутся лишь
 * номера и время поиска (метаданные, правило 1); звука в кэше нет.
 *
 * Поиск идёт в фоне в области [scope] (она живёт, пока живёт приложение: копию ждут и строки
 * списков, и сервис воспроизведения, а сервис может пересоздаваться), не больше [PARALLEL]
 * запросов сразу. Тот, кого сервис ждёт прямо сейчас ([lookupBlocking]), идёт вне очереди, но
 * делит запрос с уже начатым поиском того же трека. Состояние читается через снимок Compose:
 * строки перерисуются, когда копия найдётся.
 */
class Relink(
    private val api: YandexApi,
    private val tokens: () -> String?,
    private val file: File,
    private val scope: CoroutineScope,
) {
    private class Entry(val copy: CopyRef?, val atSec: Long)

    private val lock = Any()
    private val writeLock = Any()
    private val entries = LinkedHashMap<String, Entry>()

    /** Найденные в этом сеансе треки-копии: громкость, альбом, обложка, подсказка о тексте. */
    private val found = HashMap<String, Track>()
    private val inFlight = HashMap<String, Deferred<Lookup>>()

    /** Не искать этот трек раньше (после сбоя): время по [SystemClock.elapsedRealtime]. */
    private val retryAt = HashMap<String, Long>()
    private val limit = Semaphore(PARALLEL)
    private val saveScheduled = AtomicBoolean(false)

    @Volatile private var loaded = false

    init {
        instance = this
    }

    // ------------------------------------------------------------ состояние

    /** Прочитать кэш с диска, если ещё не читали. Блокирует: вызывать не с главного потока. */
    fun ensureLoaded() {
        if (loaded) return
        val disk = readDisk()
        synchronized(lock) {
            if (loaded) return
            loaded = true
            // Уже найденное в этом сеансе новее файла.
            for ((id, entry) in disk) entries.putIfAbsent(id, entry)
            bump()
        }
    }

    /** Найденная копия трека [id] (свежая запись кэша). */
    fun copyOf(id: String): CopyRef? {
        if (revision.intValue < 0) return null
        return synchronized(lock) { entries[id]?.takeIf { fresh(it) }?.copy }
    }

    /** Трек-копия целиком, если он найден в этом сеансе. */
    fun copyTrack(copyId: String): Track? = synchronized(lock) { found[copyId] }

    /** Есть ли свежая запись о треке [id]: копия, «копии нет» или ничего. */
    fun state(id: String): RelinkState {
        if (revision.intValue < 0) return RelinkState.Unknown
        return synchronized(lock) {
            val entry = entries[id]?.takeIf { fresh(it) }
            when {
                entry == null -> RelinkState.Unknown
                entry.copy == null -> RelinkState.Missing
                else -> RelinkState.Found
            }
        }
    }

    /**
     * Копия, которая реально играет вместо [track]: он недоступен, копия найдена, а FckCensor
     * не подменил звук. Лайк, история и очередь остаются за оригиналом.
     */
    fun playingCopy(track: Track): CopyRef? {
        if (track.available) return null
        val copy = copyOf(track.id) ?: return null
        val sound = FckCensor.current?.playingOf(track)?.sound
        return if (sound == null || (sound is FckSound.Yandex && sound.id == track.id)) copy else null
    }

    /**
     * Трек недоступен, FckCensor его не играет, копию ещё не искали (или поиск не удался):
     * в очередь он идёт, а звук решит поиск перед воспроизведением.
     */
    fun worthQueueing(track: Track): Boolean =
        !track.available && !FckCensor.hasSound(track.id) && searchable(track) && state(track.id) == RelinkState.Unknown

    // ------------------------------------------------------------ поиск

    private fun searchable(track: Track): Boolean =
        AccountHolder.uid != null && track.sourceTitle.isNotBlank() && track.sourceArtists.isNotEmpty()

    /**
     * Поставить трек в очередь поиска: видимые строки и следующие в очереди. Недоступный,
     * без звука от FckCensor, без свежей записи, не в работе и не в паузе после сбоя. Дёшево
     * при повторных вызовах.
     */
    fun want(track: Track) {
        if (track.available || FckCensor.hasSound(track.id) || !searchable(track)) return
        if (!loaded) {
            scope.launch {
                ensureLoaded()
                want(track)
            }
            return
        }
        startSearch(track, limited = true)
    }

    /**
     * Копия для сервиса воспроизведения, который ждёт её прямо сейчас (поток загрузки, не
     * главный): ответ из кэша мгновенный, иначе поиск не дольше [SEARCH_TIMEOUT_MS].
     */
    fun lookupBlocking(track: Track): Lookup {
        if (AccountHolder.uid == null) return Lookup.Failed
        if (track.sourceTitle.isBlank() || track.sourceArtists.isEmpty()) return Lookup.Missing
        ensureLoaded()
        cached(track.id)?.let { return it }
        val search = startSearch(track, limited = false) ?: return cached(track.id) ?: Lookup.Failed
        return try {
            runBlocking { withTimeoutOrNull(SEARCH_TIMEOUT_MS) { search.await() } } ?: Lookup.Failed
        } catch (e: InterruptedException) {
            // Загрузку отменили: возвращаем флаг, копия не получена.
            Thread.currentThread().interrupt()
            Lookup.Failed
        } catch (e: CancellationException) {
            Lookup.Failed
        }
    }

    private fun cached(id: String): Lookup? = synchronized(lock) {
        val entry = entries[id]?.takeIf { fresh(it) } ?: return@synchronized null
        val copy = entry.copy ?: return@synchronized Lookup.Missing
        Lookup.Found(copy, found[copy.id])
    }

    /** Запустить поиск трека или вернуть уже идущий; `null` — искать не нужно (запись свежая или пауза после сбоя). */
    private fun startSearch(track: Track, limited: Boolean): Deferred<Lookup>? {
        val search = synchronized(lock) {
            inFlight[track.id]?.let { return it }
            if (entries[track.id]?.let { fresh(it) } == true) return null
            // Паузу после сбоя соблюдает фоновый поиск; воспроизведение пробует снова.
            if (limited && (retryAt[track.id] ?: 0L) > SystemClock.elapsedRealtime()) return null
            val created = scope.async(start = CoroutineStart.LAZY) { run(track, limited) }
            inFlight[track.id] = created
            created
        }
        search.start()
        return search
    }

    private suspend fun run(track: Track, limited: Boolean): Lookup {
        var lookup: Lookup = Lookup.Failed
        try {
            lookup = if (limited) limit.withPermit { find(track) } else find(track)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Log.w(TAG, "поиск копии не удался (трек ${track.id}): ${e.javaClass.simpleName}")
        } finally {
            finish(track.id, lookup)
        }
        return lookup
    }

    /** Один запрос «первый исполнитель + название», страница 0 (как `find_copy`). */
    private suspend fun find(original: Track): Lookup {
        val token = tokens() ?: return Lookup.Failed
        val artist = original.sourceArtists.firstOrNull()?.name.orEmpty()
        val query = "$artist ${original.sourceTitle}"
        val result = try {
            withTimeoutOrNull(SEARCH_TIMEOUT_MS) { api.search(token, SearchKind.Track, query, 0) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Log.w(TAG, "поиск копии не удался (трек ${original.id}): ${e.javaClass.simpleName}")
            return Lookup.Failed
        } ?: return Lookup.Failed
        val pick = pick(original, result.tracks) ?: return Lookup.Missing
        return Lookup.Found(CopyRef(pick.id, pick.albumId), pick)
    }

    private fun finish(id: String, lookup: Lookup) {
        synchronized(lock) {
            inFlight.remove(id)
            val now = nowSec()
            when (lookup) {
                is Lookup.Found -> {
                    put(id, Entry(lookup.copy, now))
                    lookup.track?.let {
                        if (found.size > FOUND_MAX) found.clear()
                        found[it.id] = it
                    }
                    retryAt.remove(id)
                }
                Lookup.Missing -> {
                    put(id, Entry(null, now))
                    retryAt.remove(id)
                }
                Lookup.Failed -> retryAt[id] = SystemClock.elapsedRealtime() + RETRY_AFTER_MS
            }
            bump()
        }
        when (lookup) {
            is Lookup.Found -> {
                Log.i(TAG, "копия трека $id: ${lookup.copy.id}")
                scheduleSave()
            }
            Lookup.Missing -> {
                Log.i(TAG, "копии трека $id нет")
                scheduleSave()
            }
            Lookup.Failed -> Log.i(TAG, "копия трека $id: поиск не удался")
        }
    }

    private fun put(id: String, entry: Entry) {
        entries.remove(id)
        entries[id] = entry
        while (entries.size > MAX_ENTRIES) {
            val oldest = entries.keys.firstOrNull() ?: break
            entries.remove(oldest)
        }
    }

    // ------------------------------------------------------------ диск

    /** Свежесть: найдено — 30 дней, «нет копии» — 7; запись «из будущего» (часы ушли назад) не верим. */
    private fun fresh(entry: Entry, at: Long = nowSec()): Boolean {
        val age = at - entry.atSec
        val max = if (entry.copy != null) FOUND_FRESH_SEC else MISSING_FRESH_SEC
        return age in 0..max
    }

    private fun nowSec(): Long = System.currentTimeMillis() / 1000

    /** Запись кэша в фоне, не чаще раза за [SAVE_DELAY_MS]. */
    private fun scheduleSave() {
        if (!saveScheduled.compareAndSet(false, true)) return
        scope.launch {
            delay(SAVE_DELAY_MS)
            saveScheduled.set(false)
            write()
        }
    }

    /** Кэш с диска. Нет файла, он слишком большой или битый — пусто, без ошибки. */
    private fun readDisk(): Map<String, Entry> {
        val out = LinkedHashMap<String, Entry>()
        try {
            if (!file.isFile || file.length() > FILE_LIMIT) return out
            val root = Json.parseToJsonElement(file.readText()) as? JsonObject ?: return out
            val tracks = root["tracks"] as? JsonObject ?: return out
            val now = nowSec()
            for ((id, value) in tracks) {
                val obj = value as? JsonObject ?: continue
                val at = (obj["at"] as? JsonPrimitive)?.longOrNull ?: continue
                if (!YandexApi.plausibleId(id)) continue
                val copyObj = obj["copy"] as? JsonObject
                var copy: CopyRef? = null
                if (copyObj != null) {
                    val copyId = (copyObj["id"] as? JsonPrimitive)?.takeIf { it.isString }?.content ?: continue
                    val album = (copyObj["album_id"] as? JsonPrimitive)?.takeIf { it.isString }?.content
                    if (!YandexApi.plausibleId(copyId) || copyId == id) continue
                    if (album != null && !YandexApi.plausibleId(album)) continue
                    copy = CopyRef(copyId, album)
                }
                val entry = Entry(copy, at)
                if (fresh(entry, now)) out[id] = entry
                if (out.size >= MAX_ENTRIES) break
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Log.w(TAG, "кэш копий не прочитан: ${e.javaClass.simpleName}")
            out.clear()
        }
        return out
    }

    /** Запись через временный файл и переименование; сбой не мешает работе (кэш только ускоряет). */
    private fun write() = synchronized(writeLock) {
        // Две записи подряд (медленный диск) не пишут в один временный файл одновременно.
        try {
            val snapshot = synchronized(lock) { entries.entries.map { it.key to it.value } }
            val text = buildJsonObject {
                put(
                    "tracks",
                    buildJsonObject {
                        for ((id, entry) in snapshot) {
                            put(
                                id,
                                buildJsonObject {
                                    val copy = entry.copy
                                    if (copy == null) {
                                        put("copy", JsonNull)
                                    } else {
                                        put(
                                            "copy",
                                            buildJsonObject {
                                                put("id", copy.id)
                                                copy.albumId?.let { put("album_id", it) }
                                            },
                                        )
                                    }
                                    put("at", entry.atSec)
                                },
                            )
                        }
                    },
                )
            }.toString()
            file.parentFile?.mkdirs()
            val tmp = File(file.parentFile, "${file.name}.tmp")
            tmp.writeText(text)
            if (!tmp.renameTo(file)) {
                file.delete()
                tmp.renameTo(file)
            }
        } catch (e: Exception) {
            Log.w(TAG, "кэш копий не записан: ${e.javaClass.simpleName}")
        }
    }

    companion object {
        private const val TAG = "Relink"

        const val SEARCH_TIMEOUT_MS = 10_000L
        private const val PARALLEL = 2
        private const val RETRY_AFTER_MS = 60_000L
        private const val FOUND_FRESH_SEC = 30L * 24 * 3600
        private const val MISSING_FRESH_SEC = 7L * 24 * 3600
        private const val MAX_ENTRIES = 5000
        private const val FILE_LIMIT = 2L * 1024 * 1024
        private const val FOUND_MAX = 500
        private const val SAVE_DELAY_MS = 500L

        /** Разница длительностей, при которой трек ещё считается тем же. */
        private const val DURATION_SLACK_MS = 3_000L

        @Volatile private var instance: Relink? = null

        /** Созданный экземпляр (после старта приложения) или `null`. */
        val current: Relink? get() = instance

        /** Счётчик для снимка Compose: растёт, когда что-то узнали о копиях. */
        private val revision = mutableIntStateOf(0)

        private fun bump() {
            revision.intValue += 1
        }

        /** У трека найдена копия (для [Track.playable]); чтение подписывает экран на изменения. */
        fun hasCopy(id: String): Boolean = instance?.copyOf(id) != null

        /** Версия для сравнения: нет версии — пустая строка. */
        private fun versionKey(track: Track): String = track.version?.let(ExplicitMatch::normalize).orEmpty()

        /** Пометка для сравнения: «та же» важнее ближайшей длительности. */
        private fun warning(track: Track): Int = if (track.explicit) 1 else if (track.clean) 2 else 0

        /**
         * Копия [original] среди [candidates] (как `pick_copy`): доступный трек с другим номером,
         * тот же исполнитель, название и версия равны после [ExplicitMatch.normalize] (точное
         * равенство, не `sameTitle`), длительности известны у обоих и отличаются не больше чем
         * на 3 с. Из подходящих сначала с той же пометкой (`explicit` или `clean`), потом с
         * ближайшей длительностью. Сравнивается то, что прислал Яндекс (`source*`), а не
         * подменённое FckCensor.
         */
        internal fun pick(original: Track, candidates: List<Track>): Track? {
            val title = ExplicitMatch.normalize(original.sourceTitle)
            if (title.isEmpty()) return null
            val version = versionKey(original)
            val length = original.durationMs ?: return null
            return candidates
                .filter { c ->
                    c.available && c.id != original.id &&
                        ExplicitMatch.sameArtist(original.sourceArtists, c.sourceArtists) &&
                        ExplicitMatch.normalize(c.sourceTitle) == title &&
                        versionKey(c) == version
                }
                .mapNotNull { c ->
                    val other = c.durationMs ?: return@mapNotNull null
                    val gap = Math.abs(length - other)
                    if (gap <= DURATION_SLACK_MS) Triple(warning(c) != warning(original), gap, c) else null
                }
                .minWithOrNull(compareBy<Triple<Boolean, Long, Track>>({ it.first }, { it.second }))
                ?.third
        }
    }
}
