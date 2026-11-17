package ru.plastinka.data

import android.util.Log
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put
import java.io.File

/**
 * Подбор explicit-версии трека среди результатов поиска Яндекса. Правила те же, что у
 * `pick_explicit` на десктопе (`src/plugins/fckcensor.rs`): explicit, доступен, не сам трек,
 * тот же исполнитель, то же название, длительность в пределах 10 секунд. Сравнивается то, что
 * прислал Яндекс (`source*`), а не подменённое списком.
 */
internal object ExplicitMatch {

    /** Допустимая разница длительности, мс. */
    private const val DURATION_SLACK_MS = 10_000L

    /**
     * Название для сравнения: нижний регистр, «ё» как «е», только буквы, цифры и звёздочки
     * цензуры («с*ка»); пробел, дефис и тире — один пробел между словами.
     */
    fun normalize(text: String): String {
        val out = StringBuilder(text.length)
        var space = true
        for (raw in text.lowercase(java.util.Locale.ROOT)) {
            val ch = if (raw == 'ё') 'е' else raw
            if (ch.isLetterOrDigit() || ch == '*') {
                out.append(ch)
                space = false
            } else if (!space && (ch.isWhitespace() || ch == '-' || ch == '—')) {
                out.append(' ')
                space = true
            }
        }
        return out.toString().trimEnd()
    }

    /** Есть общий исполнитель: по номеру, а если номера нет у одного из них — по названию. */
    fun sameArtist(a: List<ArtistRef>, b: List<ArtistRef>): Boolean = a.any { left ->
        b.any { right ->
            val x = left.id
            val y = right.id
            if (x != null && y != null) x == y else normalize(left.name) == normalize(right.name)
        }
    }

    /**
     * Названия совпадают или одно начинается с другого. Звёздочка цензуры подходит к любой
     * одной букве: «м*лния» и «молния» — одно слово.
     */
    fun sameTitle(a: String, b: String): Boolean {
        val left = normalize(a)
        val right = normalize(b)
        if (left.isEmpty() || right.isEmpty()) return false
        // Сравниваем по общей длине: так проверяется и «начинается с».
        return left.zip(right).all { (x, y) -> x == y || (x == '*' && y != ' ') || (y == '*' && x != ' ') }
    }

    /** Версия [original] без цензуры среди [candidates]; из подходящих — с ближайшей длительностью. */
    fun pick(original: Track, candidates: List<Track>): Track? {
        if (original.sourceTitle.isBlank()) return null
        return candidates
            .filter { c ->
                c.explicit && c.available && c.id != original.id &&
                    sameArtist(original.sourceArtists, c.sourceArtists) &&
                    sameTitle(original.sourceTitle, c.sourceTitle)
            }
            .mapNotNull { c ->
                val a = original.durationMs
                val b = c.durationMs
                val gap = if (a != null && b != null) Math.abs(a - b) else 0L
                if (gap <= DURATION_SLACK_MS) gap to c else null
            }
            .minByOrNull { it.first }
            ?.second
    }
}

/** Что известно о поиске explicit-версии трека. */
sealed interface ExplicitLookup {
    /** Не искали (или результат устарел). */
    data object Missing : ExplicitLookup

    /** Искали: explicit-версии на Яндексе нет. */
    data object NotFound : ExplicitLookup

    /** Нашли: играть трек [id]. */
    data class Found(val id: String) : ExplicitLookup
}

/**
 * Найденные explicit-версии: номер трека → номер explicit-трека или «такого нет», и когда
 * искали (`cacheDir/plugins/fckcensor-explicit.json`, формат
 * `{"found": {"<id>": {"to": "<id>" | null, "at": <мс>}}}`). Только метаданные: номера, не
 * звук. Результат живёт 30 дней. Файл читается один раз по [ensureLoaded] (на потоке
 * ввода-вывода), запись — через временный файл и переименование; сбой записи не мешает работе.
 */
class ExplicitCache(private val file: File) {

    private class Entry(val to: String?, val atMs: Long)

    private val map = LinkedHashMap<String, Entry>()
    private var loaded = false

    /** Результат поиска для [id], если он свежий. Файл здесь не читается: безопасно на любом потоке. */
    @Synchronized
    fun get(id: String): ExplicitLookup {
        val entry = map[id] ?: return ExplicitLookup.Missing
        if (stale(entry.atMs)) return ExplicitLookup.Missing
        val to = entry.to ?: return ExplicitLookup.NotFound
        return ExplicitLookup.Found(to)
    }

    /** Запомнить результат поиска ([to] — `null`, если explicit-версии нет) и записать файл. */
    @Synchronized
    fun put(id: String, to: String?) {
        map.remove(id)
        map[id] = Entry(to, System.currentTimeMillis())
        while (map.size > MAX_ENTRIES) {
            val oldest = map.keys.firstOrNull() ?: break
            map.remove(oldest)
        }
        write()
    }

    /** Прочитать файл, если ещё не читали. Битый или слишком большой файл — пустой кэш, без ошибки. */
    @Synchronized
    fun ensureLoaded() {
        if (loaded) return
        loaded = true
        try {
            if (!file.isFile || file.length() > FILE_LIMIT) return
            val root = Json.parseToJsonElement(file.readText()) as? JsonObject ?: return
            val found = root["found"] as? JsonObject ?: return
            for ((id, value) in found) {
                val obj = value as? JsonObject ?: continue
                val at = (obj["at"] as? JsonPrimitive)?.longOrNull ?: continue
                val to = (obj["to"] as? JsonPrimitive)?.takeIf { it !is JsonNull }?.content
                if (!plausibleId(id) || (to != null && !plausibleId(to)) || stale(at)) continue
                // Уже добавленное в этом сеансе новее файла.
                map.putIfAbsent(id, Entry(to, at))
            }
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            Log.w(TAG, "кэш explicit-версий не прочитан: ${e.javaClass.simpleName}")
        }
    }

    private fun write() {
        try {
            file.parentFile?.mkdirs()
            val tmp = File(file.parentFile, "${file.name}.tmp")
            tmp.writeText(
                buildJsonObject {
                    put(
                        "found",
                        buildJsonObject {
                            for ((id, entry) in map) {
                                put(
                                    id,
                                    buildJsonObject {
                                        // null записывается как JSON null: «искали, версии нет».
                                        put("to", entry.to)
                                        put("at", entry.atMs)
                                    },
                                )
                            }
                        },
                    )
                }.toString(),
            )
            if (!tmp.renameTo(file)) {
                file.delete()
                tmp.renameTo(file)
            }
        } catch (e: Exception) {
            Log.w(TAG, "кэш explicit-версий не записан: ${e.javaClass.simpleName}")
        }
    }

    private fun plausibleId(id: String): Boolean = plausibleNumericId(id) && id.any { it != '0' }

    /** Устарел, а запись «из будущего» (часы ушли назад) тоже не верим. */
    private fun stale(atMs: Long): Boolean {
        val age = System.currentTimeMillis() - atMs
        return age < 0 || age > FRESH_MS
    }

    private companion object {
        const val TAG = "FckExplicit"
        const val FRESH_MS = 30L * 24 * 3600 * 1000
        const val MAX_ENTRIES = 5000
        const val FILE_LIMIT = 2L * 1024 * 1024
    }
}
