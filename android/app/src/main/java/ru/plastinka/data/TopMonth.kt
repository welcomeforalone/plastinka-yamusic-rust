package ru.plastinka.data

/** Форма слова для числа: [1, 2–4, 5–20] (как `texts::plural` на десктопе). */
private fun plural(count: Long, one: String, few: String, many: String): String {
    val mod10 = count % 10
    val mod100 = count % 100
    return when {
        mod10 == 1L && mod100 != 11L -> one
        mod10 in 2L..4L && mod100 !in 12L..14L -> few
        else -> many
    }
}

/**
 * Сколько слушали исполнителя, как в топе месяца: «3,5 часа», «2 часа»,
 * «40 минут». Часы округляются до получаса. Ноль — `null` (подпись не показываем).
 */
fun listenedText(seconds: Long): String? {
    if (seconds <= 0L) return null
    val halves = (seconds + 900L) / 1800L
    if (halves < 2L) {
        val minutes = ((seconds + 30L) / 60L).coerceAtLeast(1L)
        return "$minutes ${plural(minutes, "минута", "минуты", "минут")}"
    }
    val hours = halves / 2L
    // С дробью — «часа»: «1,5 часа», «21,5 часа».
    return if (halves % 2L == 0L) "$hours ${plural(hours, "час", "часа", "часов")}" else "$hours,5 часа"
}
