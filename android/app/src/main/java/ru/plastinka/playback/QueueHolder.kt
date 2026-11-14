package ru.plastinka.playback

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
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
    )

    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state

    private val _likes = MutableStateFlow<Set<String>>(emptySet())
    val likedIds: StateFlow<Set<String>> = _likes

    fun set(tracks: List<Track>, startIndex: Int, title: String, isWave: Boolean = false) {
        _state.value = State(tracks, startIndex, title, isWave)
    }

    /** Добавить догруженную порцию волны в конец очереди. */
    fun append(tracks: List<Track>) {
        _state.value = _state.value.copy(tracks = _state.value.tracks + tracks)
    }

    fun setCurrent(index: Int) {
        _state.value = _state.value.copy(current = index)
    }

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

    fun currentTrack(): Track? = _state.value.tracks.getOrNull(_state.value.current)
}

/** Трек состояния очереди. */
fun QueueHolder.State.currentTrack(): Track? = tracks.getOrNull(current)
