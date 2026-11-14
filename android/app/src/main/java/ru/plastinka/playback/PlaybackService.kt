package ru.plastinka.playback

import android.content.Intent
import android.util.Log
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player
import androidx.media3.datasource.DataSource
import androidx.media3.datasource.ResolvingDataSource
import androidx.media3.datasource.okhttp.OkHttpDataSource
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService
import ru.plastinka.PlastinkaApp
import ru.plastinka.data.StreamResult
import ru.plastinka.data.TrackUris

/**
 * Фоновое воспроизведение: Media3-сессия с уведомлением, наушники, блокировка.
 * Адрес `plastinka://track/<id>` превращается в реальный URL при открытии:
 * сначала замена FckCensor, потом поток Яндекса (get-file-info, запасной
 * download-info) — как на десктопе.
 */
class PlaybackService : MediaSessionService() {

    private var mediaSession: MediaSession? = null

    override fun onCreate() {
        super.onCreate()
        val app = application as PlastinkaApp

        val httpSource = OkHttpDataSource.Factory(app.http)
            .setUserAgent("Plastinka/0.1 Android")

        val resolvingSource = ResolvingDataSource.Factory(httpSource) { dataSpec ->
            val trackId = TrackUris.trackIdOf(dataSpec.uri.toString())
                ?: throw java.io.IOException("не трек: ${dataSpec.uri}")
            val url = resolve(trackId)
                ?: throw java.io.IOException("звук трека $trackId недоступен")
            dataSpec.buildUpon().setUri(url).build()
        }

        val player = ExoPlayer.Builder(this)
            .setMediaSourceFactory(DefaultMediaSourceFactory(resolvingSource))
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(C.USAGE_MEDIA)
                    .setContentType(C.AUDIO_CONTENT_TYPE_MUSIC)
                    .build(),
                true,
            )
            .setHandleAudioBecomingNoisy(true)
            .setWakeMode(C.WAKE_MODE_NETWORK)
            .build()

        player.addListener(object : Player.Listener {
            override fun onMediaItemTransition(mediaItem: MediaItem?, reason: Int) {
                QueueHolder.setCurrent(player.currentMediaItemIndex)
            }
        })

        mediaSession = MediaSession.Builder(this, player).build()
    }

    /** FckCensor-замена → поток Яндекса. */
    private fun resolve(trackId: String): String? {
        val app = application as PlastinkaApp
        val token = app.tokenStore.load() ?: return null
        app.fckCensor.replacementUrl(trackId)?.let { return it }
        return when (val result = app.api.streamUrlBlocking(token, trackId)) {
            is StreamResult.Url -> result.url
            else -> {
                Log.w(TAG, "поток трека $trackId не получен: $result")
                null
            }
        }
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaSession? =
        mediaSession

    override fun onTaskRemoved(rootIntent: Intent?) {
        val player = mediaSession?.player
        if (player == null || !player.playWhenReady) {
            stopSelf()
        }
    }

    override fun onDestroy() {
        mediaSession?.run {
            player.release()
            release()
            mediaSession = null
        }
        super.onDestroy()
    }

    companion object {
        private const val TAG = "PlaybackService"

        fun mediaItemOf(track: ru.plastinka.data.Track): MediaItem =
            MediaItem.Builder()
                .setUri(TrackUris.forTrack(track.id))
                .setMediaId(track.id)
                .setMediaMetadata(
                    MediaMetadata.Builder()
                        .setTitle(track.title.ifEmpty { "Трек ${track.id}" })
                        .setArtist(track.artistNames.ifEmpty { null })
                        .setAlbumTitle(track.albumTitle)
                        .setArtworkUri(
                            track.coverUrl()?.let { android.net.Uri.parse(it) }
                        )
                        .build(),
                )
                .build()
    }
}
