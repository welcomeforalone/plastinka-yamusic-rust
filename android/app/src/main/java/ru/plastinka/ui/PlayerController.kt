package ru.plastinka.ui

import android.content.ComponentName
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import ru.plastinka.data.Track
import ru.plastinka.playback.PlaybackService
import ru.plastinka.playback.QueueHolder

/** Обёртка над MediaController: запустить очередь, управлять воспроизведением. */
class PlayerController(val controller: MediaController) {

    fun playQueue(tracks: List<Track>, startIndex: Int, title: String) {
        QueueHolder.set(tracks, startIndex, title)
        val items = tracks.map(PlaybackService::mediaItemOf)
        controller.setMediaItems(items, startIndex, 0)
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

    val isPlaying: Boolean get() = controller.isPlaying
    val positionMs: Long get() = controller.currentPosition.coerceAtLeast(0)
    val durationMs: Long get() = controller.duration.coerceAtLeast(0)

    fun release() = controller.release()

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
