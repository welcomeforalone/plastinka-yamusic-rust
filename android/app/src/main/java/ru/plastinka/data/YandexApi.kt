package ru.plastinka.data

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.FormBody
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

    /** Поиск по трекам. */
    suspend fun searchTracks(token: String, query: String, page: Int = 0): List<Track> =
        withContext(Dispatchers.IO) {
            val obj = get(token, "$API_BASE/search?type=track&page=$page&text=${urlEncode(query)}")
            obj["result"]?.jsonObject?.get("tracks")?.jsonObject?.get("results")?.jsonArray
                ?.mapNotNull(::parseTrack)
                .orEmpty()
        }

    // -------------------------------------------------------------- поток

    /**
     * Ссылка на звук трека: `get-file-info` (AAC/FLAC, HMAC-подпись), при
     * неудаче — запасной путь `download-info` (MP3, MD5-подпись). Как в
     * десктопном клиенте.
     */
    suspend fun streamUrl(token: String, trackId: String, lossless: Boolean = false): StreamResult =
        withContext(Dispatchers.IO) {
            val primary = fileInfoUrl(token, trackId, lossless)
            if (primary is StreamResult.Url) return@withContext primary
            fallbackMp3Url(token, trackId).takeIf { it is StreamResult.Url || it == StreamResult.Unavailable }
                ?: primary
        }

    private fun fileInfoUrl(token: String, trackId: String, lossless: Boolean): StreamResult {
        val ts = (System.currentTimeMillis() / 1000).toString()
        val quality = if (lossless) "lossless" else "nq"
        val codecs = if (lossless) "flac,flac-mp4,aac-mp4,aac,mp3" else "aac-mp4,aac,mp3"
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
        kotlinx.coroutines.runBlocking { streamUrl(token, trackId) }

    // -------------------------------------------------------------- волна

    /** Порция треков «Моей волны». */
    data class WaveBatch(
        val session: String,
        val batchId: String,
        val from: String?,
        val tracks: List<Track>,
    )

    /** Запустить «Мою волну» и получить первую порцию. */
    suspend fun startWave(token: String): WaveBatch = withContext(Dispatchers.IO) {
        val body = kotlinx.serialization.json.buildJsonObject {
            put("seeds", kotlinx.serialization.json.buildJsonArray { add(kotlinx.serialization.json.JsonPrimitive(WAVE_SEED)) })
            put("queue", kotlinx.serialization.json.buildJsonArray {})
            put("includeTracksInResponse", kotlinx.serialization.json.JsonPrimitive(true))
            put("includeWaveModel", kotlinx.serialization.json.JsonPrimitive(true))
            put("interactive", kotlinx.serialization.json.JsonPrimitive(true))
        }
        val text = post(token, "$API_BASE/rotor/session/new", body)
        parseBatch(text, null) ?: throw IllegalStateException("волна не началась")
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
            val stamp = java.text.SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss.SSS'Z'")
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

    // ------------------------------------------------------- подборки лендинга

    /** «Плейлист дня», «Дежавю», «Премьера» — блок personal-playlists лендинга. */
    suspend fun personalPlaylists(token: String): List<ru.plastinka.data.GeneratedPlaylist> =
        withContext(Dispatchers.IO) {
            val obj = get(token, "$API_BASE/landing3?blocks=personalplaylists")
            obj["result"]?.jsonObject?.get("blocks")?.jsonArray
                ?.firstOrNull { it.jsonObject["type"]?.jsonPrimitive?.content == "personal-playlists" }
                ?.jsonObject?.get("entities")?.jsonArray
                ?.mapNotNull { entity ->
                    // Плейлист спрятан в data.data.
                    val pl = entity.jsonObject["data"]?.jsonObject?.get("data")?.jsonObject
                        ?: return@mapNotNull null
                    ru.plastinka.data.GeneratedPlaylist(
                        uid = pl["uid"]?.jsonPrimitive?.content?.toLongOrNull()
                            ?: return@mapNotNull null,
                        kind = pl["kind"]?.jsonPrimitive?.content?.toLongOrNull()
                            ?: return@mapNotNull null,
                        title = pl["title"]?.jsonPrimitive?.content ?: return@mapNotNull null,
                        coverUri = pl["cover"]?.jsonObject?.get("uri")?.jsonPrimitive?.content
                            ?: pl["ogImage"]?.jsonPrimitive?.content,
                        trackCount = pl["trackCount"]?.jsonPrimitive?.content?.toIntOrNull() ?: 0,
                    )
                }
                .orEmpty()
        }

    /** Треки подборки: `GET /users/{uid}/playlists/{kind}`. */
    suspend fun playlistTracks(token: String, ownerUid: Long, kind: Long): List<Track> =
        withContext(Dispatchers.IO) {
            val obj = get(token, "$API_BASE/users/$ownerUid/playlists/$kind")
            obj["result"]?.jsonObject?.get("tracks")?.jsonArray
                ?.mapNotNull { it.jsonObject["track"]?.let(::parseTrack) }
                ?.filter { it.available }
                .orEmpty()
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
        )
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
        private val ALLOWED_HOSTS = listOf(
            "oauth.yandex.ru", "oauth.yandex.com", "ya.ru", "yandex.ru", "yandex.com",
        )

        /** Только https и только хосты Яндекса. */
        fun allowedUrl(url: String): Boolean {
            if (!url.startsWith("https://")) return false
            val host = url.removePrefix("https://").substringBefore('/').lowercase()
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
