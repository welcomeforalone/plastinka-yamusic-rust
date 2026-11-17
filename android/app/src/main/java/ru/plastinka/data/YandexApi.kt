package ru.plastinka.data

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.FormBody
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.OkHttpClient
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Request
import java.security.MessageDigest
import java.util.concurrent.TimeUnit
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * Клиент неофициального API Яндекс Музыки. Портировано из десктопного клиента
 * (Rust): те же эндпоинты, заголовки и подписи ссылок на поток.
 */
class YandexApi(private val http: OkHttpClient) {

    val json = Json { ignoreUnknownKeys = true }

    // -------------------------------------------------------------- вход

    /** Шаг 1 входа: запросить код устройства. */
    suspend fun requestDeviceCode(deviceId: String): DeviceCode = withContext(Dispatchers.IO) {
        val body = FormBody.Builder()
            .add("client_id", CLIENT_ID)
            .add("device_id", deviceId)
            .add("device_name", "Plastinka Android")
            .build()
        val response = http.newCall(
            Request.Builder().url("$OAUTH_BASE/device/code").post(body).build()
        ).execute()
        response.use { resp ->
            val text = resp.body?.string().orEmpty()
            check(resp.isSuccessful) { "oauth ответил ${resp.code}" }
            val obj = json.parseToJsonElement(text).jsonObject
            val url = obj["verification_url"]!!.jsonPrimitive.content
            check(url.startsWith("https://") && ALLOWED_HOSTS.any { url.removePrefix("https://").startsWith("$it/") })
            DeviceCode(
                deviceCode = obj["device_code"]!!.jsonPrimitive.content,
                userCode = obj["user_code"]!!.jsonPrimitive.content,
                verificationUrl = url,
                intervalSec = (obj["interval"]?.jsonPrimitive?.content?.toLongOrNull()) ?: 5,
                expiresInSeconds = (obj["expires_in"]?.jsonPrimitive?.content?.toLongOrNull()) ?: 300,
            )
        }
    }

    /** Шаг 2 входа: опрос сервера. `null` — ещё не подтверждено. */
    suspend fun pollToken(deviceCode: DeviceCode): String? = withContext(Dispatchers.IO) {
        val body = FormBody.Builder()
            .add("grant_type", "device_code")
            .add("code", deviceCode.deviceCode)
            .add("client_id", CLIENT_ID)
            .add("client_secret", CLIENT_SECRET)
            .build()
        val response = http.newCall(
            Request.Builder().url("$OAUTH_BASE/token").post(body).build()
        ).execute()
        response.use { resp ->
            val text = resp.body?.string().orEmpty()
            val obj = json.parseToJsonElement(text).jsonObject
            when (obj["error"]?.jsonPrimitive?.content) {
                null, "slow_down" -> obj["access_token"]?.jsonPrimitive?.content
                "authorization_pending" -> null
                else -> throw IllegalStateException("вход отклонён: ${obj["error"]}")
            }
        }
    }

    // ------------------------------------------------------------- статус

    suspend fun accountStatus(token: String): AccountInfo = withContext(Dispatchers.IO) {
        // Ответ завёрнут в `result`: result.account.uid, result.plus/subscription.
        val result = get(token, "$API_BASE/account/status")["result"]?.jsonObject
            ?: throw IllegalStateException("нет result у статуса")
        val account = result["account"]?.jsonObject
            ?: throw IllegalStateException("нет account у статуса")
        val uid = account["uid"]?.jsonPrimitive?.content?.toLongOrNull()
            ?: throw UnauthorizedException()
        val hasPlusFlag = { node: kotlinx.serialization.json.JsonElement? ->
            (node as? kotlinx.serialization.json.JsonObject)
                ?.get("hasPlus")?.jsonPrimitive?.content == "true"
        }
        AccountInfo(
            uid = uid,
            displayName = account["displayName"]?.jsonPrimitive?.content
                ?: account["login"]?.jsonPrimitive?.content ?: "",
            hasPlus = hasPlusFlag(result["plus"]) || hasPlusFlag(result["subscription"]),
        )
    }

    // -------------------------------------------------------------- лайки

    /** ID треков из «Мне нравится». */
    suspend fun likedIds(token: String, uid: Long): List<String> = withContext(Dispatchers.IO) {
        val obj = get(token, "$API_BASE/users/$uid/likes/tracks")
        obj["result"]?.jsonObject?.get("library")?.jsonObject?.get("tracks")?.jsonArray
            ?.mapNotNull { it.jsonObject["id"]?.jsonPrimitive?.content }
            .orEmpty()
    }

    /** Поставить/снять «Нравится». */
    suspend fun setLike(token: String, uid: Long, trackId: String, like: Boolean) {
        withContext(Dispatchers.IO) {
            val action = if (like) "add" else "remove"
            val body = FormBody.Builder().add("track-ids", trackId).build()
            http.newCall(
                Request.Builder()
                    .url("$API_BASE/users/$uid/likes/tracks/$action")
                    .header("Authorization", "OAuth $token")
                    .header("X-Yandex-Music-Client", MUSIC_CLIENT)
                    .post(body).build()
            ).execute().use { /* результат не критичен: список обновится перечитыванием */ }
        }
    }

    /**
     * «Не нравится» треку: запись в аккаунт. Зовёт только Slopless и только
     * после отдельного согласия пользователя. Токен — только в заголовке;
     * ошибка — исключение с кодом ответа (без токена и адреса).
     */
    suspend fun addDisliked(token: String, uid: Long, trackId: String) {
        withContext(Dispatchers.IO) {
            if (!isNumericId(trackId)) return@withContext
            val body = FormBody.Builder().add("track-ids", trackId).build()
            http.newCall(
                Request.Builder()
                    .url("$API_BASE/users/$uid/dislikes/tracks/add-multiple")
                    .header("Authorization", "OAuth $token")
                    .header("X-Yandex-Music-Client", MUSIC_CLIENT)
                    .post(body).build(),
            ).execute().use { resp ->
                if (!resp.isSuccessful) throw IllegalStateException("не нравится: ответ ${resp.code}")
            }
        }
    }

    // -------------------------------------------------------------- треки

    /** Метаданные треков пачками (сервис отвечает 400 на плохой ID — режем пополам). */
    suspend fun tracks(token: String, ids: List<String>): List<Track> {
        if (ids.isEmpty()) return emptyList()
        return withContext(Dispatchers.IO) {
            val out = mutableListOf<Track>()
            var batch = ids
            var failed = 0
            while (batch.isNotEmpty()) {
                val good = try {
                    val body = FormBody.Builder()
                        .add("track-ids", batch.joinToString(","))
                        .add("with-positions", "false")
                        .build()
                    val response = http.newCall(
                        Request.Builder()
                            .url("$API_BASE/tracks")
                            .header("Authorization", "OAuth $token")
                            .header("X-Yandex-Music-Client", MUSIC_CLIENT)
                            .post(body).build()
                    ).execute()
                    response.use { resp ->
                        val text = resp.body?.string().orEmpty()
                        if (!resp.isSuccessful) throw IllegalArgumentException("batch rejected")
                        val arr = json.parseToJsonElement(text).jsonObject["result"]!!.jsonArray
                        out += arr.mapNotNull(::parseTrack)
                        true
                    }
                } catch (_: IllegalArgumentException) {
                    false
                }
                if (good) {
                    failed = 0
                    break
                }
                failed++
                if (batch.size == 1 || failed > 4) break
                batch = batch.drop(batch.size / 2)
            }
            out
        }
    }

    /**
     * Поиск `GET /search`: вкладка «Всё» (`type=all`) или один тип со страницами.
     * Страница не больше 99 (дальше сервер отвечает 400), пустой запрос не шлём.
     */
    suspend fun search(token: String, kind: SearchKind, query: String, page: Int = 0): SearchResult =
        withContext(Dispatchers.IO) {
            if (query.isBlank() || page !in 0..SEARCH_MAX_PAGE) return@withContext SearchResult()
            val url = "$API_BASE/search?type=${kind.apiType}&page=$page&nocorrect=false" +
                "&text=${urlEncode(query)}"
            parseSearch(get(token, url), kind, page)
        }

    /**
     * Разбор ответа поиска. Нет `result` или он строка (`"text-is-empty"`) —
     * пустой результат; битый элемент списка и неизвестный тип лучшего
     * результата пропускаются, остальное остаётся.
     */
    internal fun parseSearch(obj: JsonObject, kind: SearchKind, page: Int): SearchResult {
        val result = obj["result"] as? JsonObject ?: return SearchResult()
        fun block(name: String): JsonObject? = result[name] as? JsonObject
        fun items(name: String): JsonArray = (block(name)?.get("results") as? JsonArray) ?: JsonArray(emptyList())
        val tracks = items("tracks")
        val albums = items("albums")
        val artists = items("artists")
        val playlists = items("playlists")
        val best = (result["best"] as? JsonObject)?.let { b ->
            val r = b["result"] ?: return@let null
            when (b.str("type")) {
                "track" -> parseTrackSafe(r)?.let { SearchBest.TrackHit(it) }
                "album" -> parseAlbumCard(r)?.let { SearchBest.AlbumHit(it) }
                "artist" -> parseArtistCard(r)?.let { SearchBest.ArtistHit(it) }
                "playlist" -> parsePlaylistCard(r, liked = false)?.let { SearchBest.PlaylistHit(it) }
                else -> null
            }
        }
        // Блок выбранного типа: общее число и размер страницы для подгрузки.
        val own = block(kind.apiType + "s")
        val total = own?.str("total")?.toIntOrNull() ?: 0
        val perPage = own?.str("perPage")?.toIntOrNull() ?: 0
        val rawCount = when (kind) {
            SearchKind.Track -> tracks.size
            SearchKind.Album -> albums.size
            SearchKind.Artist -> artists.size
            SearchKind.Playlist -> playlists.size
            SearchKind.All -> 0
        }
        return SearchResult(
            best = best,
            tracks = tracks.mapNotNull { parseTrackSafe(it) },
            albums = albums.mapNotNull(::parseAlbumCard),
            artists = artists.mapNotNull(::parseArtistCard),
            playlists = playlists.mapNotNull { parsePlaylistCard(it, liked = false) },
            hasNext = kind != SearchKind.All && rawCount > 0 && perPage > 0 &&
                (page + 1) * perPage < total && page < SEARCH_MAX_PAGE,
        )
    }

    // -------------------------------------------------------------- поток

    /**
     * Ссылка на звук трека: `get-file-info` (AAC/FLAC, HMAC-подпись), при
     * неудаче — запасной путь `download-info` (MP3, MD5-подпись). Как в
     * десктопном клиенте.
     */
    suspend fun streamUrl(
        token: String,
        trackId: String,
        quality: AudioQuality = AudioQuality.Normal,
    ): StreamResult =
        withContext(Dispatchers.IO) {
            val primary = fileInfoUrl(token, trackId, quality)
            if (primary is StreamResult.Url) return@withContext primary
            fallbackMp3Url(token, trackId).takeIf { it is StreamResult.Url || it == StreamResult.Unavailable }
                ?: primary
        }

    private fun fileInfoUrl(token: String, trackId: String, audio: AudioQuality): StreamResult {
        val ts = (System.currentTimeMillis() / 1000).toString()
        // Один и тот же `quality` и `codecs` идут и в подпись, и в запрос.
        val quality = audio.level
        val codecs = audio.codecs
        val message = "$ts$trackId$quality${codecs.replace(",", "")}$TRANSPORT"
        val mac = Mac.getInstance("HmacSHA256").apply {
            init(SecretKeySpec(SIGN_KEY.toByteArray(), "HmacSHA256"))
        }
        val sign = java.util.Base64.getEncoder()
            .encodeToString(mac.doFinal(message.toByteArray()))
            .trimEnd('=')
        return try {
            val obj = get(
                token,
                "$API_BASE/get-file-info?ts=$ts&trackId=$trackId&quality=$quality" +
                    "&codecs=${urlEncode(codecs)}&transports=$TRANSPORT&sign=$sign",
            )
            val info = obj["result"]?.jsonObject?.get("downloadInfo")?.jsonObject
                ?: return StreamResult.Error("нет downloadInfo")
            if (info["key"]?.jsonPrimitive?.content != null) return StreamResult.Error("поток зашифрован")
            val url = info["url"]?.jsonPrimitive?.content ?: return StreamResult.Error("нет ссылки")
            if (!allowedUrl(url)) return StreamResult.Error("чужой хост")
            StreamResult.Url(
                url = url,
                kbps = info["bitrate"]?.jsonPrimitive?.content?.toIntOrNull() ?: 0,
                codec = info["codec"]?.jsonPrimitive?.content ?: "",
            )
        } catch (e: UnauthorizedException) {
            StreamResult.Unauthorized
        } catch (e: Exception) {
            StreamResult.Error(e.message ?: "get-file-info не удался")
        }
    }

    private fun fallbackMp3Url(token: String, trackId: String): StreamResult {
        return try {
            val obj = get(token, "$API_BASE/tracks/$trackId/download-info")
            val arr = obj["result"]?.jsonArray ?: return StreamResult.Unavailable
            val best = arr.mapNotNull { item ->
                val v = item.jsonObject
                val codec = v["codec"]?.jsonPrimitive?.content ?: return@mapNotNull null
                val preview = v["preview"]?.jsonPrimitive?.content == "true"
                val url = v["downloadInfoUrl"]?.jsonPrimitive?.content ?: return@mapNotNull null
                val kbps = v["bitrateInKbps"]?.jsonPrimitive?.content?.toIntOrNull() ?: 0
                if (codec == "mp3" && !preview && allowedUrl(url)) kbps to url else null
            }.maxByOrNull { it.first } ?: return StreamResult.Unavailable

            // XML с полями подписи
            val xml = http.newCall(Request.Builder().url(best.second).build())
                .execute().use { resp -> if (resp.isSuccessful) resp.body?.string() else null }
                ?: return StreamResult.Error("download-info XML не получен")
            val host = xmlTag(xml, "host") ?: return StreamResult.Error("нет host")
            val path = xmlTag(xml, "path") ?: return StreamResult.Error("нет path")
            val ts = xmlTag(xml, "ts") ?: return StreamResult.Error("нет ts")
            val s = xmlTag(xml, "s") ?: return StreamResult.Error("нет s")
            val md5 = MessageDigest.getInstance("MD5").digest(
                (SIGN_SALT + path.removePrefix("/") + s).toByteArray()
            ).joinToString("") { "%02x".format(it) }
            val url = "https://$host/get-mp3/$md5/$ts$path"
            if (!allowedUrl(url)) return StreamResult.Error("чужой хост")
            StreamResult.Url(url, best.first, "mp3")
        } catch (e: UnauthorizedException) {
            StreamResult.Unauthorized
        } catch (e: Exception) {
            StreamResult.Error(e.message ?: "download-info не удался")
        }
    }

    /** Блокирующая версия для ResolvingDataSource (открывается в потоке загрузчика). */
    fun streamUrlBlocking(token: String, trackId: String): StreamResult =
        kotlinx.coroutines.runBlocking { streamUrl(token, trackId, Prefs.quality) }

    // -------------------------------------------------------------- волна

    /** Порция треков «Моей волны». */
    data class WaveBatch(
        val session: String,
        val batchId: String,
        val from: String?,
        val tracks: List<Track>,
    )

    /**
     * Запустить «Мою волну» и получить первую порцию. `seeds` — из выбранных
     * настроек волны ([WaveSettings.seeds]); без них обычная «Моя волна».
     */
    suspend fun startWave(token: String, seeds: List<String> = listOf(WAVE_SEED)): WaveBatch = withContext(Dispatchers.IO) {
        val body = kotlinx.serialization.json.buildJsonObject {
            put("seeds", kotlinx.serialization.json.buildJsonArray { seeds.forEach { add(kotlinx.serialization.json.JsonPrimitive(it)) } })
            put("queue", kotlinx.serialization.json.buildJsonArray {})
            put("includeTracksInResponse", kotlinx.serialization.json.JsonPrimitive(true))
            put("includeWaveModel", kotlinx.serialization.json.JsonPrimitive(true))
            put("interactive", kotlinx.serialization.json.JsonPrimitive(true))
        }
        val text = post(token, "$API_BASE/rotor/session/new", body)
        parseBatch(text, null) ?: throw IllegalStateException("волна не началась")
    }

    /** Занятия для настроек волны: станции типа `activity` из `GET /rotor/stations/list`. */
    suspend fun waveActivities(token: String): List<WaveActivity> = withContext(Dispatchers.IO) {
        val obj = get(token, "$API_BASE/rotor/stations/list?language=ru")
        WaveSettings.parseActivities(obj) ?: throw IllegalStateException("неожиданный ответ")
    }

    /** Следующая порция. `null` — сессия протухла: нужна новая. */
    suspend fun waveTracks(token: String, session: String, queueIds: List<String>): WaveBatch? =
        withContext(Dispatchers.IO) {
            val recent = queueIds.takeLast(WAVE_QUEUE_LEN)
            val body = kotlinx.serialization.json.buildJsonObject {
                put(
                    "queue",
                    kotlinx.serialization.json.buildJsonArray {
                        recent.forEach { add(kotlinx.serialization.json.JsonPrimitive(it)) }
                    },
                )
                put("feedbacks", kotlinx.serialization.json.buildJsonArray {})
            }
            val text = post(token, "$API_BASE/rotor/session/$session/tracks", body)
            parseBatch(text, session)
        }

    /** Событие волны (radioStarted / trackStarted / trackFinished / skip). Ошибки не критичны. */
    suspend fun waveFeedback(
        token: String,
        session: String,
        batchId: String,
        from: String?,
        type: String,
        trackId: String? = null,
        playedSeconds: Double? = null,
    ) = withContext(Dispatchers.IO) {
        try {
            val stamp = java.text.SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss.SSS'Z'", java.util.Locale.US)
                .apply { timeZone = java.util.TimeZone.getTimeZone("UTC") }
                .format(java.util.Date())
            val body = kotlinx.serialization.json.buildJsonObject {
                put(
                    "event",
                    kotlinx.serialization.json.buildJsonObject {
                        put("type", kotlinx.serialization.json.JsonPrimitive(type))
                        put("timestamp", kotlinx.serialization.json.JsonPrimitive(stamp))
                        from?.let { put("from", kotlinx.serialization.json.JsonPrimitive(it)) }
                        trackId?.let { put("trackId", kotlinx.serialization.json.JsonPrimitive(it)) }
                        playedSeconds?.let { put("totalPlayedSeconds", kotlinx.serialization.json.JsonPrimitive(it)) }
                    },
                )
                put("batchId", kotlinx.serialization.json.JsonPrimitive(batchId))
            }
            post(token, "$API_BASE/rotor/session/$session/feedback", body)
        } catch (_: Exception) {
            // Как на десктопе: воспроизведение от обратной связи не зависит.
        }
    }

    /** Идентификатор трека для событий и очереди волны: `id:albumId`. */
    fun waveTrackId(track: Track): String =
        if (track.albumId != null) "${track.id}:${track.albumId}" else track.id

    // ------------------------------------------------- полки коллекции

    /** «Встречали в Моей волне»: `GET /landing-blocks/artists` (ответ без обёртки `result`). */
    suspend fun metArtists(token: String): List<ArtistCard> = withContext(Dispatchers.IO) {
        parseMetArtists(get(token, "$API_BASE/landing-blocks/artists?count=$MET_ARTISTS"))
    }

    /** Новые релизы для пользователя: `GET /landing-blocks/new-releases`. */
    suspend fun newReleases(token: String): List<AlbumCard> = withContext(Dispatchers.IO) {
        parseNewReleases(get(token, "$API_BASE/landing-blocks/new-releases?count=$NEW_RELEASES"))
    }

    /** «Собираем для вас» с описаниями: `GET /landing-blocks/personal-playlists`. */
    suspend fun personalShelf(token: String): List<PlaylistCard> = withContext(Dispatchers.IO) {
        parsePersonalShelf(get(token, "$API_BASE/landing-blocks/personal-playlists?count=10"))
    }

    /** Разбор `{"items":[{"data":{"artist":…}}]}`; повтор исполнителя пропускается (ключи списка). */
    internal fun parseMetArtists(root: JsonObject): List<ArtistCard> {
        val body = root["result"] as? JsonObject ?: root
        return (body["items"] as? JsonArray).orEmpty().mapNotNull { item ->
            val data = (item as? JsonObject)?.get("data") as? JsonObject ?: return@mapNotNull null
            data["artist"]?.let(::parseArtistCard)
        }.distinctBy { it.id }
    }

    /**
     * Разбор `{"newReleases":[{"album":{…"albumType","cover":{"uri"}},"artists":[…],"releaseDate":…}]}`:
     * у альбома `albumType` вместо `type`, обложка в `cover.uri`, исполнители рядом с ним.
     */
    internal fun parseNewReleases(root: JsonObject): List<AlbumCard> {
        val body = root["result"] as? JsonObject ?: root
        return (body["newReleases"] as? JsonArray).orEmpty().mapNotNull { item ->
            val obj = item as? JsonObject ?: return@mapNotNull null
            val album = obj["album"] as? JsonObject ?: return@mapNotNull null
            val id = album.str("id")?.takeIf(::isNumericId) ?: return@mapNotNull null
            val title = album.str("title")?.takeIf { it.isNotBlank() } ?: return@mapNotNull null
            val artists = ((obj["artists"] as? JsonArray) ?: (album["artists"] as? JsonArray)).orEmpty()
                .mapNotNull { r ->
                    val o = r as? JsonObject ?: return@mapNotNull null
                    ArtistRef(o.str("id"), o.str("name") ?: return@mapNotNull null)
                }
            AlbumCard(
                id = id,
                title = title,
                version = album.str("version")?.takeIf { it.isNotBlank() },
                year = obj.str("releaseDate")?.take(4)?.toIntOrNull(),
                kind = when (album.str("albumType") ?: album.str("type")) {
                    "single" -> AlbumKind.Single
                    "compilation" -> AlbumKind.Compilation
                    else -> AlbumKind.Album
                },
                artists = artists,
                coverUri = (album.str("cover", "uri") ?: album.str("coverUri"))?.takeIf { it.contains("%%") },
                trackCount = 0,
            )
        }.distinctBy { it.id }
    }

    /** Разбор `{"items":[{"data":{"playlist":…,"description":…}}]}`: карточка плейлиста и его описание. */
    internal fun parsePersonalShelf(root: JsonObject): List<PlaylistCard> {
        val body = root["result"] as? JsonObject ?: root
        return (body["items"] as? JsonArray).orEmpty().mapNotNull { item ->
            val data = (item as? JsonObject)?.get("data") as? JsonObject ?: return@mapNotNull null
            val card = data["playlist"]?.let { parsePlaylistCard(it, liked = false) } ?: return@mapNotNull null
            card.copy(note = data.str("description")?.takeIf { it.isNotBlank() })
        }.distinctBy { it.uid to it.kind }
    }

    // ------------------------------------------------------------ топ месяца

    /** Исполнители топа месяца: `GET /personal/top/artists/month` (ответ без обёртки `result`). */
    suspend fun topArtists(token: String): List<TopArtist> = withContext(Dispatchers.IO) {
        parseTopArtists(get(token, "$API_BASE/personal/top/artists/month"))
    }

    /**
     * Страница «Топ месяца»: исполнители и треки. Ошибка любого из двух запросов —
     * ошибка страницы. Треки догружаются по идентификаторам в порядке топа; не
     * догрузившийся пропускается вместе со своим числом прослушиваний.
     */
    suspend fun monthTop(token: String): MonthTop = withContext(Dispatchers.IO) {
        val artists = parseTopArtists(get(token, "$API_BASE/personal/top/artists/month"))
        val entries = parseTopTrackEntries(get(token, "$API_BASE/personal/top/tracks/month"))
        val loaded = tracks(token, entries.map { it.first }).associateBy { it.id }
        val tracks = mutableListOf<Track>()
        val plays = mutableListOf<Int>()
        for ((id, count) in entries) {
            val track = loaded[id] ?: continue
            tracks += track
            plays += count
        }
        MonthTop(artists, tracks, plays)
    }

    /**
     * Разбор `{"artists":[{"artist":…,"listenTimeSeconds":…,"top":{"position":…,"progress":…}}]}`.
     * Элемент без годной карточки исполнителя пропускается, остальное терпимо.
     */
    internal fun parseTopArtists(root: JsonObject): List<TopArtist> {
        val body = root["result"] as? JsonObject ?: root
        return (body["artists"] as? JsonArray).orEmpty().mapNotNull { item ->
            val obj = item as? JsonObject ?: return@mapNotNull null
            val card = parseArtistCard(obj["artist"] ?: return@mapNotNull null) ?: return@mapNotNull null
            val top = obj["top"] as? JsonObject
            TopArtist(
                card = card,
                position = (top?.get("position") as? JsonPrimitive)?.content?.toIntOrNull(),
                trend = when ((top?.get("progress") as? JsonPrimitive)?.content) {
                    "up" -> Trend.Up
                    "down" -> Trend.Down
                    "same" -> Trend.Same
                    "new" -> Trend.New
                    else -> null
                },
                listenedSeconds = ((obj["listenTimeSeconds"] as? JsonPrimitive)?.content?.toLongOrNull() ?: 0L)
                    .coerceAtLeast(0L),
            )
            // Повтор исполнителя сломал бы ключи списка: оставляем первое вхождение.
        }.distinctBy { it.card.id }
    }

    /**
     * Разбор `{"tracks":[{"track":{"track":{"id":…}},"playCount":8}]}`: пары
     * «идентификатор, число прослушиваний». Элемент без годного числового
     * идентификатора пропускается, нет `playCount` — 0.
     */
    internal fun parseTopTrackEntries(root: JsonObject): List<Pair<String, Int>> {
        val body = root["result"] as? JsonObject ?: root
        return (body["tracks"] as? JsonArray).orEmpty().mapNotNull { item ->
            val obj = item as? JsonObject ?: return@mapNotNull null
            val inner = (obj["track"] as? JsonObject)?.get("track") as? JsonObject
            val id = (inner?.get("id") as? JsonPrimitive)?.content?.takeIf(::isNumericId)
                ?: return@mapNotNull null
            val count = (obj["playCount"] as? JsonPrimitive)?.content?.toIntOrNull()?.coerceAtLeast(0) ?: 0
            id to count
        }
    }

    /** Треки подборки: `GET /users/{uid}/playlists/{kind}`. */
    suspend fun playlistTracks(token: String, ownerUid: Long, kind: Long): List<Track> =
        withContext(Dispatchers.IO) {
            val obj = get(token, "$API_BASE/users/$ownerUid/playlists/$kind")
            // Недоступные треки остаются в списке (серые), в очередь их отсеивает плеер.
            ((obj["result"] as? JsonObject)?.get("tracks") as? JsonArray).orEmpty()
                .mapNotNull { item -> (item as? JsonObject)?.get("track")?.let { parseTrackSafe(it) } }
        }

    // ------------------------------------------------------------ коллекция

    /**
     * Свои плейлисты (без «Мне нравится», `kind` 3) и понравившиеся чужие.
     * Один из двух запросов не удался — берём второй; оба — ошибка.
     */
    suspend fun userPlaylists(token: String, uid: Long): List<PlaylistCard> =
        withContext(Dispatchers.IO) {
            val own = runCatching {
                val arr = get(token, "$API_BASE/users/$uid/playlists/list")["result"]?.jsonArray
                    ?: throw IllegalStateException("нет result у плейлистов")
                arr.mapNotNull { parsePlaylistCard(it, liked = false) }
                    .filter { it.kind != LIKED_PLAYLIST_KIND }
            }
            val liked = runCatching {
                val arr = get(token, "$API_BASE/users/$uid/likes/playlists")["result"]?.jsonArray
                    ?: throw IllegalStateException("нет result у понравившихся плейлистов")
                // Недавно отмеченные первыми, как на десктопе.
                recentFirst(arr, "playlist").mapNotNull { parsePlaylistCard(it, liked = true) }
            }
            // Ошибка авторизации важнее частичного результата.
            (own.exceptionOrNull() as? UnauthorizedException)?.let { throw it }
            (liked.exceptionOrNull() as? UnauthorizedException)?.let { throw it }
            if (own.isFailure && liked.isFailure) {
                throw own.exceptionOrNull() ?: IllegalStateException("плейлисты не получены")
            }
            val mine = own.getOrDefault(emptyList())
            mine + liked.getOrDefault(emptyList()).filter { l ->
                mine.none { it.uid == l.uid && it.kind == l.kind }
            }
        }

    /** Понравившиеся альбомы, недавние первыми. */
    suspend fun likedAlbums(token: String, uid: Long): List<AlbumCard> = withContext(Dispatchers.IO) {
        val arr = get(token, "$API_BASE/users/$uid/likes/albums?rich=true")["result"] as? JsonArray
            ?: throw IllegalStateException("нет result у альбомов")
        recentFirst(arr, "album").mapNotNull(::parseAlbumCard)
    }

    /** Понравившиеся исполнители, недавние первыми. */
    suspend fun likedArtists(token: String, uid: Long): List<ArtistCard> = withContext(Dispatchers.IO) {
        val arr = get(token, "$API_BASE/users/$uid/likes/artists?with-timestamps=true")["result"] as? JsonArray
            ?: throw IllegalStateException("нет result у исполнителей")
        recentFirst(arr, "artist").mapNotNull(::parseArtistCard)
    }

    /** Альбом с треками: диски склеиваются по порядку. */
    suspend fun albumWithTracks(token: String, id: String): AlbumDetails = withContext(Dispatchers.IO) {
        if (!isNumericId(id)) throw NotFoundException()
        val result = get(token, "$API_BASE/albums/$id/with-tracks")["result"] as? JsonObject
            ?: throw IllegalStateException("нет result у альбома")
        val card = parseAlbumCard(result) ?: throw NotFoundException()
        val tracks = (result["volumes"] as? JsonArray).orEmpty().flatMap { volume ->
            (volume as? JsonArray).orEmpty().mapNotNull { parseTrackSafe(it) }
        }
        AlbumDetails(card, tracks)
    }

    /**
     * Исполнитель: `brief-info` (шапка, популярные треки, первые альбомы). Если
     * в нём ровно 9 альбомов, а всего больше, догружается `direct-albums`.
     */
    suspend fun artistBrief(token: String, id: String): ArtistDetails = withContext(Dispatchers.IO) {
        if (!isNumericId(id)) throw NotFoundException()
        val result = get(token, "$API_BASE/artists/$id/brief-info")["result"] as? JsonObject
            ?: throw IllegalStateException("нет result у исполнителя")
        val card = (result["artist"] as? JsonObject)?.let(::parseArtistCard)
            ?: throw NotFoundException()
        val popular = (result["popularTracks"] as? JsonArray).orEmpty().mapNotNull { parseTrackSafe(it) }
        var albums = (result["albums"] as? JsonArray).orEmpty().mapNotNull(::parseAlbumCard)
        if (albums.size == BRIEF_ALBUMS && card.albumsCount > albums.size) {
            try {
                directAlbums(token, id, card.albumsCount).takeIf { it.isNotEmpty() }?.let { albums = it }
            } catch (_: Exception) {
                // Дозагрузка не удалась: остаются первые альбомы из brief-info.
            }
        }
        ArtistDetails(card, popular, albums)
    }

    /**
     * Страница «Все треки» исполнителя: `GET /artists/{id}/tracks?page=N&page-size=100`,
     * `page` с нуля. Ответ `{"result":{"pager":{"total":N},"tracks":[Track]}}`; элемент
     * без годных данных пропускается, остальное терпимо.
     */
    suspend fun artistTracks(token: String, id: String, page: Int): ArtistTracksPage =
        withContext(Dispatchers.IO) {
            if (!isNumericId(id)) throw NotFoundException()
            val result = get(token, "$API_BASE/artists/$id/tracks?page=$page&page-size=$ARTIST_TRACKS_PAGE")["result"]
                as? JsonObject ?: throw IllegalStateException("нет result у треков исполнителя")
            parseArtistTracks(result)
        }

    internal fun parseArtistTracks(result: JsonObject): ArtistTracksPage {
        val raw = (result["tracks"] as? JsonArray).orEmpty()
        val tracks = raw.mapNotNull { parseTrackSafe(it) }
        val total = (result.str("pager", "total") ?: result.str("total"))?.toIntOrNull() ?: 0
        return ArtistTracksPage(tracks, maxOf(total, tracks.size), raw.size)
    }

    /** Все альбомы исполнителя страницами по 100, не больше 10 страниц. */
    private fun directAlbums(token: String, id: String, expected: Int): List<AlbumCard> {
        val out = mutableListOf<AlbumCard>()
        for (page in 0 until MAX_ALBUM_PAGES) {
            val obj = get(token, "$API_BASE/artists/$id/direct-albums?page=$page&page-size=100&sort-by=year")
            val items = ((obj["result"] as? JsonObject)?.get("albums") as? JsonArray)
                .orEmpty().mapNotNull(::parseAlbumCard)
            out += items
            if (items.isEmpty() || out.size >= expected) break
        }
        return out
    }

    /** Элементы списка коллекции: распаковка обёртки и сортировка по `timestamp` (новые первыми). */
    private fun recentFirst(arr: JsonArray, wrapper: String): List<JsonObject> =
        arr.mapNotNull { it as? JsonObject }
            .sortedByDescending { it.str("timestamp") ?: "" }
            .map { (it[wrapper] as? JsonObject) ?: it }

    internal fun parseTrackSafe(element: kotlinx.serialization.json.JsonElement): Track? =
        try {
            parseTrack(element)
        } catch (_: Exception) {
            null
        }

    private fun hasError(obj: JsonObject): Boolean =
        obj["error"].let { it != null && it !is kotlinx.serialization.json.JsonNull }

    internal fun parseAlbumCard(element: kotlinx.serialization.json.JsonElement): AlbumCard? {
        val a = element as? JsonObject ?: return null
        if (hasError(a)) return null
        val id = a.str("id")?.takeIf(::isNumericId) ?: return null
        val title = a.str("title")?.takeIf { it.isNotBlank() } ?: return null
        val artists = (a["artists"] as? JsonArray).orEmpty().mapNotNull { r ->
            val o = r as? JsonObject ?: return@mapNotNull null
            ArtistRef(o.str("id"), o.str("name") ?: return@mapNotNull null)
        }
        return AlbumCard(
            id = id,
            title = title,
            version = a.str("version")?.takeIf { it.isNotBlank() },
            year = a.str("year")?.toIntOrNull(),
            kind = when (a.str("type")) {
                "single" -> AlbumKind.Single
                "compilation" -> AlbumKind.Compilation
                else -> AlbumKind.Album
            },
            artists = artists,
            coverUri = (a.str("coverUri") ?: a.str("ogImage"))?.takeIf { it.contains("%%") },
            trackCount = a.str("trackCount")?.toIntOrNull() ?: 0,
        )
    }

    internal fun parseArtistCard(element: kotlinx.serialization.json.JsonElement): ArtistCard? {
        val a = element as? JsonObject ?: return null
        if (hasError(a)) return null
        val id = a.str("id")?.takeIf(::isNumericId) ?: return null
        val name = a.str("name")?.takeIf { it.isNotBlank() } ?: return null
        return ArtistCard(
            id = id,
            name = name,
            coverUri = (a.str("cover", "uri") ?: a.str("ogImage"))?.takeIf { it.contains("%%") },
            albumsCount = a.str("counts", "directAlbums")?.toIntOrNull() ?: 0,
        )
    }

    // ------------------------------------------------------------- история

    /**
     * Сообщение о прослушивании: `POST /play-audio` (форма). Поля — как у
     * десктопа. Токен только в заголовке. Ошибка — исключение с кодом ответа
     * (без токена и адреса); вызывающий её глотает.
     */
    suspend fun playAudio(
        token: String,
        uid: Long,
        trackId: String,
        albumId: String?,
        playId: String,
        lengthSeconds: Int?,
        playedSeconds: Int,
        startedAtMs: Long,
    ) {
        withContext(Dispatchers.IO) {
            if (!isNumericId(trackId)) return@withContext
            val form = FormBody.Builder()
                .add("track-id", trackId)
                .add("from-cache", "false")
                .add("from", PLAY_FROM)
                .add("play-id", playId)
                .add("uid", uid.toString())
                .add("timestamp", utcStamp(startedAtMs))
                .add("client-now", utcStamp(System.currentTimeMillis()))
                .add("total-played-seconds", playedSeconds.toString())
                .add("end-position-seconds", playedSeconds.toString())
            albumId?.takeIf(::isNumericId)?.let { form.add("album-id", it) }
            lengthSeconds?.let { form.add("track-length-seconds", it.toString()) }
            http.newCall(
                Request.Builder()
                    .url("$API_BASE/play-audio")
                    .header("Authorization", "OAuth $token")
                    .header("X-Yandex-Music-Client", MUSIC_CLIENT)
                    .post(form.build()).build(),
            ).execute().use { resp ->
                if (!resp.isSuccessful) throw IllegalStateException("play-audio: ответ ${resp.code}")
            }
        }
    }

    /** Трек из ответа истории: полная модель или только идентификатор. */
    private class RawSlot(val track: Track?, val id: String?)
    private class RawGroup(val context: HistoryContext, val slots: List<RawSlot>)
    private class RawDay(val date: java.time.LocalDate?, val groups: List<RawGroup>)

    /**
     * История с сервера: `GET /music-history`. Дни свежие первыми; полные модели
     * есть у первых треков, остальные догружаются пачкой. Не больше 30 дней и 300
     * треков. Формат пересказан по справке, вживую не проверен.
     */
    suspend fun history(token: String): List<HistoryDay> = withContext(Dispatchers.IO) {
        val result = get(token, "$API_BASE/music-history?fullModelsCount=$HISTORY_FULL_MODELS")["result"] as? JsonObject
            ?: throw IllegalStateException("нет result у истории")
        val tabs = result["historyTabs"] as? JsonArray
            ?: throw IllegalStateException("нет historyTabs у истории")
        var budget = HISTORY_MAX_TRACKS
        val rawDays = mutableListOf<RawDay>()
        for (tab in tabs.take(HISTORY_MAX_DAYS)) {
            if (budget <= 0) break
            val tabObj = tab as? JsonObject ?: continue
            val date = tabObj.str("date")?.take(10)?.let { runCatching { java.time.LocalDate.parse(it) }.getOrNull() }
            val groups = mutableListOf<RawGroup>()
            for (item in (tabObj["items"] as? JsonArray).orEmpty()) {
                val itemObj = item as? JsonObject ?: continue
                val context = parseHistoryContext(itemObj["context"]) ?: continue
                val slots = (itemObj["tracks"] as? JsonArray).orEmpty()
                    .mapNotNull(::parseHistorySlot).take(budget)
                budget -= slots.size
                if (slots.isNotEmpty()) groups += RawGroup(context, slots)
            }
            if (groups.isNotEmpty()) rawDays += RawDay(date, groups)
        }
        val missing = rawDays.flatMap { d -> d.groups.flatMap { g -> g.slots } }
            .mapNotNull { it.id }.distinct()
        val loaded = if (missing.isEmpty()) emptyMap() else tracks(token, missing).associateBy { it.id }
        // Недоступные и не найденные (без названия) отбрасываются, пустые группы и дни — тоже.
        rawDays.mapNotNull { day ->
            val groups = day.groups.mapNotNull { g ->
                val tracks = g.slots.mapNotNull { it.track ?: it.id?.let(loaded::get) }
                    .filter { it.title.isNotBlank() }
                tracks.takeIf { it.isNotEmpty() }?.let { HistoryGroup(g.context, it) }
            }
            groups.takeIf { it.isNotEmpty() }?.let { HistoryDay(day.date, it) }
        }
    }

    /** `{type, data:{itemId, fullModel}}` у трека. */
    private fun parseHistorySlot(element: kotlinx.serialization.json.JsonElement): RawSlot? {
        val data = (element as? JsonObject)?.get("data") as? JsonObject ?: return null
        (data["fullModel"] as? JsonObject)?.let { parseTrackSafe(it) }?.let { return RawSlot(it, null) }
        val id = (data["itemId"] as? JsonObject)?.str("trackId") ?: return null
        return RawSlot(null, id)
    }

    /** `{type, data:{itemId, fullModel}}` у контекста. */
    private fun parseHistoryContext(element: kotlinx.serialization.json.JsonElement?): HistoryContext? {
        val o = element as? JsonObject ?: return null
        val kind = o.str("type") ?: return null
        val data = o["data"] as? JsonObject
        val itemId = data?.get("itemId")
        val full = data?.get("fullModel") as? JsonObject
        // Идентификатор — в itemId (строкой или объектом {id}) или в самой модели.
        val single = ((itemId as? JsonObject)?.str("id")
            ?: (itemId as? JsonPrimitive)?.takeIf { it !is kotlinx.serialization.json.JsonNull }?.content
            ?: full?.str("id"))?.takeIf(::isNumericId)
        return when (kind) {
            "album" -> HistoryContext.Album(single ?: return null, full?.str("title") ?: "Альбом")
            "artist" -> HistoryContext.Artist(single ?: return null, full?.str("name") ?: "Исполнитель")
            "playlist" -> {
                val idObj = itemId as? JsonObject
                val uid = (idObj?.str("uid") ?: full?.str("owner", "uid"))?.takeIf(::isNumericId) ?: return null
                val plKind = (idObj?.str("kind") ?: full?.str("kind"))?.takeIf(::isNumericId) ?: return null
                HistoryContext.Playlist(uid, plKind, full?.str("title") ?: "Плейлист")
            }
            "wave" -> HistoryContext.Wave
            else -> HistoryContext.Other(full?.str("title") ?: full?.str("name") ?: "Подборка")
        }
    }

    /** Плейлист из списка: элемент либо сам плейлист, либо обёртка `{playlist}`. */
    internal fun parsePlaylistCard(element: kotlinx.serialization.json.JsonElement, liked: Boolean): PlaylistCard? {
        val outer = (element as? JsonObject) ?: return null
        val pl = (outer["playlist"] as? JsonObject) ?: outer
        val uid = (pl.str("uid") ?: pl.str("owner", "uid"))?.toLongOrNull() ?: return null
        val kind = pl.str("kind")?.toLongOrNull() ?: return null
        val title = pl.str("title") ?: return null
        val cover = pl["cover"] as? JsonObject
        val coverUri = cover?.str("uri")?.takeIf { it.contains("%%") }
            ?: (cover?.get("itemsUri") as? kotlinx.serialization.json.JsonArray)
                ?.firstNotNullOfOrNull { (it as? JsonPrimitive)?.content?.takeIf { s -> s.contains("%%") } }
            ?: pl.str("ogImage")?.takeIf { it.contains("%%") }
        return PlaylistCard(
            uid = uid,
            kind = kind,
            title = title,
            coverUri = coverUri,
            trackCount = pl.str("trackCount")?.toIntOrNull() ?: 0,
            liked = liked,
        )
    }

    private fun parseBatch(text: String, fallbackSession: String?): WaveBatch? {
        val result = json.parseToJsonElement(text).jsonObject["result"]?.jsonObject
            ?: throw IllegalStateException("нет result у волны")
        if (result["unknownSession"]?.jsonPrimitive?.content == "true") return null
        val session = result["radioSessionId"]?.jsonPrimitive?.content ?: fallbackSession
            ?: throw IllegalStateException("нет radioSessionId")
        val batchId = result["batchId"]?.jsonPrimitive?.content
            ?: throw IllegalStateException("нет batchId")
        val from = result["wave"]?.jsonObject?.get("idForFrom")?.jsonPrimitive?.content
        val tracks = result["sequence"]?.jsonArray
            ?.filter { it.jsonObject["type"]?.jsonPrimitive?.content != "nontrack" }
            ?.mapNotNull { element ->
                val item = element.jsonObject
                val track = item["track"]?.let(::parseTrack) ?: return@mapNotNull null
                if (!track.available) return@mapNotNull null
                // VibeColorizer: derivedColors и hue из ответа волны.
                val colors = item["track"]?.jsonObject?.get("derivedColors")?.jsonObject
                val vibe = ru.plastinka.data.VibeColors(
                    accent = colors?.get("accent")?.jsonPrimitive?.content?.takeIf { it.startsWith("#") },
                    average = colors?.get("average")?.jsonPrimitive?.content?.takeIf { it.startsWith("#") },
                    hue = item["trackParameters"]?.jsonObject?.get("hue")
                        ?.jsonPrimitive?.content?.toDoubleOrNull(),
                )
                track.copy(
                    vibe = vibe.takeIf { it.accent != null || it.average != null || it.hue != null },
                )
            }
            .orEmpty()
        return WaveBatch(session, batchId, from, tracks)
    }

    // ------------------------------------------------------------- служебное

    private fun post(
        token: String,
        url: String,
        body: kotlinx.serialization.json.JsonObject,
    ): String {
        val request = Request.Builder()
            .url(url)
            .header("Authorization", "OAuth $token")
            .header("X-Yandex-Music-Client", MUSIC_CLIENT)
            .post(
                body.toString()
                    .toRequestBody("application/json; charset=utf-8".toMediaType()),
            )
            .build()
        val response = http.newCall(request).execute()
        return response.use { resp ->
            when {
                resp.code == 401 || resp.code == 403 -> throw UnauthorizedException()
                !resp.isSuccessful -> throw IllegalStateException("сервер ответил ${resp.code}")
                else -> resp.body?.string().orEmpty()
            }
        }
    }

    private class UnauthorizedException : Exception()

    private fun get(token: String, url: String): JsonObject {
        val response = http.newCall(
            Request.Builder()
                .url(url)
                .header("Authorization", "OAuth $token")
                .header("X-Yandex-Music-Client", MUSIC_CLIENT)
                .get().build()
        ).execute()
        return response.use { resp ->
            val text = resp.body?.string().orEmpty()
            when {
                resp.code == 401 || resp.code == 403 -> throw UnauthorizedException()
                !resp.isSuccessful -> throw IllegalStateException("сервер ответил ${resp.code}")
                else -> json.parseToJsonElement(text).jsonObject
            }
        }
    }

    private fun parseTrack(element: kotlinx.serialization.json.JsonElement): Track? {
        val t = element.jsonObject
        val id = t["id"]?.jsonPrimitive?.content ?: return null
        val artists = t["artists"]?.jsonArray?.mapNotNull { a ->
            val obj = a.jsonObject
            ArtistRef(
                id = obj["id"]?.jsonPrimitive?.content,
                name = obj["name"]?.jsonPrimitive?.content ?: return@mapNotNull null,
            )
        }.orEmpty()
        val album = t["albums"]?.jsonArray?.firstOrNull()?.jsonObject
        return Track(
            id = id,
            title = t["title"]?.jsonPrimitive?.content ?: "",
            artists = artists,
            albumId = album?.get("id")?.jsonPrimitive?.content,
            albumTitle = album?.get("title")?.jsonPrimitive?.content,
            durationMs = t["durationMs"]?.jsonPrimitive?.content?.toLongOrNull(),
            available = t["available"]?.jsonPrimitive?.content != "false",
            coverUri = album?.get("coverUri")?.jsonPrimitive?.content
                ?: t["coverUri"]?.jsonPrimitive?.content,
            loudness = parseLoudness(t["r128"]),
            explicit = (t["contentWarning"] as? JsonPrimitive)?.content == "explicit",
        )
    }

    /** `r128`: `{"i": -9.4, "tp": 0.2}`; без обоих конечных чисел — `null`. */
    private fun parseLoudness(element: kotlinx.serialization.json.JsonElement?): Loudness? {
        val obj = element as? JsonObject ?: return null
        fun number(key: String): Double? =
            (obj[key] as? JsonPrimitive)?.content?.toDoubleOrNull()?.takeIf { it.isFinite() }
        return Loudness(number("i") ?: return null, number("tp") ?: return null)
    }

    companion object {
        const val API_BASE = "https://api.music.yandex.net"
        const val OAUTH_BASE = "https://oauth.yandex.ru"
        const val MUSIC_CLIENT = "YandexMusicAndroid/24023621"
        const val CLIENT_ID = "23cabbbdc6cd418abb4b39c32c41195d"
        const val CLIENT_SECRET = "53bc75238f0c4d08a118e51fe9203300"
        const val SIGN_KEY = "p93jhgh689SBReK6ghtw62"
        const val SIGN_SALT = "XGRlBW9FXlekgbPrRHuSiA"
        const val TRANSPORT = "raw"
        const val WAVE_SEED = "user:onyourwave"
        const val WAVE_QUEUE_LEN = 5

        /** `kind` плейлиста «Мне нравится»: у него свой раздел. */
        const val LIKED_PLAYLIST_KIND = 3L

        /** Сколько альбомов отдаёт `brief-info` и сколько страниц `direct-albums` читаем. */
        private const val BRIEF_ALBUMS = 9
        /** Треков на страницу «Все треки» (как на десктопе). */
        const val ARTIST_TRACKS_PAGE = 100
        /** Сколько просить у полок коллекции (как на десктопе). */
        private const val MET_ARTISTS = 16
        private const val NEW_RELEASES = 30
        private const val MAX_ALBUM_PAGES = 10

        /** Последняя страница поиска: на больших номерах сервер отвечает 400. */
        const val SEARCH_MAX_PAGE = 99

        /** Откуда играем (поле `from`): наш клиент, без личных данных. */
        private const val PLAY_FROM = "plastinka"

        private val STAMP = java.time.format.DateTimeFormatter
            .ofPattern("yyyy-MM-dd'T'HH:mm:ss.SSS'Z'")
            .withZone(java.time.ZoneOffset.UTC)

        /** RFC 3339 UTC с миллисекундами. */
        fun utcStamp(ms: Long): String = STAMP.format(java.time.Instant.ofEpochMilli(ms))

        /** Пределы истории: полные модели в ответе, дни и треки. */
        private const val HISTORY_FULL_MODELS = 50
        private const val HISTORY_MAX_DAYS = 30
        private const val HISTORY_MAX_TRACKS = 300

        /** Идентификатор для адреса запроса: только цифры. */
        fun isNumericId(id: String): Boolean = id.length in 1..20 && id.all { it in '0'..'9' }
        private val ALLOWED_HOSTS = listOf(
            "oauth.yandex.ru", "oauth.yandex.com", "ya.ru", "yandex.ru", "yandex.com",
        )

        /**
         * Только https и только хосты Яндекса. Хост берётся разбором адреса,
         * а не подстрокой: `https://чужой.хост?x=.yandex.net` не проходит.
         */
        fun allowedUrl(url: String): Boolean {
            val parsed = url.toHttpUrlOrNull() ?: return false
            if (!parsed.isHttps) return false
            val host = parsed.host.lowercase()
            return host.endsWith(".yandex.net") || host.endsWith(".yandex.ru")
        }

        fun urlEncode(text: String): String =
            java.net.URLEncoder.encode(text, Charsets.UTF_8.name())

        fun xmlTag(xml: String, tag: String): String? {
            val open = "<$tag>"
            val close = "</$tag>"
            val start = xml.indexOf(open)
            if (start < 0) return null
            val end = xml.indexOf(close, start)
            if (end < 0) return null
            return xml.substring(start + open.length, end).trim()
        }

        fun defaultHttp(): OkHttpClient = OkHttpClient.Builder()
            .connectTimeout(15, TimeUnit.SECONDS)
            .readTimeout(30, TimeUnit.SECONDS)
            .build()
    }
}
