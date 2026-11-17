package ru.plastinka.data

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/** Что список FckCensor меняет у трека. Поля, которых в списке нет, — `null`. */
data class TrackPatch(
    val title: String?,
    val artists: List<ArtistRef>?,
    /** Шаблон обложки без схемы, с `%%` (хост из [COVER_HOSTS]). */
    val coverUri: String?,
    /**
     * `__fckCensor.audioSourceId`: трек Яндекса, у которого брать звук (обычным потоком
     * Яндекса). Второй в порядке решения о звуке, см. [FckCensor.resolveSound].
     */
    val audioSourceId: String?,
) {
    /** Список меняет то, что видно на экране (название, исполнителей или обложку). */
    val changesDisplay: Boolean get() = title != null || artists != null || coverUri != null
}

/** Что список FckCensor меняет у альбома. */
data class AlbumPatch(
    val title: String?,
    val artists: List<ArtistRef>?,
    val coverUri: String?,
    /**
     * Состав альбома: номера треков Яндекса по порядку (диски подряд). Если не пуст, страница
     * альбома показывает эти треки вместо состава Яндекса ([FckCensor.withAlbumVolumes]).
     */
    val volumes: List<String>,
    /** `__fckCensor.replaceArtistsInAlbumVolumes`: исполнители треков альбома = исполнители альбома. */
    val replaceArtists: Boolean,
)

/** Хосты, с которых берутся обложки из списка (как `ALLOWED_HOSTS` на десктопе). */
private val COVER_HOSTS = setOf("avatars.yandex.net", "avatars.mds.yandex.net", "t2.genius.com")

/** Разобранный `list_v2.json`: подмены по номерам треков и альбомов и ссылки на звук. */
class CensorList(
    val tracks: Map<String, TrackPatch>,
    val albums: Map<String, AlbumPatch>,
    /** Прямые ссылки на звук без цензуры из `tracksStorages`: номер трека → https-адрес. */
    val urls: Map<String, String>,
    /**
     * Номера треков из хранилищ звука: у них на Яндексе цензурная версия. Вместе с треками из
     * [tracks] и пометкой `clean` решают, искать ли explicit-версию ([censored]).
     */
    val censoredIds: Set<String> = emptySet(),
) {
    /** Сколько треков список меняет: названием, исполнителями, обложкой или звуком. */
    val count: Int = (urls.keys + tracks.filterValues { it.changesDisplay }.keys).size

    /**
     * Нужна ли треку версия без цензуры (как `censored` на десктопе): explicit-треку не нужна;
     * остальным — если Яндекс сам пометил его `clean`, он есть в хранилищах звука или в
     * подменах списка.
     */
    fun censored(track: Track): Boolean =
        !track.explicit && (track.clean || track.id in censoredIds || track.id in tracks)

    companion object {
        val EMPTY = CensorList(emptyMap(), emptyMap(), emptyMap())

        /**
         * Разбор списка. Не JSON или нет `sources` — `IllegalArgumentException` (список повреждён);
         * непонятная запись внутри пропускается, остальное работает.
         */
        fun parse(text: String): CensorList {
            val root = Json.parseToJsonElement(text) as? JsonObject
                ?: throw IllegalArgumentException("список не объект")
            val sources = root["sources"] as? JsonArray
                ?: throw IllegalArgumentException("в списке нет sources")
            val tracks = HashMap<String, TrackPatch>()
            val albums = HashMap<String, AlbumPatch>()
            val urls = HashMap<String, String>()
            val censored = HashSet<String>()
            for (source in sources) {
                val src = source as? JsonObject ?: continue
                (src["tracks"] as? JsonObject)?.forEach { (key, value) ->
                    val id = idOf(key) ?: return@forEach
                    tracks[id] = trackPatch(value as? JsonObject ?: return@forEach)
                }
                (src["albums"] as? JsonObject)?.forEach { (key, value) ->
                    val id = idOf(key) ?: return@forEach
                    albums[id] = albumPatch(value as? JsonObject ?: return@forEach)
                }
                (src["tracksStorages"] as? JsonArray)?.forEach { storage ->
                    parseStorage(storage as? JsonObject ?: return@forEach, urls, censored)
                }
            }
            return CensorList(tracks, albums, urls, censored)
        }

        private fun trackPatch(obj: JsonObject) = TrackPatch(
            title = text(obj["title"]),
            artists = artists(obj["artists"]),
            coverUri = cover(obj["coverUri"]),
            audioSourceId = numericId((obj["__fckCensor"] as? JsonObject)?.get("audioSourceId")),
        )

        private fun albumPatch(obj: JsonObject): AlbumPatch {
            val volumes = (obj["volumes"] as? JsonArray).orEmpty()
                .flatMap { disc -> (disc as? JsonArray).orEmpty() }
                .mapNotNull { track -> numericId((track as? JsonObject)?.get("id") ?: track) }
            return AlbumPatch(
                title = text(obj["title"]),
                artists = artists(obj["artists"]),
                coverUri = cover(obj["coverUri"]),
                volumes = volumes,
                replaceArtists = (obj["__fckCensor"] as? JsonObject)
                    ?.get("replaceArtistsInAlbumVolumes")
                    ?.let { (it as? JsonPrimitive)?.content } == "true",
            )
        }

        /** Хранилище звука: `urlTemplate` с `%%`, номера треков в `trackIds` и `tracks`. */
        private fun parseStorage(storage: JsonObject, urls: MutableMap<String, String>, censored: MutableSet<String>) {
            val ids = (storage["trackIds"] as? JsonArray).orEmpty()
                .mapNotNull { item -> numericId((item as? JsonObject)?.get("id") ?: item) }
            val files = storage["tracks"] as? JsonObject
            censored += ids
            files?.keys?.mapNotNullTo(censored, ::idOf)
            val template = (storage["urlTemplate"] as? JsonPrimitive)?.content
                ?.let(::storageTemplate) ?: return
            // trackIds: файл — шаблон с номером самого трека.
            for (id in ids) urls[id] = template.replace("%%", id)
            // tracks: номер трека → номер файла, готовая ссылка или объект с url.
            files?.forEach { (key, value) ->
                val id = idOf(key) ?: return@forEach
                val url = fileUrl(value, template) ?: return@forEach
                urls[id] = url
            }
        }

        /** Шаблон ссылки хранилища: обязателен `%%`; без схемы подразумевается https. */
        private fun storageTemplate(raw: String): String? {
            if (raw.length > 1800 || !raw.contains("%%") || raw.any { it.isWhitespace() || it.isISOControl() }) return null
            val url = if (raw.contains("://")) raw else "https://$raw"
            return url.takeIf(::plausibleUrl)
        }

        /** Адрес файла из значения хранилища: номер или строка без схемы — в шаблон, ссылка — как есть. */
        private fun fileUrl(value: JsonElement, template: String): String? {
            val raw = when (value) {
                is JsonPrimitive -> value.takeIf { it !is JsonNull }?.content
                is JsonObject -> (value["url"] as? JsonPrimitive)?.takeIf { it !is JsonNull }?.content
                else -> null
            } ?: return null
            if (raw.length > 1800 || raw.any { it.isWhitespace() || it.isISOControl() }) return null
            val url = if (raw.contains("://")) raw else template.replace("%%", raw)
            return url.takeIf(::plausibleUrl)
        }

        private fun plausibleUrl(url: String): Boolean =
            url.startsWith("https://") && url.length <= 2000 &&
                !url.any { it.isWhitespace() || it.isISOControl() }

        /** Ключ-номер из списка: только цифры, не все нули. */
        private fun idOf(key: String): String? =
            key.takeIf { plausibleNumericId(it) && it.any { c -> c != '0' } }

        private fun numericId(element: JsonElement?): String? =
            (element as? JsonPrimitive)?.takeIf { it !is JsonNull }?.content?.let(::idOf)

        /** Текст для показа: без управляющих знаков и знаков смены направления, не пустой. */
        private fun clean(text: String): String? {
            val cleaned = text.map { ch ->
                if (ch.isISOControl() || ch in '\u2028'..'\u2029' || ch in '\u202a'..'\u202e' || ch in '\u2066'..'\u2069') ' ' else ch
            }.joinToString("").trim()
            return cleaned.ifEmpty { null }
        }

        private fun text(element: JsonElement?): String? =
            (element as? JsonPrimitive)?.takeIf { it.isString }?.content?.let(::clean)

        /** Шаблон обложки: без схемы, с `%%` и с разрешённого хоста; иначе остаётся обложка Яндекса. */
        private fun cover(element: JsonElement?): String? {
            val uri = text(element) ?: return null
            if (uri.contains("://") || !uri.contains("%%")) return null
            return uri.takeIf { it.substringBefore('/') in COVER_HOSTS }
        }

        private fun artists(element: JsonElement?): List<ArtistRef>? {
            val list = (element as? JsonArray)?.mapNotNull { item ->
                val obj = item as? JsonObject ?: return@mapNotNull null
                val name = text(obj["name"]) ?: return@mapNotNull null
                ArtistRef(numericId(obj["id"]), name)
            }.orEmpty()
            return list.ifEmpty { null }
        }
    }
}
