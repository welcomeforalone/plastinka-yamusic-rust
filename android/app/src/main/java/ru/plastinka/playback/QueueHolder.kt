package ru.plastinka.playback

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import ru.plastinka.data.HistoryContext
import ru.plastinka.data.Track

/** Очередь воспроизведения, общая для UI и сервиса. */
object QueueHolder {

    data class State(
        val tracks: List<Track> = emptyList(),
        /** Индекс играющего трека или -1. */
        val current: Int = -1,
        val title: String = "",
        /** Очередь — «Моя волна»: сервис шлёт события и догружает порции. */
        val isWave: Boolean = false,
        /** Откуда очередь: для локального журнала прослушиваний. */
        val context: HistoryContext = HistoryContext.Other(""),
    )

    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state

    /** Сколько треков помним в «Недавно играли» (как на десктопе). */
    const val RECENT_LIMIT = 20

    /**
     * История сеанса: треки, которые зазвучали и сменились другим, новые в
     * конце. Только в памяти, между запусками не хранится.
     */
    private val _session = MutableStateFlow<List<Track>>(emptyList())
    val sessionHistory: StateFlow<List<Track>> = _session

    /** Трек зазвучал и сменился. Подряд идущий повтор того же трека не дублируется. */
    fun recordSession(track: Track) {
        _session.update { list ->
            if (list.lastOrNull()?.id == track.id) list else (list + track).takeLast(RECENT_LIMIT)
        }
    }

    private val _likes = MutableStateFlow<Set<String>>(emptySet())
    val likedIds: StateFlow<Set<String>> = _likes

    fun set(
        tracks: List<Track>,
        startIndex: Int,
        title: String,
        isWave: Boolean = false,
        source: HistoryContext? = null,
    ) {
        // Источник не назван: волна или просто подпись очереди.
        val context = source ?: if (isWave) HistoryContext.Wave else HistoryContext.Other(title)
        _state.value = State(tracks, startIndex, title, isWave, context)
    }

    /** Добавить догруженную порцию волны в конец очереди. */
    fun append(tracks: List<Track>) {
        _state.value = _state.value.copy(tracks = _state.value.tracks + tracks)
    }

    fun setCurrent(index: Int) {
        _state.value = _state.value.copy(current = index)
    }

    /**
     * Трек, который пользователь включил сам (нажал в списке или в очереди):
     * Slopless его не пропускает. Следующий/предыдущий выбор не меняют.
     */
    @Volatile
    var chosenId: String? = null

    fun setLikes(ids: Set<String>) {
        _likes.value = ids
    }

    fun toggleLike(trackId: String): Boolean {
        val now = _likes.value.toMutableSet()
        val liked = if (now.contains(trackId)) {
            now.remove(trackId); false
        } else {
            now.add(trackId); true
        }
        _likes.value = now
        return liked
    }

    /** Убрать сердце только у нас: сервер сам снимает «Нравится» при «Не нравится». */
    fun removeLike(trackId: String) {
        if (_likes.value.contains(trackId)) _likes.value = _likes.value - trackId
    }

    fun currentTrack(): Track? = _state.value.tracks.getOrNull(_state.value.current)
}

/** Трек состояния очереди. */
fun QueueHolder.State.currentTrack(): Track? = tracks.getOrNull(current)

/**
 * «Недавно играли»: сборка из трёх источников (как `merge_recent` на десктопе).
 * Порядок важности: история сеанса (`session`, от старых к новым), уже
 * сыгранные треки очереди перед текущим (`queueBefore`, ближайший первым),
 * серверная история (`server`, свежие первыми). Повторы по id отбрасываются:
 * остаётся вхождение, ближайшее к текущему; сам текущий трек не показывается.
 * Не больше `limit`; результат от старых к новым — самые свежие рядом с
 * «Сейчас играет».
 */
fun mergeRecent(
    currentId: String?,
    session: List<Track>,
    queueBefore: List<Track>,
    server: List<Track>,
    limit: Int,
): List<Track> {
    val seen = HashSet<String>()
    currentId?.let(seen::add)
    val out = ArrayList<Track>()
    for (track in session.asReversed() + queueBefore + server) {
        if (out.size >= limit) break
        if (seen.add(track.id)) out += track
    }
    out.reverse()
    return out
}
