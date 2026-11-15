package ru.plastinka.ui

import android.content.ComponentName
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import ru.plastinka.data.Track
import ru.plastinka.playback.PlaybackService
import ru.plastinka.playback.QueueHolder

/** Обёртка над MediaController: запустить очередь, управлять воспроизведением. */
class PlayerController(val controller: MediaController) {

    /** Играет ли сейчас: меняется мгновенно по событию плеера, без опроса. */
    private val _isPlaying = MutableStateFlow(controller.isPlaying)
    val isPlaying: StateFlow<Boolean> = _isPlaying

    /** Перемешивание и повтор: настоящие режимы MediaController. */
    private val _shuffle = MutableStateFlow(controller.shuffleModeEnabled)
    val shuffle: StateFlow<Boolean> = _shuffle
    private val _repeatMode = MutableStateFlow(controller.repeatMode)
    val repeatMode: StateFlow<Int> = _repeatMode

    init {
        controller.addListener(object : Player.Listener {
            override fun onIsPlayingChanged(isPlaying: Boolean) {
                _isPlaying.value = isPlaying
            }

            override fun onShuffleModeEnabledChanged(shuffleModeEnabled: Boolean) {
                _shuffle.value = shuffleModeEnabled
            }

            override fun onRepeatModeChanged(repeatMode: Int) {
                _repeatMode.value = repeatMode
            }
        })
    }

    fun toggleShuffle() {
        controller.shuffleModeEnabled = !controller.shuffleModeEnabled
    }

    /** Повтор по кругу: выкл, вся очередь, один трек. */
    fun cycleRepeat() {
        controller.repeatMode = when (controller.repeatMode) {
            Player.REPEAT_MODE_OFF -> Player.REPEAT_MODE_ALL
            Player.REPEAT_MODE_ALL -> Player.REPEAT_MODE_ONE
            else -> Player.REPEAT_MODE_OFF
        }
    }

    fun playQueue(
        tracks: List<Track>,
        startIndex: Int,
        title: String,
        isWave: Boolean = false,
        /** Откуда очередь (альбом, плейлист…): попадает в локальный журнал прослушиваний. */
        source: ru.plastinka.data.HistoryContext? = null,
    ) {
        // Пустой список или индекс вне списка: текущую очередь не трогаем.
        if (tracks.getOrNull(startIndex) == null) return
        // Недоступные треки в очередь не попадают; индекс пересчитывается:
        // стартуем с нажатого трека или с ближайшего доступного после него.
        val target = tracks.drop(startIndex).firstOrNull { it.available } ?: return
        val playable = tracks.filter { it.available }
        val index = playable.indexOfFirst { it === target }
        if (index < 0) return
        if (isWave) {
            // Волна идёт линейно и догружается по индексу: режимы сбрасываем.
            controller.shuffleModeEnabled = false
            controller.repeatMode = Player.REPEAT_MODE_OFF
        }
        QueueHolder.set(playable, index, title, isWave, source)
        val items = playable.map(PlaybackService::mediaItemOf)
        controller.setMediaItems(items, index, 0)
        controller.prepare()
        controller.play()
    }

    fun toggle() {
        if (controller.isPlaying) controller.pause() else controller.play()
    }

    fun next() = controller.seekToNextMediaItem()
    fun previous() = controller.seekToPreviousMediaItem()
    fun seekTo(positionMs: Long) = controller.seekTo(positionMs)
    fun seekTo(index: Int, positionMs: Long) = controller.seekTo(index, positionMs)

    val isPlayingNow: Boolean get() = controller.isPlaying
    val positionMs: Long get() = controller.currentPosition.coerceAtLeast(0)
    val durationMs: Long get() = controller.duration.coerceAtLeast(0)

    fun release() = controller.release()

    /** Остановить воспроизведение, очистить очередь плеера и отпустить контроллер. */
    fun stopAndRelease() {
        try {
            controller.stop()
            controller.clearMediaItems()
        } catch (_: Exception) {
            // Контроллер мог уже отключиться — освободить его всё равно нужно.
        }
        controller.release()
    }

    companion object {
        fun build(context: android.content.Context, onReady: (PlayerController?) -> Unit) {
            val token = SessionToken(context, ComponentName(context, PlaybackService::class.java))
            val future = MediaController.Builder(context, token).buildAsync()
            future.addListener({
                try {
                    onReady(PlayerController(future.get()))
                } catch (_: Exception) {
                    onReady(null)
                }
            }, java.util.concurrent.Executor { it.run() })
        }
    }
}
