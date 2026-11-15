package ru.plastinka.playback

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update

/**
 * Кодек и битрейт потока, который реально играет: сервис кладёт сюда данные
 * при получении ссылки, плеер показывает значок для текущего трека. Только в
 * памяти; ссылки на поток здесь нет.
 */
object QualityHolder {

    data class Quality(val codec: String, val kbps: Int) {
        /** Подпись значка: «MP3 · 320», «AAC · 256», «FLAC»; кодек неизвестен — `null`. */
        val label: String?
            get() {
                val name = when {
                    codec.startsWith("flac", ignoreCase = true) -> return "FLAC"
                    codec.startsWith("aac", ignoreCase = true) -> "AAC"
                    codec.equals("mp3", ignoreCase = true) -> "MP3"
                    else -> return null
                }
                return if (kbps > 0) "$name · $kbps" else name
            }
    }

    private const val MAX_TRACKS = 20

    private val _state = MutableStateFlow<Map<String, Quality>>(emptyMap())
    val state: StateFlow<Map<String, Quality>> = _state

    /** Поток трека получен. */
    fun put(trackId: String, codec: String, kbps: Int) {
        // Сервис может получать ссылки с разных потоков: обновление атомарное.
        _state.update { current ->
            val next = LinkedHashMap(current)
            next.remove(trackId)
            next[trackId] = Quality(codec, kbps)
            while (next.size > MAX_TRACKS) {
                val oldest = next.keys.firstOrNull() ?: break
                next.remove(oldest)
            }
            next
        }
    }

    /** Трек играет не потоком Яндекса (замена FckCensor) или поток не получен: значка нет. */
    fun clear(trackId: String) {
        _state.update { if (it.containsKey(trackId)) it - trackId else it }
    }
}
