package ru.plastinka.ui

import android.content.Context
import android.graphics.Bitmap
import android.graphics.drawable.BitmapDrawable
import android.util.LruCache
import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
import androidx.compose.runtime.produceState
import androidx.compose.ui.platform.LocalContext
import coil.imageLoader
import coil.request.ImageRequest
import coil.request.SuccessResult
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * Средний цвет обложки для VibeColorizer (как `covers.average` на десктопе).
 * Обложку грузит уже подключённый Coil (обычный кэш обложек), пиксели
 * усредняются в памяти; результат хранится в памяти, на диск ничего не пишется.
 */
object CoverColors {
    /** Сторона уменьшенной копии, по которой считаем среднее. */
    private const val SAMPLE = 32
    private const val CACHE_SIZE = 256

    private val cache = LruCache<String, Int>(CACHE_SIZE)

    /** Уже посчитанный цвет (`0xRRGGBB`) или `null`. */
    fun cached(url: String): Int? = cache.get(url)

    /** Средний цвет обложки `0xRRGGBB`; `null`, если обложка не загрузилась. */
    suspend fun average(context: Context, url: String): Int? {
        cache.get(url)?.let { return it }
        val request = ImageRequest.Builder(context)
            .data(url)
            .size(SAMPLE)
            .allowHardware(false)
            .build()
        val result = context.imageLoader.execute(request) as? SuccessResult ?: return null
        val source = (result.drawable as? BitmapDrawable)?.bitmap ?: return null
        val color = withContext(Dispatchers.Default) { averageOf(source) }
        if (color != null) cache.put(url, color)
        return color
    }

    /** Среднее по пикселям уменьшенной копии; полностью прозрачные не считаются. */
    private fun averageOf(source: Bitmap): Int? {
        val small = Bitmap.createScaledBitmap(source, SAMPLE, SAMPLE, true)
        val pixels = IntArray(SAMPLE * SAMPLE)
        small.getPixels(pixels, 0, SAMPLE, 0, 0, SAMPLE, SAMPLE)
        if (small !== source) small.recycle()
        var r = 0L
        var g = 0L
        var b = 0L
        var count = 0L
        for (pixel in pixels) {
            if ((pixel ushr 24) == 0) continue
            r += (pixel shr 16) and 0xFF
            g += (pixel shr 8) and 0xFF
            b += pixel and 0xFF
            count++
        }
        if (count == 0L) return null
        return ((r / count).toInt() shl 16) or ((g / count).toInt() shl 8) or (b / count).toInt()
    }
}

/** Средний цвет обложки по ссылке; `null` — ссылки нет, ещё считается или не вышло. */
@Composable
fun rememberCoverColor(url: String?): State<Int?> {
    val context = LocalContext.current
    return produceState(url?.let(CoverColors::cached), url) {
        value = if (url == null) null else CoverColors.average(context, url)
    }
}
