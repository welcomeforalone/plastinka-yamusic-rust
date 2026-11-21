package ru.plastinka.sync

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import ru.plastinka.data.Track

/** Снимок того, что сейчас делает плеер (без Android: порт отдаёт его из ExoPlayer и `QueueHolder`). */
data class PlayerSnap(
    val track: Track?,
    /** Должно звучать: `playWhenReady`, а не «звук идёт прямо сейчас». */
    val playing: Boolean,
    val positionMs: Long,
    /** Звук ждёт данных. */
    val buffering: Boolean = false,
    val shuffle: Boolean = false,
    /** `off`, `all`, `one`. */
    val repeat: String = "off",
    val wave: Boolean = false,
    /** Следующий трек очереди (для показа). */
    val next: Track? = null,
    /** Сколько раз трек перезапускался повтором («повторять трек»): новый перезапуск — новое `state`. */
    val restarts: Int = 0,
)

/**
 * Что нужно «Слушать вместе» от плеера. Все вызовы — из одного потока (главного). Реализация
 * в [ru.plastinka.playback.PlaybackService] работает с ExoPlayer, в тестах — заглушка.
 */
interface SyncPlayerPort {
    fun snapshot(): PlayerSnap

    /** Ведомый: включить чужой трек с позиции; очередь для показа — [track] и [next]. */
    fun startRemote(track: Track, next: Track?, positionMs: Long, play: Boolean)

    /** Ведомый: тот же трек, другое `next` (только очередь для показа). */
    fun updateNext(track: Track, next: Track?)
    fun setPlayWhenReady(play: Boolean)
    fun seekTo(positionMs: Long)

    /** Ведомый больше не слушает (связь закрыта): убрать из очереди показ чужого `next`. */
    fun dropRemoteQueue(track: Track?)

    /** Ведущий выполняет команду ведомого как свою кнопку: `next`, `previous`, `shuffle`, `repeat`, `dislike`. */
    fun perform(action: String)
    fun toast(text: String)

    /** Пришёл `liked`: только обновить сердце. */
    fun setLike(trackId: String, liked: Boolean)
}

enum class PlayRole { None, Master, Slave }

/** Общее для службы воспроизведения и экранов (один процесс): состояние «вместе» и доступ к логике. */
object SyncPlayBridge {
    val ui = MutableStateFlow(SyncPlayUi())

    /** Роль ведомого: у него режимы shuffle и repeat — мастера, а кнопки идут командами. */
    val isSlave: Boolean get() = ui.value.role == PlayRole.Slave
}

/** Для экранов: роль устройства, имя собеседника и чужие режимы (у ведомого их показываем вместо своих). */
data class SyncPlayUi(
    val role: PlayRole = PlayRole.None,
    val peerName: String? = null,
    val shuffle: Boolean = false,
    val repeat: String = "off",
    val wave: Boolean = false,
    val next: Track? = null,
)

/**
 * Воспроизведение вместе (§4.4, §5): начальный обмен, ведущий и ведомый. Чистая логика без
 * потоков и Android: события подаются в [onEvent], [onPlayerChanged], [onSeek], [onSecond],
 * [onReady] из одного потока. Свои изменения отличаются от применённых по сети сравнением
 * снимка плеера с базой: после применения чужого состояния база обновляется, поэтому поздние
 * асинхронные колбэки плеера ничего не отправляют.
 */
class SyncPlayback(
    private val selfId: String,
    private val port: SyncPlayerPort,
    private val send: (SyncMessage) -> Boolean,
    private val clock: () -> Long,
    private val delayMs: () -> Long = { 0L },
    private val peerNameOf: (PeerInfo) -> String = { it.name },
    /** Куда публиковать состояние для экранов (в приложении — общий [SyncPlayBridge.ui]). */
    private val _ui: MutableStateFlow<SyncPlayUi> = MutableStateFlow(SyncPlayUi()),
) {
    val ui: StateFlow<SyncPlayUi> = _ui.asStateFlow()

    private val counter = RevCounter(selfId)
    private val adjuster = SlaveAdjuster()

    private var peer: PeerInfo? = null
    private var role: Role? = null
    private var joined = false
    private var linked = false
    private var initialDone = false
    private var awaitingPeerInitial = false
    private var myInitial: SyncState? = null
    private var owner: String? = null
    private var applied: Rev? = null
    private var lastShared: SyncState? = null

    /** Счётчик своих перемоток: растёт, когда пользователь перемотал, но не когда мы применили чужую. */
    private var ownSeeks = 0
    private var pendingRemoteSeeks = 0
    private var pendingSince = 0L
    private var baseline: Baseline? = null
    private var applying = false
    private var takeover = false

    /** Ждём первый `tick` после нового трека или перемотки ведущего: он исправляет расхождение сразу. */
    private var awaitFirstTick = false

    /** Ведущий до обрыва связи (§4.4, восстановление): id, собеседник и время обрыва. */
    private var rememberedOwner: String? = null
    private var rememberedPeer: String? = null
    private var rememberedAt = 0L

    /** Ведущий для начального `state` этой связи (восстановление) или `null`. */
    private var restoredOwner: String? = null

    private data class Baseline(
        val trackId: String?,
        val playing: Boolean,
        val seeks: Int,
        val shuffle: Boolean,
        val repeat: String,
        val wave: Boolean,
        val nextId: String?,
        val restarts: Int,
    )

    private fun baselineOf(s: PlayerSnap) = Baseline(s.track?.id, s.playing, ownSeeks, s.shuffle, s.repeat, s.wave, s.next?.id, s.restarts)

    /** Связь готова, обмен завершён, и мы слушаем чужую очередь. */
    val isSlave: Boolean get() = linked && initialDone && owner != null && owner != selfId

    val isLinked: Boolean get() = linked

    // ---- События связи ----

    fun onEvent(e: SyncEvent) {
        when (e) {
            is SyncEvent.Linked -> onLinked(e)
            is SyncEvent.Unlinked -> onUnlinked(e)
            is SyncEvent.Message -> onMessage(e.msg)
            is SyncEvent.Paired -> Unit
        }
    }

    /** Связь уже была готова до подписки (служба воспроизведения пересоздана): считаем её восстановленной. */
    fun onAlreadyLinked(peer: PeerInfo, role: Role) = onLinked(SyncEvent.Linked(peer, role, join = false))

    private fun onLinked(e: SyncEvent.Linked) {
        peer = e.peer
        role = e.role
        joined = e.join && e.role == Role.Initiator
        linked = true
        initialDone = false
        owner = null
        applied = null
        lastShared = null
        awaitFirstTick = false
        adjuster.reset()
        // Восстановление (без `join`, тот же собеседник, не позже 10 минут): ведущий прежний.
        restoredOwner = if (!e.join && rememberedPeer == e.peer.id && clock() - rememberedAt <= RESTORE_WINDOW_MS) {
            rememberedOwner
        } else {
            null
        }
        rememberedOwner = null
        publish()
        sendInitial()
    }

    private fun onUnlinked(e: SyncEvent.Unlinked) {
        val wasSlave = isSlave
        // Оборвалась сама: ведущего помним для восстановления; «Отключить» и выход — забываем.
        if (e.dropped && !e.selfInitiated) {
            rememberedOwner = owner
            rememberedPeer = e.peer.id
            rememberedAt = clock()
        } else {
            rememberedOwner = null
        }
        linked = false
        initialDone = false
        awaitingPeerInitial = false
        owner = null
        applied = null
        lastShared = null
        myInitial = null
        takeover = false
        adjuster.reset()
        port.toast(e.text)
        if (wasSlave) port.dropRemoteQueue(port.snapshot().track)
        baseline = baselineOf(port.snapshot())
        publish()
    }

    private fun onMessage(m: SyncMessage) {
        if (!linked) return
        when (m) {
            is SyncMessage.State -> onState(m.state)
            is SyncMessage.Tick -> onTick(m)
            is SyncMessage.Command -> if (initialDone && owner == selfId) port.perform(m.action)
            is SyncMessage.Liked -> port.setLike(m.track, m.liked)
            else -> Unit
        }
    }

    // ---- Начальный обмен (§4.4) ----

    private fun sendInitial() {
        val snap = port.snapshot()
        val st = SyncState(
            rev = counter.current,
            by = selfId,
            owner = restoredOwner ?: selfId,
            track = snap.track,
            playing = snap.playing,
            positionMs = snap.positionMs,
            at = clock(),
            next = snap.next,
            shuffle = snap.shuffle,
            repeat = snap.repeat,
            wave = snap.wave,
            initial = true,
            join = joined,
        )
        myInitial = st
        awaitingPeerInitial = true
        send(SyncMessage.State(st))
    }

    private fun onInitial(remote: SyncState) {
        // Чужое начальное состояние без нашего (служба пересоздана у одной стороны): ответить своим.
        if (!awaitingPeerInitial) sendInitial()
        awaitingPeerInitial = false
        val local = myInitial ?: return
        val myRole = role ?: return
        counter.observe(maxOf(local.rev, remote.rev))
        applying = true
        try {
            when (SyncLogic.chooseInitial(local, remote, myRole)) {
                InitialChoice.Local -> {
                    owner = selfId
                    applied = Rev(local.rev, local.by)
                    lastShared = local
                }
                InitialChoice.Remote -> {
                    applied = Rev(remote.rev, remote.by)
                    lastShared = remote
                    initialDone = true
                    applyAsSlave(remote)
                }
                InitialChoice.None -> {
                    owner = null
                    applied = null
                }
            }
            initialDone = true
            baseline = baselineOf(port.snapshot())
        } finally {
            applying = false
        }
        publish()
    }

    // ---- Входящие состояния ----

    private fun onState(st: SyncState) {
        if (st.initial) {
            onInitial(st)
            return
        }
        if (!initialDone) return
        val incoming = Rev(st.rev, st.by)
        val newer = counter.isNewer(incoming, applied)
        counter.observe(st.rev)
        if (!newer) return
        applied = incoming
        lastShared = st
        applying = true
        try {
            if (st.owner == selfId) applyAsMaster(st) else applyAsSlave(st)
            baseline = baselineOf(port.snapshot())
        } finally {
            applying = false
        }
        publish()
    }

    /** Состояние ведомого (пауза, перемотка) у ведущего: применить, если расхождение больше порога. */
    private fun applyAsMaster(st: SyncState) {
        owner = selfId
        val snap = port.snapshot()
        if (st.track?.id != snap.track?.id) return
        if (snap.playing != st.playing) port.setPlayWhenReady(st.playing)
        val target = targetPosition(st, withDelay = false)
        if (SyncLogic.needsSeek(snap.positionMs, target)) remoteSeek(target)
    }

    private fun applyAsSlave(st: SyncState) {
        owner = st.owner
        val track = st.track
        val snap = port.snapshot()
        if (track == null) {
            if (snap.playing) port.setPlayWhenReady(false)
            return
        }
        val target = targetPosition(st, withDelay = true)
        if (snap.track?.id != track.id) {
            port.startRemote(track, st.next, target, st.playing)
            adjuster.noteOwnStartOrSeek(clock())
            awaitFirstTick = st.playing
            return
        }
        if (snap.next?.id != st.next?.id) port.updateNext(track, st.next)
        if (snap.playing != st.playing) port.setPlayWhenReady(st.playing)
        if (SyncLogic.needsSeek(snap.positionMs, target)) {
            remoteSeek(target)
            adjuster.noteOwnStartOrSeek(clock())
            awaitFirstTick = st.playing
        }
    }

    private fun targetPosition(st: SyncState, withDelay: Boolean): Long =
        // `at` уже в своих часах, смещение 0.
        SyncLogic.positionNow(st.positionMs, st.at, st.playing, 0L, clock(), if (withDelay && st.playing) delayMs() else 0L)

    private fun remoteSeek(positionMs: Long) {
        pendingRemoteSeeks++
        pendingSince = clock()
        port.seekTo(positionMs)
    }

    // ---- Подстройка по tick (§5.2) ----

    private fun onTick(t: SyncMessage.Tick) {
        if (!isSlave) return
        val rev = applied?.rev ?: return
        if (t.rev != rev) return
        val snap = port.snapshot()
        // Первый tick без буферизации у ведущего после нового трека или перемотки (§5.2).
        val first = awaitFirstTick && !t.buffering
        if (first) awaitFirstTick = false
        val target = adjuster.onTick(
            t, rev, 0L, clock(), snap.positionMs, snap.buffering, delayMs(),
            immediate = first && !snap.buffering,
        )
        if (target != null) {
            applying = true
            try {
                remoteSeek(target)
                baseline = baselineOf(port.snapshot())
            } finally {
                applying = false
            }
        }
    }

    // ---- Свои изменения ----

    /** Перемотка на плеере (причина SEEK): пользователь или мы сами? Свои чужие перемотки отсеиваются по счётчику. */
    fun onSeek() {
        if (pendingRemoteSeeks > 0 && clock() - pendingSince < PENDING_SEEK_MS) {
            pendingRemoteSeeks--
        } else {
            pendingRemoteSeeks = 0
            ownSeeks++
        }
        onPlayerChanged()
    }

    /** Плеер или очередь изменились. Сравнивает снимок с базой и при своём изменении шлёт `state`. */
    fun onPlayerChanged() {
        if (applying) return
        val snap = port.snapshot()
        if (!linked || !initialDone) {
            baseline = baselineOf(snap)
            return
        }
        val cur = baselineOf(snap)
        val base = baseline
        baseline = cur
        val force = takeover
        takeover = false
        if (base == null) return
        if (cur == base && !force) return
        if (isSlave) {
            // Свой трек у ведомого не отслеживаем (иначе ошибка или конец трека сошли бы за выбор).
            if (cur.playing != base.playing || cur.seeks != base.seeks) sendState(snap)
            return
        }
        if (snap.track == null && owner == null) return
        if (owner == null) owner = selfId
        sendState(snap)
        publish()
    }

    /** Звук пошёл после запуска или перемотки (STATE_READY): ведущий сразу шлёт `tick`. */
    fun onReady() {
        if (isMasterPlaying()) sendTick(port.snapshot())
    }

    /** Раз в секунду: ведущий шлёт `tick`, пока играет. */
    fun onSecond() {
        if (pendingRemoteSeeks > 0 && clock() - pendingSince >= PENDING_SEEK_MS) pendingRemoteSeeks = 0
        if (isMasterPlaying()) sendTick(port.snapshot())
    }

    private fun isMasterPlaying(): Boolean =
        linked && initialDone && owner == selfId && port.snapshot().let { it.track != null && it.playing }

    private fun sendTick(snap: PlayerSnap) {
        val rev = applied?.rev ?: counter.current
        send(SyncMessage.Tick(rev, snap.positionMs.coerceAtLeast(0), clock(), snap.buffering))
    }

    private fun sendState(snap: PlayerSnap) {
        val rev = counter.next()
        val slave = isSlave
        val shared = lastShared
        val st = SyncState(
            rev = rev.rev,
            by = selfId,
            owner = owner ?: selfId,
            track = snap.track,
            playing = snap.playing,
            // Позиция «синхронная»: у ведомого минус его поправка задержки.
            positionMs = (snap.positionMs - if (slave) delayMs() else 0L).coerceAtLeast(0),
            at = clock(),
            next = if (slave) shared?.next else snap.next,
            shuffle = if (slave) shared?.shuffle ?: false else snap.shuffle,
            repeat = if (slave) shared?.repeat ?: "off" else snap.repeat,
            wave = if (slave) shared?.wave ?: false else snap.wave,
        )
        applied = rev
        lastShared = st
        send(SyncMessage.State(st))
    }

    // ---- Кнопки ведомого и «Нравится» ----

    /** Ведомый выбрал свой трек или список, запустил волну: устройство становится ведущим. */
    fun becomeMaster() {
        if (!linked || !initialDone) return
        owner = selfId
        takeover = true
        publish()
    }

    /** Команда ведущему (следующий, предыдущий, shuffle, repeat, dislike). */
    fun command(action: String): Boolean {
        if (!isSlave) return false
        return send(SyncMessage.Command(action))
    }

    /** «Нравится» поставили или сняли у себя успешно: сообщить второму устройству. */
    fun onLiked(trackId: String, liked: Boolean) {
        if (linked) send(SyncMessage.Liked(trackId, liked))
    }

    /** Выполнить действие без отправки состояния (ошибка трека у ведомого и подобное). */
    fun quiet(block: () -> Unit) {
        applying = true
        try {
            block()
            baseline = baselineOf(port.snapshot())
        } finally {
            applying = false
        }
    }

    private fun publish() {
        val l = lastShared
        val slave = isSlave
        _ui.update {
            when {
                !linked -> SyncPlayUi()
                slave -> SyncPlayUi(
                    PlayRole.Slave,
                    peer?.let(peerNameOf),
                    l?.shuffle ?: false,
                    l?.repeat ?: "off",
                    l?.wave ?: false,
                    l?.next,
                )
                else -> SyncPlayUi(PlayRole.Master, peer?.let(peerNameOf))
            }
        }
    }

    companion object {
        private const val PENDING_SEEK_MS = 1_500L

        /** Сколько после обрыва помним ведущего (§5.3). */
        private const val RESTORE_WINDOW_MS = 600_000L

        /** Текущее воспроизведение вместе (одно на процесс; ставит служба воспроизведения). */
        @Volatile
        var current: SyncPlayback? = null
    }
}
