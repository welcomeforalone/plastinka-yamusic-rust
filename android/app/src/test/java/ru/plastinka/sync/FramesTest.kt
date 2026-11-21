package ru.plastinka.sync

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlinx.coroutines.runBlocking

class FramesTest {
    private fun reader(bytes: ByteArray) = StreamFrameTransport(ByteArrayInputStream(bytes), ByteArrayOutputStream())

    private fun lenPrefix(n: Long) =
        byteArrayOf((n ushr 24).toByte(), (n ushr 16).toByte(), (n ushr 8).toByte(), n.toByte())

    @Test
    fun roundTripAndPrefix() = runBlocking {
        val out = ByteArrayOutputStream()
        StreamFrameTransport(ByteArrayInputStream(ByteArray(0)), out).writeFrame(byteArrayOf(1, 2, 3))
        assertContentEquals(byteArrayOf(0, 0, 0, 3, 1, 2, 3), out.toByteArray())
        assertContentEquals(byteArrayOf(1, 2, 3), reader(out.toByteArray()).readFrame())
    }

    @Test
    fun maxLengthIsAccepted() = runBlocking {
        val frame = Frames.withPrefix(ByteArray(65536) { 7 })
        assertEquals(65536, reader(frame).readFrame().size)
    }

    @Test
    fun zeroLengthIsError() {
        val e = assertFailsWith<SyncException> { runBlocking { reader(lenPrefix(0)).readFrame() } }
        assertEquals(SyncErrorCode.Frame, e.code)
    }

    @Test
    fun tooLongIsErrorBeforeReadingBody() {
        val e = assertFailsWith<SyncException> { runBlocking { reader(lenPrefix(65537)).readFrame() } }
        assertEquals(SyncErrorCode.Frame, e.code)
        // Огромная длина (старший бит) не приводит к выделению памяти.
        val e2 = assertFailsWith<SyncException> { runBlocking { reader(lenPrefix(0xFFFF_FFFFL)).readFrame() } }
        assertEquals(SyncErrorCode.Frame, e2.code)
    }

    @Test
    fun cannotWriteBadLength() {
        assertFailsWith<SyncException> { Frames.withPrefix(ByteArray(0)) }
        assertFailsWith<SyncException> { Frames.withPrefix(ByteArray(65537)) }
    }

    @Test
    fun truncatedFrameIsClosed() {
        val frame = lenPrefix(10) + byteArrayOf(1, 2)
        val e = assertFailsWith<SyncException> { runBlocking { reader(frame).readFrame() } }
        assertEquals(SyncErrorCode.Closed, e.code)
    }
}
