package ru.plastinka.data

import java.time.LocalDate
import java.time.ZoneId

/** Откуда играли: контекст очереди (и серверной истории, и локального журнала). */
sealed interface HistoryContext {
    data class Album(val id: String, val title: String) : HistoryContext
    data class Artist(val id: String, val name: String) : HistoryContext
    data class Playlist(val uid: String, val kind: String, val title: String) : HistoryContext
    data object Wave : HistoryContext
    data object Liked : HistoryContext
    data class Other(val title: String) : HistoryContext

    /** Подпись контекста. */
    val label: String
        get() = when (this) {
            is Album -> "Альбом «$title»"
            is Artist -> "Исполнитель $name"
            is Playlist -> "Плейлист «$title»"
            Wave -> "Моя волна"
            Liked -> "Мне нравится"
            is Other -> title
        }
}

/** Группа: контекст и треки, которые из него играли. */
data class HistoryGroup(val context: HistoryContext, val tracks: List<Track>)

/** День истории; `date == null` — сервер дату не прислал. */
data class HistoryDay(val date: LocalDate?, val groups: List<HistoryGroup>)

/** Прослушивание из локального журнала. */
data class LocalPlay(val track: Track, val context: HistoryContext, val date: LocalDate, val atMs: Long)

/** Сегодняшний день по часовому поясу устройства. */
fun todayLocal(): LocalDate = LocalDate.now(ZoneId.systemDefault())

/** Подпись дня: «Сегодня», «Вчера», «12 ноября» (с годом, если он не нынешний); без даты — «Раньше». */
fun dayLabel(date: LocalDate?, today: LocalDate): String {
    if (date == null) return "Раньше"
    val ago = today.toEpochDay() - date.toEpochDay()
    return when (ago) {
        0L -> "Сегодня"
        1L -> "Вчера"
        else -> {
            val month = MONTHS.getOrNull(date.monthValue - 1).orEmpty()
            if (date.year == today.year) "${date.dayOfMonth} $month" else "${date.dayOfMonth} $month ${date.year}"
        }
    }
}

private val MONTHS = listOf(
    "января", "февраля", "марта", "апреля", "мая", "июня",
    "июля", "августа", "сентября", "октября", "ноября", "декабря",
)

/**
 * Серверные дни (свежие первыми) плюс локальные прослушивания (`local`, от
 * старых к новым): как `merge_days` на десктопе. Локальные встают в начало
 * своего дня, свежие первыми, без повторов трека; подряд идущие из одного
 * контекста — одна группа. Тот же трек в тот же день с сервера убирается.
 * Дня нет у сервера — он встаёт на своё место по дате.
 */
fun mergeHistory(server: List<HistoryDay>, local: List<LocalPlay>): List<HistoryDay> {
    val days = server.toMutableList()
    val dates = local.asReversed().map { it.date }.distinct()
    for (date in dates) {
        val groups = localGroups(local, date)
        val ids = groups.flatMap { it.tracks }.map { it.id }.toSet()
        val at = days.indexOfFirst { it.date == date }
        if (at >= 0) {
            val rest = days[at].groups.mapNotNull { g ->
                g.tracks.filter { it.id !in ids }.takeIf { it.isNotEmpty() }?.let { HistoryGroup(g.context, it) }
            }
            days[at] = HistoryDay(date, join(groups, rest))
        } else {
            // Перед первым днём старше этого (или без даты).
            val pos = days.indexOfFirst { it.date == null || it.date < date }.takeIf { it >= 0 } ?: days.size
            days.add(pos, HistoryDay(date, groups))
        }
    }
    return days
}

/** Группы прослушиваний дня: свежие первыми, без повторов трека. */
private fun localGroups(local: List<LocalPlay>, date: LocalDate): List<HistoryGroup> {
    val seen = HashSet<String>()
    val groups = mutableListOf<HistoryGroup>()
    for (play in local.asReversed()) {
        if (play.date != date || !seen.add(play.track.id)) continue
        val last = groups.lastOrNull()
        if (last != null && last.context == play.context) {
            groups[groups.lastIndex] = last.copy(tracks = last.tracks + play.track)
        } else {
            groups += HistoryGroup(play.context, listOf(play.track))
        }
    }
    return groups
}

/** Сначала `first`, потом `rest`; на стыке один контекст — одна группа. */
private fun join(first: List<HistoryGroup>, rest: List<HistoryGroup>): List<HistoryGroup> {
    val head = rest.firstOrNull() ?: return first
    val last = first.lastOrNull()
    return if (last != null && last.context == head.context) {
        first.dropLast(1) + last.copy(tracks = last.tracks + head.tracks) + rest.drop(1)
    } else {
        first + rest
    }
}

/** Плитка блока «Недавнее». */
data class RecentItem(
    val context: HistoryContext,
    val coverUri: String?,
    val subtitle: String,
)

/**
 * Последние альбомы и плейлисты, откуда играли (свежие первыми, без «Моей
 * волны» и повторов, не больше `limit`). Число треков плейлиста и обложку
 * берём из списка плейлистов пользователя, если плейлист там есть.
 */
fun recentItems(days: List<HistoryDay>, playlists: List<PlaylistCard>, limit: Int = 6): List<RecentItem> {
    val out = mutableListOf<RecentItem>()
    val seen = HashSet<String>()
    for (group in days.flatMap { it.groups }) {
        if (out.size >= limit) break
        val ctx = group.context
        val first = group.tracks.firstOrNull()
        when (ctx) {
            is HistoryContext.Album -> {
                if (!seen.add("a:${ctx.id}")) continue
                val artist = first?.artistNames?.takeIf { it.isNotEmpty() }
                out += RecentItem(
                    ctx, first?.coverUri,
                    if (artist != null) "Альбом · $artist" else "Альбом",
                )
            }
            is HistoryContext.Playlist -> {
                if (!seen.add("p:${ctx.uid}:${ctx.kind}")) continue
                val card = playlists.firstOrNull {
                    it.uid.toString() == ctx.uid && it.kind.toString() == ctx.kind
                }
                out += RecentItem(
                    ctx, card?.coverUri ?: first?.coverUri,
                    if (card != null) "Плейлист · ${tracksCountText(card.trackCount)}" else "Плейлист",
                )
            }
            else -> Unit
        }
    }
    return out
}

/** «1 трек / 2 трека / 5 треков» (то же, что `tracksCount` в UI, но без зависимости от него). */
private fun tracksCountText(n: Int): String {
    val word = when {
        n % 100 in 11..14 -> "треков"
        n % 10 == 1 -> "трек"
        n % 10 in 2..4 -> "трека"
        else -> "треков"
    }
    return "$n $word"
}
