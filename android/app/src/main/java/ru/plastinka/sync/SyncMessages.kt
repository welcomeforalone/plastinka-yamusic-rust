package ru.plastinka.sync

import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put
import ru.plastinka.data.ArtistRef
import ru.plastinka.data.Loudness
import ru.plastinka.data.Track

/** Состояние воспроизведения (§4.3). Поля и имена как в спеке. */
data class SyncState(
    val rev: Long,
    val by: String,
    val owner: String,
    val track: Track?,
    val playing: Boolean,
    /** Позиция в момент [at] (часы отправителя). */
    val positionMs: Long,
    val at: Long,
    val next: Track? = null,
    val shuffle: Boolean = false,
    /** `off`, `all` или `one`. */
    val repeat: String = "off",
    val wave: Boolean = false,
    val initial: Boolean = false,
    val join: Boolean = false,
)

/** Сообщение готовой связи и проверки (§3.4, §4). */
sealed interface SyncMessage {
    data class Ping(val n: Long, val t0: Long) : SyncMessage
    data class Pong(val n: Long, val t0: Long, val t1: Long) : SyncMessage
    data class State(val state: SyncState) : SyncMessage
    data class Tick(val rev: Long, val positionMs: Long, val at: Long, val buffering: Boolean) : SyncMessage

    /** `next`, `previous`, `shuffle`, `repeat`, `dislike`. */
    data class Command(val action: String) : SyncMessage
    data class Liked(val track: String, val liked: Boolean) : SyncMessage
    data class Bye(val reason: String) : SyncMessage

    /** Проверка узнавания; [mac] — 32 байта. */
    class Auth(val mac: ByteArray) : SyncMessage
    data object Confirm : SyncMessage
}

/** Разбор и сборка сообщений. Неизвестное и битое не бросает исключений: разбор даёт `null` («пропустить»). */
object SyncMessages {
    val COMMANDS = setOf("next", "previous", "shuffle", "repeat", "dislike")
    val REPEAT_MODES = setOf("off", "all", "one")

    private const val MAX_TEXT = 512
    private const val MAX_ARTISTS = 20
    private const val MAX_DURATION_MS = 86_400_000L
    private val ID_RE = Regex("[0-9A-Za-z_-]{1,64}")
    private val DIGITS_RE = Regex("[0-9]{1,20}")
    private val COVER_HOSTS = listOf("avatars.yandex.net/", "avatars.mds.yandex.net/")

    private val json = Json

    /** Разобрать JSON-объект кадра; не объект и битый JSON — `null`. */
    fun parseObject(bytes: ByteArray): JsonObject? = try {
        json.parseToJsonElement(bytes.toString(Charsets.UTF_8)) as? JsonObject
    } catch (_: SerializationException) {
        null
    } catch (_: IllegalArgumentException) {
        null
    }

    /** Сообщение из расшифрованных байт или `null`: неизвестный `t`, битые поля. */
    fun parse(bytes: ByteArray): SyncMessage? {
        val obj = parseObject(bytes) ?: return null
        return try {
            parseObject(obj)
        } catch (_: BadMessage) {
            null
        }
    }

    fun encode(msg: SyncMessage): ByteArray = toJson(msg).toString().toByteArray(Charsets.UTF_8)

    fun toJson(msg: SyncMessage): JsonObject = when (msg) {
        is SyncMessage.Ping -> buildJsonObject {
            put("t", "ping"); put("n", msg.n); put("t0", msg.t0)
        }
        is SyncMessage.Pong -> buildJsonObject {
            put("t", "pong"); put("n", msg.n); put("t0", msg.t0); put("t1", msg.t1)
        }
        is SyncMessage.State -> stateJson(msg.state)
        is SyncMessage.Tick -> buildJsonObject {
            put("t", "tick"); put("rev", msg.rev); put("positionMs", msg.positionMs)
            put("at", msg.at); put("buffering", msg.buffering)
        }
        is SyncMessage.Command -> buildJsonObject { put("t", "command"); put("action", msg.action) }
        is SyncMessage.Liked -> buildJsonObject {
            put("t", "liked"); put("track", msg.track); put("liked", msg.liked)
        }
        is SyncMessage.Bye -> buildJsonObject { put("t", "bye"); put("reason", msg.reason) }
        is SyncMessage.Auth -> buildJsonObject {
            put("t", "auth"); put("mac", java.util.Base64.getEncoder().encodeToString(msg.mac))
        }
        SyncMessage.Confirm -> buildJsonObject { put("t", "confirm") }
    }

    private fun stateJson(s: SyncState): JsonObject = buildJsonObject {
        put("t", "state")
        put("rev", s.rev)
        put("by", s.by)
        put("owner", s.owner)
        put("track", s.track?.let { trackJson(it) } ?: JsonNull)
        put("playing", s.playing)
        put("positionMs", s.positionMs)
        put("at", s.at)
        put("next", s.next?.let { trackJson(it) } ?: JsonNull)
        put("shuffle", s.shuffle)
        put("repeat", s.repeat)
        put("wave", s.wave)
        put("initial", s.initial)
        put("join", s.join)
    }

    // ---- Трек (§4.1) ----

    /** Трек для сети: настоящий (без подмен FckCensor), поля `source*`. */
    fun trackJson(t: Track): JsonObject = buildJsonObject {
        put("id", t.id)
        put("albumId", t.albumId)
        put("albumTitle", t.albumTitle)
        put("title", t.sourceTitle)
        put("version", t.version)
        put(
            "artists",
            buildJsonArray {
                for (a in t.sourceArtists) {
                    add(buildJsonObject { put("id", a.id); put("name", a.name) })
                }
            },
        )
        put("durationMs", t.durationMs)
        put("coverUri", t.sourceCoverUri)
        put("available", t.available)
        put("warning", if (t.explicit) "explicit" else if (t.clean) "clean" else null)
        put(
            "loudness",
            t.loudness?.let { l ->
                buildJsonObject { put("integrated", l.integrated); put("truePeak", l.truePeak) }
            } ?: JsonNull,
        )
    }

    /** Проверенный трек из JSON или `null`, если он не прошёл проверки §4.1 (кроме `coverUri`). */
    fun parseTrack(el: JsonElement?): Track? = try {
        trackOrNull(el)
    } catch (_: BadMessage) {
        null
    }

    // ---- Разбор ----

    internal class BadMessage : Exception()

    private fun bad(): Nothing = throw BadMessage()

    private fun parseObject(o: JsonObject): SyncMessage? = when (o.reqString("t")) {
        "ping" -> SyncMessage.Ping(o.reqLong("n"), o.reqLong("t0"))
        "pong" -> SyncMessage.Pong(o.reqLong("n"), o.reqLong("t0"), o.reqLong("t1"))
        "state" -> SyncMessage.State(parseState(o))
        "tick" -> SyncMessage.Tick(
            rev = o.reqLong("rev").also { if (it < 0) bad() },
            positionMs = o.reqLong("positionMs").also { if (it < 0) bad() },
            at = o.reqLong("at"),
            buffering = o.bool("buffering"),
        )
        "command" -> o.reqString("action").takeIf { it in COMMANDS }?.let { SyncMessage.Command(it) }
        "liked" -> SyncMessage.Liked(
            track = o.reqString("track").also { if (!ID_RE.matches(it)) bad() },
            liked = o.reqBool("liked"),
        )
        "bye" -> SyncMessage.Bye(o.optString("reason", 64) ?: "")
        "auth" -> {
            val mac = try {
                java.util.Base64.getDecoder().decode(o.reqString("mac"))
            } catch (_: IllegalArgumentException) {
                bad()
            }
            if (mac.size != 32) bad()
            SyncMessage.Auth(mac)
        }
        "confirm" -> SyncMessage.Confirm
        else -> null
    }

    private fun parseState(o: JsonObject): SyncState {
        val rev = o.reqLong("rev")
        val positionMs = o.reqLong("positionMs")
        if (rev < 0 || positionMs < 0) bad()
        val repeat = o.optString("repeat", 8) ?: "off"
        if (repeat !in REPEAT_MODES) bad()
        return SyncState(
            rev = rev,
            by = o.reqString("by").also { if (it.isEmpty() || it.length > 64) bad() },
            owner = o.reqString("owner").also { if (it.isEmpty() || it.length > 64) bad() },
            track = trackOrNull(o["track"]),
            playing = o.bool("playing"),
            positionMs = positionMs,
            at = o.reqLong("at"),
            next = trackOrNull(o["next"]),
            shuffle = o.bool("shuffle"),
            repeat = repeat,
            wave = o.bool("wave"),
            initial = o.bool("initial"),
            join = o.bool("join"),
        )
    }

    private fun trackOrNull(el: JsonElement?): Track? {
        if (el == null || el is JsonNull) return null
        val o = el as? JsonObject ?: bad()
        val id = o.reqString("id")
        if (!ID_RE.matches(id)) bad()
        val albumId = o.optDigits("albumId")
        val artistsEl = o["artists"]
        val artists = when (artistsEl) {
            null, is JsonNull -> emptyList()
            is JsonArray -> {
                if (artistsEl.size > MAX_ARTISTS) bad()
                artistsEl.map { a ->
                    val ao = a as? JsonObject ?: bad()
                    ArtistRef(ao.optDigits("id"), ao.optString("name", MAX_TEXT) ?: "")
                }
            }
            else -> bad()
        }
        val duration = o.optLong("durationMs")
        if (duration != null && (duration < 0 || duration > MAX_DURATION_MS)) bad()
        val cover = o.optString("coverUri", Int.MAX_VALUE)
            ?.takeIf { c -> c.contains("%%") && c.all { ch -> ch.code in 0x21..0x7e } && COVER_HOSTS.any { c.startsWith(it) } }
        val warning = o.optString("warning", 16)
        if (warning != null && warning != "explicit" && warning != "clean") bad()
        val loudness = when (val l = o["loudness"]) {
            null, is JsonNull -> null
            is JsonObject -> {
                val integrated = l.reqDouble("integrated")
                val truePeak = l.reqDouble("truePeak")
                Loudness(integrated, truePeak)
            }
            else -> bad()
        }
        return Track(
            id = id,
            sourceTitle = o.optString("title", MAX_TEXT) ?: "",
            sourceArtists = artists,
            albumId = albumId,
            albumTitle = o.optString("albumTitle", MAX_TEXT),
            durationMs = duration,
            // Поля нет — трек считается недоступным (§4.1).
            available = o.bool("available", false),
            sourceCoverUri = cover,
            loudness = loudness,
            explicit = warning == "explicit",
            clean = warning == "clean",
            version = o.optString("version", MAX_TEXT),
        )
    }

    // ---- Помощники разбора ----

    private fun JsonObject.reqString(key: String): String {
        val p = this[key] as? JsonPrimitive ?: bad()
        if (!p.isString) bad()
        return p.content
    }

    /** Строка, `null` (поля нет или `null`); не строка или длиннее [max] — ошибка. */
    private fun JsonObject.optString(key: String, max: Int): String? {
        val el = this[key]
        if (el == null || el is JsonNull) return null
        val p = el as? JsonPrimitive ?: bad()
        if (!p.isString || p.content.codePointCount(0, p.content.length) > max) bad()
        return p.content
    }

    private fun JsonObject.optDigits(key: String): String? =
        optString(key, 20)?.also { if (!DIGITS_RE.matches(it)) bad() }

    private fun JsonObject.optLong(key: String): Long? {
        val el = this[key]
        if (el == null || el is JsonNull) return null
        val p = el as? JsonPrimitive ?: bad()
        if (p.isString) bad()
        return p.longOrNull ?: bad()
    }

    private fun JsonObject.reqLong(key: String): Long = optLong(key) ?: bad()

    private fun JsonObject.reqDouble(key: String): Double {
        val p = this[key] as? JsonPrimitive ?: bad()
        if (p.isString) bad()
        val d = p.doubleOrNull ?: bad()
        if (!d.isFinite()) bad()
        return d
    }

    private fun JsonObject.bool(key: String, default: Boolean = false): Boolean {
        val el = this[key]
        if (el == null || el is JsonNull) return default
        val p = el as? JsonPrimitive ?: bad()
        if (p.isString) bad()
        return p.booleanOrNull ?: bad()
    }

    private fun JsonObject.reqBool(key: String): Boolean {
        val el = this[key]
        if (el == null || el is JsonNull) bad()
        return bool(key)
    }
}
