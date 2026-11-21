package ru.plastinka.sync

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue
import ru.plastinka.data.Track

/** Плеер-заглушка: состояние в полях, асинхронные колбэки ExoPlayer — по явному вызову [flush]. */
class FakePort : SyncPlayerPort {
    var track: Track? = null
    var playing = false
    var pos = 0L
    var next: Track? = null
    var shuffle = false
    var repeat = "off"
    var wave = false
    var restarts = 0
    val calls = mutableListOf<String>()
    val toasts = mutableListOf<String>()
    val likes = mutableListOf<Pair<String, Boolean>>()
    var core: SyncPlayback? = null
    private val callbacks = mutableListOf<String>()

    override fun snapshot() = PlayerSnap(track, playing, pos, false, shuffle, repeat, wave, next, restarts)

    override fun startRemote(track: Track, next: Track?, positionMs: Long, play: Boolean) {
        calls += "start:${track.id}@$positionMs:$play"
        this.track = track
        this.next = next
        pos = positionMs
        playing = play
        callbacks += "changed"
    }

    override fun updateNext(track: Track, next: Track?) {
        calls += "next:${next?.id}"
        this.next = next
    }

    override fun setPlayWhenReady(play: Boolean) {
        calls += "playing:$play"
        playing = play
        callbacks += "changed"
    }

    override fun seekTo(positionMs: Long) {
        calls += "seek:$positionMs"
        pos = positionMs
        callbacks += "seek"
    }

    override fun dropRemoteQueue(track: Track?) {
        calls += "drop"
        next = null
    }

    override fun perform(action: String) {
        calls += "perform:$action"
    }

    override fun toast(text: String) {
        toasts += text
    }

    override fun setLike(trackId: String, liked: Boolean) {
        likes += trackId to liked
    }

    /** Поздние колбэки плеера по уже применённым чужим изменениям. */
    fun flush() {
        val list = callbacks.toList()
        callbacks.clear()
        for (c in list) if (c == "seek") core?.onSeek() else core?.onPlayerChanged()
    }
}

class SyncPlaybackTest {
    private val idA = "1".repeat(32)
    private val idB = "2".repeat(32)
    private var now = 10_000L

    private inner class Dev(val id: String, val delay: Long = 0) {
        val port = FakePort()
        val outbox = mutableListOf<SyncMessage>()
        val core = SyncPlayback(id, port, { outbox += it; true }, { now }, { delay })

        init {
            port.core = core
        }

        fun sent(): List<SyncState> = outbox.filterIsInstance<SyncMessage.State>().map { it.state }
    }

    private fun peerInfo(id: String) = PeerInfo(id, "Устройство ${id.first()}", "android")

    /** Переносит сообщения между устройствами, пока они есть. */
    private fun pump(a: Dev, b: Dev) {
        repeat(20) {
            if (a.outbox.isEmpty() && b.outbox.isEmpty()) return
            val fromA = a.outbox.toList().also { a.outbox.clear() }
            val fromB = b.outbox.toList().also { b.outbox.clear() }
            fromA.forEach { b.core.onEvent(SyncEvent.Message(it)) }
            fromB.forEach { a.core.onEvent(SyncEvent.Message(it)) }
        }
    }

    /** A — инициатор, B — ответчик. */
    private fun link(a: Dev, b: Dev, join: Boolean = false) {
        a.core.onEvent(SyncEvent.Linked(peerInfo(b.id), Role.Initiator, join))
        b.core.onEvent(SyncEvent.Linked(peerInfo(a.id), Role.Responder, false))
        pump(a, b)
        a.port.flush()
        b.port.flush()
        pump(a, b)
    }

    private fun track(id: String) = sampleTrack(id)

    private fun playing(d: Dev, id: String = "t1", pos: Long = 5_000) {
        d.port.track = track(id)
        d.port.playing = true
        d.port.pos = pos
    }

    @Test
    fun trackOnlyOnOneSideMakesItMaster() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a)
        link(a, b)
        assertFalse(a.core.isSlave)
        assertTrue(b.core.isSlave)
        assertEquals("t1", b.port.track?.id)
        assertTrue(b.port.playing)
        assertEquals(PlayRole.Master, a.core.ui.value.role)
        assertEquals(PlayRole.Slave, b.core.ui.value.role)
        assertEquals("Устройство 1", b.core.ui.value.peerName)
        // Начальный обмен: ни одного лишнего state после него.
        assertTrue(a.outbox.isEmpty() && b.outbox.isEmpty())
    }

    @Test
    fun playingSideWinsWhenBothHaveTracks() {
        val a = Dev(idA)
        val b = Dev(idB)
        a.port.track = track("a1")
        a.port.playing = false
        playing(b, "b1")
        link(a, b)
        assertTrue(a.core.isSlave)
        assertEquals("b1", a.port.track?.id)
        assertFalse(b.core.isSlave)
    }

    @Test
    fun joinTakesResponderState() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a, "a1")
        playing(b, "b1")
        link(a, b, join = true)
        assertTrue(a.core.isSlave)
        assertEquals("b1", a.port.track?.id)
    }

    @Test
    fun nobodyPlayingMeansNoOwner() {
        val a = Dev(idA)
        val b = Dev(idB)
        link(a, b)
        assertFalse(a.core.isSlave)
        assertFalse(b.core.isSlave)
        // Первый, кто запустил трек, становится ведущим.
        playing(b, "b1")
        b.core.onPlayerChanged()
        pump(a, b)
        assertTrue(a.core.isSlave)
        assertEquals("b1", a.port.track?.id)
    }

    @Test
    fun slavePauseReachesMasterAndDoesNotEcho() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a)
        link(a, b)
        a.outbox.clear()
        b.port.playing = false
        b.core.onPlayerChanged()
        assertEquals(1, b.sent().size)
        assertEquals(idA, b.sent().first().owner)
        pump(a, b)
        assertFalse(a.port.playing)
        // Поздний колбэк плеера у ведущего после применения чужой паузы ничего не отправляет.
        a.port.flush()
        assertTrue(a.outbox.isEmpty())
        // И ведомый не получает эхо.
        b.port.flush()
        assertTrue(b.outbox.isEmpty())
        assertFalse(b.port.playing)
    }

    @Test
    fun slaveSeekAppliedOnlyAboveThreshold() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a, pos = 5_000)
        link(a, b)
        a.port.calls.clear()
        // Ведомый перемотал на 30 с: ведущий следует.
        b.port.pos = 30_000
        b.core.onSeek()
        pump(a, b)
        assertTrue(a.port.calls.any { it == "seek:30000" })
        a.port.flush()
        assertTrue(a.outbox.isEmpty())
        // Перемотка в пределах порога: ведущий не двигается.
        a.port.calls.clear()
        b.port.pos = a.port.pos + 100
        b.core.onSeek()
        pump(a, b)
        assertTrue(a.port.calls.none { it.startsWith("seek") })
    }

    @Test
    fun masterTrackChangeStartsTrackOnSlave() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a, "t1")
        link(a, b)
        now += 2_000
        a.port.track = track("t2")
        a.port.pos = 0
        a.port.next = track("t3")
        a.core.onPlayerChanged()
        now += 500
        pump(a, b)
        assertEquals("t2", b.port.track?.id)
        assertEquals("t3", b.port.next?.id)
        // Запуск с позиции «сейчас»: 0 + 500 мс прошло с отправки.
        assertTrue(b.port.calls.last { it.startsWith("start") }.startsWith("start:t2@500"), b.port.calls.toString())
        b.port.flush()
        assertTrue(b.outbox.isEmpty())
    }

    @Test
    fun staleStateIsIgnored() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a)
        link(a, b)
        a.outbox.clear()
        // Устаревшая ревизия от «ведущего»: rev 0 не новее применённой.
        val old = sampleState(rev = 0, by = idA, track = track("old"))
        b.core.onEvent(SyncEvent.Message(SyncMessage.State(old.copy(owner = idA))))
        assertEquals("t1", b.port.track?.id)
    }

    @Test
    fun slaveButtonsGoToMasterAsCommands() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a)
        link(a, b)
        assertTrue(b.core.command("next"))
        pump(a, b)
        assertTrue(a.port.calls.contains("perform:next"))
        assertFalse(a.core.command("next"))
        // Команда, пришедшая не ведущему, игнорируется.
        a.port.calls.clear()
        a.core.onEvent(SyncEvent.Message(SyncMessage.Command("next")))
        b.core.onEvent(SyncEvent.Message(SyncMessage.Command("previous")))
        assertTrue(a.port.calls.isNotEmpty())
        assertTrue(b.port.calls.none { it.startsWith("perform") })
    }

    @Test
    fun endOfTrackOnSlaveChangesNothing() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a)
        link(a, b)
        b.port.calls.clear()
        // Трек кончился у ведомого: playWhenReady не меняется, плеер лишь в STATE_ENDED.
        b.port.pos = 215_000
        b.core.onPlayerChanged()
        assertTrue(b.outbox.isEmpty())
        assertTrue(b.port.calls.isEmpty())
        assertEquals("t1", b.port.track?.id)
        assertTrue(b.core.isSlave)
        // Ведущий пошёл дальше: ведомый перешёл на новый трек.
        a.port.track = track("t2")
        a.port.pos = 0
        a.core.onPlayerChanged()
        pump(a, b)
        assertEquals("t2", b.port.track?.id)
    }

    @Test
    fun slaveTrackChangeWithoutTakeoverIsNotSent() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a)
        link(a, b)
        // Что-то поменяло трек у ведомого само (не выбор пользователя): ничего не уходит.
        b.port.track = track("other")
        b.core.onPlayerChanged()
        assertTrue(b.outbox.isEmpty())
        assertTrue(b.core.isSlave)
    }

    @Test
    fun slaveChoosingTrackTakesOver() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a, "t1")
        link(a, b)
        b.core.becomeMaster()
        b.port.track = track("mine")
        b.port.pos = 0
        b.core.onPlayerChanged()
        pump(a, b)
        assertFalse(b.core.isSlave)
        assertTrue(a.core.isSlave)
        assertEquals("mine", a.port.track?.id)
        assertEquals(PlayRole.Master, b.core.ui.value.role)
    }

    @Test
    fun takeoverWithSameTrackStillSendsState() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a, "t1")
        link(a, b)
        b.core.becomeMaster()
        b.core.onPlayerChanged()
        pump(a, b)
        assertFalse(b.core.isSlave)
        // Ведущий теперь B: его пауза доходит до A.
        b.port.playing = false
        b.core.onPlayerChanged()
        pump(a, b)
        assertFalse(a.port.playing)
    }

    @Test
    fun tickAdjustsSlaveAfterTwoDrifts() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a, pos = 10_000)
        link(a, b)
        b.port.calls.clear()
        // Ведомый отстаёт на секунду; ведущий шлёт tick раз в секунду.
        val drift = 1_000L
        repeat(3) {
            now += 1_000
            a.port.pos += 1_000
            b.port.pos += 1_000 - (if (it == 0) drift else 0)
            a.core.onSecond()
            pump(a, b)
            b.port.flush()
        }
        // Первое расхождение: пауза после запуска 1,5 с; второе и далее — перемотка на позицию ведущего.
        val seeks = b.port.calls.filter { it.startsWith("seek") }
        assertTrue(seeks.isNotEmpty(), b.port.calls.toString())
        b.port.flush()
        assertTrue(b.outbox.isEmpty(), "подстройка не должна сходить за перемотку пользователя")
    }

    @Test
    fun masterSendsTickOnlyWhilePlaying() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a)
        link(a, b)
        a.core.onSecond()
        assertEquals(1, a.outbox.filterIsInstance<SyncMessage.Tick>().size)
        a.outbox.clear()
        a.port.playing = false
        a.core.onSecond()
        a.core.onReady()
        assertTrue(a.outbox.isEmpty())
        // Ведомый tick не шлёт.
        b.core.onSecond()
        assertTrue(b.outbox.isEmpty())
    }

    @Test
    fun unlinkKeepsPlayingAndShowsMessage() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a)
        link(a, b)
        b.core.onEvent(SyncEvent.Unlinked(peerInfo(idA), "Связь с Устройство 1 разорвана", dropped = false, selfInitiated = false))
        assertEquals(listOf("Связь с Устройство 1 разорвана"), b.port.toasts)
        assertTrue(b.port.calls.contains("drop"))
        assertTrue(b.port.playing)
        assertEquals(PlayRole.None, b.core.ui.value.role)
        assertFalse(b.core.isSlave)
        // После разрыва свои изменения никуда не уходят.
        b.port.playing = false
        b.core.onPlayerChanged()
        assertTrue(b.outbox.isEmpty())
    }

    @Test
    fun likedIsRelayedWithoutEcho() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a)
        link(a, b)
        a.core.onLiked("t1", true)
        pump(a, b)
        assertEquals(listOf("t1" to true), b.port.likes)
        assertTrue(b.outbox.isEmpty())
    }

    @Test
    fun slaveDelayShiftsStartPosition() {
        val a = Dev(idA)
        val b = Dev(idB, delay = 80)
        playing(a, pos = 5_000)
        link(a, b)
        now += 1_000
        a.port.track = track("t2")
        a.port.pos = 0
        a.core.onPlayerChanged()
        pump(a, b)
        assertTrue(b.port.calls.last { it.startsWith("start") }.startsWith("start:t2@80:true"), b.port.calls.toString())
        // Позиция в собственном state ведомого — синхронная, без поправки.
        b.port.pos = 280
        b.port.playing = false
        b.core.onPlayerChanged()
        assertEquals(200L, b.sent().last().positionMs)
    }

    @Test
    fun restoredServiceReExchangesInitialState() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a, pos = 40_000)
        link(a, b)
        // Служба воспроизведения у B пересоздана: связь уже готова, обмен идёт заново.
        val b2 = Dev(idB)
        b2.port.track = null
        b2.core.onAlreadyLinked(peerInfo(idA), Role.Responder)
        pump(a, b2)
        assertTrue(b2.core.isSlave)
        assertEquals("t1", b2.port.track?.id)
        assertFalse(a.core.isSlave)
        // Ведущий не завёл бесконечный обмен.
        assertTrue(a.outbox.isEmpty() && b2.outbox.isEmpty())
    }

    @Test
    fun slaveShowsMastersModes() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a)
        a.port.shuffle = true
        a.port.repeat = "all"
        a.port.next = track("t9")
        link(a, b)
        val ui = b.core.ui.value
        assertTrue(ui.shuffle)
        assertEquals("all", ui.repeat)
        assertEquals("t9", ui.next?.id)
        assertEquals("t9", b.port.next?.id)
        // Ведомый, поставивший паузу, копирует режимы ведущего, а не свои.
        b.port.playing = false
        b.core.onPlayerChanged()
        val s = b.sent().last()
        assertTrue(s.shuffle)
        assertEquals("all", s.repeat)
        assertEquals("t9", s.next?.id)
        assertNull(a.core.ui.value.next)
    }

    @Test
    fun quietActionIsNotSent() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a)
        link(a, b)
        // Трек не включился у ведомого: пауза без отправки.
        b.core.quiet { b.port.playing = false }
        b.core.onPlayerChanged()
        assertTrue(b.outbox.isEmpty())
        assertFalse(a.port.calls.contains("playing:false"))
    }

    @Test
    fun masterChangesBeforeExchangeAreNotSentAsStates() {
        val a = Dev(idA)
        a.core.onEvent(SyncEvent.Linked(peerInfo(idB), Role.Initiator, false))
        a.outbox.clear()
        playing(a)
        a.core.onPlayerChanged()
        assertTrue(a.outbox.isEmpty())
    }

    @Test
    fun routerSendsSlaveButtonsAsCommands() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a)
        link(a, b)
        val rb = SyncButtonRouter { b.core }
        assertTrue(rb.next())
        assertTrue(rb.previous())
        assertTrue(rb.shuffle())
        assertTrue(rb.repeat())
        assertTrue(rb.dislike())
        // Выбор из очереди: другая запись — команда, та же — обычная перемотка плеером.
        assertTrue(rb.seekToIndex(0, 1))
        assertFalse(rb.seekToIndex(0, 0))
        pump(a, b)
        assertEquals(
            listOf("perform:next", "perform:previous", "perform:shuffle", "perform:repeat", "perform:dislike", "perform:next"),
            a.port.calls.filter { it.startsWith("perform") },
        )
        // Своё состояние ведомый при этом не менял и ничего не отправил, кроме команд.
        assertTrue(b.sent().isEmpty())
    }

    @Test
    fun routerLeavesMasterAndSoloAlone() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a)
        val soloRouter = SyncButtonRouter { a.core }
        assertFalse(soloRouter.next())
        link(a, b)
        val ra = SyncButtonRouter { a.core }
        assertFalse(ra.next())
        assertFalse(ra.previous())
        assertFalse(ra.shuffle())
        assertFalse(SyncButtonRouter { null }.next())
        assertTrue(a.outbox.none { it is SyncMessage.Command })
    }

    @Test
    fun routerNewListMakesSlaveMaster() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a)
        link(a, b)
        val rb = SyncButtonRouter { b.core }
        rb.newList()
        assertFalse(b.core.isSlave)
        // После выбора своего списка кнопки уже не уходят командами.
        assertFalse(rb.next())
    }

    @Test
    fun trackEndOnSlaveDoesNotTouchQueueOrSendState() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a)
        a.port.next = track("n1")
        link(a, b)
        b.port.calls.clear()
        // Плеер ведомого дошёл до конца: позиция в конце, playWhenReady прежний.
        b.port.pos = 215_000
        b.core.onPlayerChanged()
        b.core.onSecond()
        b.core.onReady()
        assertTrue(b.outbox.isEmpty())
        assertTrue(b.port.calls.isEmpty(), b.port.calls.toString())
        assertEquals("n1", b.port.next?.id)
    }

    private fun drop(a: Dev, b: Dev, dropped: Boolean = true) {
        val ua = SyncEvent.Unlinked(peerInfo(b.id), "x", dropped = dropped, selfInitiated = false)
        val ub = SyncEvent.Unlinked(peerInfo(a.id), "x", dropped = dropped, selfInitiated = false)
        a.core.onEvent(ua)
        b.core.onEvent(ub)
    }

    @Test
    fun masterStaysMasterAfterRestore() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a, "t1", 5_000)
        link(a, b)
        // У ведомого rev больше: его пауза и возобновление подняли счётчик.
        b.port.playing = false
        b.core.onPlayerChanged()
        pump(a, b)
        b.port.playing = true
        b.core.onPlayerChanged()
        pump(a, b)
        drop(a, b)
        // Пока связи нет, ведущий встал на паузу, а ведомый продолжает играть (по состоянию «играет только ведомый»).
        a.port.playing = false
        now += 30_000
        link(a, b)
        assertFalse(a.core.isSlave, "ведущий остался ведущим")
        assertTrue(b.core.isSlave)
        assertEquals("t1", b.port.track?.id)
    }

    @Test
    fun restoreIsForgottenAfterWindowOrBye() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a, "t1")
        link(a, b)
        drop(a, b)
        now += 601_000
        // Окно 10 минут прошло: обычные правила — играет только ведомый, он и становится ведущим.
        a.port.playing = false
        link(a, b)
        assertTrue(a.core.isSlave)

        val c = Dev(idA)
        val d = Dev(idB)
        playing(c, "t1")
        link(c, d)
        drop(c, d, dropped = false)
        c.port.playing = false
        link(c, d)
        assertTrue(c.core.isSlave, "после «Отключить» роли не помнятся")
    }

    @Test
    fun repeatRestartSendsStateAndSlaveRewinds() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a, "t1", 215_000)
        link(a, b)
        b.port.pos = 215_000
        // Трек кончился и стартовал заново по повтору: позиция 0, счётчик перезапусков вырос.
        now += 1_000
        a.port.pos = 0
        a.port.restarts++
        a.core.onPlayerChanged()
        assertEquals(1, a.sent().size)
        assertEquals(0L, a.sent().first().positionMs)
        pump(a, b)
        assertTrue(b.port.calls.contains("seek:0"), b.port.calls.toString())
        b.port.flush()
        assertTrue(b.outbox.isEmpty())
    }

    @Test
    fun firstTickFixesDriftAtOnce() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a, "t1", 0)
        link(a, b)
        // Ведущий сменил трек; ведомый стартовал по прикидке, а у ведущего звук пошёл на 700 мс позже.
        a.port.track = track("t2")
        a.port.pos = 0
        a.core.onPlayerChanged()
        pump(a, b)
        b.port.calls.clear()
        now += 300
        b.port.pos = 300
        // Через 300 мс (внутри паузы 1,5 с) у ведущего позиция только 0: первый tick исправляет сразу.
        a.port.pos = 0
        a.core.onReady()
        pump(a, b)
        assertEquals(listOf("seek:0"), b.port.calls.filter { it.startsWith("seek") })
        // Последующие tick-и подчиняются обычным правилам: один расходящийся не перематывает.
        b.port.calls.clear()
        now += 1_000
        a.port.pos = 1_000
        b.port.pos = 2_000
        a.core.onSecond()
        pump(a, b)
        assertTrue(b.port.calls.none { it.startsWith("seek") })
    }

    @Test
    fun firstTickWaitsForNonBufferingMaster() {
        val a = Dev(idA)
        val b = Dev(idB)
        playing(a, "t1", 0)
        link(a, b)
        a.port.track = track("t2")
        a.port.pos = 0
        a.core.onPlayerChanged()
        val rev = a.sent().last().rev
        pump(a, b)
        b.port.calls.clear()
        now += 300
        // Tick с buffering=true пропускается и «первым» не считается.
        b.core.onEvent(SyncEvent.Message(SyncMessage.Tick(rev, 0, now, true)))
        assertTrue(b.port.calls.none { it.startsWith("seek") })
        b.port.pos = 300
        b.core.onEvent(SyncEvent.Message(SyncMessage.Tick(rev, 0, now, false)))
        assertTrue(b.port.calls.any { it == "seek:0" }, b.port.calls.toString())
    }
}
