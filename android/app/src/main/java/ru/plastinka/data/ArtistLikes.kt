package ru.plastinka.data

/** Правка списка «Исполнители» коллекции при нажатии на сердце (чистая логика, без сети). */
object ArtistLikes {

    /** Новый исполнитель встаёт в начало без дублей; `like = false` — исполнитель убирается. */
    fun apply(list: List<ArtistCard>, card: ArtistCard, like: Boolean): List<ArtistCard> =
        if (like) listOf(card) + list.filter { it.id != card.id } else list.filter { it.id != card.id }

    /**
     * Откат: [before] — список до нажатия. Поставленное сердце убираем из текущего списка,
     * снятое возвращаем на прежнее место (если исполнителя в списке до нажатия не было — ничего).
     */
    fun rollback(current: List<ArtistCard>, before: List<ArtistCard>, card: ArtistCard, wasLiked: Boolean): List<ArtistCard> {
        val rest = current.filter { it.id != card.id }
        if (!wasLiked) return rest
        val index = before.indexOfFirst { it.id == card.id }.coerceIn(0, rest.size)
        return rest.take(index) + (before.firstOrNull { it.id == card.id } ?: card) + rest.drop(index)
    }
}
