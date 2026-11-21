package ru.plastinka.data

/**
 * CopyTrackName: строка «Исполнитель — Название» или свой формат для буфера
 * обмена. В сеть не ходит. Правила — как в десктопном `src/plugins/copy_name.rs`.
 */
object CopyName {
    const val DEFAULT_FORMAT = "{artist} — {title}"

    /** Длиннее формат не нужен: это одна строка. */
    const val MAX_FORMAT = 200

    /** Формат из настроек: пустой или отсутствующий — по умолчанию, длинный обрезается. */
    fun sanitize(raw: String?): String =
        raw?.takeIf { it.isNotBlank() }?.take(MAX_FORMAT) ?: DEFAULT_FORMAT

    /**
     * Текст для буфера: `{artist}` — исполнители через запятую, `{title}` —
     * название, `{album}` — альбом. Остальное — как есть. Подстановка за один
     * проход: название с «{artist}» внутри второй раз не раскрывается.
     * Пустой формат — [DEFAULT_FORMAT].
     */
    fun format(format: String, track: Track): String {
        val pattern = if (format.isBlank()) DEFAULT_FORMAT else format
        val values = listOf(
            "{artist}" to track.artistNames.ifEmpty { "Неизвестный исполнитель" },
            "{title}" to track.displayTitle.ifEmpty { "Без названия" },
            "{album}" to (track.albumTitle ?: ""),
        )
        val out = StringBuilder(pattern.length + 32)
        var i = 0
        while (i < pattern.length) {
            val open = pattern.indexOf('{', i)
            if (open < 0) {
                out.append(pattern, i, pattern.length)
                break
            }
            out.append(pattern, i, open)
            val hit = values.firstOrNull { (key, _) -> pattern.startsWith(key, open) }
            if (hit != null) {
                out.append(hit.second)
                i = open + hit.first.length
            } else {
                out.append('{')
                i = open + 1
            }
        }
        return out.toString().trim()
    }
}
