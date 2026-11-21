package ru.plastinka.ui

import android.content.ComponentName
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.StateFlow
import ru.plastinka.data.Track
import ru.plastinka.playback.PlaybackService
import ru.plastinka.playback.QueueHolder
import ru.plastinka.sync.PlayRole
import ru.plastinka.sync.SyncPlayBridge

/** Обёртка над MediaController: запустить очередь, управлять воспроизведением. */
class PlayerController(val controller: MediaController) {

    /** Играет ли сейчас: меняется мгновенно по событию плеера, без опроса. */
    private val _isPlaying = MutableStateFlow(controller.isPlaying)
    val isPlaying: StateFlow<Boolean> = _isPlaying

    /**
     * Пользователь хочет звук: плеер должен играть (`playWhenReady`), пока ждёт сеть — тоже.
     * По нему значок паузы на кнопках Play и действие [toggle]: во время буферизации паузу
     * ставить можно. Остановленный плеер (ошибка, конец очереди) — «не хочет».
     */
    private val _playWhenReady = MutableStateFlow(wantsSound())
    val playWhenReady: StateFlow<Boolean> = _playWhenReady

    /** Звук ждёт данных (сеть): на кнопке Play крутится тонкий ободок. */
    private val _buffering = MutableStateFlow(isBuffering())
    val buffering: StateFlow<Boolean> = _buffering

    private fun wantsSound(): Boolean =
        controller.playWhenReady &&
            controller.playbackState != Player.STATE_IDLE &&
            controller.playbackState != Player.STATE_ENDED

    private fun isBuffering(): Boolean =
        controller.playWhenReady && controller.playbackState == Player.STATE_BUFFERING

    /**
     * Перемешивание и повтор: режимы MediaController. Когда это устройство слушает чужую очередь,
     * показываются режимы ведущего (свои у ведомого выключены, кнопки идут ведущему командами).
     */
    private val _shuffle = MutableStateFlow(shownShuffle())
    val shuffle: StateFlow<Boolean> = _shuffle
    private val _repeatMode = MutableStateFlow(shownRepeat())
    val repeatMode: StateFlow<Int> = _repeatMode

    private fun shownShuffle(): Boolean = SyncPlayBridge.ui.value.let {
        if (it.role == PlayRole.Slave) it.shuffle else controller.shuffleModeEnabled
    }

    private fun shownRepeat(): Int = SyncPlayBridge.ui.value.let {
        if (it.role == PlayRole.Slave) {
            when (it.repeat) {
                "all" -> Player.REPEAT_MODE_ALL
                "one" -> Player.REPEAT_MODE_ONE
                else -> Player.REPEAT_MODE_OFF
            }
        } else {
            controller.repeatMode
        }
    }

    private val modesScope = kotlinx.coroutines.CoroutineScope(
        kotlinx.coroutines.SupervisorJob() + kotlinx.coroutines.Dispatchers.Main.immediate,
    )

    init {
        controller.addListener(object : Player.Listener {
            override fun onIsPlayingChanged(isPlaying: Boolean) {
                _isPlaying.value = isPlaying
            }

            override fun onPlayWhenReadyChanged(playWhenReady: Boolean, reason: Int) {
                _playWhenReady.value = wantsSound()
                _buffering.value = isBuffering()
            }

            override fun onPlaybackStateChanged(playbackState: Int) {
                _playWhenReady.value = wantsSound()
                _buffering.value = isBuffering()
            }

            override fun onShuffleModeEnabledChanged(shuffleModeEnabled: Boolean) {
                _shuffle.value = shownShuffle()
            }

            override fun onRepeatModeChanged(repeatMode: Int) {
                _repeatMode.value = shownRepeat()
            }
        })
        // Режимы ведущего пришли или связь закрылась: перечитать показываемые значения.
        modesScope.launch {
            SyncPlayBridge.ui.collect {
                _shuffle.value = shownShuffle()
                _repeatMode.value = shownRepeat()
            }
        }
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

    /**
     * «Повторять трек» в волне: выкл ↔ один трек. Это тот же режим плеера, что видят
     * кнопка в полноэкранном плеере и чип на экране «Моя волна».
     */
    fun toggleWaveRepeat() {
        controller.repeatMode =
            if (controller.repeatMode == Player.REPEAT_MODE_ONE) Player.REPEAT_MODE_OFF else Player.REPEAT_MODE_ONE
    }

    /** Повтор списка пользователя, пока играет волна (в ней повтор свой): вернуть при выходе из волны. */
    private var listRepeat = Player.REPEAT_MODE_OFF

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
        // Волна не повторяет треки: одинаковые в первой порции отбрасываются (нажатый остаётся).
        var from = startIndex
        val tracks = if (isWave) {
            // Нажатый трек остаётся на своём месте, из его копий и прочих дублей — первый.
            val seen = hashSetOf(tracks.getOrNull(startIndex)?.id.orEmpty())
            val kept = ArrayList<Track>()
            tracks.forEachIndexed { i, t ->
                if (i == startIndex) {
                    from = kept.size
                    kept += t
                } else if (seen.add(t.id)) {
                    kept += t
                }
            }
            kept
        } else {
            tracks
        }
        val startIndex = from
        // Недоступные треки без звука и без копии в очередь не попадают; индекс пересчитывается:
        // стартуем с нажатого трека или с ближайшего играбельного после него. Недоступный трек,
        // у которого копию ещё не искали, остаётся: сервис найдёт её перед воспроизведением
        // (иначе трек, до которого не докрутили, выпал бы из очереди, хотя копия у него есть).
        val relink = ru.plastinka.data.Relink.current
        val queueable = { t: Track -> t.playable || relink?.worthQueueing(t) == true }
        val target = tracks.drop(startIndex).firstOrNull(queueable) ?: return
        val playable = tracks.filter(queueable)
        val index = playable.indexOfFirst { it === target }
        if (index < 0) return
        val wasWave = QueueHolder.state.value.isWave
        if (isWave) {
            // Волна идёт линейно и догружается по индексу: перемешивание выключаем,
            // повтор трека начинается выключенным (повтор списка запоминаем до выхода).
            controller.shuffleModeEnabled = false
            if (!wasWave) listRepeat = controller.repeatMode
            controller.repeatMode = Player.REPEAT_MODE_OFF
        } else if (wasWave) {
            // Повтор одного трека был волновым: у списков остаётся прежний выбор пользователя.
            controller.repeatMode = listRepeat
        }
        QueueHolder.chosenId = if (isWave) null else target.id
        QueueHolder.set(playable, index, title, isWave, source)
        if (isWave) PlaybackService.waveQueued(playable.map { it.id })
        val items = playable.map(PlaybackService::mediaItemOf)
        controller.setMediaItems(items, index, 0)
        controller.prepare()
        controller.play()
    }

    /**
     * Play/пауза по `playWhenReady`: во время буферизации тоже можно поставить на паузу.
     * Плеер, остановленный ошибкой, перед Play готовится заново (позиция сохранена).
     */
    fun toggle() {
        if (_playWhenReady.value) {
            controller.pause()
        } else {
            if (controller.playbackState == Player.STATE_IDLE) controller.prepare()
            controller.play()
        }
    }

    /**
     * «Следующий»: на последнем треке волны ждём порцию и переходим к первому новому
     * (как на десктопе), в остальных случаях — обычный переход.
     */
    fun next() {
        if (QueueHolder.state.value.isWave && !controller.hasNextMediaItem()) {
            PlaybackService.requestNextInWave()
        } else {
            controller.seekToNextMediaItem()
        }
    }

    /** «Назад»: после 3 секунд трека — к его началу, раньше — к предыдущему (как на десктопе). */
    fun previous() = controller.seekToPrevious()
    fun seekTo(positionMs: Long) = controller.seekTo(positionMs)
    fun seekTo(index: Int, positionMs: Long) {
        // Выбор из очереди — осознанный: ИИ-трек не пропускаем.
        QueueHolder.chosenId = QueueHolder.state.value.tracks.getOrNull(index)?.id
        controller.seekTo(index, positionMs)
    }

    /**
     * Индекс трека, на который перейдёт «Следующий»: с учётом перемешивания и
     * повтора списка. `null` — дальше ничего нет.
     */
    fun nextIndex(): Int? {
        // Ведомый показывает «Далее» из `next` ведущего: в очереди для показа это вторая запись.
        val sync = SyncPlayBridge.ui.value
        if (sync.role == PlayRole.Slave) {
            val id = sync.next?.id ?: return null
            return QueueHolder.state.value.tracks.indexOfFirst { it.id == id }.takeIf { it >= 0 }
        }
        return controller.nextMediaItemIndex.takeIf { it != C.INDEX_UNSET && it != controller.currentMediaItemIndex }
    }

    /**
     * Индексы уже сыгранных треков перед текущим, ближайший первым (до `limit`):
     * по порядку перемешивания, если оно включено, иначе по списку.
     */
    fun previousIndices(limit: Int): List<Int> {
        val timeline = controller.currentTimeline
        val shuffle = controller.shuffleModeEnabled
        val out = ArrayList<Int>()
        var at = controller.currentMediaItemIndex
        while (out.size < limit) {
            at = timeline.getPreviousWindowIndex(at, Player.REPEAT_MODE_OFF, shuffle)
            if (at == C.INDEX_UNSET) break
            out += at
        }
        return out
    }

    val isPlayingNow: Boolean get() = controller.isPlaying
    val positionMs: Long get() = controller.currentPosition.coerceAtLeast(0)
    val durationMs: Long get() = controller.duration.coerceAtLeast(0)

    fun release() {
        modesScope.cancel()
        controller.release()
    }

    /** Остановить воспроизведение, очистить очередь плеера и отпустить контроллер. */
    fun stopAndRelease() {
        try {
            controller.stop()
            controller.clearMediaItems()
        } catch (_: Exception) {
            // Контроллер мог уже отключиться — освободить его всё равно нужно.
        }
        release()
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
