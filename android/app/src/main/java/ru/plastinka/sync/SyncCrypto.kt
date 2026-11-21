package ru.plastinka.sync

import java.math.BigInteger
import java.nio.ByteBuffer
import java.security.AlgorithmParameters
import java.security.GeneralSecurityException
import java.security.KeyFactory
import java.security.KeyPairGenerator
import java.security.MessageDigest
import java.security.PrivateKey
import java.security.SecureRandom
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.security.spec.ECParameterSpec
import java.security.spec.ECPoint
import java.security.spec.ECPrivateKeySpec
import java.security.spec.ECPublicKeySpec
import javax.crypto.Cipher
import javax.crypto.KeyAgreement
import javax.crypto.Mac
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/** Эфемерная пара P-256: закрытый ключ и открытый в виде SEC1 без сжатия (65 байт). */
class EcKeyPair internal constructor(
    internal val privateKey: PrivateKey,
    val publicSec1: ByteArray,
)

/** Криптопримитивы протокола (§3.2–3.3): только JCA, ничего лишнего. */
object SyncCrypto {
    const val PUBLIC_KEY_LEN = 65
    const val NONCE_LEN = 16
    const val TAG_BITS = 128
    const val TAG_LEN = 16

    private val random = SecureRandom()

    // Параметры кривой P-256 для проверки точки: y² = x³ − 3x + b (mod p).
    private val P = BigInteger("FFFFFFFF00000001000000000000000000000000FFFFFFFFFFFFFFFFFFFFFFFF", 16)
    private val B = BigInteger("5AC635D8AA3A93E7B3EBBD55769886BC651D06B0CC53B0F63BCE3C3E27D2604B", 16)

    private val curve: ECParameterSpec by lazy {
        val ap = AlgorithmParameters.getInstance("EC")
        ap.init(ECGenParameterSpec("secp256r1"))
        ap.getParameterSpec(ECParameterSpec::class.java)
    }

    fun randomBytes(n: Int): ByteArray = ByteArray(n).also { random.nextBytes(it) }

    fun sha256(vararg parts: ByteArray): ByteArray {
        val md = MessageDigest.getInstance("SHA-256")
        for (p in parts) md.update(p)
        return md.digest()
    }

    fun hmacSha256(key: ByteArray, vararg parts: ByteArray): ByteArray {
        val mac = Mac.getInstance("HmacSHA256")
        // HMAC с пустым ключом равен HMAC с нулевым: SecretKeySpec пустой ключ не берёт.
        mac.init(SecretKeySpec(if (key.isEmpty()) ByteArray(1) else key, "HmacSHA256"))
        for (p in parts) mac.update(p)
        return mac.doFinal()
    }

    /** Сравнение за постоянное время. */
    fun constantTimeEquals(a: ByteArray, b: ByteArray): Boolean = MessageDigest.isEqual(a, b)

    /** HKDF-Extract (RFC 5869). */
    fun hkdfExtract(salt: ByteArray, ikm: ByteArray): ByteArray = hmacSha256(salt, ikm)

    /** HKDF-Expand (RFC 5869) на HMAC-SHA256. */
    fun hkdfExpand(prk: ByteArray, info: ByteArray, length: Int): ByteArray {
        require(length in 0..255 * 32) { "Слишком длинный вывод HKDF" }
        val out = ByteArray(length)
        var prev = ByteArray(0)
        var pos = 0
        var counter = 1
        while (pos < length) {
            prev = hmacSha256(prk, prev, info, byteArrayOf(counter.toByte()))
            val n = minOf(prev.size, length - pos)
            prev.copyInto(out, pos, 0, n)
            pos += n
            counter++
        }
        return out
    }

    /** Новая эфемерная пара. */
    fun generateKeyPair(): EcKeyPair {
        val gen = KeyPairGenerator.getInstance("EC")
        gen.initialize(ECGenParameterSpec("secp256r1"), random)
        val kp = gen.generateKeyPair()
        val w = (kp.public as ECPublicKey).w
        return EcKeyPair(kp.private, encodePoint(w.affineX, w.affineY))
    }

    /**
     * Пара из фиксированного скаляра (тестовые векторы). Открытый ключ в JCA из скаляра не
     * вычислить, поэтому он передаётся готовым и проверяется на принадлежность кривой.
     */
    fun keyPairFromScalar(scalar: ByteArray, publicSec1: ByteArray): EcKeyPair {
        parsePublicKey(publicSec1)
        val key = try {
            KeyFactory.getInstance("EC").generatePrivate(ECPrivateKeySpec(BigInteger(1, scalar), curve))
        } catch (e: GeneralSecurityException) {
            throw SyncException(SyncErrorCode.Crypto, "Неверный закрытый ключ", cause = e)
        }
        return EcKeyPair(key, publicSec1.copyOf())
    }

    /** Проверка открытого ключа: 65 байт, `0x04`, координаты меньше p, точка на кривой. */
    fun isValidPublicKey(sec1: ByteArray): Boolean = decodePoint(sec1) != null

    /** Общий секрет `Z` — 32 байта координаты X. */
    fun ecdh(own: EcKeyPair, peerPublicSec1: ByteArray): ByteArray {
        val peer = parsePublicKey(peerPublicSec1)
        try {
            val ka = KeyAgreement.getInstance("ECDH")
            ka.init(own.privateKey)
            ka.doPhase(peer, true)
            val z = ka.generateSecret()
            if (z.size != 32) throw SyncException(SyncErrorCode.Crypto, "Неверный общий секрет")
            return z
        } catch (e: GeneralSecurityException) {
            throw SyncException(SyncErrorCode.Crypto, "Не удалось согласовать ключ", cause = e)
        }
    }

    /** Nonce кадра: 4 нулевых байта и счётчик направления `u64` big-endian. */
    fun frameNonce(counter: Long): ByteArray = ByteBuffer.allocate(12).putInt(0).putLong(counter).array()

    /** AES-256-GCM, пустой AAD; результат — шифротекст и 16 байт тега. */
    fun encrypt(key: ByteArray, counter: Long, plain: ByteArray): ByteArray {
        try {
            val c = Cipher.getInstance("AES/GCM/NoPadding")
            c.init(Cipher.ENCRYPT_MODE, SecretKeySpec(key, "AES"), GCMParameterSpec(TAG_BITS, frameNonce(counter)))
            return c.doFinal(plain)
        } catch (e: GeneralSecurityException) {
            throw SyncException(SyncErrorCode.Crypto, "Не удалось зашифровать сообщение", cause = e)
        }
    }

    fun decrypt(key: ByteArray, counter: Long, data: ByteArray): ByteArray {
        if (data.size < TAG_LEN) throw SyncException(SyncErrorCode.Crypto, "Слишком короткий шифрованный кадр")
        try {
            val c = Cipher.getInstance("AES/GCM/NoPadding")
            c.init(Cipher.DECRYPT_MODE, SecretKeySpec(key, "AES"), GCMParameterSpec(TAG_BITS, frameNonce(counter)))
            return c.doFinal(data)
        } catch (e: GeneralSecurityException) {
            throw SyncException(SyncErrorCode.Crypto, "Кадр не расшифровался: он повреждён или подменён", cause = e)
        }
    }

    private fun encodePoint(x: BigInteger, y: BigInteger): ByteArray {
        val out = ByteArray(PUBLIC_KEY_LEN)
        out[0] = 4
        fixed32(x).copyInto(out, 1)
        fixed32(y).copyInto(out, 33)
        return out
    }

    private fun fixed32(v: BigInteger): ByteArray {
        val raw = v.toByteArray()
        val out = ByteArray(32)
        // Без знакового нуля в начале; длиннее 32 байт значение быть не может (проверено выше).
        val start = if (raw.size > 32) raw.size - 32 else 0
        val n = raw.size - start
        raw.copyInto(out, 32 - n, start, raw.size)
        return out
    }

    private fun decodePoint(sec1: ByteArray): ECPoint? {
        if (sec1.size != PUBLIC_KEY_LEN || sec1.firstOrNull()?.toInt() != 4) return null
        val x = BigInteger(1, sec1.copyOfRange(1, 33))
        val y = BigInteger(1, sec1.copyOfRange(33, 65))
        if (x >= P || y >= P) return null
        val left = y.multiply(y).mod(P)
        val right = x.pow(3).subtract(x.multiply(BigInteger.valueOf(3))).add(B).mod(P)
        if (left != right) return null
        return ECPoint(x, y)
    }

    private fun parsePublicKey(sec1: ByteArray): java.security.PublicKey {
        val point = decodePoint(sec1)
            ?: throw SyncException(SyncErrorCode.Crypto, "Открытый ключ собеседника не лежит на кривой P-256")
        try {
            return KeyFactory.getInstance("EC").generatePublic(ECPublicKeySpec(point, curve))
        } catch (e: GeneralSecurityException) {
            throw SyncException(SyncErrorCode.Crypto, "Открытый ключ собеседника не принят", cause = e)
        }
    }
}
