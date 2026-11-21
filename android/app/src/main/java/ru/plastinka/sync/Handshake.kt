package ru.plastinka.sync

import java.util.Base64
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put

/** Это устройство в рукопожатии. */
data class DeviceInfo(
    /** 32 символа hex в нижнем регистре. */
    val id: String,
    val name: String,
    /** `windows`, `linux`, `macos`, `android`. */
    val os: String,
    /** Метка аккаунта: 12 символов hex (§2). */
    val acc: String,
)

/** Собеседник по его `hello`. Имя уже обрезано до 64 символов. */
data class PeerInfo(val id: String, val name: String, val os: String)

enum class Role(val char: Char) {
    Initiator('i'),
    Responder('r'),
    ;

    val other: Role get() = if (this == Initiator) Responder else Initiator
}

/** Ответ ответчика на вопрос «занят ли я» (§3.1, §6). */
enum class HandshakeBusy {
    Free,

    /** Уже есть связь или идёт сопряжение: открытый `bye busy`. */
    Busy,

    /** Встречное подключение, оставлено наше исходящее: закрыть без `bye`. */
    DropSilently,
}

/** Настройки рукопожатия. Хранилище секретов и занятость — снаружи. */
class HandshakeConfig(
    val self: DeviceInfo,
    /** Сохранённый секрет сопряжения для устройства с таким id или `null`. */
    val secretFor: (peerId: String) -> ByteArray?,
    /**
     * Ответчик: решение по инициатору с таким id после проверки его `hello`. Вызывается один раз;
     * [HandshakeBusy.Free] означает, что слот связи занят под это рукопожатие.
     */
    val isBusy: (initiatorId: String) -> HandshakeBusy = { HandshakeBusy.Free },
    /** Ожидание каждого кадра, мс. */
    val frameTimeoutMs: Long = 10_000,
    /** Ожидание решения о сопряжении, мс (§3.4). */
    val confirmTimeoutMs: Long = 120_000,
    /** Для тестов: фиксированные эфемерный ключ и nonce вместо случайных. */
    internal val testKeyPair: EcKeyPair? = null,
    internal val testNonce: ByteArray? = null,
)

/** Что покажет окно, когда рукопожатие дошло до проверки (§3.4). */
sealed interface HandshakeMode {
    /** Оба устройства знают друг друга: проверка идёт сама, без кода. */
    data object Recognition : HandshakeMode

    /**
     * Сопряжение: показать [code] («123 456») и имя собеседника, ждать решения пользователя
     * и завершить [decision]: `true` — «Коды совпадают», `false` — «Отмена». Если не завершить
     * за `confirmTimeoutMs`, сопряжение отклоняется само.
     */
    class Pairing(val code: String, val decision: CompletableDeferred<Boolean>) : HandshakeMode {
        /** Код для показа: «123 456». */
        val displayCode: String get() = SessionKeys.formatCode(code)
    }
}

/** Итог успешного рукопожатия. */
class HandshakeResult(
    val peer: PeerInfo,
    val role: Role,
    /** `true` — узнавание (сопряжение уже было), `false` — только что сопряжены. */
    val recognized: Boolean,
    /** Новый секрет сопряжения (32 байта) — сохранить под `peer.id`; при узнавании `null`. */
    val newSecret: ByteArray?,
    val channel: SecureChannel,
)

/** Ключи сессии из `TH` и `Z` (§3.2). */
class SessionKeys(
    val i2r: ByteArray,
    val r2i: ByteArray,
    /** Код сопряжения, 6 цифр с ведущими нулями. */
    val sas: String,
    val pair: ByteArray,
) {
    companion object {
        /** `TH`: SHA-256 от трёх кадров вместе с префиксами длины (кадры — только содержимое). */
        fun transcriptHash(frame1: ByteArray, frame2: ByteArray, frame3: ByteArray): ByteArray =
            SyncCrypto.sha256(Frames.withPrefix(frame1), Frames.withPrefix(frame2), Frames.withPrefix(frame3))

        fun derive(z: ByteArray, th: ByteArray): SessionKeys {
            val prk = SyncCrypto.hkdfExtract(th, z)
            fun expand(info: String, len: Int) = SyncCrypto.hkdfExpand(prk, info.toByteArray(Charsets.US_ASCII), len)
            val sasBytes = expand("plastinka/1 sas", 4)
            var v = 0L
            for (b in sasBytes) v = (v shl 8) or (b.toLong() and 0xFF)
            return SessionKeys(
                i2r = expand("plastinka/1 i2r", 32),
                r2i = expand("plastinka/1 r2i", 32),
                sas = (v % 1_000_000).toString().padStart(6, '0'),
                pair = expand("plastinka/1 pair", 32),
            )
        }

        /** «123456» → «123 456». */
        fun formatCode(sas: String): String = if (sas.length == 6) "${sas.take(3)} ${sas.drop(3)}" else sas

        /** `HMAC-SHA256(secret, "plastinka/1 auth " || role || TH)`; [role] — роль отправителя. */
        fun authMac(secret: ByteArray, role: Role, th: ByteArray): ByteArray = SyncCrypto.hmacSha256(
            secret,
            "plastinka/1 auth ".toByteArray(Charsets.US_ASCII),
            byteArrayOf(role.char.code.toByte()),
            th,
        )
    }
}

/** Рукопожатие §3.1–3.4. Любая неудача закрывает [FrameTransport] и бросает [SyncException]. */
object Handshake {
    private val HEX32 = Regex("[0-9a-f]{32}")
    private const val MAX_NAME = 64
    private val OS_RE = Regex("[a-z0-9]{1,16}")
    private val b64e = Base64.getEncoder()
    private val b64d = Base64.getDecoder()

    /**
     * Сторона, которая подключилась. [expectedPeerId] — id из TXT-записи mDNS (по нему
     * ищется секрет и ставится `known`); при подключении по адресу — `null`.
     * [onMode] вызывается один раз, когда известно, узнавание это или сопряжение.
     */
    suspend fun initiate(
        transport: FrameTransport,
        cfg: HandshakeConfig,
        expectedPeerId: String?,
        join: Boolean = false,
        onMode: (PeerInfo, HandshakeMode) -> Unit = { _, _ -> },
    ): HandshakeResult = guarded(transport) {
        val expectedSecret = expectedPeerId?.let { cfg.secretFor(it) }
        val key = cfg.testKeyPair ?: SyncCrypto.generateKeyPair()
        val nonce = cfg.testNonce ?: SyncCrypto.randomBytes(SyncCrypto.NONCE_LEN)
        val commit = SyncCrypto.sha256(key.publicSec1, nonce)

        val f1 = helloFrame(cfg.self, Role.Initiator, expectedSecret != null) {
            put("commit", b64e.encodeToString(commit))
        }
        transport.writeFrame(f1)

        val f2 = timed(cfg.frameTimeoutMs, "ответа") { transport.readFrame() }
        val o2 = SyncMessages.parseObject(f2) ?: throw protocol("Неверный ответ собеседника")
        throwIfBye(o2)
        val peer = try {
            checkHello(o2, Role.Responder, cfg.self)
        } catch (e: HelloRejected) {
            sendBye(transport, "version")
            throw SyncException(SyncErrorCode.Refused, "Другое устройство использует другую версию протокола", "version")
        }
        if (peer.acc != cfg.self.acc) {
            sendBye(transport, "account")
            throw SyncException(SyncErrorCode.Refused, "На другом устройстве другой аккаунт Яндекса", "account")
        }
        if (expectedPeerId != null && expectedPeerId != peer.info.id) {
            throw protocol("Устройство ответило под другим идентификатором")
        }
        val peerKey = decodeB64(o2, "key", SyncCrypto.PUBLIC_KEY_LEN)
        decodeB64(o2, "nonce", SyncCrypto.NONCE_LEN)
        if (!SyncCrypto.isValidPublicKey(peerKey)) throw SyncException(SyncErrorCode.Crypto, "Неверный ключ собеседника")

        val f3 = buildJsonObject {
            put("t", "reveal")
            put("key", b64e.encodeToString(key.publicSec1))
            put("nonce", b64e.encodeToString(nonce))
        }.toString().toByteArray(Charsets.UTF_8)
        transport.writeFrame(f3)

        val z = SyncCrypto.ecdh(key, peerKey)
        verify(
            transport, cfg, Role.Initiator, peer.info, f1, f2, f3, z,
            knownSelf = expectedSecret != null, knownPeer = peer.known, secret = expectedSecret, onMode = onMode,
        )
    }

    /** Сторона, которая приняла соединение. */
    suspend fun respond(
        transport: FrameTransport,
        cfg: HandshakeConfig,
        onMode: (PeerInfo, HandshakeMode) -> Unit = { _, _ -> },
    ): HandshakeResult = guarded(transport) {
        val f1 = timed(cfg.frameTimeoutMs, "приветствия") { transport.readFrame() }
        val o1 = SyncMessages.parseObject(f1) ?: throw protocol("Неверное приветствие")
        val peer = try {
            checkHello(o1, Role.Initiator, cfg.self)
        } catch (e: HelloRejected) {
            sendBye(transport, "version")
            throw SyncException(SyncErrorCode.Refused, "Собеседник использует другую версию протокола", "version")
        }
        if (peer.acc != cfg.self.acc) {
            sendBye(transport, "account")
            throw SyncException(SyncErrorCode.Refused, "У собеседника другой аккаунт Яндекса", "account")
        }
        when (cfg.isBusy(peer.info.id)) {
            HandshakeBusy.Free -> Unit
            HandshakeBusy.Busy -> {
                sendBye(transport, "busy")
                throw SyncException(SyncErrorCode.Refused, "Уже есть связь с другим устройством", "busy")
            }
            // Встречное подключение (§6): второе соединение закрывается без `bye`.
            HandshakeBusy.DropSilently ->
                throw SyncException(SyncErrorCode.Closed, "Встречное подключение: соединение закрыто", "simultaneous")
        }
        val commit = decodeB64(o1, "commit", 32)
        val secret = cfg.secretFor(peer.info.id)

        val key = cfg.testKeyPair ?: SyncCrypto.generateKeyPair()
        val nonce = cfg.testNonce ?: SyncCrypto.randomBytes(SyncCrypto.NONCE_LEN)
        val f2 = helloFrame(cfg.self, Role.Responder, secret != null) {
            put("key", b64e.encodeToString(key.publicSec1))
            put("nonce", b64e.encodeToString(nonce))
        }
        transport.writeFrame(f2)

        val f3 = timed(cfg.frameTimeoutMs, "ключа") { transport.readFrame() }
        val o3 = SyncMessages.parseObject(f3) ?: throw protocol("Неверный ключ собеседника")
        if ((o3["t"] as? JsonPrimitive)?.takeIf { it.isString }?.content != "reveal") {
            throw protocol("Ожидался ключ собеседника")
        }
        val peerKey = decodeB64(o3, "key", SyncCrypto.PUBLIC_KEY_LEN)
        val peerNonce = decodeB64(o3, "nonce", SyncCrypto.NONCE_LEN)
        if (!SyncCrypto.constantTimeEquals(SyncCrypto.sha256(peerKey, peerNonce), commit)) {
            throw SyncException(SyncErrorCode.Crypto, "Ключ собеседника не совпал с обязательством: соединение закрыто")
        }
        if (!SyncCrypto.isValidPublicKey(peerKey)) throw SyncException(SyncErrorCode.Crypto, "Неверный ключ собеседника")

        val z = SyncCrypto.ecdh(key, peerKey)
        verify(
            transport, cfg, Role.Responder, peer.info, f1, f2, f3, z,
            knownSelf = secret != null, knownPeer = peer.known, secret = secret, onMode = onMode,
        )
    }

    // ---- Проверка: узнавание или сопряжение (§3.4) ----

    private suspend fun verify(
        transport: FrameTransport,
        cfg: HandshakeConfig,
        role: Role,
        peer: PeerInfo,
        f1: ByteArray,
        f2: ByteArray,
        f3: ByteArray,
        z: ByteArray,
        knownSelf: Boolean,
        knownPeer: Boolean,
        secret: ByteArray?,
        onMode: (PeerInfo, HandshakeMode) -> Unit,
    ): HandshakeResult {
        val th = SessionKeys.transcriptHash(f1, f2, f3)
        val keys = SessionKeys.derive(z, th)
        val sendKey = if (role == Role.Initiator) keys.i2r else keys.r2i
        val recvKey = if (role == Role.Initiator) keys.r2i else keys.i2r
        val channel = SecureChannel(transport, sendKey, recvKey)

        if (knownSelf && knownPeer && secret != null) {
            onMode(peer, HandshakeMode.Recognition)
            recognize(channel, cfg, role, secret, th)
            return HandshakeResult(peer, role, recognized = true, newSecret = null, channel = channel)
        }

        val decision = CompletableDeferred<Boolean>()
        onMode(peer, HandshakeMode.Pairing(keys.sas, decision))
        pair(channel, cfg, decision)
        return HandshakeResult(peer, role, recognized = false, newSecret = keys.pair, channel = channel)
    }

    private suspend fun recognize(channel: SecureChannel, cfg: HandshakeConfig, role: Role, secret: ByteArray, th: ByteArray) {
        channel.send(SyncMessage.Auth(SessionKeys.authMac(secret, role, th)))
        val msg = timed(cfg.frameTimeoutMs, "проверки") { receiveStrict(channel) }
        when (msg) {
            is SyncMessage.Auth -> {
                val expected = SessionKeys.authMac(secret, role.other, th)
                if (!SyncCrypto.constantTimeEquals(expected, msg.mac)) {
                    sendByeEncrypted(channel, "auth")
                    throw SyncException(
                        SyncErrorCode.AuthFailed,
                        "Другое устройство не узнало это. Подтвердите код заново",
                        "auth",
                    )
                }
            }
            is SyncMessage.Bye -> throw byeFromPeer(msg.reason)
            else -> throw protocol("Неожиданное сообщение при проверке")
        }
    }

    /** Ждём решения пользователя и `confirm`/`bye` собеседника одновременно. */
    private suspend fun pair(channel: SecureChannel, cfg: HandshakeConfig, decision: CompletableDeferred<Boolean>) = coroutineScope {
        // Ошибка чтения (bye, обрыв, мусор) обрывает весь coroutineScope и ожидание решения.
        val peerConfirm = async {
            val msg = withTimeoutOrNull(cfg.confirmTimeoutMs) { receiveStrict(channel) }
                ?: throw rejectedByTimeout(channel)
            when (msg) {
                is SyncMessage.Confirm -> Unit
                is SyncMessage.Bye -> throw byeFromPeer(msg.reason)
                else -> throw protocol("Неожиданное сообщение при сопряжении")
            }
        }
        val agreed = withTimeoutOrNull(cfg.confirmTimeoutMs) { decision.await() } ?: false
        if (!agreed) {
            sendByeEncrypted(channel, "rejected")
            throw SyncException(SyncErrorCode.Rejected, "Сопряжение отменено", "rejected")
        }
        channel.send(SyncMessage.Confirm)
        peerConfirm.await()
    }

    /** При проверке всё, кроме `auth`, `confirm` и `bye`, — повод закрыть соединение. */
    private suspend fun receiveStrict(channel: SecureChannel): SyncMessage =
        channel.receive() ?: throw protocol("Неожиданное сообщение при проверке")

    private suspend fun rejectedByTimeout(channel: SecureChannel): SyncException {
        sendByeEncrypted(channel, "rejected")
        return SyncException(SyncErrorCode.Rejected, "Сопряжение отменено: время вышло", "rejected")
    }

    // ---- Открытые кадры ----

    private class Hello(val info: PeerInfo, val acc: String, val known: Boolean)

    /** Кадр 1 или 2 не прошёл проверку версии, роли или id. */
    private class HelloRejected : Exception()

    private fun helloFrame(self: DeviceInfo, role: Role, known: Boolean, extra: kotlinx.serialization.json.JsonObjectBuilder.() -> Unit): ByteArray =
        buildJsonObject {
            put("t", "hello")
            put("v", 1)
            put("role", role.char.toString())
            put("id", self.id)
            put("name", cleanName(self.name))
            put("os", self.os)
            put("acc", self.acc)
            extra()
            put("known", known)
        }.toString().toByteArray(Charsets.UTF_8)

    /**
     * Проверки `hello` собеседника с ролью [expected] (§3.1): не `hello` — [SyncException] без `bye`;
     * версия, роль, формат id — [HelloRejected] (`bye version`); свой id — [SyncException] без `bye`.
     */
    private fun checkHello(o: JsonObject, expected: Role, self: DeviceInfo): Hello {
        if (o.str("t") != "hello") throw protocol("Ожидалось приветствие")
        val v = (o["v"] as? JsonPrimitive)?.takeIf { !it.isString }?.longOrNull
        val id = o.str("id")
        if (v != 1L || o.str("role") != expected.char.toString() || id == null || !HEX32.matches(id)) {
            throw HelloRejected()
        }
        if (id == self.id) throw protocol("Соединение с самим собой")
        val acc = o.str("acc") ?: throw protocol("В приветствии нет метки аккаунта")
        val known = (o["known"] as? JsonPrimitive)?.takeIf { !it.isString }?.booleanOrNull ?: false
        val os = o.str("os")?.takeIf { OS_RE.matches(it) } ?: "unknown"
        return Hello(PeerInfo(id, cleanName(o.str("name") ?: ""), os), acc, known)
    }

    private fun JsonObject.str(key: String): String? =
        (this[key] as? JsonPrimitive)?.takeIf { it.isString }?.content

    private fun decodeB64(o: JsonObject, key: String, size: Int): ByteArray {
        val bytes = try {
            b64d.decode(o.str(key) ?: throw protocol("В кадре нет поля «$key»"))
        } catch (e: IllegalArgumentException) {
            throw protocol("Поле «$key» не base64")
        }
        if (bytes.size != size) throw protocol("Поле «$key» неверной длины")
        return bytes
    }

    /** Имя без управляющих символов, пустое — «Устройство», не длиннее 64 кодовых точек. */
    private fun cleanName(name: String): String {
        val sb = StringBuilder()
        name.codePoints().filter { !Character.isISOControl(it) }.forEach { sb.appendCodePoint(it) }
        val clean = sb.toString()
        if (clean.isEmpty()) return "Устройство"
        if (clean.codePointCount(0, clean.length) <= MAX_NAME) return clean
        return clean.substring(0, clean.offsetByCodePoints(0, MAX_NAME))
    }

    private fun throwIfBye(o: JsonObject) {
        if (o.str("t") == "bye") throw byeFromPeer(o.str("reason") ?: "")
    }

    private fun byeFromPeer(reason: String): SyncException = SyncException(
        SyncErrorCode.PeerBye,
        when (reason) {
            "version" -> "Обновите Plastinka на другом устройстве"
            "account" -> "На другом устройстве другой аккаунт Яндекса"
            "busy" -> "Другое устройство уже связано с третьим"
            "auth" -> "Другое устройство не узнало это. Подтвердите код заново"
            "rejected" -> "Сопряжение отменено на другом устройстве"
            "user", "shutdown" -> "Другое устройство разорвало связь"
            else -> "Другое устройство закрыло соединение"
        },
        reason,
    )

    private fun protocol(text: String) = SyncException(SyncErrorCode.Protocol, text)

    private suspend fun sendBye(transport: FrameTransport, reason: String) {
        try {
            val body = buildJsonObject { put("t", "bye"); put("reason", reason) }.toString().toByteArray(Charsets.UTF_8)
            transport.writeFrame(body)
        } catch (_: SyncException) {
            // Соединение уже закрывается: отказ собеседнику — по возможности.
        }
    }

    private suspend fun sendByeEncrypted(channel: SecureChannel, reason: String) {
        try {
            channel.send(SyncMessage.Bye(reason))
        } catch (_: SyncException) {
        }
    }

    private suspend fun <T : Any> timed(ms: Long, what: String, block: suspend () -> T): T =
        withTimeoutOrNull(ms) { block() }
            ?: throw SyncException(SyncErrorCode.Timeout, "Нет $what от другого устройства")

    /** Закрыть соединение при любой неудаче, в том числе при отмене. */
    private suspend fun guarded(transport: FrameTransport, block: suspend () -> HandshakeResult): HandshakeResult {
        var ok = false
        try {
            val r = block()
            ok = true
            return r
        } finally {
            if (!ok) transport.close()
        }
    }
}
