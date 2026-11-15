package ru.plastinka.data

/** Строка текста с меткой времени; пустой текст — пауза. */
data class LyricLine(val timeMs: Long, val text: String)

/**
 * Разбор текста с метками времени (LRC). Чистые функции без Android.
 * Формат неформальный, разбор мягкий: битая строка пропускается, остальные
 * не страдают. Логика та же, что в десктопном `src/lyrics/lrc.rs`.
 */
object LrcParser {

    /**
     * Строки без меток и метаданные (`[ar:…]`, `[offset:…]`) пропускаются;
     * порядок по времени, при равенстве — как в тексте.
     */
    fun parse(text: String): List<LyricLine> {
        val body = text.removePrefix("﻿")
        val out = ArrayList<LyricLine>()
        for (raw in body.lineSequence()) parseLine(raw, out)
        return out.sortedBy { it.timeMs }
    }

    /** Одна строка: одна или несколько меток подряд и текст после них. */
    private fun parseLine(raw: String, out: MutableList<LyricLine>) {
        var rest = raw.trimStart()
        val times = ArrayList<Long>()
        while (rest.startsWith("[")) {
            val close = rest.indexOf(']')
            if (close < 0) break
            val ms = timestamp(rest.substring(1, close))
            if (ms != null) {
                times.add(ms)
                rest = rest.substring(close + 1).trimStart()
            } else if (times.isEmpty()) {
                // Метаданные в начале строки: вся строка не текст песни.
                return
            } else {
                break
            }
        }
        if (times.isEmpty()) return
        val text = stripWordTags(rest).trim()
        for (t in times) out.add(LyricLine(t, text))
    }

    /** Убирает посословные метки `<мм:сс.хх>`; прочие угловые скобки остаются текстом. */
    private fun stripWordTags(text: String): String {
        if (!text.contains('<')) return text
        val sb = StringBuilder(text.length)
        var i = 0
        while (i < text.length) {
            val c = text[i]
            if (c == '<') {
                val end = text.indexOf('>', i + 1)
                if (end > 0 && timestamp(text.substring(i + 1, end)) != null) {
                    i = end + 1
                    continue
                }
            }
            sb.append(c)
            i++
        }
        return sb.toString()
    }

    /**
     * `мм:сс`, `мм:сс.х`, `мм:сс.хх`, `мм:сс.ххх`, `мм:сс:хх` → миллисекунды.
     * До двоеточия и в секундах — только цифры (так отсекаются `ar`, `offset`).
     */
    fun timestamp(tag: String): Long? {
        val colon = tag.indexOf(':')
        if (colon < 0) return null
        val minutes = number(tag.substring(0, colon), 6) ?: return null
        val rest = tag.substring(colon + 1)
        val sep = rest.indexOfFirst { it == '.' || it == ':' }
        val seconds = number(if (sep < 0) rest else rest.substring(0, sep), 4) ?: return null
        val millis = if (sep < 0) 0L else fractionMillis(rest.substring(sep + 1)) ?: return null
        return (minutes * 60 + seconds) * 1000 + millis
    }

    private fun number(text: String, maxDigits: Int): Long? =
        if (text.isNotEmpty() && text.length <= maxDigits && text.all { it in '0'..'9' }) {
            text.toLongOrNull()
        } else {
            null
        }

    /** Дробная часть секунды: `5` — 500 мс, `50` — 500, `500` — 500; лишнее после третьей цифры отбрасывается. */
    private fun fractionMillis(fraction: String): Long? {
        if (fraction.isEmpty() || !fraction.all { it in '0'..'9' }) return null
        val first = fraction.take(3)
        val value = first.toLongOrNull() ?: return null
        var scale = 1L
        repeat(3 - first.length) { scale *= 10 }
        return value * scale
    }

    /** Номер строки, которая звучит в `positionMs`: последняя с меткой не позже позиции; до первой — `null`. */
    fun activeLine(lines: List<LyricLine>, positionMs: Long): Int? {
        var lo = 0
        var hi = lines.size
        while (lo < hi) {
            val mid = (lo + hi) ushr 1
            if ((lines.getOrNull(mid)?.timeMs ?: Long.MAX_VALUE) <= positionMs) lo = mid + 1 else hi = mid
        }
        return (lo - 1).takeIf { it >= 0 }
    }
}
