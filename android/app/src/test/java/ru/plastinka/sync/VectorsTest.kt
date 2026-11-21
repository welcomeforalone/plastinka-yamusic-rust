package ru.plastinka.sync

import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlinx.coroutines.async
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.supervisorScope
import kotlinx.coroutines.withTimeout

/**
 * Тестовые векторы спеки docs/api/sync.md §8 (посчитаны десктопной реализацией).
 * Кадры берутся как есть (точные байты), заново не сериализуются.
 */
class VectorsTest {
    private val initScalar = "11".repeat(32)
    private val respScalar = "22".repeat(32)
    private val initPublic =
        "040217e617f0b6443928278f96999e69a23a4f2c152bdf6d6cdf66e5b80282d4ed194a7debcb97712d2dda3ca85aa8765a56f45fc758599652f2897c65306e5794"
    private val respPublic =
        "04d65a93977caa3d1b081852ff57a79e465f1660577304baead505dd3a48589cf350185e895372df6221ea3a137557e473fddb6755f05bd507c3c533fce9c91285"
    private val nonceI = "a1".repeat(16)
    private val nonceR = "b2".repeat(16)
    private val pairSecret = "5a".repeat(32)

    private val f1 = """{"t":"hello","v":1,"role":"i","id":"11111111111111111111111111111111","name":"Plastinka PC","os":"windows","acc":"0123456789ab","commit":"LaBgKnhsnKQOwMVrdnPYGyZBs+nloU8Mq2y+TZ6VUPc=","known":true}"""
    private val f2 = """{"t":"hello","v":1,"role":"r","id":"22222222222222222222222222222222","name":"Телефон","os":"android","acc":"0123456789ab","key":"BNZak5d8qj0bCBhS/1ennkZfFmBXcwS66tUF3TpIWJzzUBheiVNy32Ih6joTdVfkc/3bZ1XwW9UHw8Uz/OnJEoU=","nonce":"srKysrKysrKysrKysrKysg==","known":true}"""
    private val f3 = """{"t":"reveal","key":"BAIX5hfwtkQ5KCePlpmeaaI6TywVK99tbN9m5bgCgtTtGUp968uXcS0t2jyoWqh2Wlb0X8dYWZZS8ol8ZTBuV5Q=","nonce":"oaGhoaGhoaGhoaGhoaGhoQ=="}"""

    private val expectedTh = "b0975fc0f9e6817e94caf067f7845881c06c61c5dc1e03709860fc3c62c53495"
    private val expectedZ = "ccfc261f58193c98ca4ad4a53bbac6f0ee29bc4d48438090446908622ca79af6"
    private val expectedI2r = "92191036d962bbf4011354b33ff1a524d31f17c511f14c95dca01e9426094672"
    private val expectedR2i = "b78c07d294610f55e93f6cdc4c3d1629a606ed2febaffe37e8824bd45b2d1174"
    private val expectedSas = "809495"
    private val expectedPair = "c31c8ee574529acf74388b1e6a3aca29dd5fa7bb99eaf0469b8e919482c0206d"
    private val expectedMacI = "54b26c7acca254b46b2f0a811102b933404423a5a91786bb8f821b55cebdf6d1"
    private val expectedMacR = "0055176eb754a2ef5eaeb275586d918e9f88be4e4c5aab4631c046a331f859ca"
    private val plainI2r = """{"t":"ping","n":1,"t0":1000}"""
    private val expectedEncI2r = "c670538cef41bdbede912d8b317d5bd1b4798fc6807e6406b1c155f4c914844af3c83f74e083684e167ebb50"
    private val plainR2i = """{"t":"pong","n":1,"t0":1000,"t1":2000}"""
    private val expectedEncR2i = "ba47d94ac26142a579f5e09a25da3387c53eef29edd72aba96e84976f681874d6a0c61fee24bb77b7ebbaafeb63abebe2120db9784d5"

    private fun bytes(s: String) = s.toByteArray(Charsets.UTF_8)

    @Test
    fun cryptoVectors() {
        val init = SyncCrypto.keyPairFromScalar(hex(initScalar), hex(initPublic))
        val resp = SyncCrypto.keyPairFromScalar(hex(respScalar), hex(respPublic))
        assertEquals(expectedZ, SyncCrypto.ecdh(init, resp.publicSec1).toHex())
        assertEquals(expectedZ, SyncCrypto.ecdh(resp, init.publicSec1).toHex())

        // commit из кадра 1 — SHA-256(pub_i || nonce_i).
        assertEquals(
            "LaBgKnhsnKQOwMVrdnPYGyZBs+nloU8Mq2y+TZ6VUPc=",
            java.util.Base64.getEncoder().encodeToString(SyncCrypto.sha256(hex(initPublic), hex(nonceI))),
        )

        val th = SessionKeys.transcriptHash(bytes(f1), bytes(f2), bytes(f3))
        assertEquals(expectedTh, th.toHex())
        val keys = SessionKeys.derive(hex(expectedZ), th)
        assertEquals(expectedI2r, keys.i2r.toHex())
        assertEquals(expectedR2i, keys.r2i.toHex())
        assertEquals(expectedSas, keys.sas)
        assertEquals("809 495", SessionKeys.formatCode(keys.sas))
        assertEquals(expectedPair, keys.pair.toHex())

        val secret = hex(pairSecret)
        assertEquals(expectedMacI, SessionKeys.authMac(secret, Role.Initiator, th).toHex())
        assertEquals(expectedMacR, SessionKeys.authMac(secret, Role.Responder, th).toHex())

        assertEquals(expectedEncI2r, SyncCrypto.encrypt(keys.i2r, 0, bytes(plainI2r)).toHex())
        assertEquals(expectedEncR2i, SyncCrypto.encrypt(keys.r2i, 0, bytes(plainR2i)).toHex())
    }

    /** Полное рукопожатие с фиксированными ключами даёт на проводе те же три кадра, байт в байт. */
    @Test
    fun handshakeProducesSpecFrames() {
        val secret = hex(pairSecret)
        val idI = "1".repeat(32)
        val idR = "2".repeat(32)
        val cfgI = HandshakeConfig(
            self = DeviceInfo(idI, "Plastinka PC", "windows", "0123456789ab"),
            secretFor = { if (it == idR) secret else null },
            testKeyPair = SyncCrypto.keyPairFromScalar(hex(initScalar), hex(initPublic)),
            testNonce = hex(nonceI),
        )
        val cfgR = HandshakeConfig(
            self = DeviceInfo(idR, "Телефон", "android", "0123456789ab"),
            secretFor = { if (it == idI) secret else null },
            testKeyPair = SyncCrypto.keyPairFromScalar(hex(respScalar), hex(respPublic)),
            testNonce = hex(nonceR),
        )
        val (ti, tr) = memoryPair()
        val (ri, rr) = runBlocking {
            withTimeout(30_000) {
                supervisorScope {
                    val a = async { Handshake.initiate(ti, cfgI, idR) }
                    val b = async { Handshake.respond(tr, cfgR) }
                    a.await() to b.await()
                }
            }
        }
        assertEquals(f1, ti.sent[0].toString(Charsets.UTF_8))
        assertEquals(f2, tr.sent[0].toString(Charsets.UTF_8))
        assertEquals(f3, ti.sent[1].toString(Charsets.UTF_8))
        assertEquals(true, ri.recognized)
        assertEquals(true, rr.recognized)

        // Первое шифрованное сообщение каждой стороны — auth с mac из вектора, счётчик 0.
        val keys = SessionKeys.derive(hex(expectedZ), hex(expectedTh))
        val authI = SyncCrypto.decrypt(keys.i2r, 0, ti.sent[2])
        val msgI = assertIs<SyncMessage.Auth>(SyncMessages.parse(authI))
        assertContentEquals(hex(expectedMacI), msgI.mac)
        val authR = SyncCrypto.decrypt(keys.r2i, 0, tr.sent[1])
        val msgR = assertIs<SyncMessage.Auth>(SyncMessages.parse(authR))
        assertContentEquals(hex(expectedMacR), msgR.mac)
    }
}
