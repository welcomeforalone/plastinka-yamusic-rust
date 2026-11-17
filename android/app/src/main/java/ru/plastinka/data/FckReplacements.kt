package ru.plastinka.data

import android.util.Log
import androidx.compose.runtime.mutableIntStateOf
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.io.File

/**
 * Ручная замена звука трека FckCensor (как `Replacement` на десктопе, `src/plugins/fckcensor.rs`):
 * другой трек Яндекса или свой файл на устройстве. Хранятся только метаданные: номер трека
 * Яндекса и подпись либо адрес `content://` файла и его имя. Сам файл только читается плеером.
 */
sealed interface Replacement {
    /** Подпись для меню. */
    val label: String

    /** Трек Яндекса [id]; [title] — «Исполнитель — Название» для подписи. */
    data class YandexTrack(val id: String, val title: String?) : Replacement {
        override val label: String get() = title?.takeIf { it.isNotBlank() } ?: id
    }

    /** Свой файл: [uri] — адрес `content://` с постоянным разрешением на чтение, [name] — имя файла. */
    data class LocalFile(val uri: String, val name: String) : Replacement {
        override val label: String get() = name.ifBlank { "файл" }
    }
}

/** Ссылка на трек Яндекс Музыки (как `track_from_link` на десктопе). */
internal object TrackLink {

    private val ZONES = listOf("ru", "com", "by", "kz", "uz")

    /**
     * Номер трека из ссылки `…/album/1/track/2` или `…/track/2` на `music.yandex.ru|com|by|kz|uz`
     * (с `https://`, `http://` или без схемы, с `www.`). Не ссылка на трек — `null`; чужой домен
     * (`music.yandex.ru.evil.example`) тоже.
     */
    fun trackId(text: String): String? {
        val trimmed = text.trim()
        val rest = trimmed.removePrefix("https://").takeIf { it.length != trimmed.length }
            ?: trimmed.removePrefix("http://")
        val slash = rest.indexOf('/')
        if (slash < 0) return null
        val host = rest.substring(0, slash).lowercase(java.util.Locale.ROOT).removePrefix("www.")
        if (ZONES.none { host == "music.yandex.$it" }) return null
        val path = rest.substring(slash + 1).substringBefore('?').substringBefore('#')
        val parts = path.split('/').filter { it.isNotEmpty() }
        val at = parts.indexOf("track")
        if (at < 0) return null
        return parts.getOrNull(at + 1)?.takeIf(::plausibleTrackId)
    }
}

/** Номер трека: только цифры, не длиннее 32 знаков, не все нули. */
internal fun plausibleTrackId(id: String): Boolean = plausibleNumericId(id) && id.any { it != '0' }

/** Расширения аудиофайлов для замены (как `AUDIO_EXTENSIONS` на десктопе). */
internal val AUDIO_EXTENSIONS = listOf("mp3", "flac", "ogg", "oga", "wav", "m4a", "aac")

/** Типы файлов для системного выбора: те же форматы, что и [AUDIO_EXTENSIONS]. */
val AUDIO_MIME_TYPES = arrayOf(
    "audio/mpeg", "audio/mp3",
    "audio/flac", "audio/x-flac",
    "audio/ogg", "application/ogg",
    "audio/wav", "audio/x-wav", "audio/vnd.wave", "audio/wave",
    "audio/mp4", "audio/x-m4a", "audio/m4a",
    "audio/aac", "audio/aacp", "audio/x-aac",
)

/** Имя файла подходит под список форматов замены. */
internal fun isAudioName(name: String): Boolean =
    name.substringAfterLast('.', "").lowercase(java.util.Locale.ROOT) in AUDIO_EXTENSIONS

/**
 * Ручные замены: номер трека → замена. Файл `filesDir/fckcensor-replacements.json`, формат
 * `{"version":1,"tracks":{"<id>":{"track":"<id>","title":"…"} | {"file":"content://…","name":"…"}}}`.
 * Запись через временный файл и переименование; битый файл или запись читаются как «замен нет»,
 * без ошибки. Только метаданные: ни звука, ни копии файла здесь нет. «Сбросить всё» замены не
 * трогает (на десктопе так же). Чтение файла — один раз, [ensureLoaded] на потоке ввода-вывода.
 */
class Replacements(private val file: File) {

    private val map = LinkedHashMap<String, Replacement>()
    private var loaded = false

    /** Растёт при любом изменении: экраны читают его и перерисовываются. */
    private val versionState = mutableIntStateOf(0)

    /** Подписать текущую композицию на изменения замен. */
    fun observe(): Int = versionState.intValue

    @Synchronized
    fun get(id: String): Replacement? = map[id]

    @Synchronized
    fun has(id: String): Boolean = map.containsKey(id)

    @Synchronized
    fun count(): Int = map.size

    /** Адрес файла у какой-либо замены (кроме, возможно, уже убранной). */
    @Synchronized
    fun usesUri(uri: String): Boolean = map.values.any { it is Replacement.LocalFile && it.uri == uri }

    /** Заменить или добавить; возвращает прежнюю замену трека. Файл не пишется: см. [save]. */
    @Synchronized
    fun put(id: String, replacement: Replacement): Replacement? {
        val old = map.put(id, replacement)
        versionState.intValue += 1
        return old
    }

    /** Убрать замену трека; возвращает убранную. Файл не пишется: см. [save]. */
    @Synchronized
    fun remove(id: String): Replacement? {
        val old = map.remove(id)
        if (old != null) versionState.intValue += 1
        return old
    }

    /** Прочитать файл, если ещё не читали. Битый или слишком большой файл — пустой список. */
    @Synchronized
    fun ensureLoaded() {
        if (loaded) return
        loaded = true
        try {
            if (!file.isFile || file.length() > FILE_LIMIT) return
            val root = Json.parseToJsonElement(file.readText()) as? JsonObject ?: return
            val tracks = root["tracks"] as? JsonObject ?: return
            for ((id, value) in tracks) {
                if (map.size >= MAX_ENTRIES) break
                if (!plausibleTrackId(id)) continue
                val parsed = parse(value as? JsonObject ?: continue) ?: continue
                // Уже добавленное в этом сеансе новее файла.
                map.putIfAbsent(id, parsed)
            }
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            Log.w(TAG, "замены не прочитаны: ${e.javaClass.simpleName}")
        }
        versionState.intValue += 1
    }

    /** Свой файл приоритетнее трека, как у `Replacement::sound` на десктопе. */
    private fun parse(obj: JsonObject): Replacement? {
        val uri = (obj["file"] as? JsonPrimitive)?.takeIf { it.isString }?.content
        if (uri != null && isContentUri(uri)) {
            val name = (obj["name"] as? JsonPrimitive)?.takeIf { it.isString }?.content.orEmpty()
            return Replacement.LocalFile(uri, name.take(NAME_LIMIT))
        }
        val track = (obj["track"] as? JsonPrimitive)?.takeIf { it.isString }?.content
        if (track != null && plausibleTrackId(track)) {
            val title = (obj["title"] as? JsonPrimitive)?.takeIf { it.isString }?.content
            return Replacement.YandexTrack(track, title?.take(NAME_LIMIT))
        }
        return null
    }

    /** Записать файл: временный файл и переименование. `false` — не вышло (замены остались в памяти). */
    @Synchronized
    fun save(): Boolean = try {
        file.parentFile?.mkdirs()
        val tmp = File(file.parentFile, "${file.name}.tmp")
        tmp.writeText(
            buildJsonObject {
                put("version", 1)
                put(
                    "tracks",
                    buildJsonObject {
                        for ((id, item) in map) {
                            put(
                                id,
                                buildJsonObject {
                                    when (item) {
                                        is Replacement.YandexTrack -> {
                                            put("track", item.id)
                                            item.title?.let { put("title", it) }
                                        }
                                        is Replacement.LocalFile -> {
                                            put("file", item.uri)
                                            put("name", item.name)
                                        }
                                    }
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
        true
    } catch (e: Exception) {
        Log.w(TAG, "замены не записаны: ${e.javaClass.simpleName}")
        false
    }

    companion object {
        private const val TAG = "FckReplacements"
        const val FILE_NAME = "fckcensor-replacements.json"
        private const val FILE_LIMIT = 2L * 1024 * 1024
        private const val MAX_ENTRIES = 5000
        private const val NAME_LIMIT = 300

        /** Только адреса `content://`: другие схемы (`file://`, `http://`) замена не принимает. */
        fun isContentUri(uri: String): Boolean = uri.startsWith("content://") && uri.length <= 4096
    }
}
