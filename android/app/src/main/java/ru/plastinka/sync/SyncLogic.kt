package ru.plastinka.sync

import kotlin.math.abs

/** Версия состояния: сначала `rev`, затем `by` как строка (§4.3). */
data class Rev(val rev: Long, val by: String) : Comparable<Rev> {
    override fun compareTo(other: Rev): Int {
        val c = rev.compareTo(other.rev)
        return if (c != 0) c else by.compareTo(other.by)
    }
}

/** Счётчик ревизий устройства (§4.3). */
class RevCounter(private val selfId: String, start: Long = 0) {
    var current: Long = start
        private set

    /** Своё изменение: `rev = текущий + 1`, `by = свой id`. */
    fun next(): Rev {
        current++
        return Rev(current, selfId)
    }

    /** Принятое: `текущий = max(текущий, rev)`. */
    fun observe(rev: Long) {
        if (rev > current) current = rev
    }

    /** Новее ли [incoming] последней применённой версии [applied]. */
    fun isNewer(incoming: Rev, applied: Rev?): Boolean = applied == null || incoming > applied
}

/** Кто выиграл начальный обмен (§4.4). */
enum class InitialChoice {
    /** Общее состояние — моё. */
    Local,

    /** Общее состояние — собеседника: принять его как ведомый. */
    Remote,

    /** Трека нет ни у кого: общего состояния и ведущего нет. */
    None,
}

object SyncLogic {
    /** Порог расхождения позиции, мс (§5.1, §5.2). */
    const val SEEK_THRESHOLD_MS = 120L

    /**
     * Выбор общего состояния при начальном обмене (§4.4, правила 1–5). Обе стороны зовут её
     * со своим состоянием [local], чужим [remote] и своей ролью [localRole] и приходят к
     * одному результату.
     */
    fun chooseInitial(local: SyncState, remote: SyncState, localRole: Role): InitialChoice {
        val hasLocal = local.track != null
        val hasRemote = remote.track != null
        // 1. Трек есть только у одной стороны.
        if (hasLocal && !hasRemote) return InitialChoice.Local
        if (!hasLocal && hasRemote) return InitialChoice.Remote
        // 6. Трека нет ни у кого.
        if (!hasLocal) return InitialChoice.None
        // 2. Связь восстановилась: у обоих один и тот же ведущий, и это одна из сторон.
        if (local.owner == remote.owner) {
            if (local.owner == local.by) return InitialChoice.Local
            if (local.owner == remote.by) return InitialChoice.Remote
        }
        // 3. Трек у обеих, играет только одна.
        if (local.playing != remote.playing) {
            return if (local.playing) InitialChoice.Local else InitialChoice.Remote
        }
        // 4. Инициатор нажал «Подключить»: берётся состояние ответчика.
        val initiatorJoin = if (localRole == Role.Initiator) local.join else remote.join
        if (initiatorJoin) {
            return if (localRole == Role.Responder) InitialChoice.Local else InitialChoice.Remote
        }
        // 5. Большая пара (rev, by).
        return if (Rev(local.rev, local.by) > Rev(remote.rev, remote.by)) InitialChoice.Local else InitialChoice.Remote
    }

    /**
     * Позиция «сейчас» по состоянию собеседника (§4.3): `positionMs + (сейчас − (at − offset))`,
     * если играет, иначе `positionMs`. [offsetMs] — из [ClockEstimator]; `null` (замеров ещё нет) —
     * без поправки на прошедшее время. [delayMs] — своя поправка задержки (−500…+500).
     */
    fun positionNow(
        positionMs: Long,
        at: Long,
        playing: Boolean,
        offsetMs: Long?,
        nowMs: Long,
        delayMs: Long = 0,
    ): Long {
        val elapsed = if (playing && offsetMs != null) nowMs - (at - offsetMs) else 0L
        return maxOf(0L, positionMs + elapsed + delayMs)
    }

    /** Нужна ли перемотка: расхождение больше порога. */
    fun needsSeek(ownPositionMs: Long, targetMs: Long): Boolean = abs(ownPositionMs - targetMs) > SEEK_THRESHOLD_MS
}

/**
 * Подстройка ведомого по `tick` (§5.2). Состояния у себя не хранит, кроме счётчика
 * подряд расходящихся `tick` и времени последней перемотки. Все моменты — свои часы.
 */
class SlaveAdjuster(
    private val thresholdMs: Long = SyncLogic.SEEK_THRESHOLD_MS,
    private val minIntervalMs: Long = 4_000,
    private val graceMs: Long = 1_500,
) {
    private var diverged = 0
    private var lastAdjustAt: Long? = null
    private var lastOwnChangeAt: Long? = null

    /** Мы сами запустили трек или перемотали (пользователь, `state`): 1,5 с не подстраиваемся. */
    fun noteOwnStartOrSeek(nowMs: Long) {
        lastOwnChangeAt = nowMs
        diverged = 0
    }

    /** Сброс при смене трека, ревизии или разрыве связи. */
    fun reset() {
        diverged = 0
        lastAdjustAt = null
        lastOwnChangeAt = null
    }

    /**
     * Обработать `tick`. Вернёт позицию (мс), на которую надо перемотать, или `null`.
     * [currentRev] — ревизия применённого состояния, [ownPositionMs] — своя позиция сейчас,
     * [ownBuffering] — плеер сам буферизуется, [delayMs] — своя поправка задержки.
     */
    fun onTick(
        tick: SyncMessage.Tick,
        currentRev: Long,
        offsetMs: Long?,
        nowMs: Long,
        ownPositionMs: Long,
        ownBuffering: Boolean,
        delayMs: Long = 0,
        /**
         * Первый `tick` без буферизации после нового трека или перемотки ведущего: расхождение
         * больше порога исправляется сразу, без второго `tick` и без пауз (§5.2).
         */
        immediate: Boolean = false,
    ): Long? {
        if (tick.rev != currentRev) return null
        if (tick.buffering || ownBuffering || offsetMs == null) {
            diverged = 0
            return null
        }
        if (immediate) {
            val target = SyncLogic.positionNow(tick.positionMs, tick.at, true, offsetMs, nowMs, delayMs)
            diverged = 0
            if (kotlin.math.abs(ownPositionMs - target) <= thresholdMs) return null
            lastAdjustAt = nowMs
            lastOwnChangeAt = nowMs
            return target
        }
        val sinceOwn = lastOwnChangeAt?.let { nowMs - it }
        if (sinceOwn != null && sinceOwn < graceMs) {
            diverged = 0
            return null
        }
        val target = SyncLogic.positionNow(tick.positionMs, tick.at, true, offsetMs, nowMs, delayMs)
        if (kotlin.math.abs(ownPositionMs - target) > thresholdMs) diverged++ else diverged = 0
        if (diverged < 2) return null
        val since = lastAdjustAt?.let { nowMs - it }
        if (since != null && since < minIntervalMs) return null
        lastAdjustAt = nowMs
        lastOwnChangeAt = nowMs
        diverged = 0
        return target
    }
}
