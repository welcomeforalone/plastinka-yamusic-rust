package ru.plastinka.playback

import kotlinx.coroutines.Job
import java.util.concurrent.ConcurrentLinkedQueue

/**
 * Аккаунт с подтверждённым Плюсом для сообщений о прослушивании. `uid`
 * ставится только после успешной проверки подписки (правило 2) и сбрасывается
 * при выходе: без него `play-audio` не шлётся. Отложенные отправки при выходе
 * отменяются.
 */
object AccountHolder {

    @Volatile
    var uid: Long? = null
        private set

    private val jobs = ConcurrentLinkedQueue<Job>()

    /** Подписка (служба «Слушать вместе»): аккаунт с Плюсом появился или пропал. */
    @Volatile
    var onChanged: (() -> Unit)? = null

    /** Подписка подтверждена: сообщения о прослушивании разрешены. */
    fun allow(uid: Long) {
        this.uid = uid
        onChanged?.invoke()
    }

    /** Выход или потеря подписки: отправка запрещена, незавершённые отправки отменены. */
    fun clear() {
        uid = null
        while (true) {
            jobs.poll()?.cancel() ?: break
        }
        onChanged?.invoke()
    }

    /** Запомнить фоновую отправку, чтобы её можно было отменить при выходе. */
    fun track(job: Job) {
        jobs.add(job)
        job.invokeOnCompletion { jobs.remove(job) }
    }
}
