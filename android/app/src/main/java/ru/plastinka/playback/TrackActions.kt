package ru.plastinka.playback

import android.util.Log
import androidx.media3.common.Player
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import ru.plastinka.PlastinkaApp
import ru.plastinka.data.YandexApi

/**
 * Действия над играющим треком, общие для приложения и системного плеера
 * (кнопки «Нравится» и «Не нравится» в шторке): одна логика, а не две копии.
 * Запись в аккаунт — только при подтверждённой подписке ([AccountHolder.uid]);
 * токен берётся из хранилища и уходит только в заголовок запроса.
 * Все функции вызываются на главном потоке.
 */
object TrackActions {

    private const val TAG = "TrackActions"

    private val mainHandler = android.os.Handler(android.os.Looper.getMainLooper())

    private fun toast(app: PlastinkaApp, text: String) {
        mainHandler.post { android.widget.Toast.makeText(app, text, android.widget.Toast.LENGTH_SHORT).show() }
    }

    /** Треки, у которых запрос «Нравится» ещё идёт: повторные нажатия по ним игнорируются. */
    private val busy: MutableSet<String> = java.util.concurrent.ConcurrentHashMap.newKeySet()

    /**
     * «Нравится» (как `toggle_like` на десктопе): сердце меняется сразу на целевое значение,
     * пока запрос идёт, повторные нажатия по треку игнорируются, ошибка возвращает сердце
     * как было и показывает причину. Токен не принят — событие повторного входа.
     */
    fun toggleLike(app: PlastinkaApp, scope: CoroutineScope, trackId: String) {
        val token = app.tokenStore.load() ?: return
        val uid = AccountHolder.uid ?: return
        if (!busy.add(trackId)) return
        val target = !QueueHolder.likedIds.value.contains(trackId)
        QueueHolder.setLike(trackId, target)
        scope.launch {
            try {
                app.api.setLike(token, uid, trackId, target)
            } catch (e: CancellationException) {
                QueueHolder.setLike(trackId, !target, rollback = true)
                throw e
            } catch (e: Exception) {
                QueueHolder.setLike(trackId, !target, rollback = true)
                if (e is YandexApi.UnauthorizedException) AuthEvents.needRelogin()
                val what = if (target) "поставить" else "снять"
                toast(app, "Не удалось $what «Нравится»: ${failureReason(e)}")
            } finally {
                busy.remove(trackId)
            }
        }
    }

    /** Причина для сообщения: без текста исключений, где может оказаться адрес запроса. */
    private fun failureReason(e: Exception): String = when {
        e is java.io.IOException -> "нет соединения с сервером"
        e is YandexApi.UnauthorizedException -> "вход больше не действует"
        e is YandexApi.HttpStatusException -> "сервер ответил ${e.code}"
        else -> e.javaClass.simpleName
    }

    /**
     * «Не нравится» играющему треку (запись в аккаунт по явному нажатию): сердце
     * убирается, трек пропускается, в «Не нравится» аккаунта уходит настоящий трек
     * Яндекса (замена FckCensor подменяет только звук). В волне переход закрывает
     * трек событием skip, а если следующего ещё нет — ждёт порцию; вне волны это
     * обычный переход к следующему треку очереди (обратная связь волны не шлётся).
     * Следующего нет и это не волна — пауза. Ошибка записи не мешает пропуску:
     * пользователь видит сообщение. Отменить «Не нравится» нельзя (как на десктопе).
     * `player` — плеер, которым управляет вызывающий (контроллер приложения или сам
     * ExoPlayer в сервисе); `null` — только запись.
     */
    fun dislikeCurrent(app: PlastinkaApp, scope: CoroutineScope, player: Player?) {
        val state = QueueHolder.state.value
        val track = state.currentTrack() ?: return
        val uid = AccountHolder.uid ?: return
        val token = app.tokenStore.load() ?: return
        QueueHolder.removeLike(track.id)
        if (player != null) {
            val hasNext = player.hasNextMediaItem()
            if (state.isWave) PlaybackService.markDislike(waitForNext = !hasNext)
            if (hasNext) player.seekToNextMediaItem() else player.pause()
        }
        val job = scope.launch {
            try {
                app.api.addDisliked(token, uid, track.id)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Log.w(TAG, "не нравится не поставлено (трек ${track.id}): ${e.message}")
                toast(app, "Не удалось поставить «Не нравится». Проверьте соединение.")
            }
        }
        AccountHolder.track(job)
    }
}
