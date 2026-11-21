package ru.plastinka.playback

import androidx.media3.common.C
import androidx.media3.datasource.DataSourceException
import androidx.media3.datasource.HttpDataSource
import androidx.media3.exoplayer.upstream.DefaultLoadErrorHandlingPolicy
import androidx.media3.exoplayer.upstream.LoadErrorHandlingPolicy
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow

/**
 * Поток трека не получить по причине самого трека (недоступен, зашифрован, чужой хост,
 * нет ссылки, проблема файла замены). Повторять бесполезно: сервис покажет причину и
 * включит следующий трек. [reason] — по-русски, без адресов и токенов. [replacementFile] —
 * причина в своём файле замены FckCensor (удалён, доступ потерян).
 */
class TrackUnavailableException(
    val reason: String,
    val replacementFile: Boolean = false,
    /** Недоступный трек, копии которого в другом релизе нет: пропускается молча, как на десктопе. */
    val noCopy: Boolean = false,
) : java.io.IOException(reason)

/** Яндекс отказал в доступе (токен не принят): нужен повторный вход, повторять нечего. */
class StreamAuthException : java.io.IOException("вход устарел")

/** Результат решения адреса трека: ссылка либо ошибка с причиной (её увидит плеер). */
sealed interface ResolvedSound {
    data class Url(val url: String) : ResolvedSound
    data class Failed(val error: java.io.IOException) : ResolvedSound
}

/**
 * События, которые сервис воспроизведения отправляет приложению. Сейчас одно: токен не
 * принят, нужен повторный вход. Без подписчиков событие теряется (оно повторится с
 * ближайшей ошибкой).
 */
object AuthEvents {
    private val _relogin = MutableSharedFlow<Unit>(extraBufferCapacity = 1, onBufferOverflow = BufferOverflow.DROP_OLDEST)
    val relogin: SharedFlow<Unit> = _relogin

    fun needRelogin() {
        _relogin.tryEmit(Unit)
    }
}

/**
 * Повторы загрузки как на десктопе: паузы 1, 2, 4, 8, 8 с, пять попыток. Без повторов
 * остаются ошибки самого трека и авторизации. Протухшая ссылка (HTTP 401/403/404/410 от
 * хранилища) повторяется, но не больше трёх раз подряд: каждое открытие решает ссылку
 * заново в [PlastinkaDataSource]. 5xx, 408, 429 и сетевые сбои — обычные повторы.
 */
@androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)
class PlayLoadErrorPolicy : DefaultLoadErrorHandlingPolicy() {

    override fun getMinimumLoadableRetryCount(dataType: Int): Int = RETRIES

    override fun getRetryDelayMsFor(loadErrorInfo: LoadErrorHandlingPolicy.LoadErrorInfo): Long {
        val error = loadErrorInfo.exception
        val count = loadErrorInfo.errorCount
        // Решение базовой политики: «без повторов» для разбора, ненайденного файла и чтения за концом.
        val base = super.getRetryDelayMsFor(loadErrorInfo)
        return when {
            error is TrackUnavailableException || error is StreamAuthException -> C.TIME_UNSET
            error is HttpDataSource.InvalidResponseCodeException ->
                when {
                    DataSourceException.isCausedByPositionOutOfRange(error) -> C.TIME_UNSET
                    error.responseCode in STALE_LINK_CODES && count > MAX_RELINKS -> C.TIME_UNSET
                    else -> backoff(count)
                }
            base == C.TIME_UNSET -> C.TIME_UNSET
            else -> backoff(count)
        }
    }

    private fun backoff(errorCount: Int): Long =
        BACKOFF_MS.getOrElse(errorCount - 1) { BACKOFF_MS.last() }

    companion object {
        /** Сколько подряд сбоев загрузки терпим, прежде чем сообщить плееру об ошибке. */
        const val RETRIES = 5

        /** Сколько раз подряд повторять при протухшей ссылке. */
        const val MAX_RELINKS = 3

        /** Паузы перед повтором, мс (как `BACKOFF` на десктопе). */
        val BACKOFF_MS = longArrayOf(1_000, 2_000, 4_000, 8_000, 8_000)

        /** Ответы хранилища, по которым ссылка считается протухшей. */
        val STALE_LINK_CODES = setOf(401, 403, 404, 410)
    }
}
