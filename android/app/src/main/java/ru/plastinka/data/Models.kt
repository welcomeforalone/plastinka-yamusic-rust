package ru.plastinka.data

import kotlinx.serialization.json.JsonObject

/**
 * Трек каталога. Парсится мягко: поля нет — значение по умолчанию.
 *
 * `source*` — то, что прислал Яндекс (с цензурой). Показывают [title], [artists] и
 * [coverUri]: при включённом FckCensor и треке из списка они отдают версию без цензуры
 * (чтение идёт через снимок состояния Compose, поэтому переключатель плагина перерисовывает
 * списки сразу). Настоящий трек остаётся настоящим: [id] не меняется, лайки, «Не нравится»,
 * отчёты о прослушивании, события волны и тексты Яндекса работают по нему.
 */
data class Track(
    val id: String,
    val sourceTitle: String,
    val sourceArtists: List<ArtistRef>,
    val albumId: String?,
    val albumTitle: String?,
    val durationMs: Long?,
    val available: Boolean,
    val sourceCoverUri: String?,
    /** Цвет трека для VibeColorizer (только у порций волны). */
    val vibe: VibeColors? = null,
    /** Громкость по EBU R128 (поле `r128`), если сервис её сообщил: для нормализации. */
    val loudness: Loudness? = null,
    /** Пометка `contentWarning: explicit` (ненормативная лексика): значок «E». */
    val explicit: Boolean = false,
    /** Пометка `contentWarning: clean`: цензурная версия трека (для автопоиска explicit-версии FckCensor). */
    val clean: Boolean = false,
    /** Версия трека (`version`: «Live», «Remastered»); `null` — версии нет. */
    val version: String? = null,
) {
    /**
     * Трек можно включить: он доступен, FckCensor даёт ему звук из списка или найдена копия в
     * другом релизе ([Relink]). Как `playable` на десктопе; чтение идёт через снимок состояния
     * Compose, строки перерисовываются сразу.
     */
    val playable: Boolean get() = available || FckCensor.hasSound(id) || Relink.hasCopy(id)

    /**
     * Название без версии (с подменой FckCensor, если она есть). Им ищут: текст песни в LRCLib,
     * explicit-версию и копию. Для показа служит [displayTitle].
     */
    val title: String get() = FckCensor.trackPatch(id)?.title ?: sourceTitle

    /** Название для показа: «Название (Версия)», как `display_title` на десктопе. */
    val displayTitle: String get() = title.let { t -> version?.let { "$t ($it)" } ?: t }

    /** Исполнители для показа (см. [title]). Slopless спрашивает и [sourceArtists]. */
    val artists: List<ArtistRef> get() = FckCensor.trackPatch(id)?.artists ?: sourceArtists

    /** Обложка для показа (см. [title]). */
    val coverUri: String? get() = FckCensor.trackPatch(id)?.coverUri ?: sourceCoverUri

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
    /** Описание подборки «Собираем для вас» («Звучит по-вашему каждый день»); у обычных плейлистов нет. */
    val note: String? = null,
) {
    fun coverUrl(size: String = "200x200"): String? =
        coverUri?.takeIf { it.contains("%%") }?.let { "https://${it.replace("%%", size)}" }
}

enum class AlbumKind(val title: String) {
    Album("Альбом"),
    Single("Сингл"),
    Compilation("Сборник"),
}

/**
 * Альбом в списке или в шапке. Как у [Track]: `source*` — от Яндекса, [title], [artists] и
 * [coverUri] — для показа (с учётом FckCensor).
 */
data class AlbumCard(
    val id: String,
    val sourceTitle: String,
    val version: String?,
    val year: Int?,
    val kind: AlbumKind,
    val sourceArtists: List<ArtistRef>,
    val sourceCoverUri: String?,
    val trackCount: Int,
) {
    val title: String get() = FckCensor.albumPatch(id)?.title ?: sourceTitle
    val artists: List<ArtistRef> get() = FckCensor.albumPatch(id)?.artists ?: sourceArtists
    val coverUri: String? get() = FckCensor.albumPatch(id)?.coverUri ?: sourceCoverUri

    /** Название с версией: «Название (Deluxe)». Подменённое название идёт без версии цензурного. */
    val displayTitle: String
        get() {
            val patched = FckCensor.albumPatch(id)?.title
            return when {
                patched != null -> patched
                version != null -> "$sourceTitle ($version)"
                else -> sourceTitle
            }
        }
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

/**
 * Страница «Все треки» исполнителя. `fetched` — сколько элементов пришло (до отсева
 * нечитаемых): 0 значит, что страницы кончились, даже если `total` больше.
 */
data class ArtistTracksPage(
    val tracks: List<Track>,
    val total: Int,
    val fetched: Int,
)

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
    /**
     * Ссылку получить не вышло. `network` — сбой сети или сервера (запрос стоит повторить),
     * иначе причина в самом треке (нет ссылки, зашифрован, чужой хост). В `message` — только
     * тип сбоя или код ответа: текста, где может оказаться адрес или токен, здесь нет.
     */
    data class Error(val message: String, val network: Boolean = false) : StreamResult
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
