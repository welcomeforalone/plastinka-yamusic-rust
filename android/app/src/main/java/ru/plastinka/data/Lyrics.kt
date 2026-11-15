package ru.plastinka.data

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.jsonObject
import okhttp3.Call
import okhttp3.Callback
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import java.io.IOException
import java.security.GeneralSecurityException
import java.util.concurrent.TimeUnit
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/** Откуда текст. */
enum class LyricsSource(val title: String) {
    Yandex("Яндекс"),
    LrcLib("LRCLib"),
}

/** Найденный текст: хотя бы одно из полей непусто. */
data class Lyrics(
    val source: LyricsSource,
    /** Строки с метками времени, по возрастанию времени. */
    val synced: List<LyricLine>?,
    /** Обычный текст. */
    val plain: String?,
)

/** Чем кончился поиск текста (ошибки — исключением [LyricsException]). */
sealed interface LyricsResult {
    data class Found(val lyrics: Lyrics) : LyricsResult
    /** Трек без слов (LRCLib). */
    data object Instrumental : LyricsResult
    /** Текста нет ни в одном источнике. */
    data object Missing : LyricsResult
}

/** Ошибка получения текста; `message` — готовый текст для экрана. */
class LyricsException(message: String) : Exception(message)

/**
 * Тексты песен: Яндекс и LRCLib. Только метаданные, звук не участвует.
 * Кэш — только в памяти (последние [CACHE_SIZE] треков), на диск ничего не
 * пишется; ошибки не кэшируются. Токен уходит одному хосту — api.music.yandex.net;
 * ссылка на файл текста и LRCLib получают запрос без токена.
 */
class LyricsRepository(
    http: OkHttpClient,
    private val tokenProvider: () -> String?,
) {
    // Общий пул соединений, но свой общий потолок времени на запрос.
    private val http = http.newBuilder().callTimeout(20, TimeUnit.SECONDS).build()
    private val json = Json { ignoreUnknownKeys = true }

    private val cache = object : LinkedHashMap<String, LyricsResult>(16, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, LyricsResult>?): Boolean =
            size > CACHE_SIZE
    }

    /**
     * Ключ кэша: результат зависит от того, ищем ли мы в LRCLib. Поэтому смена
     * настройки не оставляет устаревший ответ: «нет текста» без LRCLib и
     * найденное в LRCLib лежат под разными ключами.
     */
    private fun cacheKey(trackId: String, lrclib: Boolean): String = (if (lrclib) "L:" else "Y:") + trackId

    /** Из памяти, если трек недавно искали (с нынешней настройкой LRCLib). */
    fun cached(trackId: String): LyricsResult? =
        synchronized(cache) { cache[cacheKey(trackId, Prefs.lyricsLrclib)] }

    /**
     * Ищет текст по источникам в порядке [SOURCE_ORDER]. Найден — отдаём
     * сразу; иначе «без слов» важнее ошибки, ошибка важнее «текста нет»
     * (чтобы сбой сети не выдавался за отсутствие текста).
     */
    suspend fun load(track: Track): LyricsResult {
        // Настройка читается один раз: ключ кэша и список источников согласованы.
        val lrclib = Prefs.lyricsLrclib
        val key = cacheKey(track.id, lrclib)
        synchronized(cache) { cache[key] }?.let { return it }
        var error: LyricsException? = null
        var instrumental = false
        for (source in sources(lrclib)) {
            try {
                when (val result = fetch(source, track)) {
                    is LyricsResult.Found -> {
                        put(key, result)
                        return result
                    }
                    LyricsResult.Instrumental -> instrumental = true
                    LyricsResult.Missing -> {}
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: LyricsException) {
                if (error == null) error = e
            }
        }
        if (instrumental) {
            put(key, LyricsResult.Instrumental)
            return LyricsResult.Instrumental
        }
        if (error != null) throw error
        put(key, LyricsResult.Missing)
        return LyricsResult.Missing
    }

    private fun put(key: String, result: LyricsResult) {
        synchronized(cache) { cache[key] = result }
    }

    private suspend fun fetch(source: LyricsSource, track: Track): LyricsResult =
        withContext(Dispatchers.IO) {
            try {
                when (source) {
                    LyricsSource.Yandex -> fromYandex(track)
                    LyricsSource.LrcLib -> fromLrcLib(track)
                }
            } catch (e: IOException) {
                throw LyricsException("Не удалось загрузить текст. Проверьте соединение")
            } catch (e: IllegalArgumentException) {
                // Ломаный JSON или адрес: сервис прислал не то.
                throw LyricsException(BAD_ANSWER)
            }
        }

    // ------------------------------------------------------------- Яндекс

    /** Сначала с метками времени; нет меток — обычный текст. */
    private suspend fun fromYandex(track: Track): LyricsResult {
        if (!YandexApi.isNumericId(track.id)) return LyricsResult.Missing
        yandexText(track.id, "LRC")?.let { text ->
            val lines = LrcParser.parse(text)
            return if (lines.isNotEmpty()) {
                LyricsResult.Found(Lyrics(LyricsSource.Yandex, lines, null))
            } else {
                // Пришёл текст без меток: показываем как обычный.
                LyricsResult.Found(Lyrics(LyricsSource.Yandex, null, text))
            }
        }
        yandexText(track.id, "TEXT")?.let {
            return LyricsResult.Found(Lyrics(LyricsSource.Yandex, null, it))
        }
        return LyricsResult.Missing
    }

    /** Текст в формате `format` (`LRC` или `TEXT`); `null` — текста нет. */
    private suspend fun yandexText(trackId: String, format: String): String? {
        val token = tokenProvider() ?: throw LyricsException("Войдите в аккаунт заново")
        val ts = System.currentTimeMillis() / 1000
        val url = "${YandexApi.API_BASE}/tracks/$trackId/lyrics?format=$format&timeStamp=$ts" +
            "&sign=${YandexApi.urlEncode(sign(trackId, ts))}"
        val request = Request.Builder()
            .url(url)
            .header("Authorization", "OAuth $token")
            .header("X-Yandex-Music-Client", YandexApi.MUSIC_CLIENT)
            .build()
        val link = http.newCall(request).await().use { resp ->
            when {
                resp.code == 401 -> throw LyricsException("Войдите в аккаунт заново")
                // Нет текста или нет прав на него: не «токен не действует».
                resp.code == 404 || resp.code == 403 -> return null
                !resp.isSuccessful -> throw serverError(resp.code)
                else -> parseYandexLink(readLimited(resp))
            }
        } ?: return null

        // Ссылка подписана сама, хост чужой для токена: токен сюда не передаём.
        val file = Request.Builder().url(link).build()
        return http.newCall(file).await().use { resp ->
            when {
                resp.code == 404 || resp.code == 403 || resp.code == 410 -> null
                !resp.isSuccessful -> throw serverError(resp.code)
                else -> cleanText(readLimited(resp))
            }
        }
    }

    /** `result.downloadUrl`; `null` — текста нет (в ответе `error` или нет ссылки). */
    private fun parseYandexLink(body: String): String? {
        val root = json.parseToJsonElement(body).jsonObject
        val result = root["result"] as? JsonObject
        if (result == null) {
            if (root["error"] != null) return null
            throw LyricsException(BAD_ANSWER)
        }
        val link = (result["downloadUrl"] as? JsonPrimitive)?.takeIf { it.isString }?.content ?: return null
        // Чужой хост или не https: такую ссылку не открываем.
        if (!YandexApi.allowedUrl(link)) throw LyricsException(BAD_ANSWER)
        return link
    }

    /** Подпись: HMAC-SHA256 от `trackId + timestamp`, base64 вместе с «=». */
    private fun sign(trackId: String, ts: Long): String =
        try {
            val mac = Mac.getInstance("HmacSHA256").apply {
                init(SecretKeySpec(YandexApi.SIGN_KEY.toByteArray(), "HmacSHA256"))
            }
            java.util.Base64.getEncoder().encodeToString(mac.doFinal("$trackId$ts".toByteArray()))
        } catch (e: GeneralSecurityException) {
            throw LyricsException(BAD_ANSWER)
        }

    // -------------------------------------------------------------- LRCLib

    /** Точное совпадение `/get`, при 404 — `/search`. Запрос без токена. */
    private suspend fun fromLrcLib(track: Track): LyricsResult {
        val title = track.title.trim()
        val artist = track.artistNames.trim()
        if (title.isEmpty() || artist.isEmpty()) return LyricsResult.Missing
        val seconds = track.durationMs?.let { Math.round(it / 1000.0) }?.takeIf { it in 1..3600 }

        val exact = lrcLibUrl("get") {
            addQueryParameter("track_name", title)
            addQueryParameter("artist_name", artist)
            track.albumTitle?.takeIf { it.isNotEmpty() }?.let { addQueryParameter("album_name", it) }
            seconds?.let { addQueryParameter("duration", it.toString()) }
        }
        val body = lrcLibCall(exact)
        if (body != null) return lrcLibOutcome(parseRecord(json.parseToJsonElement(body)))

        // Точного совпадения нет: ищем по названию и исполнителю (альбом сужает слишком сильно).
        val search = lrcLibUrl("search") {
            addQueryParameter("track_name", title)
            addQueryParameter("artist_name", artist)
        }
        val found = lrcLibCall(search)?.let { text ->
            (json.parseToJsonElement(text) as? JsonArray)?.mapNotNull { parseRecord(it) }
        }.orEmpty()
        return lrcLibOutcome(pick(found, track.durationMs?.let { it / 1000.0 }))
    }

    private fun lrcLibUrl(path: String, params: okhttp3.HttpUrl.Builder.() -> Unit): okhttp3.HttpUrl =
        "$LRCLIB_BASE/$path".toHttpUrl().newBuilder().apply(params).build()

    /** Тело ответа LRCLib; `null` — 404 («не найдено» — штатный ответ), остальные ошибки — исключение. */
    private suspend fun lrcLibCall(url: okhttp3.HttpUrl): String? {
        val request = Request.Builder().url(url).header("User-Agent", USER_AGENT).build()
        return http.newCall(request).await().use { resp ->
            when {
                resp.code == 404 -> null
                resp.code == 429 -> throw LyricsException(
                    "Текст недоступен: сервис LRCLib просит подождать. Повторите позже",
                )
                resp.code >= 500 -> throw LyricsException("Сервис текстов LRCLib временно недоступен")
                !resp.isSuccessful -> throw LyricsException(
                    "Сервис текстов LRCLib ответил неожиданно (код ${resp.code})",
                )
                else -> readLimited(resp, LRCLIB_MAX_BYTES)
            }
        }
    }

    /** Одна запись ответа LRCLib. */
    private class Record(
        val duration: Double?,
        val instrumental: Boolean,
        val plain: String?,
        val synced: String?,
    )

    private fun parseRecord(element: JsonElement): Record? {
        val obj = element as? JsonObject ?: return null
        fun text(key: String): String? =
            (obj[key] as? JsonPrimitive)?.takeIf { it.isString }?.content?.takeIf { it.isNotBlank() }
        return Record(
            duration = (obj["duration"] as? JsonPrimitive)?.doubleOrNull,
            instrumental = (obj["instrumental"] as? JsonPrimitive)?.booleanOrNull == true,
            plain = text("plainLyrics"),
            synced = text("syncedLyrics"),
        )
    }

    /**
     * Запись из `search`: длительность в пределах допуска, с метками
     * предпочтительнее, затем ближайшая по длительности. Без нашей длительности
     * сверять нечем — берётся первая с текстом.
     */
    private fun pick(records: List<Record>, ours: Double?): Record? =
        records
            .filter { it.instrumental || it.plain != null || it.synced != null }
            .filter { r ->
                val theirs = r.duration
                when {
                    ours == null -> true
                    theirs == null -> false
                    else -> Math.abs(ours - theirs) <= DURATION_TOLERANCE
                }
            }
            .minWithOrNull(
                compareBy<Record>(
                    { it.synced == null },
                    { r ->
                        val theirs = r.duration
                        if (ours != null && theirs != null) Math.abs(ours - theirs) else 0.0
                    },
                ),
            )

    private fun lrcLibOutcome(record: Record?): LyricsResult {
        record ?: return LyricsResult.Missing
        if (record.instrumental) return LyricsResult.Instrumental
        val synced = record.synced?.let(LrcParser::parse)?.takeIf { it.isNotEmpty() }
        if (synced == null && record.plain == null) return LyricsResult.Missing
        return LyricsResult.Found(Lyrics(LyricsSource.LrcLib, synced, record.plain))
    }

    // ------------------------------------------------------------- общее

    private fun serverError(code: Int): LyricsException =
        LyricsException(
            if (code >= 500 || code == 429) {
                "Сервис текстов временно недоступен. Повторите позже"
            } else {
                BAD_ANSWER
            },
        )

    /** Тело ответа не больше [limit] байт; больше — ответ считаем неправильным. */
    private fun readLimited(resp: Response, limit: Long = MAX_BYTES): String {
        val source = resp.body?.source() ?: return ""
        source.request(limit + 1)
        if (source.buffer.size > limit) throw LyricsException(BAD_ANSWER)
        return source.buffer.readUtf8()
    }

    companion object {
        /** Порядок источников: сначала Яндекс. */
        val SOURCE_ORDER = listOf(LyricsSource.Yandex, LyricsSource.LrcLib)

        /** Источники с учётом настройки: выключен LRCLib — к нему не обращаемся вовсе. */
        fun sources(lrclib: Boolean): List<LyricsSource> =
            SOURCE_ORDER.filter { lrclib || it != LyricsSource.LrcLib }

        const val CACHE_SIZE = 30
        private const val MAX_BYTES = 256L * 1024
        private const val LRCLIB_MAX_BYTES = 1024L * 1024
        private const val DURATION_TOLERANCE = 2.0
        private const val LRCLIB_BASE = "https://lrclib.net/api"
        private const val USER_AGENT = "Plastinka/0.1 Android"
        private const val BAD_ANSWER = "Сервис текстов прислал непонятный ответ"

        /** Текст файла: без BOM и `\r`, пустой — `null`. */
        fun cleanText(text: String): String? {
            val cleaned = text.removePrefix("﻿").replace("\r", "")
            return cleaned.takeIf { it.isNotBlank() }
        }
    }
}

/** Выполнить вызов без блокировки потока: отмена корутины отменяет и запрос. */
private suspend fun Call.await(): Response = suspendCancellableCoroutine { cont ->
    cont.invokeOnCancellation { cancel() }
    enqueue(object : Callback {
        override fun onFailure(call: Call, e: IOException) {
            if (cont.isActive) cont.resumeWithException(e)
        }

        override fun onResponse(call: Call, response: Response) {
            // Корутина уже отменена: ответ закрываем сами.
            if (cont.isActive) cont.resume(response) else response.close()
        }
    })
}
