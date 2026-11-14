package ru.plastinka.playback

import android.content.Intent
import android.util.Log
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player
import androidx.media3.datasource.ResolvingDataSource
import androidx.media3.datasource.okhttp.OkHttpDataSource
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import ru.plastinka.PlastinkaApp
import ru.plastinka.data.StreamResult
import ru.plastinka.data.Track
import ru.plastinka.data.TrackUris

/**
 * Фоновое воспроизведение: Media3-сессия с уведомлением, наушники, блокировка.
 * Адрес `plastinka://track/<id>` превращается в реальный URL при открытии:
 * сначала замена FckCensor, потом поток Яндекса — как на десктопе.
 * Для очереди-волны шлёт события rotor и догружает порции треков.
 */
class PlaybackService : MediaSessionService() {

    private var mediaSession: MediaSession? = null
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val app: PlastinkaApp get() = application as PlastinkaApp

    /** Состояние волны: сессия, последняя порция, для событий и догрузки. */
    private object Wave {
        var session: String? = null
        var batchId: String? = null
        var from: String? = null
        var started = false
        var previousTrackId: String? = null
        var startedAtMs: Long? = null
        val topUpMutex = Mutex()
    }

    override fun onCreate() {
        super.onCreate()

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
                val now = System.currentTimeMillis()
                val previousId = Wave.previousTrackId
                val previousStart = Wave.startedAtMs
                val previousTrack = previousId
                    ?.let { id -> QueueHolder.state.value.tracks.firstOrNull { it.id == id } }

                QueueHolder.setCurrent(player.currentMediaItemIndex)
                Wave.previousTrackId = QueueHolder.currentTrack()?.id
                Wave.startedAtMs = now

                if (!QueueHolder.state.value.isWave) {
                    Wave.session = null
                    Wave.started = false
                    return
                }

                // Начало сессии — один раз.
                if (!Wave.started && Wave.session != null) {
                    Wave.started = true
                    feedback("radioStarted", trackId = null, played = null)
                }

                // Закрытие прошлого трека: доиграл (≥90 %) — trackFinished, иначе skip.
                if (previousTrack != null && previousStart != null) {
                    val played = (now - previousStart) / 1000.0
                    val full = previousTrack.durationMs?.let { played >= 0.9 * it / 1000.0 } ?: false
                    feedback(
                        if (full) "trackFinished" else "skip",
                        trackId = previousTrack,
                        played = played,
                    )
                }
                QueueHolder.currentTrack()?.let { feedback("trackStarted", trackId = it) }
                topUp(player)
            }
        })

        mediaSession = MediaSession.Builder(this, player).build()
    }

    /** Событие волны: ошибки молча глотаем, как на десктопе. */
    private fun feedback(type: String, trackId: Track? = null, played: Double? = null) {
        val token = app.tokenStore.load() ?: return
        val session = Wave.session ?: return
        val batchId = Wave.batchId ?: return
        val track = trackId?.let(app.api::waveTrackId)
        scope.launch {
            app.api.waveFeedback(token, session, batchId, Wave.from, type, track, played)
        }
    }

    /** Впереди меньше двух треков — просим порцию; сессия протухла — новую. */
    private fun topUp(player: Player) {
        val token = app.tokenStore.load() ?: return
        val state = QueueHolder.state.value
        if (!state.isWave) return
        scope.launch {
            Wave.topUpMutex.withLock {
                val remaining = state.tracks.size - (state.current + 1)
                if (remaining > 2) return@withLock
                try {
                    val queueIds = state.tracks
                        .subList(0, state.current + 1)
                        .map(app.api::waveTrackId)
                    val batch = app.api.waveTracks(token, Wave.session.orEmpty(), queueIds)
                    if (batch == null) {
                        // Сессия кончилась: новая с нуля, её треки догружаем.
                        val fresh = app.api.startWave(token)
                        Wave.session = fresh.session
                        Wave.from = fresh.from
                        append(player, fresh)
                        Log.i(TAG, "волна: сессия обновлена")
                    } else {
                        Wave.batchId = batch.batchId
                        if (batch.from != null) Wave.from = batch.from
                        append(player, batch)
                        Log.i(TAG, "волна: порция ${batch.tracks.size} треков")
                    }
                } catch (e: Exception) {
                    Log.w(TAG, "волна: догрузка не удалась: ${e.message}")
                }
            }
        }
    }

    private fun append(player: Player, batch: ru.plastinka.data.YandexApi.WaveBatch) {
        if (batch.tracks.isEmpty()) return
        QueueHolder.append(batch.tracks)
        player.addMediaItems(batch.tracks.map(::mediaItemOf))
    }

    /** FckCensor-замена → поток Яндекса. */
    private fun resolve(trackId: String): String? {
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
        scope.cancel()
        mediaSession?.run {
            player.release()
            release()
            mediaSession = null
        }
        super.onDestroy()
    }

    companion object {
        private const val TAG = "PlaybackService"

        /** Вызывается после старта волны: запомнить сессию до первого трека. */
        fun waveStarted(session: String, batchId: String, from: String?) {
            Wave.session = session
            Wave.batchId = batchId
            Wave.from = from
            Wave.started = false
        }

        fun mediaItemOf(track: Track): MediaItem =
            MediaItem.Builder()
                .setUri(TrackUris.forTrack(track.id))
                .setMediaId(track.id)
                .setMediaMetadata(
                    MediaMetadata.Builder()
                        .setTitle(track.title.ifEmpty { "Трек ${track.id}" })
                        .setArtist(track.artistNames.ifEmpty { null })
                        .setAlbumTitle(track.albumTitle)
                        .setArtworkUri(track.coverUrl()?.let { android.net.Uri.parse(it) })
                        .build(),
                )
                .build()
    }
}
