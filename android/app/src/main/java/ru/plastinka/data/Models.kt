package ru.plastinka.data

import kotlinx.serialization.json.JsonObject

/** Трек каталога. Парсится мягко: поля нет — значение по умолчанию. */
data class Track(
    val id: String,
    val title: String,
    val artists: List<ArtistRef>,
    val albumId: String?,
    val albumTitle: String?,
    val durationMs: Long?,
    val available: Boolean,
    val coverUri: String?,
    /** Цвет трека для VibeColorizer (только у порций волны). */
    val vibe: VibeColors? = null,
    /** Громкость по EBU R128 (поле `r128`), если сервис её сообщил: для нормализации. */
    val loudness: Loudness? = null,
    /** Пометка `contentWarning: explicit` (ненормативная лексика): значок «E». */
    val explicit: Boolean = false,
) {
    val artistNames: String get() = artists.joinToString(", ") { it.name }
    fun coverUrl(size: String = "400x400"): String? =
        coverUri?.takeIf { it.contains("%%") }
            ?.let { "https://${it.replace("%%", size)}" }
}

/** Громкость трека по EBU R128: интегральная (LUFS) и истинный пик (дБFS). */
data class Loudness(val integrated: Double, val truePeak: Double) {
    /**
     * Поправка громкости, дБ: к цели в -14 LUFS, но так, чтобы пик после
     * усиления не вышел выше -1 дБFS; в пределах от -15 до +6 дБ (как на
     * десктопе: src/api/tracks.rs).
     */
    fun gainDb(): Double =
        minOf(TARGET_LUFS - integrated, PEAK_CEILING - truePeak).coerceIn(MIN_GAIN_DB, MAX_GAIN_DB)

    /** Множитель амплитуды 0,18…2. */
    fun gainFactor(): Float = Math.pow(10.0, gainDb() / 20.0).toFloat()

    companion object {
        const val TARGET_LUFS = -14.0
        const val PEAK_CEILING = -1.0
        const val MIN_GAIN_DB = -15.0
        const val MAX_GAIN_DB = 6.0
    }
}

/** Источники цвета волны: `derivedColors.accent` → `average` → `hue`. */
data class VibeColors(
    val accent: String?,
    val average: String?,
    val hue: Double?,
)

/** Персональная подборка с лендинга («Плейлист дня», «Дежавю», «Премьера»). */
data class GeneratedPlaylist(
    /** Владелец подборки (служебный аккаунт Яндекса). */
    val uid: Long,
    val kind: Long,
    val title: String,
    val coverUri: String?,
    val trackCount: Int,
) {
    fun coverUrl(size: String = "300x300"): String? =
        coverUri?.takeIf { it.contains("%%") }?.let { "https://${it.replace("%%", size)}" }
}

/** Плейлист пользователя: свой или понравившийся чужой. */
data class PlaylistCard(
    /** Владелец плейлиста. */
    val uid: Long,
    val kind: Long,
    val title: String,
    val coverUri: String?,
    val trackCount: Int,
    /** `true` — чужой, отмеченный «нравится». */
    val liked: Boolean = false,
) {
    fun coverUrl(size: String = "200x200"): String? =
        coverUri?.takeIf { it.contains("%%") }?.let { "https://${it.replace("%%", size)}" }
}

enum class AlbumKind(val title: String) {
    Album("Альбом"),
    Single("Сингл"),
    Compilation("Сборник"),
}

/** Альбом в списке или в шапке. */
data class AlbumCard(
    val id: String,
    val title: String,
    val version: String?,
    val year: Int?,
    val kind: AlbumKind,
    val artists: List<ArtistRef>,
    val coverUri: String?,
    val trackCount: Int,
) {
    /** Название с версией: «Название (Deluxe)». */
    val displayTitle: String get() = if (version != null) "$title ($version)" else title
    val artistNames: String get() = artists.joinToString(", ") { it.name }
    fun coverUrl(size: String = "300x300"): String? =
        coverUri?.takeIf { it.contains("%%") }?.let { "https://${it.replace("%%", size)}" }
}

/** Исполнитель в списке или в шапке. */
data class ArtistCard(
    val id: String,
    val name: String,
    val coverUri: String?,
    val albumsCount: Int,
) {
    fun coverUrl(size: String = "300x300"): String? =
        coverUri?.takeIf { it.contains("%%") }?.let { "https://${it.replace("%%", size)}" }
}

/** Как исполнитель сдвинулся в топе месяца. */
enum class Trend { Up, Down, Same, New }

/** Исполнитель из топа месяца: место, сдвиг и сколько его слушали (секунды). */
data class TopArtist(
    val card: ArtistCard,
    val position: Int?,
    val trend: Trend?,
    val listenedSeconds: Long,
)

/**
 * Страница «Топ месяца»: исполнители и треки. `plays[i]` — число прослушиваний
 * трека `tracks[i]`; список чисел совпадает по длине с треками.
 */
data class MonthTop(
    val artists: List<TopArtist>,
    val tracks: List<Track>,
    val plays: List<Int>,
)

/** Альбом с треками (диски склеены по порядку). */
data class AlbumDetails(val card: AlbumCard, val tracks: List<Track>)

/** Страница исполнителя: шапка, популярные треки, альбомы. */
data class ArtistDetails(
    val card: ArtistCard,
    val popular: List<Track>,
    val albums: List<AlbumCard>,
)

data class ArtistRef(val id: String?, val name: String)

data class AccountInfo(
    val uid: Long,
    val displayName: String,
    val hasPlus: Boolean,
    /** Аватар с бека (avatars.yandex.net), если у аккаунта он есть. */
    val avatarUrl: String? = null,
)

data class DeviceCode(
    val deviceCode: String,
    val userCode: String,
    val verificationUrl: String,
    val intervalSec: Long,
    val expiresInSeconds: Long,
)

sealed interface StreamResult {
    data class Url(val url: String, val kbps: Int, val codec: String) : StreamResult
    data object Unavailable : StreamResult
    data object Unauthorized : StreamResult
    data class Error(val message: String) : StreamResult
}

/** Внутренний «адрес» трека в очереди: plastinka://track/<id>[?alt=<id>]. */
object TrackUris {
    const val SCHEME = "plastinka"
    fun forTrack(trackId: String): String = "$SCHEME://track/$trackId"
    fun trackIdOf(uri: String): String? =
        uri.takeIf { it.startsWith("$SCHEME://track/") }?.removePrefix("$SCHEME://track/")
}

/** Мягкий разбор JSON-значений. */
internal fun JsonObject.str(vararg keys: String): String? {
    var node: kotlinx.serialization.json.JsonElement? = this
    for (key in keys) {
        node = (node as? JsonObject)?.get(key) ?: return null
    }
    // JsonNull тоже примитив с текстом «null»: это отсутствие значения.
    return node?.let { el ->
        (el as? kotlinx.serialization.json.JsonPrimitive)
            ?.takeIf { it !is kotlinx.serialization.json.JsonNull }?.content
    }
}

/** Объект не найден: неверный идентификатор или `error` в ответе. */
class NotFoundException : Exception("не найден")

/** Тип поиска: параметр `type` запроса `/search`. */
enum class SearchKind(val apiType: String) {
    All("all"),
    Track("track"),
    Album("album"),
    Artist("artist"),
    Playlist("playlist"),
}

/** Лучший результат поиска. */
sealed interface SearchBest {
    data class TrackHit(val track: Track) : SearchBest
    data class AlbumHit(val card: AlbumCard) : SearchBest
    data class ArtistHit(val card: ArtistCard) : SearchBest
    data class PlaylistHit(val card: PlaylistCard) : SearchBest
}

/** Ответ поиска: для вкладки «Всё» заполнены все списки, для типа — только его. */
data class SearchResult(
    val best: SearchBest? = null,
    val tracks: List<Track> = emptyList(),
    val albums: List<AlbumCard> = emptyList(),
    val artists: List<ArtistCard> = emptyList(),
    val playlists: List<PlaylistCard> = emptyList(),
    /** Есть следующая страница (только для одного типа). */
    val hasNext: Boolean = false,
) {
    val isEmpty: Boolean get() = tracks.isEmpty() && albums.isEmpty() && artists.isEmpty() && playlists.isEmpty()
}
