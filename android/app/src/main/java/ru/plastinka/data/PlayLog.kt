package ru.plastinka.data

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.io.File
import java.time.LocalDate
import java.time.ZoneId

/**
 * Локальный журнал прослушиваний: `filesDir/play_log.json`. Только метаданные
 * (id, название, исполнители, альбом, обложка, длительность, контекст, время);
 * аудио и ссылки на поток сюда не попадают. Не больше [LIMIT] записей. Чтение
 * мягкое: битый файл или запись — как будто их нет.
 */
object PlayLog {

    const val LIMIT = 300
    private const val FILE_NAME = "play_log.json"
    private const val VERSION = 1
    private const val SEP = "\u0001"

    private val json = Json { ignoreUnknownKeys = true }
    private var file: File? = null

    /** Прослушивания от старых к новым. */
    private val _plays = MutableStateFlow<List<LocalPlay>>(emptyList())
    val plays: StateFlow<List<LocalPlay>> = _plays

    /** Один фоновый поток: чтение и запись файла идут по очереди и не блокируют главный поток. */
    private val io = java.util.concurrent.Executors.newSingleThreadExecutor { r ->
        Thread(r, "play-log").apply { isDaemon = true }
    }

    /** Поколение: `clear()` увеличивает его, и отложенные чтение и запись прежнего поколения отменяются. */
    private val generation = java.util.concurrent.atomic.AtomicLong(0)

    @Synchronized
    fun init(context: Context) {
        if (file != null) return
        val f = File(context.filesDir, FILE_NAME)
        file = f
        val gen = generation.get()
        io.execute {
            val loaded = readFile(f)
            synchronized(this) {
                if (gen != generation.get()) return@synchronized
                // Записи, появившиеся раньше, чем дочитался файл, свежее прочитанных.
                val current = _plays.value
                val older = loaded.filter { old -> current.none { it.track.id == old.track.id && it.date == old.date } }
                _plays.value = (older + current).takeLast(LIMIT)
            }
        }
    }

    /**
     * Трек зазвучал. Прежняя запись того же трека за тот же день уходит:
     * он поднимается наверх, а не повторяется. Список обновляется сразу,
     * файл пишется в фоне.
     */
    @Synchronized
    fun record(track: Track, context: HistoryContext, now: Long = System.currentTimeMillis()) {
        val date = LocalDate.now(ZoneId.systemDefault())
        val kept = _plays.value.filter { it.track.id != track.id || it.date != date }
        _plays.value = (kept + LocalPlay(track, context, date, now)).takeLast(LIMIT)
        val gen = generation.get()
        io.execute { write(gen) }
    }

    /** Выход из аккаунта: журнал очищается, ещё не выполненные записи отменяются. */
    @Synchronized
    fun clear() {
        generation.incrementAndGet()
        _plays.value = emptyList()
        val f = file ?: return
        io.execute {
            try {
                f.delete()
                File(f.parentFile, "$FILE_NAME.tmp").delete()
            } catch (_: Exception) {
                // Не удалось удалить файл: в памяти журнал уже пуст.
            }
        }
    }

    /** Выполняется на фоновом потоке; пишет актуальный список, если журнал не очищали после постановки. */
    private fun write(gen: Long) {
        val f = file ?: return
        try {
            val snapshot = synchronized(this) {
                if (gen != generation.get()) return
                _plays.value
            }
            val text = buildJsonObject {
                put("v", VERSION)
                put("plays", buildJsonArray { snapshot.forEach { add(encode(it)) } })
            }.toString()
            // Временный файл с метаданными и переименование: файл целый или прежний.
            val tmp = File(f.parentFile, "$FILE_NAME.tmp")
            tmp.writeText(text)
            if (!tmp.renameTo(f)) {
                f.writeText(text)
                tmp.delete()
            }
        } catch (_: Exception) {
            // Диск недоступен: журнал живёт в памяти до перезапуска.
        }
    }

    private fun readFile(f: File): List<LocalPlay> {
        if (!f.isFile || f.length() > MAX_FILE) return emptyList()
        return try {
            val obj = json.parseToJsonElement(f.readText()) as? JsonObject ?: return emptyList()
            (obj["plays"] as? JsonArray).orEmpty().mapNotNull(::decode).takeLast(LIMIT)
        } catch (_: Exception) {
            emptyList()
        }
    }

    private const val MAX_FILE = 4L * 1024 * 1024

    private fun encode(p: LocalPlay): JsonObject = buildJsonObject {
        val t = p.track
        put("id", t.id)
        put("title", t.title)
        t.albumId?.let { put("album", it) }
        t.albumTitle?.let { put("albumTitle", it) }
        t.coverUri?.let { put("cover", it) }
        t.durationMs?.let { put("ms", it) }
        put("available", t.available)
        put(
            "artists",
            buildJsonArray {
                t.artists.forEach { a ->
                    add(
                        buildJsonObject {
                            a.id?.let { put("id", it) }
                            put("name", a.name)
                        },
                    )
                }
            },
        )
        put("ctx", encodeContext(p.context))
        put("date", p.date.toString())
        put("at", p.atMs)
    }

    private fun decode(element: kotlinx.serialization.json.JsonElement): LocalPlay? {
        val o = element as? JsonObject ?: return null
        val id = o.str("id") ?: return null
        val artists = (o["artists"] as? JsonArray).orEmpty().mapNotNull { a ->
            val ao = a as? JsonObject ?: return@mapNotNull null
            ArtistRef(ao.str("id"), ao.str("name") ?: return@mapNotNull null)
        }
        val track = Track(
            id = id,
            title = o.str("title") ?: "",
            artists = artists,
            albumId = o.str("album"),
            albumTitle = o.str("albumTitle"),
            durationMs = o.str("ms")?.toLongOrNull(),
            available = o.str("available") != "false",
            coverUri = o.str("cover"),
        )
        val date = try {
            LocalDate.parse(o.str("date") ?: return null)
        } catch (_: Exception) {
            return null
        }
        val context = decodeContext(o["ctx"]) ?: return null
        return LocalPlay(track, context, date, o.str("at")?.toLongOrNull() ?: 0L)
    }

    private fun encodeContext(c: HistoryContext): JsonObject = buildJsonObject {
        when (c) {
            is HistoryContext.Album -> { put("t", "album"); put("id", c.id); put("title", c.title) }
            is HistoryContext.Artist -> { put("t", "artist"); put("id", c.id); put("title", c.name) }
            is HistoryContext.Playlist -> {
                put("t", "playlist"); put("id", c.uid + SEP + c.kind); put("title", c.title)
            }
            HistoryContext.Wave -> put("t", "wave")
            HistoryContext.Liked -> put("t", "liked")
            is HistoryContext.Other -> { put("t", "other"); put("title", c.title) }
        }
    }

    private fun decodeContext(element: kotlinx.serialization.json.JsonElement?): HistoryContext? {
        val o = element as? JsonObject ?: return null
        val title = o.str("title") ?: ""
        return when (o.str("t")) {
            "album" -> HistoryContext.Album(o.str("id") ?: return null, title)
            "artist" -> HistoryContext.Artist(o.str("id") ?: return null, title)
            "playlist" -> {
                val parts = (o.str("id") ?: return null).split(SEP)
                HistoryContext.Playlist(parts.getOrNull(0) ?: return null, parts.getOrNull(1) ?: return null, title)
            }
            "wave" -> HistoryContext.Wave
            "liked" -> HistoryContext.Liked
            "other" -> HistoryContext.Other(title)
            else -> null
        }
    }
}
