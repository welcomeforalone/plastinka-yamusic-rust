package ru.plastinka.sync

import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * Шифрованная связь (§3.3): сообщения §4 в кадрах AES-256-GCM. Счётчики направлений
 * ведутся здесь; повтор или пропуск кадра не расшифруется и закрывает связь.
 * Отправлять и принимать можно из разных корутин; отправлять — из нескольких (порядок
 * счётчика и записи в сеть держит мьютекс). Приём — из одной.
 */
class SecureChannel(
    private val transport: FrameTransport,
    private val sendKey: ByteArray,
    private val recvKey: ByteArray,
) {
    private val writeLock = Mutex()
    private var sendCounter = 0L
    private var recvCounter = 0L

    /** Отправить сообщение. */
    suspend fun send(msg: SyncMessage) {
        val plain = SyncMessages.encode(msg)
        if (plain.size + SyncCrypto.TAG_LEN > Frames.MAX_LEN) {
            throw SyncException(SyncErrorCode.Frame, "Сообщение слишком большое")
        }
        writeLock.withLock {
            checkCounter(sendCounter)
            val frame = SyncCrypto.encrypt(sendKey, sendCounter, plain)
            transport.writeFrame(frame)
            sendCounter++
        }
    }

    /**
     * Принять следующее сообщение. `null` — сообщение расшифровалось, но неизвестно или
     * повреждено: его надо пропустить, связь не рвётся. Ошибка шифра — [SyncException].
     */
    suspend fun receive(): SyncMessage? {
        val frame = transport.readFrame()
        checkCounter(recvCounter)
        val plain = SyncCrypto.decrypt(recvKey, recvCounter, frame)
        recvCounter++
        return SyncMessages.parse(plain)
    }

    fun close() = transport.close()

    private fun checkCounter(c: Long) {
        if (c >= MAX_FRAMES) throw SyncException(SyncErrorCode.Crypto, "Исчерпан счётчик кадров")
    }

    private companion object {
        const val MAX_FRAMES = 1L shl 32
    }
}
