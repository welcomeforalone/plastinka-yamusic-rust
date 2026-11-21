package ru.plastinka.sync

import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.coroutines.runBlocking

class CryptoTest {
    @Test
    fun hkdfRfc5869Case1() {
        val ikm = ByteArray(22) { 0x0b }
        val salt = hex("000102030405060708090a0b0c")
        val info = hex("f0f1f2f3f4f5f6f7f8f9")
        val prk = SyncCrypto.hkdfExtract(salt, ikm)
        assertEquals("077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5", prk.toHex())
        val okm = SyncCrypto.hkdfExpand(prk, info, 42)
        assertEquals(
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865",
            okm.toHex(),
        )
    }

    // RFC 5903, раздел 8.1 (группа P-256).
    private val iPriv = "C88F01F510D9AC3F70A292DAA2316DE544E9AAB8AFE84049C62A9C57862D1433"
    private val iPub = "04DAD0B65394221CF9B051E1FECA5787D098DFE637FC90B9EF945D0C3772581180" +
        "5271A0461CDB8252D61F1C456FA3E59AB1F45B33ACCF5F58389E0577B8990BB3"
    private val rPriv = "C6EF9C5D78AE012A011164ACB397CE2088685D8F06BF9BE0B283AB46476BEE53"
    private val rPub = "04D12DFB5289C8D4F81208B70270398C342296970A0BCCB74C736FC7554494BF63" +
        "56FBF3CA366CC23E8157854C13C58D6AAC23F046ADA30F8353E74F33039872AB"
    private val shared = "D6840F6B42F6EDAFD13116E0E12565202FEF8E9ECE7DCE03812464D04B9442DE"

    @Test
    fun ecdhRfc5903() {
        val i = SyncCrypto.keyPairFromScalar(hex(iPriv), hex(iPub))
        val r = SyncCrypto.keyPairFromScalar(hex(rPriv), hex(rPub))
        assertEquals(shared.lowercase(), SyncCrypto.ecdh(i, r.publicSec1).toHex())
        assertEquals(shared.lowercase(), SyncCrypto.ecdh(r, i.publicSec1).toHex())
    }

    @Test
    fun generatedKeysAgree() {
        val a = SyncCrypto.generateKeyPair()
        val b = SyncCrypto.generateKeyPair()
        assertEquals(65, a.publicSec1.size)
        assertEquals(4, a.publicSec1[0].toInt())
        assertContentEquals(SyncCrypto.ecdh(a, b.publicSec1), SyncCrypto.ecdh(b, a.publicSec1))
    }

    @Test
    fun pointNotOnCurveIsRejected() {
        val good = hex(rPub)
        assertTrue(SyncCrypto.isValidPublicKey(good))
        val bad = good.copyOf().also { it[64] = (it[64] + 1).toByte() }
        assertFalse(SyncCrypto.isValidPublicKey(bad))
        assertFalse(SyncCrypto.isValidPublicKey(good.copyOf(64)))
        assertFalse(SyncCrypto.isValidPublicKey(good.copyOf().also { it[0] = 2 }))
        // Нулевые координаты: точка (0, 0) не на кривой.
        assertFalse(SyncCrypto.isValidPublicKey(ByteArray(65).also { it[0] = 4 }))
        val own = SyncCrypto.generateKeyPair()
        val e = assertFailsWith<SyncException> { SyncCrypto.ecdh(own, bad) }
        assertEquals(SyncErrorCode.Crypto, e.code)
    }

    @Test
    fun nonceLayout() {
        assertEquals("000000000000000000000000", SyncCrypto.frameNonce(0).toHex())
        assertEquals("000000000000000000000102", SyncCrypto.frameNonce(258).toHex())
    }

    @Test
    fun aesGcmRoundTripAndTamper() {
        val key = ByteArray(32) { it.toByte() }
        val plain = "привет".toByteArray()
        val enc = SyncCrypto.encrypt(key, 5, plain)
        assertEquals(plain.size + 16, enc.size)
        assertContentEquals(plain, SyncCrypto.decrypt(key, 5, enc))
        assertFailsWith<SyncException> { SyncCrypto.decrypt(key, 6, enc) }
        assertFailsWith<SyncException> { SyncCrypto.decrypt(key, 5, enc.copyOf().also { it[0] = (it[0] + 1).toByte() }) }
        assertFailsWith<SyncException> { SyncCrypto.decrypt(key, 5, ByteArray(3)) }
    }

    private val k1 = ByteArray(32) { 1 }
    private val k2 = ByteArray(32) { 2 }

    private fun channels(): Triple<SecureChannel, SecureChannel, MemoryTransport> {
        val (ta, tb) = memoryPair()
        return Triple(SecureChannel(ta, k1, k2), SecureChannel(tb, k2, k1), ta)
    }

    @Test
    fun channelExchangesMessages() = runBlocking {
        val (a, b) = channels()
        a.send(SyncMessage.Ping(1, 100))
        a.send(SyncMessage.Command("next"))
        assertEquals(SyncMessage.Ping(1, 100), b.receive())
        assertEquals(SyncMessage.Command("next"), b.receive())
        b.send(SyncMessage.Pong(1, 100, 200))
        assertEquals(SyncMessage.Pong(1, 100, 200), a.receive())
    }

    @Test
    fun replayedFrameIsRejected() = runBlocking {
        val (a, b, ta) = channels()
        a.send(SyncMessage.Ping(1, 100))
        assertEquals(SyncMessage.Ping(1, 100), b.receive())
        ta.writeFrame(ta.sent.first())
        val e = assertFailsWith<SyncException> { b.receive() }
        assertEquals(SyncErrorCode.Crypto, e.code)
    }

    @Test
    fun corruptedFrameIsRejected() = runBlocking {
        val (a, b, ta) = channels()
        ta.tamper = { _, f -> f.copyOf().also { it[1] = (it[1] + 1).toByte() } }
        a.send(SyncMessage.Ping(1, 100))
        val e = assertFailsWith<SyncException> { b.receive() }
        assertEquals(SyncErrorCode.Crypto, e.code)
    }

    @Test
    fun skippedFrameIsRejected() = runBlocking {
        val (a, b, ta) = channels()
        a.send(SyncMessage.Ping(1, 100))
        a.send(SyncMessage.Ping(2, 100))
        // Первый кадр «потерян»: приёмнику отдаём только второй (он шифрован счётчиком 1).
        val (tx, rx) = memoryPair()
        val receiver = SecureChannel(rx, k1, k2)
        val second = ta.sent.getOrNull(1) ?: error("нет второго кадра")
        tx.writeFrame(second)
        assertFailsWith<SyncException> { receiver.receive() }
        b.close()
    }

    @Test
    fun unknownMessageIsSkippedNotFatal() = runBlocking {
        val (a, b, ta) = channels()
        // Вручную шифруем сообщение с неизвестным t и счётчиком 0.
        ta.writeFrame(SyncCrypto.encrypt(k1, 0, """{"t":"future","x":1}""".toByteArray()))
        assertNull(b.receive())
        // Связь жива: счётчик продолжается.
        ta.writeFrame(SyncCrypto.encrypt(k1, 1, """{"t":"ping","n":3,"t0":9}""".toByteArray()))
        assertEquals(SyncMessage.Ping(3, 9), b.receive())
        a.close()
    }
}
