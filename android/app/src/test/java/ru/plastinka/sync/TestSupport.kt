package ru.plastinka.sync

import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.ClosedReceiveChannelException
import ru.plastinka.data.ArtistRef
import ru.plastinka.data.Loudness
import ru.plastinka.data.Track

fun hex(s: String): ByteArray {
    val clean = s.filter { !it.isWhitespace() }
    return ByteArray(clean.length / 2) { clean.substring(it * 2, it * 2 + 2).toInt(16).toByte() }
}

fun ByteArray.toHex(): String = joinToString("") { "%02x".format(it) }

/** Конец пары каналов в памяти: запись одного конца — чтение другого. */
class MemoryTransport(
    private val inbox: Channel<ByteArray>,
    private val outbox: Channel<ByteArray>,
) : FrameTransport {
    /** Всё, что этот конец отправил (содержимое кадров, до подмены). */
    val sent = mutableListOf<ByteArray>()

    /** Подмена исходящего кадра по его номеру (для тестов подмены). */
    var tamper: ((Int, ByteArray) -> ByteArray)? = null

    override suspend fun readFrame(): ByteArray = try {
        inbox.receive()
    } catch (e: ClosedReceiveChannelException) {
        throw SyncException(SyncErrorCode.Closed, "Соединение закрыто")
    }

    override suspend fun writeFrame(content: ByteArray) {
        Frames.checkLength(content.size.toLong())
        val index = sent.size
        val out = tamper?.invoke(index, content) ?: content
        sent.add(content)
        if (outbox.trySend(out).isFailure) throw SyncException(SyncErrorCode.Closed, "Соединение закрыто")
    }

    override fun close() {
        outbox.close()
    }
}

fun memoryPair(): Pair<MemoryTransport, MemoryTransport> {
    val ab = Channel<ByteArray>(Channel.UNLIMITED)
    val ba = Channel<ByteArray>(Channel.UNLIMITED)
    return MemoryTransport(ba, ab) to MemoryTransport(ab, ba)
}

fun sampleTrack(id: String = "12345") = Track(
    id = id,
    sourceTitle = "Название",
    sourceArtists = listOf(ArtistRef("9", "Исполнитель")),
    albumId = "678",
    albumTitle = "Альбом",
    durationMs = 215_000,
    available = true,
    sourceCoverUri = "avatars.yandex.net/get-music-content/1/abc/%%",
    loudness = Loudness(-9.1, 0.3),
    explicit = true,
    version = "Live",
)

fun sampleState(
    rev: Long = 1,
    by: String = "a".repeat(32),
    track: Track? = sampleTrack(),
    playing: Boolean = true,
    initial: Boolean = false,
    join: Boolean = false,
) = SyncState(
    rev = rev,
    by = by,
    owner = by,
    track = track,
    playing = playing,
    positionMs = 61_234,
    at = 123_456,
    next = null,
    initial = initial,
    join = join,
)
