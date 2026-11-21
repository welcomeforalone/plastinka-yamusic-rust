package ru.plastinka.sync

/**
 * Оценка сдвига часов собеседника (§4.2). Часы — любые монотонные миллисекунды
 * (на Android `SystemClock.elapsedRealtime()`); в логику их значения передаются числами,
 * поэтому класс работает и без Android. Потокобезопасен: пинги, ответы и чтение оценки идут из разных корутин.
 */
class ClockEstimator {
    private class Sample(val rtt: Long, val offset: Long)

    private val samples = ArrayDeque<Sample>()
    private var pingCounter = 0L

    /** Номер следующего `ping`; `t0` — свои часы в момент отправки. */
    @Synchronized
    fun nextPing(nowMs: Long): SyncMessage.Ping = SyncMessage.Ping(pingCounter++, nowMs)

    /** Ответ на чужой `ping`: `t1` — свои часы в момент ответа. */
    @Synchronized
    fun answer(ping: SyncMessage.Ping, nowMs: Long): SyncMessage.Pong = SyncMessage.Pong(ping.n, ping.t0, nowMs)

    /** Учесть `pong`, полученный в момент [nowMs] своих часов (`t2`). */
    @Synchronized
    fun onPong(pong: SyncMessage.Pong, nowMs: Long) {
        addSample(pong.t0, pong.t1, nowMs)
    }

    /** Замер по трём числам §4.2: `rtt = t2 − t0`, `offset = t1 − (t0 + rtt / 2)`. */
    @Synchronized
    fun addSample(t0: Long, t1: Long, t2: Long) {
        val rtt = t2 - t0
        if (rtt < 0 || rtt > MAX_RTT_MS) return
        val offset = t1 - (t0 + rtt / 2)
        samples.addLast(Sample(rtt, offset))
        while (samples.size > MAX_SAMPLES) samples.removeFirst()
    }

    /** На сколько часы собеседника впереди своих; `null`, пока нет замеров. Берётся замер с наименьшим rtt (при равенстве — свежий). */
    val offsetMs: Long?
        @Synchronized get() {
            var best: Sample? = null
            for (s in samples) {
                if (best == null || s.rtt <= best.rtt) best = s
            }
            return best?.offset
        }

    /** Момент собеседника в своих часах: `local = remote − offset`. Без оценки — `null`. */
    @Synchronized
    fun toLocal(remoteMs: Long): Long? = offsetMs?.let { remoteMs - it }

    @Synchronized
    fun reset() {
        samples.clear()
        pingCounter = 0
    }

    companion object {
        const val MAX_SAMPLES = 8
        const val MAX_RTT_MS = 1000L
    }
}
