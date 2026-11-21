package ru.plastinka.sync

/**
 * Маршрутизация кнопок плеера (§5.2): что у ведомого уходит ведущему командой, а что выполняется
 * на месте. Не зависит от Media3, поэтому проверяется на JVM; [ru.plastinka.playback.SyncForwardingPlayer]
 * лишь вызывает эти методы из команд сессии (экран, уведомление, медиакнопки, PiP).
 *
 * Каждый метод `true`, если кнопка обработана (ушла командой) и плееру её передавать не надо.
 * Ведущему и одинокому плееру ничего не перехватывается.
 */
class SyncButtonRouter(private val sync: () -> SyncPlayback?) {
    private fun slave(): SyncPlayback? = sync()?.takeIf { it.isSlave }

    /** «Следующий». */
    fun next(): Boolean = slave()?.command("next") != null

    /** «Предыдущий». */
    fun previous(): Boolean = slave()?.command("previous") != null

    /** Перемешивание: у ведомого режим меняется у ведущего. */
    fun shuffle(): Boolean = slave()?.command("shuffle") != null

    /** Повтор. */
    fun repeat(): Boolean = slave()?.command("repeat") != null

    /** «Не нравится». */
    fun dislike(): Boolean = slave()?.command("dislike") != null

    /**
     * Выбор записи очереди с номером [index] при текущей [current]: у ведомого другая запись —
     * это «Следующий» или «Предыдущий» ведущего. Тот же номер — обычная перемотка.
     */
    fun seekToIndex(current: Int, index: Int): Boolean {
        if (index == current) return false
        return if (index > current) next() else previous()
    }

    /** Новый список или трек: ведомый становится ведущим, плееру команду передаём. */
    fun newList() {
        sync()?.becomeMaster()
    }
}
