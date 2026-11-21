package ru.plastinka.playback

import android.content.Intent
import android.util.Log
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.datasource.HttpDataSource
import androidx.media3.datasource.okhttp.OkHttpDataSource
import androidx.media3.exoplayer.DefaultLoadControl
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
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import ru.plastinka.Diagnostics
import ru.plastinka.PlastinkaApp
import ru.plastinka.R
import ru.plastinka.data.FckCensor
import ru.plastinka.data.FckSound
import ru.plastinka.data.Lookup
import ru.plastinka.data.Prefs
import ru.plastinka.data.StreamResult
import ru.plastinka.data.Track
import ru.plastinka.data.TrackUris
import ru.plastinka.sync.LinkState
import ru.plastinka.sync.PlayerSnap
import ru.plastinka.sync.SyncPlayBridge
import ru.plastinka.sync.SyncPlayback
import ru.plastinka.sync.SyncPlayerPort

/**
 * Фоновое воспроизведение: Media3-сессия с уведомлением, наушники, блокировка.
 * Адрес `plastinka://track/<id>` превращается в реальный URL при открытии:
 * сначала решает FckCensor (замена, найденная explicit-версия), потом поток Яндекса —
 * как на десктопе.
 * Для очереди-волны шлёт события rotor и догружает порции треков.
 */
@androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)
class PlaybackService : MediaSessionService() {

    private var mediaSession: MediaSession? = null

    /** «Слушать вместе»: логика ведущего и ведомого; события связи приходят на главном потоке. */
    private var sync: SyncPlayback? = null
    private val syncScope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val syncTicker = object : Runnable {
        override fun run() {
            sync?.onSecond()
            mainHandler.postDelayed(this, 1_000)
        }
    }

    /** Ведомый слушает чужой трек: отчёты Яндексу, волна и автопропуск Slopless не работают. */
    private val isSlave: Boolean get() = sync?.isSlave == true

    /** Сколько раз трек перезапускался повтором: часть снимка для «Слушать вместе». */
    private var syncRestarts = 0
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val app: PlastinkaApp get() = application as PlastinkaApp

    /** Состояние волны: сессия, последняя порция, для событий и догрузки. */
    private object Wave {
        @Volatile
        var session: String? = null

        @Volatile
        var batchId: String? = null

        @Volatile
        var from: String? = null

        @Volatile
        var started = false

        /** Прошлый трек (объект, а не номер: очередь могла смениться) и время его начала. */
        var previousTrack: Track? = null
        var startedAtMs: Long? = null

        /** Номер запуска волны: растёт при новом запуске и при уходе из волны. Порция старого запуска не дописывается. */
        @Volatile
        var gen = 0

        /** Один запрос порции в полёте. */
        val topUpMutex = Mutex()

        /** Номера треков, уже бывших в этой волне (до [SEEN_LIMIT]); только главный поток. */
        private val seen = LinkedHashSet<String>()

        /** `true`, если трек в этой волне ещё не встречался (и теперь запомнен). */
        fun markSeen(id: String): Boolean {
            if (!seen.add(id)) return false
            if (seen.size > SEEN_LIMIT) {
                val iterator = seen.iterator()
                if (iterator.hasNext()) {
                    iterator.next()
                    iterator.remove()
                }
            }
            return true
        }

        fun resetSeen(ids: List<String>) {
            seen.clear()
            ids.forEach { markSeen(it) }
        }

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
        Diagnostics.event("служба воспроизведения: запущена")

        // Источник данных: plastinka://track/<id> решается в сетевой адрес —
        // сначала FckCensor, потом поток Яндекса. Звук только потоком; единственное
        // исключение — свой файл из ручной замены FckCensor (чтение, без копирования).
        val source = androidx.media3.datasource.DataSource.Factory {
            PlastinkaDataSource(this, app.http, app.fckCensor::isReplacementFile) { uri -> resolveUri(uri) }
        }

        // Буфер: трек целиком в памяти, как на десктопе (60 с минимум, до 6 минут и 48 МБ).
        // Только оперативная память: ни SimpleCache, ни файлов (правило 1).
        val loadControl = DefaultLoadControl.Builder()
            .setBufferDurationsMs(60_000, 360_000, 2_500, 5_000)
            .setTargetBufferBytes(48 * 1024 * 1024)
            .setPrioritizeTimeOverSizeThresholds(false)
            .build()

        val player = ExoPlayer.Builder(this, PlastinkaRenderersFactory(this))
            .setLoadControl(loadControl)
            .setMediaSourceFactory(
                DefaultMediaSourceFactory(source).setLoadErrorHandlingPolicy(PlayLoadErrorPolicy()),
            )
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
                if (isPlaying) {
                    // Трек зазвучал: счётчик пропусков сломанных треков начинается заново.
                    skips = 0
                    silentSkips = 0
                    playedCurrent = true
                }
                if (isPlaying && pendingLog) logPlay()
                if (isPlaying) reportSounded() else reportPaused()
            }

            /** Трек, который уже зазвучал: когда сменится другим, он попадёт в «Недавно играли». */
            private var soundedTrack: Track? = null

            /** Очередь доиграна до конца: последний трек закрывается как «доигран». */
            override fun onPlaybackStateChanged(playbackState: Int) {
                if (playbackState == Player.STATE_ENDED) {
                    Diagnostics.event("очередь доиграна (волна: ${QueueHolder.state.value.isWave})")
                    reportFinish(ended = true)
                    rememberSounded()
                    // Очередь волны кончилась: ждём порцию и переходим к первому новому треку.
                    if (QueueHolder.state.value.isWave) waitForWaveBatch(player)
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

            override fun onPlayerError(error: PlaybackException) {
                // Всё восстановление синхронно: иначе Media3 успеет снять сервис с переднего плана.
                handleError(player, error)
            }

            override fun onMediaItemTransition(mediaItem: MediaItem?, reason: Int) {
                playedCurrent = false
                // Пока ждали порцию волны, трек сменился сам или выбран другой: ждать больше нечего.
                Wave.advanceOnAppend = false
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
                applyNormalization()
                wantAhead(player)
                reportBegin()
                if (player.isPlaying) {
                    logPlay()
                    reportSounded()
                }
                val now = System.currentTimeMillis()
                val previousStart = Wave.startedAtMs
                val previousTrack = Wave.previousTrack
                // «Не нравится» всегда закрывает трек как пропущенный, как на десктопе.
                val disliked = Wave.dislikeSkip
                Wave.dislikeSkip = false
                val repeated = reason == Player.MEDIA_ITEM_TRANSITION_REASON_REPEAT

                Wave.previousTrack = QueueHolder.currentTrack()
                Wave.startedAtMs = now

                if (!QueueHolder.state.value.isWave) {
                    // Ушли из волны: последний трек волны закрываем событием до выхода.
                    if (Wave.session != null && Wave.started) {
                        closeWaveTrack(previousTrack, previousStart, now, repeated = false, disliked = disliked)
                    }
                    Wave.session = null
                    Wave.started = false
                    Wave.gen += 1
                    return
                }

                // Начало сессии — один раз.
                if (!Wave.started && Wave.session != null) {
                    Wave.started = true
                    feedback("radioStarted", trackId = null, played = null)
                }

                closeWaveTrack(previousTrack, previousStart, now, repeated, disliked)
                QueueHolder.currentTrack()?.let { feedback("trackStarted", trackId = it) }
                topUp(player)
            }
        })

        // События обратной связи волны уходят строго по порядку: один потребитель.
        scope.launch {
            for (event in feedbackQueue) {
                app.api.waveFeedback(
                    event.token, event.session, event.batchId, event.from,
                    event.type, event.trackId, event.played,
                )
            }
        }

        // «Не нравится» на последнем треке просит догрузку у сервиса.
        Wave.requestTopUp = { mainHandler.post { topUp(player) } }

        // Смена качества: играющий трек открывается заново с той же позиции.
        Prefs.onQualityChanged = {
            mainHandler.post {
                mp3Retry.clear()
                restartForQuality(player)
            }
        }

        // Slopless: ответ пришёл, когда трек уже играет, или настройки сменились — проверить ещё раз.
        Prefs.onSlopChanged = { mainHandler.post { slopCheck(player) } }
        scope.launch {
            app.slopless.version.collect { mainHandler.post { slopCheck(player) } }
        }

        // FckCensor: решение о звуке принято (замена, explicit-версия) — громкость считается по тому,
        // что реально играет.
        scope.launch {
            FckCensor.playingVersion.collect { mainHandler.post { applyNormalization() } }
        }

        // FckCensor: замену трека поставили или убрали — играющий (и следующий) трек открывается заново.
        scope.launch {
            FckCensor.replaced.collect { id -> mainHandler.post { restartForReplacement(player, id) } }
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
        startSync(player)
        mediaSession = MediaSession.Builder(this, SyncForwardingPlayer(player) { sync })
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

    // ------------------------------------------------------ «Слушать вместе»

    /**
     * Подключает воспроизведение вместе к плееру: слушатель событий плеера, секундный `tick`,
     * подписка на события связи. Подписка идёт сразу при создании службы; если связь уже готова
     * (службу пересоздали), её считаем восстановленной и обмениваемся начальным `state` заново.
     */
    private fun startSync(player: ExoPlayer) {
        val core = SyncPlayback(
            selfId = Prefs.syncDeviceId,
            port = ExoSyncPort(player),
            send = { app.sync.service.send(it) },
            clock = { android.os.SystemClock.elapsedRealtime() },
            delayMs = { Prefs.syncDelayMs.toLong() },
            _ui = SyncPlayBridge.ui,
        )
        sync = core
        SyncPlayback.current = core
        player.addListener(object : Player.Listener {
            override fun onPlayWhenReadyChanged(playWhenReady: Boolean, reason: Int) = core.onPlayerChanged()
            override fun onMediaItemTransition(mediaItem: MediaItem?, reason: Int) {
                // «Повторять трек»: тот же трек с нуля; ведомому нужно новое `state`.
                if (reason == Player.MEDIA_ITEM_TRANSITION_REASON_REPEAT) syncRestarts++
                core.onPlayerChanged()
            }

            override fun onShuffleModeEnabledChanged(shuffleModeEnabled: Boolean) = core.onPlayerChanged()
            override fun onRepeatModeChanged(repeatMode: Int) = core.onPlayerChanged()
            override fun onPlaybackStateChanged(playbackState: Int) {
                core.onPlayerChanged()
                if (playbackState == Player.STATE_READY) core.onReady()
            }

            override fun onPositionDiscontinuity(
                oldPosition: Player.PositionInfo,
                newPosition: Player.PositionInfo,
                reason: Int,
            ) {
                if (reason == Player.DISCONTINUITY_REASON_SEEK) core.onSeek()
            }

            override fun onIsPlayingChanged(isPlaying: Boolean) {
                // Пока что-то играет, связь не гасится при уходе с экрана.
                app.sync.setPlaying(isPlaying)
            }
        })
        mainHandler.postDelayed(syncTicker, 1_000)
        // События связи: Linked, Unlinked, state, tick, command, liked.
        syncScope.launch(start = kotlinx.coroutines.CoroutineStart.UNDISPATCHED) {
            app.sync.service.events.collect { core.onEvent(it) }
        }
        (app.sync.service.ui.value.link as? LinkState.Linked)?.let { core.onAlreadyLinked(it.peer, it.role) }
        // Очередь изменилась (волна догрузила порцию, сменилось `next`): сверить со снимком.
        syncScope.launch { QueueHolder.state.collect { core.onPlayerChanged() } }
    }

    /** Порт плеера для [SyncPlayback]: ExoPlayer и очередь приложения. Главный поток. */
    private inner class ExoSyncPort(private val player: ExoPlayer) : SyncPlayerPort {
        override fun snapshot(): PlayerSnap {
            val queue = QueueHolder.state.value
            val nextIndex = player.nextMediaItemIndex.takeIf { it != C.INDEX_UNSET && it != player.currentMediaItemIndex }
            return PlayerSnap(
                track = queue.currentTrack(),
                playing = player.playWhenReady,
                positionMs = player.currentPosition.coerceAtLeast(0),
                buffering = player.playWhenReady && player.playbackState == Player.STATE_BUFFERING,
                shuffle = player.shuffleModeEnabled,
                repeat = when (player.repeatMode) {
                    Player.REPEAT_MODE_ALL -> "all"
                    Player.REPEAT_MODE_ONE -> "one"
                    else -> "off"
                },
                wave = queue.isWave,
                restarts = syncRestarts,
                // У ведомого одна запись в плеере; «Далее» — следующий трек очереди для показа.
                next = if (sync?.isSlave == true) {
                    queue.tracks.getOrNull(queue.current + 1)
                } else {
                    nextIndex?.let { queue.tracks.getOrNull(it) }
                },
            )
        }

        private fun title(): String {
            val peer = (app.sync.service.ui.value.link as? LinkState.Linked)?.peer?.name
            return if (peer != null) "Слушаю с $peer" else "Слушаю вместе"
        }

        override fun startRemote(track: Track, next: Track?, positionMs: Long, play: Boolean) {
            val title = title()
            QueueHolder.chosenId = track.id
            QueueHolder.set(listOfNotNull(track, next), 0, title, false, ru.plastinka.data.HistoryContext.Other(title))
            // У ведомого очереди нет: свои режимы выключены, показываются режимы ведущего.
            player.shuffleModeEnabled = false
            player.repeatMode = Player.REPEAT_MODE_OFF
            player.setMediaItem(mediaItemOf(track), positionMs)
            player.prepare()
            player.playWhenReady = play
        }

        override fun updateNext(track: Track, next: Track?) {
            val state = QueueHolder.state.value
            QueueHolder.set(listOfNotNull(track, next), 0, state.title, false, state.context)
        }

        override fun setPlayWhenReady(play: Boolean) {
            if (play && player.playbackState == Player.STATE_IDLE) player.prepare()
            player.playWhenReady = play
        }

        override fun seekTo(positionMs: Long) = player.seekTo(positionMs)

        override fun dropRemoteQueue(track: Track?) {
            val state = QueueHolder.state.value
            if (track != null) QueueHolder.set(listOf(track), 0, state.title, false, state.context)
        }

        override fun perform(action: String) {
            val wave = QueueHolder.state.value.isWave
            when (action) {
                "next" -> if (wave && !player.hasNextMediaItem()) requestNextInWave() else player.seekToNextMediaItem()
                "previous" -> player.seekToPrevious()
                "shuffle" -> if (!wave) player.shuffleModeEnabled = !player.shuffleModeEnabled
                "repeat" -> player.repeatMode = when {
                    wave -> if (player.repeatMode == Player.REPEAT_MODE_ONE) Player.REPEAT_MODE_OFF else Player.REPEAT_MODE_ONE
                    player.repeatMode == Player.REPEAT_MODE_OFF -> Player.REPEAT_MODE_ALL
                    player.repeatMode == Player.REPEAT_MODE_ALL -> Player.REPEAT_MODE_ONE
                    else -> Player.REPEAT_MODE_OFF
                }
                "dislike" -> TrackActions.dislikeCurrent(app, scope, player)
            }
        }

        override fun toast(text: String) = this@PlaybackService.toast(text)

        override fun setLike(trackId: String, liked: Boolean) = QueueHolder.setLike(trackId, liked)
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
                else -> return Futures.immediateFuture(SessionResult(androidx.media3.session.SessionError.ERROR_NOT_SUPPORTED))
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

    /** Нормализация громкости по тому, что реально играет у текущего трека (замена FckCensor или он сам). */
    private fun applyNormalization() {
        val track = QueueHolder.currentTrack()
        Normalizer.onTrack(
            track?.let { t ->
                // Играет копия из другого релиза: её громкость (известна, если искали в этом сеансе).
                val copy = app.relink.playingCopy(t)
                if (copy != null) app.relink.copyTrack(copy.id)?.loudness else app.fckCensor.loudnessOf(t)
            },
        )
    }

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
        // Прямая ссылка FckCensor — не поток Яндекса, качество её не касается; но следующий
        // трек могли уже подготовить, поэтому перезапускаем, если до конца недалеко.
        if (track != null && app.fckCensor.playsOutsideYandex(track)) {
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
     * Замену звука трека [trackId] поставили или убрали. Играющий трек начинается заново с
     * нуля, уже с новым звуком (как на десктопе); если это следующий трек, который плеер мог
     * успеть открыть со старым звуком, открываем заново, позиция играющего не меняется. Прочие
     * треки откроются с новым звуком сами. Только на главном потоке.
     */
    private fun restartForReplacement(player: Player, trackId: String) {
        if (player.mediaItemCount == 0) return
        val state = player.playbackState
        if (state == Player.STATE_IDLE || state == Player.STATE_ENDED) return
        val index = player.currentMediaItemIndex
        val nextId = if (index + 1 < player.mediaItemCount) player.getMediaItemAt(index + 1).mediaId else null
        when (trackId) {
            player.currentMediaItem?.mediaId -> {
                // Новый звук — новое прослушивание для Яндекса: прежнее закрываем, новое начинаем.
                reportFinish(ended = false)
                reportBegin()
                player.stop()
                player.seekTo(index, 0)
                player.prepare()
            }
            nextId -> {
                val position = player.currentPosition.coerceAtLeast(0)
                player.stop()
                player.seekTo(index, position)
                player.prepare()
            }
        }
    }

    // ------------------------------------------------------ ошибки воспроизведения

    /** Сколько сломанных треков подряд пропущено (как `skipped` на десктопе); сбрасывается, когда трек зазвучал. */
    private var skips = 0

    /**
     * Сколько треков без копии подряд пропущено молча (без сообщения и не в [skips]); предел — число
     * элементов очереди (перебрали всех). Сбрасывается вместе с [skips].
     */
    private var silentSkips = 0

    /** Текущий трек уже звучал: от этого зависит текст сообщения. */
    private var playedCurrent = false

    /** Треки, которые после ошибки формата открываем как MP3 (только память). */
    private val mp3Retry: MutableSet<String> = java.util.concurrent.ConcurrentHashMap.newKeySet()

    private enum class FailKind { Auth, Audio, Network, Format, Track, Other }

    /** Разбор ошибки плеера: что случилось и что сказать пользователю (текст без адресов и токенов). */
    private class Failure(
        val kind: FailKind,
        val reason: String,
        val replacementFile: Boolean = false,
        val noCopy: Boolean = false,
    )

    private fun classify(error: PlaybackException): Failure {
        val chain = generateSequence<Throwable>(error) { it.cause }.take(10).toList()
        if (chain.any { it is StreamAuthException }) return Failure(FailKind.Auth, "вход устарел")
        chain.filterIsInstance<TrackUnavailableException>().firstOrNull()?.let {
            return Failure(FailKind.Track, it.reason, it.replacementFile, it.noCopy)
        }
        return when (error.errorCode) {
            PlaybackException.ERROR_CODE_AUDIO_TRACK_INIT_FAILED,
            PlaybackException.ERROR_CODE_AUDIO_TRACK_WRITE_FAILED,
            PlaybackException.ERROR_CODE_AUDIO_TRACK_OFFLOAD_INIT_FAILED,
            PlaybackException.ERROR_CODE_AUDIO_TRACK_OFFLOAD_WRITE_FAILED ->
                Failure(FailKind.Audio, "не удалось открыть звук устройства")
            PlaybackException.ERROR_CODE_PARSING_CONTAINER_MALFORMED,
            PlaybackException.ERROR_CODE_PARSING_CONTAINER_UNSUPPORTED,
            PlaybackException.ERROR_CODE_PARSING_MANIFEST_MALFORMED,
            PlaybackException.ERROR_CODE_PARSING_MANIFEST_UNSUPPORTED,
            PlaybackException.ERROR_CODE_DECODER_INIT_FAILED,
            PlaybackException.ERROR_CODE_DECODER_QUERY_FAILED,
            PlaybackException.ERROR_CODE_DECODING_FAILED,
            PlaybackException.ERROR_CODE_DECODING_FORMAT_EXCEEDS_CAPABILITIES,
            PlaybackException.ERROR_CODE_DECODING_FORMAT_UNSUPPORTED ->
                Failure(FailKind.Format, "формат звука не читается")
            PlaybackException.ERROR_CODE_IO_FILE_NOT_FOUND,
            PlaybackException.ERROR_CODE_IO_NO_PERMISSION ->
                Failure(FailKind.Track, "файл не открывается")
            else -> {
                val http = chain.filterIsInstance<HttpDataSource.InvalidResponseCodeException>().firstOrNull()
                when {
                    http != null && http.responseCode in PlayLoadErrorPolicy.STALE_LINK_CODES ->
                        Failure(FailKind.Track, "ссылка на звук не открылась")
                    http != null -> Failure(FailKind.Network, "сервер не отвечает")
                    error.errorCode in PlaybackException.ERROR_CODE_IO_UNSPECIFIED..IO_ERRORS_END ->
                        Failure(FailKind.Network, "нет соединения с сетью")
                    else -> Failure(FailKind.Other, "ошибка воспроизведения")
                }
            }
        }
    }

    private fun nameOf(trackId: String?): String? =
        trackId?.let { id -> QueueHolder.state.value.tracks.firstOrNull { it.id == id }?.displayTitle }
            ?.takeIf { it.isNotBlank() }

    private fun toastLong(text: String) {
        mainHandler.post { android.widget.Toast.makeText(applicationContext, text, android.widget.Toast.LENGTH_LONG).show() }
    }

    /**
     * Ошибка плеера, как `fn failed` на десктопе. Только на главном потоке и синхронно: любая
     * отложенная починка даёт Media3 время снять сервис с переднего плана.
     * Ошибка трека (недоступен, формат не читается, ссылка не открылась) — сообщение и следующий
     * трек, но не больше [MAX_SKIPS] подряд; ошибка формата сначала даёт один тихий повтор как
     * MP3. Сеть после всех повторов — пауза на месте (Play продолжит с той же секунды).
     * Токен не принят — сообщение и событие [AuthEvents.relogin]. Нет звукового вывода — пауза.
     */
    private fun handleError(player: Player, error: PlaybackException) {
        val failure = classify(error)
        if (isSlave) {
            slaveFailure(player, error, failure)
            return
        }
        val current = player.currentMediaItemIndex
        // Ошибку подготовки следующего элемента Media3 сообщает, когда он уже стал текущим.
        val failedId = player.currentMediaItem?.mediaId
        // В журнал — только номер трека, код и тип: ссылок и текста исключений здесь нет.
        Log.w(TAG, "ошибка плеера: трек $failedId, ${error.errorCodeName}, ${failure.kind}")
        Diagnostics.event("ошибка плеера: трек $failedId, ${error.errorCodeName}, ${failure.kind}")
        when (failure.kind) {
            FailKind.Auth -> {
                player.pause()
                toastLong("Вход устарел — войдите заново")
                AuthEvents.needRelogin()
            }
            FailKind.Audio -> {
                player.pause()
                toastLong("Не удалось открыть звук устройства")
            }
            FailKind.Other -> {
                player.pause()
                toastLong(stopText(failedId, failure.reason))
            }
            FailKind.Network -> {
                holdInPlace(player)
                toastLong(stopText(failedId, failure.reason))
            }
            FailKind.Format ->
                if (tryMp3(failedId)) resumeCurrent(player) else skipBroken(player, failure, failedId)
            FailKind.Track -> skipBroken(player, failure, failedId)
        }
    }

    /**
     * Трек не включился у ведомого: сообщение, пауза без `state` ведущему, связь не рвётся, ждём
     * следующий `state`. Следующий трек не включаем: очереди у ведомого нет.
     */
    private fun slaveFailure(player: Player, error: PlaybackException, failure: Failure) {
        val failedId = player.currentMediaItem?.mediaId
        Log.w(TAG, "ошибка плеера у ведомого: трек $failedId, ${error.errorCodeName}, ${failure.kind}")
        Diagnostics.event("ошибка плеера у ведомого: трек $failedId, ${error.errorCodeName}, ${failure.kind}")
        if (failure.kind == FailKind.Auth) AuthEvents.needRelogin()
        val name = nameOf(failedId)?.let { "«$it»" } ?: "трек"
        toastLong("Не удалось включить $name на этом устройстве: ${failure.reason}")
        sync?.quiet { player.pause() }
    }

    /** «Прервался» для трека, который уже звучал, иначе «Не удалось включить». */
    private fun stopText(trackId: String?, reason: String): String {
        val name = nameOf(trackId)
        return if (playedCurrent) {
            "${name?.let { "«$it»" } ?: "Трек"} прервался: $reason"
        } else {
            "Не удалось включить ${name?.let { "«$it»" } ?: "трек"}: $reason"
        }
    }

    /** Пауза на месте: позиция сохраняется, Play откроет трек заново с той же секунды. */
    private fun holdInPlace(player: Player) {
        val position = player.currentPosition.coerceAtLeast(0)
        player.pause()
        player.seekTo(player.currentMediaItemIndex, position)
    }

    /** Открыть текущий трек заново с той же позиции (звук продолжается, если шёл). */
    private fun resumeCurrent(player: Player) {
        val position = player.currentPosition.coerceAtLeast(0)
        player.seekTo(player.currentMediaItemIndex, position)
        player.prepare()
    }

    /**
     * Один молчаливый повтор трека в MP3 после ошибки формата или декодера (как на десктопе).
     * Только если сейчас играл поток Яндекса не в MP3 и MP3 для этого трека ещё не пробовали.
     */
    private fun tryMp3(trackId: String?): Boolean {
        val id = trackId ?: return false
        val codec = QualityHolder.state.value[id]?.codec ?: return false
        if (codec.equals("mp3", ignoreCase = true)) return false
        if (mp3Retry.size > MP3_RETRY_LIMIT) mp3Retry.clear()
        if (!mp3Retry.add(id)) return false
        Log.i(TAG, "повтор трека $id как MP3 после ошибки формата")
        return true
    }

    /**
     * Сломанный (текущий) трек: сообщение и следующий. Следующего нет: в волне ждём порцию,
     * иначе стоим на месте.
     */
    private fun skipBroken(player: Player, failure: Failure, failedId: String?) {
        if (failure.noCopy) {
            skipWithoutCopy(player)
            return
        }
        val subject = nameOf(failedId)?.let { "«$it»" } ?: "Трек"
        val message = if (failure.replacementFile) "$subject: ${failure.reason}" else "$subject не играет: ${failure.reason}"
        val current = player.currentMediaItemIndex
        // Повтор одного трека пропуск не зацикливает: берём следующий по порядку списка.
        val mode = if (player.repeatMode == Player.REPEAT_MODE_ONE) Player.REPEAT_MODE_OFF else player.repeatMode
        val next = player.currentTimeline.getNextWindowIndex(current, mode, player.shuffleModeEnabled)
        val hasNext = next != C.INDEX_UNSET && next != current
        when {
            skips >= MAX_SKIPS -> giveUpOn(player, failedId, failure.reason)
            hasNext -> {
                skips++
                toastLong("$message. Включаем следующий трек")
                player.seekTo(next, C.TIME_UNSET)
                player.prepare()
                player.play()
            }
            QueueHolder.state.value.isWave -> {
                skips++
                toastLong("$message. Ждём новые треки")
                waitForWaveBatch(player)
            }
            else -> giveUpOn(player, failedId, failure.reason)
        }
    }

    /**
     * Трек без копии не играбелен: пропускаем молча (как на десктопе), из очереди не удаляем.
     * Все элементы очереди перебрали или дальше нет треков — пауза с одним сообщением.
     */
    private fun skipWithoutCopy(player: Player) {
        val current = player.currentMediaItemIndex
        val mode = if (player.repeatMode == Player.REPEAT_MODE_ONE) Player.REPEAT_MODE_OFF else player.repeatMode
        val next = player.currentTimeline.getNextWindowIndex(current, mode, player.shuffleModeEnabled)
        val hasNext = next != C.INDEX_UNSET && next != current
        val total = QueueHolder.state.value.tracks.size
        when {
            hasNext && silentSkips < total -> {
                silentSkips++
                player.seekTo(next, C.TIME_UNSET)
                player.prepare()
                player.play()
            }
            hasNext -> {
                toastLong("В очереди не осталось треков, которые можно включить")
                player.pause()
            }
            else -> {
                toastLong("Дальше в очереди нет треков, которые можно включить")
                player.pause()
            }
        }
    }

    private fun giveUpOn(player: Player, trackId: String?, reason: String) {
        toastLong(stopText(trackId, reason))
        player.pause()
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
        // У ведомого нет своей очереди: ничего не пропускаем и никому не ставим «Не нравится».
        if (isSlave) return
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
     * шлём о треках, которые не зазвучали, и о звуке не с Яндекса (прямая ссылка
     * FckCensor). Если вместо трека играет другой трек Яндекса (замена, explicit-версия),
     * отчёт про него, без альбома оригинала: так делает десктоп. Что играет, становится
     * известно, когда пошёл звук, поэтому [trackId] и [albumId] уточняются в [reportSounded].
     */
    private class PlayReport(
        val track: Track,
        var trackId: String,
        var albumId: String?,
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
        // Прослушивание ведомого Яндексу сообщает ведущий (§0 спеки): одно прослушивание — один отчёт.
        if (isSlave) {
            report = null
            return
        }
        val track = QueueHolder.currentTrack()
        report = track?.let {
            PlayReport(
                track = it,
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
        if (!r.startedSent) {
            // Звук пошёл, решение о нём принято: звук не с Яндекса — не сообщаем.
            val id = app.fckCensor.reportedId(r.track)
            if (id == null) {
                report = null
                return
            }
            r.trackId = id
            val copy = if (id == r.track.id) app.relink.playingCopy(r.track) else null
            if (copy != null) {
                // Играет копия из другого релиза: id и альбом копии (как `report_begin` на десктопе).
                r.trackId = copy.id
                r.albumId = copy.albumId
            } else if (id != r.track.id) {
                // У замены другой трек: альбом оригинала к нему не относится.
                r.albumId = null
            }
        }
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

    /** Событие обратной связи волны: данные сессии берутся в момент события, отправка идёт по очереди. */
    private class FeedbackEvent(
        val token: String,
        val session: String,
        val batchId: String,
        val from: String?,
        val type: String,
        val trackId: String?,
        val played: Double?,
    )

    private val feedbackQueue = Channel<FeedbackEvent>(Channel.UNLIMITED)

    /** Событие волны: ошибки молча глотаем, как на десктопе; порядок событий сохраняется. */
    private fun feedback(type: String, trackId: Track? = null, played: Double? = null) {
        val token = app.tokenStore.load() ?: return
        val session = Wave.session ?: return
        val batchId = Wave.batchId ?: return
        val track = trackId?.let(app.api::waveTrackId)
        feedbackQueue.trySend(FeedbackEvent(token, session, batchId, Wave.from, type, track, played))
    }

    /**
     * Закрытие прошлого трека волны: доиграл (>=90 %) — trackFinished, иначе skip.
     * Повтор трека («Повторять трек») — это доигранный до конца трек и новое начало
     * того же трека: обычная пара событий, без skip (как на десктопе).
     */
    private fun closeWaveTrack(
        previousTrack: Track?,
        previousStart: Long?,
        now: Long,
        repeated: Boolean,
        disliked: Boolean,
    ) {
        if (previousTrack == null || previousStart == null) return
        val length = previousTrack.durationMs?.let { it / 1000.0 }
        val played = if (repeated && length != null) length else (now - previousStart) / 1000.0
        val full = !disliked && (repeated || (length?.let { played >= 0.9 * it } ?: false))
        feedback(
            if (full) "trackFinished" else "skip",
            trackId = previousTrack,
            played = played,
        )
    }

    /** Выполнить на главном потоке и дождаться результата (плеером можно управлять только оттуда). */
    private suspend fun <T> onMain(block: () -> T): T {
        val done = CompletableDeferred<T>()
        mainHandler.post {
            try {
                done.complete(block())
            } catch (e: Exception) {
                done.completeExceptionally(e)
            }
        }
        return done.await()
    }

    /** Снимок состояния волны, снятый на главном потоке. */
    private class WaveSnapshot(
        val gen: Int,
        val state: QueueHolder.State,
        val waiting: Boolean,
        val session: String?,
    )

    /** Ждём порцию: переход к первому новому треку сделает дописывание порции. */
    private fun waitForWaveBatch(player: Player) {
        Wave.advanceOnAppend = true
        topUp(player)
    }

    /** Впереди меньше трёх треков (или ждём порцию) — просим порцию; сессия протухла — новую. */
    private fun topUp(player: Player) {
        if (!QueueHolder.state.value.isWave) return
        val token = app.tokenStore.load() ?: return
        scope.launch {
            // Одна догрузка в полёте: пока идёт эта, она сама перечитывает состояние очереди.
            if (!Wave.topUpMutex.tryLock()) return@launch
            try {
                fillWave(player, token)
            } finally {
                Wave.topUpMutex.unlock()
            }
        }
    }

    /**
     * Догрузка в цикле, пока впереди ≤ [TOP_UP_AHEAD] треков (или ждём порцию) и волна та же.
     * Состояние очереди читается на главном потоке уже под мьютексом. Сбой — повтор с паузами
     * 2, 4, 8, 16, 30 с; порция, целиком состоявшая из уже игравших треков, — сразу ещё запрос,
     * но не больше [MAX_EMPTY] раз подряд.
     */
    private suspend fun fillWave(player: Player, token: String) {
        var startGen: Int? = null
        var empty = 0
        var failures = 0
        while (true) {
            val snap = onMain {
                WaveSnapshot(Wave.gen, QueueHolder.state.value, Wave.advanceOnAppend, Wave.session)
            }
            if (!snap.state.isWave) return
            if (startGen == null) startGen = snap.gen
            if (snap.gen != startGen) return
            val remaining = snap.state.tracks.size - (snap.state.current + 1)
            if (remaining > TOP_UP_AHEAD && !snap.waiting) return
            val applied: Int
            try {
                val queueIds = snap.state.tracks.take(snap.state.current + 1).map(app.api::waveTrackId)
                val batch = app.api.waveTracks(token, snap.session.orEmpty(), queueIds)
                // Сессия кончилась: новая с нуля (с выбранными настройками), её треки догружаем.
                val ready = batch ?: app.api.startWave(token, Prefs.waveSettings.seeds())
                val renewed = batch == null
                applied = onMain { applyBatch(player, snap.gen, ready, renewed) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                // В журнал — только тип сбоя: текст исключения может содержать адрес запроса.
                Log.w(TAG, "волна: догрузка не удалась: ${e.javaClass.simpleName}")
                Diagnostics.event("волна: догрузка не удалась: ${e.javaClass.simpleName}")
                if (snap.waiting && failures == 0) toast("Не удалось загрузить волну, пробуем ещё раз")
                val pause = TOP_UP_RETRY_MS.getOrElse(failures) { TOP_UP_RETRY_MS.last() }
                failures++
                delay(pause)
                continue
            }
            failures = 0
            // Порция устарела (волну перезапустили или из неё ушли): ничего не дописано.
            if (applied < 0) return
            if (applied > 0) {
                empty = 0
                continue
            }
            empty++
            if (empty > MAX_EMPTY) {
                onMain { giveUpWaiting(player) }
                return
            }
        }
    }

    /**
     * Дописать порцию в очередь и плеер (главный поток). Возвращает, сколько треков дописано:
     * `-1` — порция устарела, `0` — все треки в ней уже были в этой волне (отсев как `Seen` на десктопе).
     */
    private fun applyBatch(player: Player, gen: Int, batch: ru.plastinka.data.YandexApi.WaveBatch, renewed: Boolean): Int {
        if (gen != Wave.gen || !QueueHolder.state.value.isWave) return -1
        if (renewed) {
            Wave.session = batch.session
            Wave.batchId = batch.batchId
            Wave.from = batch.from
            // Новая сессия: следующий переход трека снова шлёт radioStarted.
            Wave.started = false
            Log.i(TAG, "волна: сессия обновлена")
        } else {
            Wave.batchId = batch.batchId
            if (batch.from != null) Wave.from = batch.from
        }
        val fresh = batch.tracks.filter { Wave.markSeen(it.id) }
        Log.i(TAG, "волна: порция ${batch.tracks.size} треков, новых ${fresh.size}")
        if (fresh.isEmpty()) return 0
        val first = player.mediaItemCount
        QueueHolder.append(fresh)
        player.addMediaItems(fresh.map(::mediaItemOf))
        // Ждали порцию (последний трек, конец очереди, «Не нравится»): переходим к первому новому треку.
        if (Wave.advanceOnAppend) {
            Wave.advanceOnAppend = false
            player.seekTo(first, C.TIME_UNSET)
            player.prepare()
            player.play()
        }
        return fresh.size
    }

    /** Новых треков волна не нашла: ждать порцию дальше бесполезно. */
    private fun giveUpWaiting(player: Player) {
        if (!Wave.advanceOnAppend) return
        Wave.advanceOnAppend = false
        Diagnostics.event("волна: новых треков нет, ждать порцию перестали")
        toast("Волна пока не нашла новых треков. Попробуйте позже")
        val state = player.playbackState
        if (state == Player.STATE_IDLE || state == Player.STATE_ENDED) player.pause()
    }

    /** plastinka://track/<id> → сетевой адрес потока или причина, почему его нет. */
    private fun resolveUri(uri: String): ResolvedSound {
        val trackId = TrackUris.trackIdOf(uri)
            ?: return ResolvedSound.Failed(TrackUnavailableException("адрес трека не распознан"))
        return resolve(trackId)
    }

    /**
     * Звук трека очереди: решает FckCensor (прямая ссылка, другой трек Яндекса, найденная
     * explicit-версия — поиск ждёт не дольше 10 с), иначе поток самого трека. Значок качества
     * относится к треку очереди, но показывает кодек и битрейт того потока, который играет.
     */
    private fun resolve(trackId: String): ResolvedSound {
        val token = app.tokenStore.load() ?: return ResolvedSound.Failed(StreamAuthException())
        val queued = QueueHolder.state.value.tracks.firstOrNull { it.id == trackId }
        return when (val sound = app.fckCensor.resolveSound(app.api, token, trackId, queued)) {
            is FckSound.Direct -> {
                QualityHolder.clear(trackId)
                ResolvedSound.Url(sound.url)
            }
            is FckSound.File -> {
                // Свой файл: значка качества нет; если он недоступен, плеер получит ошибку
                // с причиной, и [handleError] скажет пользователю, что случилось.
                QualityHolder.clear(trackId)
                val problem = app.fckCensor.checkFile(sound.uri)
                if (problem != null) {
                    ResolvedSound.Failed(TrackUnavailableException(problem, replacementFile = true))
                } else {
                    ResolvedSound.Url(sound.uri)
                }
            }
            is FckSound.Yandex ->
                // FckCensor ничего не меняет, а трек недоступен: играет его копия из другого релиза.
                if (sound.id == trackId && queued != null && !queued.available) {
                    relinkedStream(token, queued)
                } else {
                    streamOf(token, trackId, sound.id)
                }
            FckSound.Pending -> streamOf(token, trackId, trackId)
        }
    }

    /**
     * Звук недоступного трека: копия из другого релиза. Известна — играем; нет — поиск, не дольше
     * 10 с (как explicit-поиск FckCensor). Копии нет — трек не играет (дальше обычный пропуск),
     * поиск не удался — сбой сети: пауза на месте с сообщением. В журнал — только номера.
     */
    private fun relinkedStream(token: String, track: Track): ResolvedSound {
        app.relink.copyOf(track.id)?.let { return streamOf(token, track.id, it.id) }
        return when (val found = app.relink.lookupBlocking(track)) {
            is Lookup.Found -> {
                Diagnostics.event("копия трека ${track.id}: ${found.copy.id}")
                // Громкость копии стала известна уже после начала трека.
                mainHandler.post { applyNormalization() }
                streamOf(token, track.id, found.copy.id)
            }
            Lookup.Missing -> {
                Diagnostics.event("копии трека ${track.id} нет")
                ResolvedSound.Failed(TrackUnavailableException("в другом релизе трек не нашёлся", noCopy = true))
            }
            Lookup.Failed -> {
                Diagnostics.event("копия трека ${track.id}: поиск не удался")
                ResolvedSound.Failed(java.io.IOException("нет связи с сервером"))
            }
        }
    }

    /** Следующие в очереди недоступные треки ставятся в поиск копий заранее (как `relink_wanted` на десктопе). */
    private fun wantAhead(player: Player) {
        val tracks = QueueHolder.state.value.tracks
        var index = player.currentMediaItemIndex
        repeat(RELINK_AHEAD) {
            index = player.currentTimeline.getNextWindowIndex(index, Player.REPEAT_MODE_OFF, player.shuffleModeEnabled)
            if (index == C.INDEX_UNSET) return
            tracks.getOrNull(index)?.let(app.relink::want)
        }
    }

    /** Поток трека Яндекса [streamId] для трека очереди [trackId]; после ошибки формата — MP3. */
    private fun streamOf(token: String, trackId: String, streamId: String): ResolvedSound {
        val result = app.api.streamUrlBlocking(token, streamId, mp3Only = mp3Retry.contains(trackId))
        if (result is StreamResult.Url) {
            QualityHolder.put(trackId, result.codec, result.kbps)
            return ResolvedSound.Url(result.url)
        }
        QualityHolder.clear(trackId)
        // В журнал — только тип результата: у Url внутри ссылка на поток.
        Log.w(TAG, "поток трека $streamId не получен: ${(result as? StreamResult.Error)?.message ?: result::class.simpleName}")
        return ResolvedSound.Failed(
            when (result) {
                StreamResult.Unavailable -> TrackUnavailableException("трек недоступен")
                StreamResult.Unauthorized -> StreamAuthException()
                // Сеть или сервер: обычный сбой загрузки, плеер повторит по политике.
                is StreamResult.Error ->
                    if (result.network) {
                        java.io.IOException("нет связи с сервером")
                    } else {
                        TrackUnavailableException("не удалось получить звук трека")
                    }
                is StreamResult.Url -> java.io.IOException("ссылка уже получена")
            },
        )
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaSession? =
        mediaSession

    override fun onTaskRemoved(rootIntent: Intent?) {
        val player = mediaSession?.player
        Diagnostics.event("приложение смахнули из списка (музыка играла: ${player?.playWhenReady == true})")
        if (player == null || !player.playWhenReady) {
            stopSelf()
        }
    }

    override fun onDestroy() {
        Diagnostics.event("служба воспроизведения: остановлена")
        Prefs.onSlopChanged = null
        Prefs.onQualityChanged = null
        Wave.requestTopUp = null
        mainHandler.removeCallbacksAndMessages(null)
        syncScope.cancel()
        SyncPlayback.current = null
        SyncPlayBridge.ui.value = ru.plastinka.sync.SyncPlayUi()
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

        /** Сколько следующих треков очереди готовим заранее (поиск копий). */
        private const val RELINK_AHEAD = 3

        private const val ACTION_LIKE = "ru.plastinka.action.LIKE"
        private const val ACTION_DISLIKE = "ru.plastinka.action.DISLIKE"
        private val LIKE_COMMAND = SessionCommand(ACTION_LIKE, android.os.Bundle.EMPTY)
        private val DISLIKE_COMMAND = SessionCommand(ACTION_DISLIKE, android.os.Bundle.EMPTY)

        /** Сколько ИИ-треков подряд пропускать, прежде чем сдаться. */
        private const val MAX_AI_SKIPS = 50

        /** За сколько до конца трека плеер уже мог открыть следующий (буфер плеера до 6 минут). */
        private const val NEXT_PREPARED_WITHIN_MS = 360_000L

        /** Сколько сломанных треков подряд пропускаем (как `MAX_SKIPS` на десктопе). */
        private const val MAX_SKIPS = 5

        /** Догружаем, когда впереди текущего трека осталось не больше стольких (как `TOP_UP_AHEAD`). */
        private const val TOP_UP_AHEAD = 2

        /** Сколько порций подряд, целиком состоящих из уже игравших треков, ещё просим (как `MAX_EMPTY`). */
        private const val MAX_EMPTY = 2

        /** Сколько последних номеров треков волны помним для отсева повторов (как `SEEN_LIMIT`). */
        private const val SEEN_LIMIT = 500

        /** Паузы перед повтором неудавшейся догрузки волны, мс. */
        private val TOP_UP_RETRY_MS = longArrayOf(2_000, 4_000, 8_000, 16_000, 30_000)

        /** Сколько треков держим в списке «повторить как MP3». */
        private const val MP3_RETRY_LIMIT = 100

        /** Верхний код группы ошибок ввода-вывода (2000..2999). */
        private const val IO_ERRORS_END = 2999

        /** Вызывается после старта волны: запомнить сессию до первого трека. */
        fun waveStarted(session: String, batchId: String, from: String?) {
            Wave.session = session
            Wave.batchId = batchId
            Wave.from = from
            Wave.started = false
            Wave.dislikeSkip = false
            Wave.advanceOnAppend = false
            // Новая волна: порции прежнего запуска не дописываются, повторы и закрытие трека начинаются с нуля.
            Wave.gen += 1
            Wave.previousTrack = null
            Wave.startedAtMs = null
            Wave.resetSeen(emptyList())
        }

        /** Первая порция волны встала в очередь: её треки уже «были» (отсев повторов). */
        fun waveQueued(trackIds: List<String>) {
            Wave.resetSeen(trackIds)
        }

        /**
         * «Следующий» на последнем треке волны: ждём порцию и переходим к первому новому треку
         * (без записи в аккаунт; событие о закрытии трека пошлёт обычный переход).
         */
        fun requestNextInWave() {
            Wave.advanceOnAppend = true
            Wave.requestTopUp?.invoke()
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
                        .setTitle(track.displayTitle.ifEmpty { "Трек ${track.id}" })
                        .setArtist(track.artistNames.ifEmpty { null })
                        .setAlbumTitle(track.albumTitle)
                        .setArtworkUri(track.coverUrl()?.let { android.net.Uri.parse(it) })
                        .build(),
                )
                .build()
    }
}

/**
 * Источник данных плеера: `plastinka://track/<id>` решается в реальный адрес. Звук идёт из сети
 * (OkHttp-источник). Единственное локальное чтение — свой файл из ручной замены FckCensor по
 * `content://`, и только адрес, который есть в сохранённых заменах ([isReplacementFile]); файл
 * читается системным `ContentDataSource` как есть, без копирования и записи.
 */
@androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)
class PlastinkaDataSource(
    private val context: android.content.Context,
    private val http: okhttp3.OkHttpClient,
    private val isReplacementFile: (String) -> Boolean,
    private val resolve: (String) -> ResolvedSound,
) : androidx.media3.datasource.DataSource {

    private var inner: androidx.media3.datasource.DataSource? = null

    override fun open(dataSpec: androidx.media3.datasource.DataSpec): Long {
        val target = when (dataSpec.uri.scheme) {
            "plastinka" -> when (val resolved = resolve(dataSpec.uri.toString())) {
                is ResolvedSound.Url -> resolved.url
                is ResolvedSound.Failed -> throw resolved.error
            }
            else -> dataSpec.uri.toString()
        }
        // Звук только потоком из сети. Исключение — файл замены из сохранённого списка
        // (`content://`); file:// и любые другие схемы не принимаются.
        val source: androidx.media3.datasource.DataSource = when {
            target.startsWith("http://") || target.startsWith("https://") ->
                OkHttpDataSource.Factory(http)
                    .setUserAgent("Plastinka/0.1 Android")
                    .createDataSource()
            target.startsWith("content://") && isReplacementFile(target) ->
                androidx.media3.datasource.ContentDataSource(context)
            else -> throw TrackUnavailableException("звук трека недоступен")
        }
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
