package ru.plastinka.sync

import java.net.NetworkInterface

/** Сопряжённое устройство (несекретное: секрет лежит в [SecretStore]). */
data class PairedDevice(val id: String, val name: String, val os: String)

/** Устройство, найденное в сети (прошло фильтр §2). */
data class VisibleDevice(
    val id: String,
    val name: String,
    val os: String,
    /** Адреса IPv4, частные диапазоны первыми. */
    val addresses: List<String>,
    val port: Int,
    /** Уже сопряжено: код подтверждать не придётся. */
    val paired: Boolean,
)

/** Запрос на подтверждение кода сопряжения (§3.4). Ответ — [SyncService.decide]. */
data class PairingRequest(val peer: PeerInfo, val role: Role, val code: String, val displayCode: String)

/** Состояние связи для экранов. */
sealed interface LinkState {
    data object Idle : LinkState

    /** Подключаемся или идёт проверка узнавания; [peerName] может быть неизвестно. */
    data class Connecting(val peerName: String?) : LinkState

    /** Ждём решения пользователя о коде. */
    data class Pairing(val request: PairingRequest) : LinkState

    /** Связь готова: рукопожатие пройдено и пришёл первый `pong`. */
    data class Linked(val peer: PeerInfo, val role: Role) : LinkState

    /** Связь оборвалась сама, пробуем восстановить (§5.3). */
    data class Reconnecting(val peerName: String) : LinkState
}

/** Всё, что показывают экраны «Слушать вместе». */
data class SyncUi(
    val running: Boolean = false,
    val visible: List<VisibleDevice> = emptyList(),
    val paired: List<PairedDevice> = emptyList(),
    /** Свои адреса `ip:port` для подключения вручную. */
    val addresses: List<String> = emptyList(),
    val listenPort: Int = 0,
    val link: LinkState = LinkState.Idle,
    /** Последняя проблема для показа пользователю (понятный текст), `null` — нет. */
    val problem: String? = null,
    /** Секрет сопряжения не удалось записать в защищённое хранилище: он живёт до конца процесса. */
    val storageWarning: String? = null,
)

/** События для плеера и экранов. */
sealed interface SyncEvent {
    /** Связь готова (после первого `pong`); [join] — мы подключались сами («Подключить»). */
    data class Linked(val peer: PeerInfo, val role: Role, val join: Boolean) : SyncEvent

    /**
     * Связь закрыта. [dropped] — оборвалась сама (будет восстановление), [selfInitiated] —
     * закрыли мы («Отключить», остановка). [text] — «Связь с <имя> разорвана» или причина.
     */
    data class Unlinked(val peer: PeerInfo, val text: String, val dropped: Boolean, val selfInitiated: Boolean) : SyncEvent

    /** Входящее сообщение; `at` в `state` и `tick` уже переведено в свои часы. */
    data class Message(val msg: SyncMessage) : SyncEvent

    /** Сопряжение состоялось, секрет сохранён (или остался в памяти). */
    data class Paired(val peer: PeerInfo) : SyncEvent
}

/** Все интервалы и таймауты (мс): параметры нужны быстрым тестам. */
data class SyncTiming(
    val frameTimeoutMs: Long = 10_000,
    val confirmTimeoutMs: Long = 120_000,
    val connectTimeoutMs: Int = 3_000,
    val firstPingCount: Int = 5,
    val firstPingIntervalMs: Long = 200,
    val pingIntervalMs: Long = 3_000,
    /** Ничего не пришло столько — связь потеряна (§4.2). */
    val deadAfterMs: Long = 15_000,
    val reconnectWindowMs: Long = 600_000,
    val reconnectIntervalMs: Long = 5_000,
    val shutdownByeTimeoutMs: Long = 500,
)

/** Хранилище секретов сопряжения. Реализация на Android — зашифрованное, запасного файла нет. */
interface SecretStore {
    fun load(peerId: String): ByteArray?

    /** `true` — секрет записан надёжно. */
    fun save(peerId: String, secret: ByteArray): Boolean
    fun delete(peerId: String)
}

/** Несекретный список сопряжённых. */
interface PairedStore {
    fun load(): List<PairedDevice>
    fun save(list: List<PairedDevice>)
}

/** Свои данные для объявления в сети (§2). */
data class Announcement(val instanceName: String, val port: Int, val id: String, val acc: String, val os: String)

/** Служба, найденная в сети, как её отдаёт NSD: фильтр и сортировка — в [SyncService]. */
data class RawService(
    val serviceName: String,
    val txt: Map<String, String>,
    val addresses: List<String>,
    val port: Int,
)

/** Обнаружение устройств (на Android — NSD; в тестах — заглушка). Обратные вызовы можно звать из любого потока. */
interface Discovery {
    /** Начать объявление и поиск. Ошибка — исключение, служба продолжит работать без поиска. */
    fun start(announcement: Announcement, onFound: (RawService) -> Unit, onLost: (serviceName: String) -> Unit)
    fun stop()
}

/** Решение о встречном соединении (§6, правило «меньший id инициатора»). */
enum class IncomingDecision {
    /** Свободны: принять. */
    Accept,

    /** Занят: ответить `bye busy`. */
    Busy,

    /** Встречное подключение, наше исходящее остаётся: закрыть входящее без `bye`. */
    DropSilently,

    /** Встречное подключение, входящее остаётся: отменить своё исходящее и принять. */
    ReplaceOutgoing,
}

/** Чем занят слот единственной связи. */
enum class SlotKind { Outgoing, Incoming, Linked }

object SyncPolicy {
    /**
     * Чистое правило §6: у устройства не больше одной связи. Если идёт наше исходящее
     * подключение к тому же устройству, которое подключилось к нам, остаётся соединение
     * с меньшим id инициатора (сравнение строк). Инициатор нашего исходящего — мы.
     */
    fun decideIncoming(selfId: String, initiatorId: String, slotKind: SlotKind?, slotPeerId: String?): IncomingDecision = when {
        slotKind == null -> IncomingDecision.Accept
        slotKind == SlotKind.Outgoing && slotPeerId != null && slotPeerId == initiatorId ->
            if (initiatorId < selfId) IncomingDecision.ReplaceOutgoing else IncomingDecision.DropSilently
        else -> IncomingDecision.Busy
    }

    /** Метка аккаунта §2: первые 12 hex-символов `SHA-256("plastinka/1 account " + uid)`. */
    fun accountLabel(uid: Long): String =
        SyncCrypto.sha256("plastinka/1 account $uid".toByteArray(Charsets.UTF_8)).take(6)
            .joinToString("") { "%02x".format(it) }

    /** Имя экземпляра mDNS: не длиннее 63 байт UTF-8, обрезка по кодовым точкам. */
    fun instanceName(name: String): String {
        val sb = StringBuilder()
        var bytes = 0
        var i = 0
        while (i < name.length) {
            val cp = name.codePointAt(i)
            val n = String(Character.toChars(cp)).toByteArray(Charsets.UTF_8).size
            if (bytes + n > 63) break
            sb.appendCodePoint(cp)
            bytes += n
            i += Character.charCount(cp)
        }
        return sb.toString().ifEmpty { "Plastinka" }
    }

    private val IPV4 = Regex("""(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})""")

    /** Октеты IPv4 из строки вида `a.b.c.d` или `null`. */
    fun parseIpv4(text: String): List<Int>? {
        val m = IPV4.matchEntire(text) ?: return null
        val parts = m.groupValues.drop(1).map { it.toInt() }
        return parts.takeIf { p -> p.all { it in 0..255 } }
    }

    /** Частный диапазон (10/8, 172.16/12, 192.168/16) или петля. */
    fun isPrivate(octets: List<Int>): Boolean {
        val a = octets.getOrNull(0) ?: return false
        val b = octets.getOrNull(1) ?: return false
        return a == 10 || a == 127 || (a == 172 && b in 16..31) || (a == 192 && b == 168)
    }

    /** Только IPv4, частные диапазоны первыми, без повторов; порядок внутри групп сохраняется. */
    fun sortAddresses(addresses: List<String>): List<String> {
        val ok = addresses.distinct().filter { parseIpv4(it) != null }
        val (priv, other) = ok.partition { parseIpv4(it)?.let(::isPrivate) == true }
        return priv + other
    }

    /** `ip:port` или `ip` (порт 47800) → пара или `null`. */
    fun parseEndpoint(text: String): Pair<String, Int>? {
        val t = text.trim()
        val host = t.substringBefore(':')
        val portText = if (t.contains(':')) t.substringAfter(':') else null
        if (parseIpv4(host) == null) return null
        val port = if (portText == null) DEFAULT_PORT else portText.toIntOrNull() ?: return null
        if (port !in 1..65535) return null
        return host to port
    }

    const val DEFAULT_PORT = 47800

    /** Свои адреса IPv4 как `ip:port` (частные первыми). Ошибка перечисления — пусто. */
    fun localAddresses(port: Int): List<String> = try {
        val all = NetworkInterface.getNetworkInterfaces()?.toList().orEmpty()
            .filter { it.isUp && !it.isLoopback }
            .flatMap { it.inetAddresses.toList() }
            .mapNotNull { it.hostAddress }
        sortAddresses(all).map { "$it:$port" }
    } catch (_: java.io.IOException) {
        emptyList()
    }

    /** Понятный текст для пользователя по ошибке протокола (§6). [name] — имя собеседника, если известно. */
    fun problemText(e: SyncException, name: String?): String {
        val who = name ?: "Другое устройство"
        return when (e.code) {
            SyncErrorCode.PeerBye, SyncErrorCode.Refused -> when (e.reason) {
                "version" -> "Обновите Plastinka на ${name ?: "другом устройстве"}"
                "account" -> "На ${name ?: "другом устройстве"} другой аккаунт Яндекса"
                "busy" -> "$who уже связано с другим устройством"
                "rejected" -> "Сопряжение отменено"
                "auth" -> "$who не узнало это устройство. Подтвердите код заново"
                "user", "shutdown" -> "Связь с ${name ?: "устройством"} разорвана"
                else -> "$who закрыло соединение"
            }
            SyncErrorCode.AuthFailed -> "$who не узнало это устройство. Подтвердите код заново"
            SyncErrorCode.Rejected -> "Сопряжение отменено"
            SyncErrorCode.Timeout -> "$who не ответило. Проверьте, что Plastinka открыта на нём"
            else -> "Не удалось связаться с ${name ?: "устройством"}: соединение прервано"
        }
    }
}
