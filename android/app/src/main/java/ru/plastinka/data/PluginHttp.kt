package ru.plastinka.data

import okhttp3.Request
import okhttp3.Response

/** Идентификатор в адресе запроса плагина: только цифры, не длиннее 32 знаков. */
internal fun plausibleNumericId(id: String): Boolean =
    id.length in 1..32 && id.all { it.isDigit() }

/**
 * Запрос плагина к чужому хосту (GitHub, slopless.art): без токена Яндекса и
 * без чужих заголовков, только User-Agent приложения и тип ответа.
 */
internal fun pluginRequest(url: String): Request =
    Request.Builder()
        .url(url)
        .header("User-Agent", "Plastinka/0.1 Android")
        .header("Accept", "application/json")
        .get()
        .build()

/** Тело ответа как текст, но не больше [limit] байт; больше — `null`. */
internal fun Response.readLimited(limit: Int): String? {
    val body = body ?: return null
    val length = body.contentLength()
    if (length > limit) return null
    body.byteStream().use { stream ->
        val out = java.io.ByteArrayOutputStream()
        val buffer = ByteArray(16 * 1024)
        while (true) {
            val n = stream.read(buffer)
            if (n < 0) break
            if (out.size() + n > limit) return null
            out.write(buffer, 0, n)
        }
        return out.toString(Charsets.UTF_8.name())
    }
}
