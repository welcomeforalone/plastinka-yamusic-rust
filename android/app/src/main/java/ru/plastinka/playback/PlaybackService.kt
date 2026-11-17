package ru.plastinka.playback

import android.content.Intent
import android.util.Log
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player

import androidx.media3.datasource.okhttp.OkHttpDataSource
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.session.CommandButton
import androidx.media3.session.DefaultMediaNotificationProvider
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService
import androidx.media3.session.SessionCommand
import androidx.media3.session.SessionResult
import com.google.common.collect.ImmutableList
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import ru.plastinka.PlastinkaApp
import ru.plastinka.R
import ru.plastinka.data.FckCensor
import ru.plastinka.data.Prefs
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

        /** «Не нравится»: следующий переход закрывает трек событием skip, а не trackFinished. */
        @Volatile
        var dislikeSkip = false

        /** «Не нравится» на последнем треке: ждём порцию и переходим к первому новому. */
        @Volatile
        var advanceOnAppend = false

        /** Запросить догрузку (выставляет сервис, пока он жив). */
        @Volatile
        var requestTopUp: (() -> Unit)? = null
    }

    override fun onCreate() {
        super.onCreate()

        // Источник данных: plastinka://track/<id> решается в сетевой адрес —
        // сначала FckCensor, потом поток Яндекса. Звук только потоком.
        val source = androidx.media3.datasource.DataSource.Factory {
            PlastinkaDataSource(app.http) { uri -> resolveUri(uri) }
        }

        val player = ExoPlayer.Builder(this, PlastinkaRenderersFactory(this))
            .setMediaSourceFactory(DefaultMediaSourceFactory(source))
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
            /** Текущий трек сменился, но ещё не зазвучал: в журнал он попадёт, когда пойдёт звук. */
            private var pendingLog = false

            override fun onIsPlayingChanged(isPlaying: Boolean) {
                if (isPlaying && pendingLog) logPlay()
                if (isPlaying) reportSounded() else reportPaused()
            }

            /** Трек, который уже зазвучал: когда сменится другим, он попадёт в «Недавно играли». */
            private var soundedTrack: Track? = null

            /** Очередь доиграна до конца: последний трек закрывается как «доигран». */
            override fun onPlaybackStateChanged(playbackState: Int) {
                if (playbackState == Player.STATE_ENDED) {
                    reportFinish(ended = true)
                    rememberSounded()
                }
            }

            /** Зазвучавший трек уходит в историю сеанса (только память); незвучавшие не идут. */
            private fun rememberSounded() {
                soundedTrack?.let(QueueHolder::recordSession)
                soundedTrack = null
            }

            /** Запись в локальный журнал (только метаданные трека и откуда играл). */
            private fun logPlay() {
                pendingLog = false
                val state = QueueHolder.state.value
                soundedTrack = state.currentTrack()
                state.currentTrack()?.let { ru.plastinka.data.PlayLog.record(it, state.context) }
            }

            override fun onMediaItemTransition(mediaItem: MediaItem?, reason: Int) {
                // Прежний трек зазвучал и сменился другим: запоминаем для «Недавно играли».
                rememberSounded()
                // Звук уже идёт (автопереход к следующему) — пишем сразу, иначе ждём начала звука.
                // Прежний трек закончился или сменился: сообщаем, сколько сыграно.
                reportFinish(
                    ended = reason == Player.MEDIA_ITEM_TRANSITION_REASON_AUTO ||
                        reason == Player.MEDIA_ITEM_TRANSITION_REASON_REPEAT,
                )
                pendingLog = true
                val index = player.currentMediaItemIndex
                // Слева направо — вперёд; кнопка «Назад» (SEEK на меньший индекс) — назад.
                lastForward = reason != Player.MEDIA_ITEM_TRANSITION_REASON_SEEK || index >= lastIndex
                lastIndex = index
                QueueHolder.setCurrent(index)
                // Slopless: проверить трек после того, как переход закончит обрабатываться.
                mainHandler.post { slopCheck(player) }
                QueueHolder.currentTrack().let { now ->
                    Normalizer.onTrack(now, replaced = now != null && app.fckCensor.replacementUrl(now.id) != null)
                }
                reportBegin()
                if (player.isPlaying) {
                    logPlay()
                    reportSounded()
                }
                val now = System.currentTimeMillis()
                val previousId = Wave.previousTrackId
                val previousStart = Wave.startedAtMs
                val previousTrack = previousId
                    ?.let { id -> QueueHolder.state.value.tracks.firstOrNull { it.id == id } }
                // «Не нравится» всегда закрывает трек как пропущенный, как на десктопе.
                val disliked = Wave.dislikeSkip
                Wave.dislikeSkip = false

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
                // Повтор трека («Повторять трек») — это доигранный до конца трек и новое начало
                // того же трека: обычная пара событий, без skip (как на десктопе).
                if (previousTrack != null && previousStart != null) {
                    val repeated = reason == Player.MEDIA_ITEM_TRANSITION_REASON_REPEAT
                    val length = previousTrack.durationMs?.let { it / 1000.0 }
                    val played = if (repeated && length != null) length else (now - previousStart) / 1000.0
                    val full = !disliked && (repeated || (length?.let { played >= 0.9 * it } ?: false))
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

        // «Не нравится» на последнем треке просит догрузку у сервиса.
        Wave.requestTopUp = { mainHandler.post { topUp(player) } }

        // Смена качества: играющий трек открывается заново с той же позиции.
        Prefs.onQualityChanged = { mainHandler.post { restartForQuality(player) } }

        // Slopless: ответ пришёл, когда трек уже играет, или настройки сменились — проверить ещё раз.
        Prefs.onSlopChanged = { mainHandler.post { slopCheck(player) } }
        scope.launch {
            app.slopless.version.collect { mainHandler.post { slopCheck(player) } }
        }

        // FckCensor: список пришёл или плагин переключили — названия, исполнители и обложка
        // в шторке и на экране блокировки обновляются без перезапуска трека.
        scope.launch {
            FckCensor.revision.collect { mainHandler.post { refreshMetadata(player) } }
        }

        // Клик по уведомлению открывает приложение (singleTask — тот же экран).
        val sessionActivity = android.app.PendingIntent.getActivity(
            this,
            0,
            Intent(this, ru.plastinka.MainActivity::class.java),
            android.app.PendingIntent.FLAG_IMMUTABLE or android.app.PendingIntent.FLAG_UPDATE_CURRENT,
        )
        mediaSession = MediaSession.Builder(this, player)
            .setSessionActivity(sessionActivity)
            .setCallback(SessionCallback())
            .setCustomLayout(customLayout(liked = false))
            .build()
        // Свой малый значок вместо стандартного значка Media3 (монохромная пластинка);
        // «Не нравится» встаёт слева от «Назад», «Нравится» — справа от «Вперёд».
        setMediaNotificationProvider(
            LayoutNotificationProvider(this).apply {
                setSmallIcon(R.drawable.ic_stat_plastinka)
            },
        )
        // Сердце в шторке следит за лайком играющего трека: и при смене трека,
        // и когда лайк поставили в приложении (или в шторке).
        scope.launch {
            combine(QueueHolder.likedIds, QueueHolder.state) { liked, state ->
                state.currentTrack()?.id?.let { liked.contains(it) } ?: false
            }.distinctUntilChanged().collect { liked ->
                mainHandler.post { mediaSession?.setCustomLayout(customLayout(liked)) }
            }
        }
    }

    // ------------------------------------------------------ кнопки системного плеера

    /** Разрешает контроллерам наши команды и выполняет их. */
    private inner class SessionCallback : MediaSession.Callback {
        override fun onConnect(
            session: MediaSession,
            controller: MediaSession.ControllerInfo,
        ): MediaSession.ConnectionResult {
            // К командам по умолчанию добавляем «Нравится» и «Не нравится».
            val commands = MediaSession.ConnectionResult.DEFAULT_SESSION_COMMANDS.buildUpon()
                .add(LIKE_COMMAND)
                .add(DISLIKE_COMMAND)
                .build()
            return MediaSession.ConnectionResult.AcceptedResultBuilder(session)
                .setAvailableSessionCommands(commands)
                .build()
        }

        override fun onCustomCommand(
            session: MediaSession,
            controller: MediaSession.ControllerInfo,
            customCommand: SessionCommand,
            args: android.os.Bundle,
        ): ListenableFuture<SessionResult> {
            // Колбэк идёт на главном потоке сессии, как и нужно плееру.
            when (customCommand.customAction) {
                ACTION_LIKE -> QueueHolder.currentTrack()?.let { TrackActions.toggleLike(app, scope, it.id) }
                ACTION_DISLIKE -> TrackActions.dislikeCurrent(app, scope, session.player)
                else -> return Futures.immediateFuture(SessionResult(SessionResult.RESULT_ERROR_NOT_SUPPORTED))
            }
            return Futures.immediateFuture(SessionResult(SessionResult.RESULT_SUCCESS))
        }
    }

    /**
     * Уведомление: «Не нравится» ставится первой (слева от «Назад»), остальные кнопки —
     * как у Media3 по умолчанию («Назад», пауза, «Вперёд», затем «Нравится»).
     */
    private class LayoutNotificationProvider(context: android.content.Context) :
        DefaultMediaNotificationProvider(context) {
        override fun getMediaButtons(
            session: MediaSession,
            playerCommands: Player.Commands,
            customLayout: ImmutableList<CommandButton>,
            showPauseButton: Boolean,
        ): ImmutableList<CommandButton> {
            val buttons = super.getMediaButtons(session, playerCommands, customLayout, showPauseButton)
            val dislike = buttons.filter { it.sessionCommand?.customAction == ACTION_DISLIKE }
            return ImmutableList.copyOf(dislike + buttons.filter { it !in dislike })
        }
    }

    private fun customLayout(liked: Boolean): List<CommandButton> = listOf(
        CommandButton.Builder(CommandButton.ICON_THUMB_DOWN_UNFILLED)
            .setSessionCommand(DISLIKE_COMMAND)
            .setCustomIconResId(R.drawable.ic_dislike)
            .setDisplayName("Не нравится")
            .build(),
        CommandButton.Builder(if (liked) CommandButton.ICON_HEART_FILLED else CommandButton.ICON_HEART_UNFILLED)
            .setSessionCommand(LIKE_COMMAND)
            .setCustomIconResId(if (liked) R.drawable.ic_heart_filled else R.drawable.ic_heart)
            .setDisplayName(if (liked) "Убрать из «Мне нравится»" else "Нравится")
            .build(),
    )

    // ------------------------------------------------------ качество

    /**
     * Смена качества действует сразу, как на десктопе: играющий трек
     * открывается заново с той же позиции, пауза сохраняется. Ссылка на поток
     * решается в [PlastinkaDataSource] при открытии, поэтому достаточно
     * `stop()` (плеер отпускает загруженное, список и позиция остаются) и
     * `prepare()`: заново открываются и играющий, и следующий трек. Это не
     * смена трека: `onMediaItemTransition` не вызывается, значит нет нового
     * прослушивания, записи в журнал и пропуска для волны; сообщение о начале
     * в `play-audio` не дублируется (`startedSent` уже стоит).
     * Только на главном потоке.
     */
    private fun restartForQuality(player: Player) {
        if (player.mediaItemCount == 0) return
        val state = player.playbackState
        if (state == Player.STATE_IDLE || state == Player.STATE_ENDED) return
        val track = QueueHolder.currentTrack()
        // Замена FckCensor — не поток Яндекса, качество её не касается; но следующий
        // трек могли уже подготовить, поэтому перезапускаем, если до конца недалеко.
        if (track != null && app.fckCensor.replacementUrl(track.id) != null) {
            val left = player.duration - player.currentPosition
            if (player.duration != C.TIME_UNSET && left > NEXT_PREPARED_WITHIN_MS) return
        }
        val index = player.currentMediaItemIndex
        val position = player.currentPosition.coerceAtLeast(0)
        player.stop()
        player.seekTo(index, position)
        player.prepare()
    }

    /**
     * Обновляет подписи треков очереди (`MediaMetadata`) по текущему состоянию FckCensor.
     * Звук и позиция не меняются: ExoPlayer обновляет элемент на месте, если не менялся адрес.
     * Только на главном потоке.
     */
    private fun refreshMetadata(player: Player) {
        val byId = QueueHolder.state.value.tracks.associateBy { it.id }
        for (i in 0 until player.mediaItemCount) {
            val old = player.getMediaItemAt(i)
            val track = byId[old.mediaId] ?: continue
            val fresh = mediaItemOf(track)
            val a = old.mediaMetadata
            val b = fresh.mediaMetadata
            if (a.title == b.title && a.artist == b.artist && a.artworkUri == b.artworkUri) continue
            player.replaceMediaItem(i, fresh)
        }
    }

    // ------------------------------------------------------ Slopless

    private val mainHandler = android.os.Handler(android.os.Looper.getMainLooper())

    /** Сколько ИИ-треков подряд уже пропущено (только главный поток). */
    private var aiSkips = 0
    private var lastForward = true
    private var lastIndex = -1

    private fun toast(text: String) {
        mainHandler.post { android.widget.Toast.makeText(applicationContext, text, android.widget.Toast.LENGTH_SHORT).show() }
    }

    /**
     * Slopless на главном потоке: просит данные об исполнителях играющего и
     * следующего трека и, если играющий — ИИ-трек, пропускает его (как на
     * десктопе). Трек, включённый вручную, не пропускается. Выключенный
     * плагин ничего не делает.
     */
    private fun slopCheck(player: Player) {
        if (!Prefs.slopless) {
            aiSkips = 0
            return
        }
        val state = QueueHolder.state.value
        val track = state.currentTrack() ?: return
        app.slopless.wantTracks(listOfNotNull(track, state.tracks.getOrNull(state.current + 1)))
        if (!Prefs.slopAutoskip) return
        if (QueueHolder.chosenId == track.id || !app.slopless.isAi(track)) {
            aiSkips = 0
            return
        }
        val forward = lastForward
        val hasNeighbour = if (forward) player.hasNextMediaItem() else player.hasPreviousMediaItem()
        if (aiSkips >= MAX_AI_SKIPS || aiSkips >= state.tracks.size || !hasNeighbour) {
            // Волна сама догрузит треки: в ней просто ждём.
            if (state.isWave && !hasNeighbour && forward) return
            // Дальше одни ИИ-треки: оставляем этот на паузе и больше не трогаем.
            aiSkips = 0
            QueueHolder.chosenId = track.id
            player.pause()
            toast("Дальше только ИИ-треки: Slopless их пропускает")
            return
        }
        aiSkips++
        dislikeAi(track)
        if (forward) player.seekToNextMediaItem() else player.seekToPreviousMediaItem()
    }

    /**
     * «Не нравится» пропущенному ИИ-треку — запись в аккаунт, только после
     * согласия (переключатель включают через диалог) и при подтверждённой
     * подписке. Каждому треку не больше раза за сеанс; токен — только в заголовке.
     */
    private fun dislikeAi(track: Track) {
        if (!Prefs.slopless || !Prefs.slopDislike) return
        val uid = AccountHolder.uid ?: return
        val token = app.tokenStore.load() ?: return
        if (!app.slopless.markDisliked(track.id)) return
        val job = scope.launch {
            try {
                app.api.addDisliked(token, uid, track.id)
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                Log.w(TAG, "не нравится не поставлено (трек ${track.id}): ${e.message}")
                toast("Slopless: не удалось поставить «Не нравится»")
            }
        }
        AccountHolder.track(job)
    }

    // ------------------------------------------------------ play-audio

    /**
     * Трек, о котором сообщаем Яндексу (как `Tracker` на десктопе): сообщение о
     * начале (сыграно 0, один раз за трек) и о конце (сколько сыграно). Не
     * шлём о треках, которые не зазвучали, и о заменах FckCensor.
     */
    private class PlayReport(
        val trackId: String,
        val albumId: String?,
        val lengthSeconds: Int?,
        val playId: String,
        val startedMs: Long,
    ) {
        var startedSent = false
        var playedMs = 0L
        var resumedAt: Long? = null
    }

    private var report: PlayReport? = null

    private fun reportBegin() {
        val track = QueueHolder.currentTrack()
        // Замена FckCensor — не поток Яндекса: о ней не сообщаем.
        report = track?.takeIf { app.fckCensor.replacementUrl(it.id) == null }?.let {
            PlayReport(
                trackId = it.id,
                albumId = it.albumId,
                lengthSeconds = it.durationMs?.let { ms -> (ms / 1000).toInt() },
                playId = java.util.UUID.randomUUID().toString(),
                startedMs = System.currentTimeMillis(),
            )
        }
    }

    /** Звук пошёл: сообщение о начале (один раз за трек), дальше копится время игры. */
    private fun reportSounded() {
        val r = report ?: return
        if (r.resumedAt == null) r.resumedAt = System.currentTimeMillis()
        if (!r.startedSent) {
            r.startedSent = true
            sendReport(r, 0)
        }
    }

    private fun reportPaused() {
        val r = report ?: return
        r.resumedAt?.let { r.playedMs += System.currentTimeMillis() - it }
        r.resumedAt = null
    }

    /** Трек сменился или кончился: сообщение о сыгранном (доигран — вся длительность). */
    private fun reportFinish(ended: Boolean) {
        reportPaused()
        val r = report ?: return
        report = null
        if (!r.startedSent) return
        val length = r.lengthSeconds
        val played = if (ended && length != null) length else (r.playedMs / 1000).toInt()
        sendReport(r, if (length != null) played.coerceAtMost(length) else played)
    }

    /**
     * Отправка в фоне: без токена или аккаунта с Плюсом ничего не шлётся, ошибка
     * глотается (в лог — только код ответа и id трека), повторов нет.
     */
    private fun sendReport(r: PlayReport, playedSeconds: Int) {
        val token = app.tokenStore.load() ?: return
        val uid = AccountHolder.uid ?: return
        val job = scope.launch {
            try {
                app.api.playAudio(
                    token, uid, r.trackId, r.albumId, r.playId,
                    r.lengthSeconds, playedSeconds, r.startedMs,
                )
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                Log.w(TAG, "play-audio не отправлен (трек ${r.trackId}): ${e.message}")
            }
        }
        AccountHolder.track(job)
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
                        // Сессия кончилась: новая с нуля (с выбранными настройками), её треки догружаем.
                        val fresh = app.api.startWave(token, Prefs.waveSettings.seeds())
                        Wave.session = fresh.session
                        Wave.batchId = fresh.batchId
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
        // Плеером можно управлять только с главного потока, а догрузка идёт в фоне.
        mainHandler.post {
            QueueHolder.append(batch.tracks)
            player.addMediaItems(batch.tracks.map(::mediaItemOf))
            // «Не нравится» застало конец очереди: переходим к первому новому треку.
            if (Wave.advanceOnAppend && player.hasNextMediaItem()) {
                Wave.advanceOnAppend = false
                player.seekToNextMediaItem()
                player.play()
            }
        }
    }

    /** plastinka://track/<id> → сетевой адрес потока. */
    private fun resolveUri(uri: String): String? {
        val trackId = TrackUris.trackIdOf(uri) ?: return null
        return resolve(trackId)
    }

    /** FckCensor-замена → поток Яндекса. */
    private fun resolve(trackId: String): String? {
        val token = app.tokenStore.load() ?: return null
        app.fckCensor.replacementUrl(trackId)?.let {
            QualityHolder.clear(trackId)
            return it
        }
        return when (val result = app.api.streamUrlBlocking(token, trackId)) {
            is StreamResult.Url -> {
                QualityHolder.put(trackId, result.codec, result.kbps)
                result.url
            }
            else -> {
                QualityHolder.clear(trackId)
                // В лог — только тип результата: у Url внутри ссылка на поток.
                Log.w(TAG, "поток трека $trackId не получен: ${(result as? StreamResult.Error)?.message ?: result::class.simpleName}")
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
        Prefs.onSlopChanged = null
        Prefs.onQualityChanged = null
        Wave.requestTopUp = null
        mainHandler.removeCallbacksAndMessages(null)
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

        private const val ACTION_LIKE = "ru.plastinka.action.LIKE"
        private const val ACTION_DISLIKE = "ru.plastinka.action.DISLIKE"
        private val LIKE_COMMAND = SessionCommand(ACTION_LIKE, android.os.Bundle.EMPTY)
        private val DISLIKE_COMMAND = SessionCommand(ACTION_DISLIKE, android.os.Bundle.EMPTY)

        /** Сколько ИИ-треков подряд пропускать, прежде чем сдаться. */
        private const val MAX_AI_SKIPS = 50

        /** За сколько до конца трека плеер уже мог открыть следующий (буфер ExoPlayer ~50 с). */
        private const val NEXT_PREPARED_WITHIN_MS = 90_000L

        /** Вызывается после старта волны: запомнить сессию до первого трека. */
        fun waveStarted(session: String, batchId: String, from: String?) {
            Wave.session = session
            Wave.batchId = batchId
            Wave.from = from
            Wave.started = false
            Wave.dislikeSkip = false
            Wave.advanceOnAppend = false
        }

        /**
         * «Не нравится» в волне: следующий переход закроет трек как skip. `waitForNext` —
         * следующего трека ещё нет: ждём порцию и просим догрузку.
         */
        fun markDislike(waitForNext: Boolean) {
            Wave.dislikeSkip = true
            Wave.advanceOnAppend = waitForNext
            if (waitForNext) Wave.requestTopUp?.invoke()
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

/**
 * Источник данных плеера: `plastinka://track/<id>` решается в реальный адрес
 * (только сеть), дальше читает OkHttp-источник. Локальные файлы не читаются.
 */
class PlastinkaDataSource(
    private val http: okhttp3.OkHttpClient,
    private val resolve: (String) -> String?,
) : androidx.media3.datasource.DataSource {

    private var inner: androidx.media3.datasource.DataSource? = null

    override fun open(dataSpec: androidx.media3.datasource.DataSpec): Long {
        val target = when (dataSpec.uri.scheme) {
            "plastinka" -> resolve(dataSpec.uri.toString())
                ?: throw java.io.IOException("звук трека недоступен")
            else -> dataSpec.uri.toString()
        }
        // Звук только потоком из сети: ни file://, ни другие схемы не принимаются.
        if (!target.startsWith("http://") && !target.startsWith("https://")) {
            throw java.io.IOException("звук трека недоступен")
        }
        val source = OkHttpDataSource.Factory(http)
            .setUserAgent("Plastinka/0.1 Android")
            .createDataSource()
        inner = source
        return source.open(dataSpec.buildUpon().setUri(android.net.Uri.parse(target)).build())
    }

    override fun read(buffer: ByteArray, offset: Int, length: Int): Int =
        inner?.read(buffer, offset, length)
            ?: androidx.media3.common.C.RESULT_END_OF_INPUT

    override fun addTransferListener(transferListener: androidx.media3.datasource.TransferListener) {}

    override fun getUri(): android.net.Uri? = inner?.uri

    override fun close() {
        inner?.close()
        inner = null
    }
}
