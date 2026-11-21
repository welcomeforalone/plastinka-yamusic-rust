package ru.plastinka.sync

import java.util.Base64
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.coroutines.async
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.supervisorScope
import kotlinx.coroutines.withTimeout

class HandshakeTest {
    private val idI = "1".repeat(32)
    private val idR = "2".repeat(32)
    private val acc = "0123456789ab"

    private fun device(id: String, name: String, os: String, account: String = acc) = DeviceInfo(id, name, os, account)

    private class Outcome(
        val initiator: Result<HandshakeResult>,
        val responder: Result<HandshakeResult>,
        val modeI: HandshakeMode?,
        val modeR: HandshakeMode?,
        val transportI: MemoryTransport,
        val transportR: MemoryTransport,
    )

    /**
     * Полное рукопожатие в памяти. [decideI] и [decideR] — решения пользователей при
     * сопряжении (`null` — не отвечать).
     */
    private fun run(
        cfgI: HandshakeConfig,
        cfgR: HandshakeConfig,
        expectedPeerId: String? = idR,
        decideI: Boolean? = true,
        decideR: Boolean? = true,
        setup: (MemoryTransport, MemoryTransport) -> Unit = { _, _ -> },
    ): Outcome {
        val (ti, tr) = memoryPair()
        setup(ti, tr)
        var modeI: HandshakeMode? = null
        var modeR: HandshakeMode? = null
        val (ri, rr) = runBlocking {
            withTimeout(30_000) {
                supervisorScope {
                    val a = async {
                        runCatching {
                            Handshake.initiate(ti, cfgI, expectedPeerId) { _, m ->
                                modeI = m
                                if (m is HandshakeMode.Pairing && decideI != null) m.decision.complete(decideI)
                            }
                        }
                    }
                    val b = async {
                        runCatching {
                            Handshake.respond(tr, cfgR) { _, m ->
                                modeR = m
                                if (m is HandshakeMode.Pairing && decideR != null) m.decision.complete(decideR)
                            }
                        }
                    }
                    a.await() to b.await()
                }
            }
        }
        return Outcome(ri, rr, modeI, modeR, ti, tr)
    }

    private fun cfg(
        id: String,
        os: String,
        secrets: Map<String, ByteArray> = emptyMap(),
        account: String = acc,
        busy: Boolean = false,
        confirmMs: Long = 120_000,
    ) = HandshakeConfig(
        self = device(id, "Устройство $os", os, account),
        secretFor = { secrets[it] },
        isBusy = { if (busy) HandshakeBusy.Busy else HandshakeBusy.Free },
        frameTimeoutMs = 5_000,
        confirmTimeoutMs = confirmMs,
    )

    private fun Result<HandshakeResult>.error(): SyncException {
        val e = exceptionOrNull()
        assertIs<SyncException>(e)
        return e
    }

    private fun assertTalk(a: HandshakeResult, b: HandshakeResult) = runBlocking {
        a.channel.send(SyncMessage.Ping(1, 10))
        assertEquals(SyncMessage.Ping(1, 10), b.channel.receive())
        b.channel.send(SyncMessage.Pong(1, 10, 20))
        assertEquals(SyncMessage.Pong(1, 10, 20), a.channel.receive())
    }

    @Test
    fun pairingBothConfirm() {
        val o = run(cfg(idI, "windows"), cfg(idR, "android"))
        val i = o.initiator.getOrThrow()
        val r = o.responder.getOrThrow()
        assertFalse(i.recognized)
        assertFalse(r.recognized)
        val secretI = assertNotNull(i.newSecret)
        assertEquals(32, secretI.size)
        assertContentEquals(secretI, r.newSecret)
        val pi = assertIs<HandshakeMode.Pairing>(o.modeI)
        val pr = assertIs<HandshakeMode.Pairing>(o.modeR)
        assertEquals(pi.code, pr.code)
        assertTrue(Regex("[0-9]{6}").matches(pi.code))
        assertEquals("${pi.code.take(3)} ${pi.code.drop(3)}", pi.displayCode)
        assertEquals(idR, i.peer.id)
        assertEquals(idI, r.peer.id)
        assertEquals("Устройство android", i.peer.name)
        assertEquals("android", i.peer.os)
        assertEquals(Role.Initiator, i.role)
        assertTalk(i, r)
    }

    @Test
    fun pairingByAddressWithoutKnownId() {
        val o = run(cfg(idI, "windows"), cfg(idR, "android"), expectedPeerId = null)
        assertContentEquals(o.initiator.getOrThrow().newSecret, o.responder.getOrThrow().newSecret)
    }

    @Test
    fun pairingRejectedByInitiator() {
        val o = run(cfg(idI, "windows"), cfg(idR, "android"), decideI = false, decideR = true)
        assertEquals(SyncErrorCode.Rejected, o.initiator.error().code)
        val re = o.responder.error()
        assertEquals(SyncErrorCode.PeerBye, re.code)
        assertEquals("rejected", re.reason)
    }

    @Test
    fun pairingRejectedByResponder() {
        val o = run(cfg(idI, "windows"), cfg(idR, "android"), decideI = true, decideR = false)
        assertEquals(SyncErrorCode.Rejected, o.responder.error().code)
        assertEquals("rejected", o.initiator.error().reason)
    }

    @Test
    fun pairingTimeoutRejects() {
        val o = run(
            cfg(idI, "windows", confirmMs = 200),
            cfg(idR, "android", confirmMs = 200),
            decideI = null,
            decideR = null,
        )
        // Таймеры сторон срабатывают почти одновременно: чей сработал раньше, тот отказал сам
        // (Rejected), а второй мог успеть получить его `bye rejected` (PeerBye) до своего таймера.
        val errors = listOf(o.initiator.error(), o.responder.error())
        for (e in errors) {
            val ok = e.code == SyncErrorCode.Rejected ||
                (e.code == SyncErrorCode.PeerBye && e.reason == "rejected")
            assertTrue(ok, "неожиданный итог таймаута: ${e.code} ${e.reason}")
        }
        assertTrue(errors.any { it.code == SyncErrorCode.Rejected })
    }

    @Test
    fun peerByeWhileWaitingEndsWaiting() {
        // Ответчик отказал сразу, инициатор ещё не решил: ожидание решения не должно висеть.
        val o = run(cfg(idI, "windows"), cfg(idR, "android"), decideI = null, decideR = false)
        assertEquals(SyncErrorCode.Rejected, o.responder.error().code)
        assertEquals(SyncErrorCode.PeerBye, o.initiator.error().code)
    }

    @Test
    fun recognitionWithSameSecret() {
        val s = ByteArray(32) { 9 }
        val o = run(
            cfg(idI, "windows", mapOf(idR to s)),
            cfg(idR, "android", mapOf(idI to s)),
        )
        val i = o.initiator.getOrThrow()
        val r = o.responder.getOrThrow()
        assertTrue(i.recognized)
        assertTrue(r.recognized)
        assertNull(i.newSecret)
        assertIs<HandshakeMode.Recognition>(o.modeI)
        assertIs<HandshakeMode.Recognition>(o.modeR)
        assertTalk(i, r)
    }

    @Test
    fun recognitionWithWrongSecretFailsOnBothSides() {
        val o = run(
            cfg(idI, "windows", mapOf(idR to ByteArray(32) { 1 })),
            cfg(idR, "android", mapOf(idI to ByteArray(32) { 2 })),
        )
        assertEquals(SyncErrorCode.AuthFailed, o.initiator.error().code)
        assertEquals(SyncErrorCode.AuthFailed, o.responder.error().code)
        assertEquals("auth", o.initiator.error().reason)
    }

    @Test
    fun oneSideForgotMeansPairing() {
        val s = ByteArray(32) { 9 }
        val o = run(cfg(idI, "windows", mapOf(idR to s)), cfg(idR, "android"))
        assertIs<HandshakeMode.Pairing>(o.modeI)
        assertIs<HandshakeMode.Pairing>(o.modeR)
        assertContentEquals(o.initiator.getOrThrow().newSecret, o.responder.getOrThrow().newSecret)
    }

    private fun replaceField(frame: ByteArray, field: String, newValue: String): ByteArray {
        val text = frame.toString(Charsets.UTF_8)
        val replaced = Regex("\"$field\":\"[^\"]*\"").replace(text, "\"$field\":\"$newValue\"")
        assertTrue(replaced != text, "поле $field не найдено")
        return replaced.toByteArray(Charsets.UTF_8)
    }

    @Test
    fun forgedCommitAborts() {
        val fake = Base64.getEncoder().encodeToString(ByteArray(32))
        val o = run(cfg(idI, "windows"), cfg(idR, "android"), setup = { ti, _ ->
            ti.tamper = { n, f -> if (n == 0) replaceField(f, "commit", fake) else f }
        })
        assertEquals(SyncErrorCode.Crypto, o.responder.error().code)
        assertTrue(o.initiator.isFailure)
    }

    @Test
    fun forgedRevealAborts() {
        val fake = Base64.getEncoder().encodeToString(ByteArray(16))
        val o = run(cfg(idI, "windows"), cfg(idR, "android"), setup = { ti, _ ->
            ti.tamper = { n, f -> if (n == 1) replaceField(f, "nonce", fake) else f }
        })
        assertEquals(SyncErrorCode.Crypto, o.responder.error().code)
        assertTrue(o.initiator.isFailure)
    }

    @Test
    fun forgedResponderFrameBreaksTranscript() {
        // Подмена имени в кадре 2: ключи у сторон разные, кадры не расшифруются.
        val o = run(cfg(idI, "windows"), cfg(idR, "android"), setup = { _, tr ->
            tr.tamper = { n, f -> if (n == 0) replaceField(f, "name", "Чужое") else f }
        })
        assertTrue(o.initiator.isFailure)
        assertTrue(o.responder.isFailure)
    }

    @Test
    fun foreignAccountRefused() {
        val o = run(cfg(idI, "windows"), cfg(idR, "android", account = "ffffffffffff"))
        val re = o.responder.error()
        assertEquals(SyncErrorCode.Refused, re.code)
        assertEquals("account", re.reason)
        val ie = o.initiator.error()
        assertEquals(SyncErrorCode.PeerBye, ie.code)
        assertEquals("account", ie.reason)
    }

    @Test
    fun busyResponderRefuses() {
        val o = run(cfg(idI, "windows"), cfg(idR, "android", busy = true))
        assertEquals("busy", o.responder.error().reason)
        assertEquals("busy", o.initiator.error().reason)
    }

    @Test
    fun wrongVersionRefused() = runBlocking {
        val (ti, tr) = memoryPair()
        val hello = """{"t":"hello","v":2,"role":"i","id":"$idI","name":"x","os":"windows","acc":"$acc","commit":"AAAA","known":false}"""
        ti.writeFrame(hello.toByteArray())
        val r = runCatching { Handshake.respond(tr, cfg(idR, "android")) }
        val e = r.exceptionOrNull()
        assertIs<SyncException>(e)
        assertEquals("version", e.reason)
        val reply = SyncMessages.parseObject(ti.readFrame())
        assertEquals("bye", (reply?.get("t") as? kotlinx.serialization.json.JsonPrimitive)?.content)
        assertEquals("version", (reply?.get("reason") as? kotlinx.serialization.json.JsonPrimitive)?.content)
    }

    @Test
    fun garbageHelloClosesWithoutHang() = runBlocking {
        val (ti, tr) = memoryPair()
        ti.writeFrame("не json".toByteArray())
        val r = runCatching { Handshake.respond(tr, cfg(idR, "android")) }
        assertEquals(SyncErrorCode.Protocol, (r.exceptionOrNull() as? SyncException)?.code)
    }

    @Test
    fun silentPeerTimesOut() = runBlocking {
        val (_, tr) = memoryPair()
        val c = HandshakeConfig(device(idR, "x", "android"), { null }, frameTimeoutMs = 100)
        val r = runCatching { Handshake.respond(tr, c) }
        assertEquals(SyncErrorCode.Timeout, (r.exceptionOrNull() as? SyncException)?.code)
    }

    @Test
    fun nameCleanedAndOsValidated() {
        val c = HandshakeConfig(device(idI, "a\u0007b", "Bad OS!"), { null }, frameTimeoutMs = 5_000)
        val peer = run(c, cfg(idR, "android")).responder.getOrThrow().peer
        assertEquals("ab", peer.name)
        assertEquals("unknown", peer.os)
    }

    @Test
    fun emptyNameBecomesDefault() {
        val c = HandshakeConfig(device(idI, "\u0001\u0002", "windows"), { null }, frameTimeoutMs = 5_000)
        assertEquals("Устройство", run(c, cfg(idR, "android")).responder.getOrThrow().peer.name)
    }

    private fun silentClose(frame: String) = runBlocking {
        val (ti, tr) = memoryPair()
        ti.writeFrame(frame.toByteArray())
        val r = runCatching { Handshake.respond(tr, cfg(idR, "android")) }
        assertEquals(SyncErrorCode.Protocol, (r.exceptionOrNull() as? SyncException)?.code)
        assertTrue(tr.sent.isEmpty())
    }

    private fun helloFrom(id: String) =
        """{"t":"hello","v":1,"role":"i","id":"$id","name":"x","os":"windows","acc":"$acc","commit":"AAAA","known":false}"""

    @Test
    fun notHelloIsClosedSilently() = silentClose("""{"t":"ping","n":1,"t0":1}""")

    @Test
    fun connectionToSelfIsClosedSilently() = silentClose(helloFrom(idR))

    @Test
    fun badCommitIsClosedSilently() = silentClose(helloFrom(idI))

    @Test
    fun longNameIsTruncatedTo64() {
        val c = HandshakeConfig(device(idI, "я".repeat(100), "windows"), { null }, frameTimeoutMs = 5_000)
        val o = run(c, cfg(idR, "android"))
        assertEquals(64, o.responder.getOrThrow().peer.name.length)
    }
}
