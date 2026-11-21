package ru.plastinka.sync

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class ClockTest {
    @Test
    fun offsetFormula() {
        val c = ClockEstimator()
        assertNull(c.offsetMs)
        // rtt = 100, offset = 5050 - (1000 + 50) = 4000.
        c.addSample(1000, 5050, 1100)
        assertEquals(4000L, c.offsetMs)
        assertEquals(1050L, c.toLocal(5050))
    }

    @Test
    fun smallestRttWins() {
        val c = ClockEstimator()
        c.addSample(0, 1000, 400) // rtt 400, offset 800
        c.addSample(1000, 2010, 1020) // rtt 20, offset 1000
        c.addSample(2000, 3500, 2300) // rtt 300, offset 1350
        assertEquals(1000L, c.offsetMs)
    }

    @Test
    fun slowSamplesAreDropped() {
        val c = ClockEstimator()
        c.addSample(0, 5, 1001)
        assertNull(c.offsetMs)
        c.addSample(0, 5, -3)
        assertNull(c.offsetMs)
        c.addSample(0, 500, 1000)
        assertEquals(0L, c.offsetMs)
    }

    @Test
    fun onlyLastEightSamples() {
        val c = ClockEstimator()
        c.addSample(0, 11, 2) // rtt 2, offset 10: лучший, но самый старый
        repeat(8) { c.addSample(100L * (it + 1), 100L * (it + 1) + 500 + 5, 100L * (it + 1) + 10) } // rtt 10, offset 500
        assertEquals(500L, c.offsetMs)
    }

    @Test
    fun pingPongHelpers() {
        val a = ClockEstimator()
        val b = ClockEstimator()
        val ping = a.nextPing(100)
        val pong = b.answer(ping, 5_000)
        a.onPong(pong, 110)
        // rtt 10, offset = 5000 - (100 + 5) = 4895.
        assertEquals(4895L, a.offsetMs)
        assertEquals(1L, a.nextPing(200).n)
    }
}

class LogicTest {
    private val idA = "a".repeat(32)
    private val idB = "b".repeat(32)

    private fun st(by: String, rev: Long, track: Boolean, playing: Boolean, join: Boolean = false) =
        sampleState(rev = rev, by = by, track = if (track) sampleTrack() else null, playing = playing, initial = true, join = join)

    /** Обе стороны должны прийти к одному общему состоянию. */
    private fun choose(init: SyncState, resp: SyncState): InitialChoice {
        val fromI = SyncLogic.chooseInitial(init, resp, Role.Initiator)
        val fromR = SyncLogic.chooseInitial(resp, init, Role.Responder)
        // Local у инициатора = Remote у ответчика и наоборот.
        val expected = when (fromI) {
            InitialChoice.Local -> InitialChoice.Remote
            InitialChoice.Remote -> InitialChoice.Local
            InitialChoice.None -> InitialChoice.None
        }
        assertEquals(expected, fromR)
        return fromI
    }

    @Test
    fun rule1OnlyOneHasTrack() {
        assertEquals(InitialChoice.Local, choose(st(idA, 1, true, false), st(idB, 9, false, false)))
        assertEquals(InitialChoice.Remote, choose(st(idA, 9, false, false), st(idB, 1, true, false)))
    }

    @Test
    fun rule2OnlyOnePlaying() {
        assertEquals(InitialChoice.Remote, choose(st(idA, 9, true, false), st(idB, 1, true, true)))
        assertEquals(InitialChoice.Local, choose(st(idA, 1, true, true), st(idB, 9, true, false)))
    }

    @Test
    fun rule3JoinTakesResponder() {
        // Инициатор нажал «Подключить»: побеждает ответчик, даже если у инициатора rev больше.
        assertEquals(InitialChoice.Remote, choose(st(idA, 99, true, true, join = true), st(idB, 1, true, true)))
    }

    @Test
    fun rule4LargerRevThenBy() {
        assertEquals(InitialChoice.Local, choose(st(idA, 5, true, true), st(idB, 4, true, true)))
        assertEquals(InitialChoice.Remote, choose(st(idA, 4, true, true), st(idB, 5, true, true)))
        // rev равны — больше строка by.
        assertEquals(InitialChoice.Remote, choose(st(idA, 5, true, true), st(idB, 5, true, true)))
        assertEquals(InitialChoice.Local, choose(st(idB, 5, true, true), st(idA, 5, true, true)))
    }

    @Test
    fun rule5NoTracks() {
        assertEquals(InitialChoice.None, choose(st(idA, 1, false, false), st(idB, 2, false, false)))
    }

    @Test
    fun revOrderingAndCounter() {
        assertTrue(Rev(2, "a") > Rev(1, "z"))
        assertTrue(Rev(1, "b") > Rev(1, "a"))
        val c = RevCounter(idA, 3)
        assertEquals(Rev(4, idA), c.next())
        c.observe(10)
        assertEquals(10L, c.current)
        c.observe(5)
        assertEquals(10L, c.current)
        assertEquals(Rev(11, idA), c.next())
        assertTrue(c.isNewer(Rev(1, "a"), null))
        assertFalse(c.isNewer(Rev(1, "a"), Rev(1, "a")))
        assertTrue(c.isNewer(Rev(1, "b"), Rev(1, "a")))
    }

    @Test
    fun positionNow() {
        // Играет: позиция + прошедшее. at собеседника 5000 = наши 1000 (offset 4000), сейчас 1500.
        assertEquals(10_500L, SyncLogic.positionNow(10_000, 5000, true, 4000, 1500))
        assertEquals(10_520L, SyncLogic.positionNow(10_000, 5000, true, 4000, 1500, delayMs = 20))
        assertEquals(10_000L, SyncLogic.positionNow(10_000, 5000, false, 4000, 1500))
        assertEquals(10_000L, SyncLogic.positionNow(10_000, 5000, true, null, 1500))
        assertEquals(0L, SyncLogic.positionNow(100, 5000, false, 0, 0, delayMs = -500))
    }

    @Test
    fun seekThreshold() {
        assertFalse(SyncLogic.needsSeek(1000, 1120))
        assertTrue(SyncLogic.needsSeek(1000, 1121))
        assertTrue(SyncLogic.needsSeek(1121, 1000))
    }

    private fun tick(rev: Long, pos: Long, at: Long, buffering: Boolean = false) = SyncMessage.Tick(rev, pos, at, buffering)

    @Test
    fun adjustNeedsTwoTicksInARow() {
        val a = SlaveAdjuster()
        // Часы одинаковы (offset 0). Цель 10000 при now = at.
        assertNull(a.onTick(tick(1, 10_000, 1000), 1, 0, 1000, 9_000, false))
        assertEquals(11_000L, a.onTick(tick(1, 11_000, 2000), 1, 0, 2000, 10_000, false))
    }

    @Test
    fun adjustResetsWhenBackInSync() {
        val a = SlaveAdjuster()
        assertNull(a.onTick(tick(1, 10_000, 1000), 1, 0, 1000, 9_000, false))
        assertNull(a.onTick(tick(1, 11_000, 2000), 1, 0, 2000, 11_050, false))
        assertNull(a.onTick(tick(1, 12_000, 3000), 1, 0, 3000, 11_000, false))
    }

    @Test
    fun adjustHonoursThrottleAndGrace() {
        val a = SlaveAdjuster()
        a.onTick(tick(1, 10_000, 1000), 1, 0, 1000, 0, false)
        assertEquals(11_000L, a.onTick(tick(1, 11_000, 2000), 1, 0, 2000, 0, false))
        // 1 с после своей перемотки — пауза 1,5 с.
        assertNull(a.onTick(tick(1, 12_000, 3000), 1, 0, 3000, 0, false))
        assertNull(a.onTick(tick(1, 13_000, 4000), 1, 0, 4000, 0, false)) // первый расходящийся после паузы
        // Второй расходящийся, но с прошлой подстройки 3 с (< 4 с).
        assertNull(a.onTick(tick(1, 14_000, 5000), 1, 0, 5000, 0, false))
        // Прошло 4 с.
        assertEquals(15_000L, a.onTick(tick(1, 15_000, 6000), 1, 0, 6000, 0, false))
    }

    @Test
    fun adjustIgnoresBufferingAndOtherRev() {
        val a = SlaveAdjuster()
        assertNull(a.onTick(tick(1, 10_000, 1000, buffering = true), 1, 0, 1000, 0, false))
        assertNull(a.onTick(tick(1, 11_000, 2000, buffering = true), 1, 0, 2000, 0, false))
        assertNull(a.onTick(tick(1, 12_000, 3000), 1, 0, 3000, 0, false))
        // Своя буферизация.
        assertNull(a.onTick(tick(1, 13_000, 4000), 1, 0, 4000, 0, true))
        assertNull(a.onTick(tick(1, 14_000, 5000), 1, 0, 5000, 0, true))
        // Чужая ревизия.
        assertNull(a.onTick(tick(2, 15_000, 6000), 1, 0, 6000, 0, false))
        assertNull(a.onTick(tick(2, 16_000, 7000), 1, 0, 7000, 0, false))
        // Нет оценки часов.
        assertNull(a.onTick(tick(1, 17_000, 8000), 1, null, 8000, 0, false))
    }

    @Test
    fun adjustSkipsGraceAfterOwnStart() {
        val a = SlaveAdjuster()
        a.noteOwnStartOrSeek(900)
        // Пауза 1,5 с кончается в 2400: tick в 1000 и 2000 не считаются.
        assertNull(a.onTick(tick(1, 10_000, 1000), 1, 0, 1000, 0, false))
        assertNull(a.onTick(tick(1, 11_000, 2000), 1, 0, 2000, 0, false))
        assertNull(a.onTick(tick(1, 12_000, 3000), 1, 0, 3000, 0, false)) // первый расходящийся
        assertEquals(13_000L, a.onTick(tick(1, 13_000, 4000), 1, 0, 4000, 0, false))
    }

    @Test
    fun adjustUsesClockOffsetAndDelay() {
        val a = SlaveAdjuster()
        // at собеседника 5000 при offset 4000 — наши 1000; сейчас 1200: цель = 10000 + 200 + 30.
        assertNull(a.onTick(tick(1, 10_000, 5000), 1, 4000, 1200, 0, false, 30))
        assertEquals(10_230L, a.onTick(tick(1, 10_000, 5000), 1, 4000, 1200, 0, false, 30))
    }

    @Test
    fun ruleRestoredOwnerKeepsMaster() {
        // Связь восстановилась: оба называют ведущим A. Побеждает A, даже если у B rev больше и играет только B.
        val a = sampleState(rev = 1, by = idA, track = sampleTrack(), playing = false, initial = true)
        val b = sampleState(rev = 9, by = idB, track = sampleTrack("b"), playing = true, initial = true).copy(owner = idA)
        assertEquals(InitialChoice.Local, SyncLogic.chooseInitial(a, b, Role.Initiator))
        assertEquals(InitialChoice.Remote, SyncLogic.chooseInitial(b, a, Role.Responder))
        // Ведущий B — наоборот.
        val b2 = b.copy(owner = idB)
        val a2 = a.copy(owner = idB)
        assertEquals(InitialChoice.Remote, SyncLogic.chooseInitial(a2, b2, Role.Initiator))
        assertEquals(InitialChoice.Local, SyncLogic.chooseInitial(b2, a2, Role.Responder))
        // Общий владелец — не из сторон: правило не действует (дальше по остальным).
        val a3 = a.copy(owner = "x".repeat(32))
        val b3 = b.copy(owner = "x".repeat(32))
        assertEquals(InitialChoice.Remote, SyncLogic.chooseInitial(a3, b3, Role.Initiator))
        // «Трек только у одной» важнее.
        val none = b.copy(owner = idA, track = null)
        assertEquals(InitialChoice.Local, SyncLogic.chooseInitial(a, none, Role.Initiator))
    }

    @Test
    fun firstTickCorrectsImmediately() {
        val adj = SlaveAdjuster()
        adj.noteOwnStartOrSeek(1_000)
        // Через 100 мс после своего запуска (пауза 1,5 с): обычный tick молчит, первый — перематывает сразу.
        assertNull(adj.onTick(SyncMessage.Tick(1, 20_000, 1_100, false), 1, 0, 1_100, 5_000, false))
        assertEquals(
            20_000L,
            adj.onTick(SyncMessage.Tick(1, 20_000, 1_100, false), 1, 0, 1_100, 5_000, false, immediate = true),
        )
        // В пределах порога ничего не делает.
        assertNull(adj.onTick(SyncMessage.Tick(1, 20_000, 1_200, false), 1, 0, 1_200, 20_100, false, immediate = true))
    }
}
