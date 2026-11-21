package ru.plastinka.sync

import java.io.DataInputStream
import java.io.EOFException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine

/** Что пошло не так в «Слушать вместе». Код нужен интерфейсу, текст — пользователю. */
enum class SyncErrorCode {
    /** Длина кадра вне 1..65536 или оборванный кадр. */
    Frame,

    /** Соединение закрыто или оборвано. */
    Closed,

    /** Собеседник не ответил вовремя. */
    Timeout,

    /** Нарушен порядок или формат сообщений. */
    Protocol,

    /** Не расшифровалось, неверный ключ, повтор кадра. */
    Crypto,

    /** Собеседник прислал `bye` (причина в [SyncException.reason]). */
    PeerBye,

    /** Собеседник не прошёл проверку `auth`; мы закрыли соединение с `bye auth`. */
    AuthFailed,

    /** Сопряжение не подтверждено нашей стороной (отказ или таймаут); отправлено `bye rejected`. */
    Rejected,

    /** Мы отказали собеседнику открытым `bye` (причина в [SyncException.reason]: version, account, busy). */
    Refused,
}

/** Ошибка протокола с понятным текстом по-русски. Её ловит вызывающий код, приложение она не роняет. */
class SyncException(
    val code: SyncErrorCode,
    message: String,
    /** Причина из `bye` (свой отказ или присланный собеседником). */
    val reason: String? = null,
    cause: Throwable? = null,
) : Exception(message, cause)

/**
 * Кадровый обмен: одно сообщение — один кадр. Длину кадра на проводе (4 байта, big-endian)
 * добавляет и снимает реализация; в вызовах — только содержимое.
 * Реализации: [StreamFrameTransport] (сокет) и пара каналов в памяти (тесты).
 */
interface FrameTransport {
    /** Следующий кадр. Неверная длина, обрыв — [SyncException]. */
    suspend fun readFrame(): ByteArray

    /** Отправить кадр длиной 1..65536 байт. */
    suspend fun writeFrame(content: ByteArray)

    /** Закрыть соединение; безопасно вызывать повторно. */
    fun close()
}

object Frames {
    const val MAX_LEN = 65536
    const val PREFIX_LEN = 4

    /** Длина вне 1..65536 — ошибка кадра. */
    fun checkLength(len: Long) {
        if (len < 1 || len > MAX_LEN) {
            throw SyncException(SyncErrorCode.Frame, "Некорректная длина кадра: $len")
        }
    }

    /** Кадр ровно как он уходит в сеть: 4 байта длины (big-endian) и содержимое. */
    fun withPrefix(content: ByteArray): ByteArray {
        checkLength(content.size.toLong())
        val out = ByteArray(PREFIX_LEN + content.size)
        val len = content.size
        out[0] = (len ushr 24).toByte()
        out[1] = (len ushr 16).toByte()
        out[2] = (len ushr 8).toByte()
        out[3] = len.toByte()
        content.copyInto(out, PREFIX_LEN)
        return out
    }
}

/**
 * Кадры поверх пары потоков (сокета). Ввод-вывод блокирующий, идёт на `Dispatchers.IO`.
 * Отмена чтения или записи закрывает соединение: заблокированный вызов тут же получает
 * ошибку и поток освобождается.
 */
class StreamFrameTransport(
    input: InputStream,
    private val output: OutputStream,
    private val onClose: () -> Unit = {},
) : FrameTransport {
    private val input = DataInputStream(input)
    private val ioScope = CoroutineScope(Dispatchers.IO + SupervisorJob())

    @Volatile
    private var closed = false

    override suspend fun readFrame(): ByteArray = io {
        val len = readLength()
        Frames.checkLength(len)
        val buf = ByteArray(len.toInt())
        input.readFully(buf)
        buf
    }

    override suspend fun writeFrame(content: ByteArray) {
        val frame = Frames.withPrefix(content)
        io {
            output.write(frame)
            output.flush()
        }
    }

    override fun close() {
        if (closed) return
        closed = true
        try {
            input.close()
        } catch (_: IOException) {
        }
        try {
            output.close()
        } catch (_: IOException) {
        }
        try {
            onClose()
        } catch (_: IOException) {
        }
    }

    private fun readLength(): Long {
        var v = 0L
        repeat(Frames.PREFIX_LEN) { v = (v shl 8) or input.readUnsignedByte().toLong() }
        return v
    }

    private suspend fun <T> io(block: () -> T): T = suspendCancellableCoroutine { cont ->
        cont.invokeOnCancellation { close() }
        ioScope.launch {
            val result = try {
                Result.success(block())
            } catch (e: SyncException) {
                Result.failure(e)
            } catch (e: EOFException) {
                Result.failure(SyncException(SyncErrorCode.Closed, "Соединение закрыто", cause = e))
            } catch (e: IOException) {
                Result.failure(SyncException(SyncErrorCode.Closed, "Обрыв соединения", cause = e))
            }
            result.fold({ cont.resume(it) }, { cont.resumeWithException(it) })
        }
    }
}
