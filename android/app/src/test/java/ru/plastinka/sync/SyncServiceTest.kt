package ru.plastinka.sync

import java.net.ServerSocket
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.math.abs
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertNotNull
import kotlin.test.assertTrue
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout

class MemSecrets : SecretStore {
    val map = ConcurrentHashMap<String, ByteArray>()

    @Volatile
    var failSave = false
    override fun load(peerId: String): ByteArray? = map[peerId]
    override fun save(peerId: String, secret: ByteArray): Boolean {
        if (failSave) return false
        map[peerId] = secret
        return true
    }

    override fun delete(peerId: String) {
        map.remove(peerId)
    }
}

class MemPaired : PairedStore {
    @Volatile
    var list: List<PairedDevice> = emptyList()
    override fun load() = list
    override fun save(list: List<PairedDevice>) {
        this.list = list
    }
}

class FakeDiscovery : Discovery {
    private var found: ((RawService) -> Unit)? = null
    private var lost: ((String) -> Unit)? = null
    var announcement: Announcement? = null
    override fun start(announcement: Announcement, onFound: (RawService) -> Unit, onLost: (String) -> Unit) {
        this.announcement = announcement
        found = onFound
        lost = onLost
    }

    override fun stop() {
        found = null
        lost = null
    }

    fun emit(raw: RawService) = found?.invoke(raw)
    fun lose(name: String) = lost?.invoke(name)
}

class SyncServiceTest {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val idA = "1".repeat(32)
    private val idB = "2".repeat(32)
    private val idC = "3".repeat(32)
    private val acc = "0123456789ab"
    private val extraSockets = CopyOnWriteArrayList<ServerSocket>()

    private val fast = SyncTiming(
        frameTimeoutMs = 5_000,
        confirmTimeoutMs = 10_000,
        connectTimeoutMs = 2_000,
        firstPingIntervalMs = 50,
        pingIntervalMs = 100,
        deadAfterMs = 3_000,
        reconnectWindowMs = 20_000,
        reconnectIntervalMs = 100,
        shutdownByeTimeoutMs = 500,
    )

    private inner class Node(
        val id: String,
        val name: String,
        clockBase: Long,
        timing: SyncTiming = fast,
    ) {
        val secrets = MemSecrets()
        val paired = MemPaired()
        val disc = FakeDiscovery()
        val events = CopyOnWriteArrayList<SyncEvent>()
        val clock: () -> Long = { clockBase + System.nanoTime() / 1_000_000 }
        val svc = SyncService(secrets, paired, disc, scope, clock, timing, preferredPort = 0)
        val device = DeviceInfo(id, name, "android", acc)

        init {
            scope.launch(Dispatchers.Unconfined, start = CoroutineStart.UNDISPATCHED) {
                svc.events.collect { events.add(it) }
            }
            svc.start(device)
        }

        val port: Int get() = svc.ui.value.listenPort

        fun raw() = RawService(
            serviceName = name,
            txt = mapOf("v" to "1", "id" to id, "acc" to acc, "os" to "android"),
            addresses = listOf("127.0.0.1"),
            port = port,
        )

        fun await(timeoutMs: Long = 15_000, pred: (SyncUi) -> Boolean): SyncUi =
            runBlocking { withTimeout(timeoutMs) { svc.ui.first(pred) } }

        fun linkedCount() = events.count { it is SyncEvent.Linked }

        fun awaitLinkedEvents(n: Int) {
            runBlocking {
                withTimeout(15_000) {
                    while (linkedCount() < n) delay(20)
                }
            }
        }
    }

    private fun introduce(a: Node, b: Node) {
        a.disc.emit(b.raw())
        b.disc.emit(a.raw())
    }

    private fun pair(a: Node, b: Node) {
        a.svc.connect(b.id)
        a.await { it.link is LinkState.Pairing }
        b.await { it.link is LinkState.Pairing }
        a.svc.decide(true)
        b.svc.decide(true)
        a.await { it.link is LinkState.Linked }
        b.await { it.link is LinkState.Linked }
    }

    private fun sleep(ms: Long) = runBlocking { delay(ms) }

    @AfterTest
    fun tearDown() {
        extraSockets.forEach { runCatching { it.close() } }
        scope.cancel()
    }

    @Test
    fun pairingAndMessageExchange() {
        val a = Node(idA, "Узел A", 0)
        val b = Node(idB, "Узел B", 5_000)
        introduce(a, b)
        assertEquals(idB, a.await { it.visible.isNotEmpty() }.visible.first().id)

        a.svc.connect(idB)
        val pa = assertIs<LinkState.Pairing>(a.await { it.link is LinkState.Pairing }.link)
        val pb = assertIs<LinkState.Pairing>(b.await { it.link is LinkState.Pairing }.link)
        assertEquals(pa.request.code, pb.request.code)
        assertEquals(idB, pa.request.peer.id)
        assertEquals(Role.Initiator, pa.request.role)
        // Пока решил один, связи нет.
        a.svc.decide(true)
        sleep(200)
        assertFalse(b.svc.ui.value.link is LinkState.Linked)
        b.svc.decide(true)
        a.await { it.link is LinkState.Linked }
        b.await { it.link is LinkState.Linked }

        val secretA = assertNotNull(a.secrets.map[idB])
        assertContentEquals(secretA, b.secrets.map[idA])
        assertEquals(listOf(PairedDevice(idB, "Узел B", "android")), a.paired.list)
        assertEquals(listOf(PairedDevice(idA, "Узел A", "android")), b.paired.list)
        assertTrue(a.events.any { it is SyncEvent.Paired })
        assertTrue(a.svc.ui.value.visible.first().paired)

        // state: at уходит в часах отправителя, получатель видит его в своих (смещение часов 5000 мс).
        assertTrue(a.svc.send(SyncMessage.State(sampleState(by = idA).copy(at = a.clock()))))
        val got = awaitMessage(b) { it is SyncMessage.State } as SyncMessage.State
        assertTrue(abs(got.state.at - b.clock()) < 300, "at=${got.state.at} clock=${b.clock()}")
        assertEquals(61_234L, got.state.positionMs)
        assertEquals("12345", got.state.track?.id)

        // tick в обратную сторону.
        assertTrue(b.svc.send(SyncMessage.Tick(5, 1000, b.clock(), false)))
        val tick = awaitMessage(a) { it is SyncMessage.Tick } as SyncMessage.Tick
        assertTrue(abs(tick.at - a.clock()) < 300)
        assertEquals(5L, tick.rev)

        assertTrue(a.svc.send(SyncMessage.Command("next")))
        assertEquals(SyncMessage.Command("next"), awaitMessage(b) { it is SyncMessage.Command })
        assertFalse(a.svc.send(SyncMessage.Ping(1, 1)))
    }

    private fun awaitMessage(n: Node, pred: (SyncMessage) -> Boolean): SyncMessage = runBlocking {
        withTimeout(15_000) {
            var found: SyncMessage? = null
            while (found == null) {
                found = n.events.filterIsInstance<SyncEvent.Message>().firstOrNull { pred(it.msg) }?.msg
                if (found == null) delay(20)
            }
            found
        }
    }

    @Test
    fun byeUserThenRecognitionWithoutCode() {
        val a = Node(idA, "Узел A", 0)
        val b = Node(idB, "Узел B", 777)
        introduce(a, b)
        pair(a, b)

        a.svc.disconnect()
        a.await { it.link is LinkState.Idle }
        b.await { it.link is LinkState.Idle }
        val ua = a.events.filterIsInstance<SyncEvent.Unlinked>().single()
        val ub = b.events.filterIsInstance<SyncEvent.Unlinked>().single()
        assertTrue(ua.selfInitiated)
        assertFalse(ua.dropped)
        assertFalse(ub.selfInitiated)
        assertFalse(ub.dropped)
        assertTrue(ub.text.contains("разорвана"))
        // Восстановления после «Отключить» нет.
        sleep(500)
        assertEquals(LinkState.Idle, b.svc.ui.value.link)
        assertEquals(LinkState.Idle, a.svc.ui.value.link)

        // Повторное подключение узнаётся без кода: до Linked дойдёт без решений пользователя.
        a.svc.connect(idB)
        a.await { it.link is LinkState.Linked }
        b.await { it.link is LinkState.Linked }
        assertEquals(2, a.linkedCount())
    }

    @Test
    fun droppedConnectionIsRestoredAutomatically() {
        val a = Node(idA, "Узел A", 0)
        val b = Node(idB, "Узел B", 1234)
        introduce(a, b)
        pair(a, b)

        a.svc.dropConnectionForTest()
        val ub = runBlocking {
            withTimeout(15_000) {
                while (b.events.none { it is SyncEvent.Unlinked }) delay(20)
                b.events.filterIsInstance<SyncEvent.Unlinked>().first()
            }
        }
        assertTrue(ub.dropped)
        assertFalse(ub.selfInitiated)
        // Обе стороны восстанавливают связь сами (встречное подключение разрешается правилом §6).
        a.awaitLinkedEvents(2)
        b.awaitLinkedEvents(2)
        a.await { it.link is LinkState.Linked }
        b.await { it.link is LinkState.Linked }
    }

    @Test
    fun silentPeerIsDetectedByWatchdog() {
        val quiet = fast.copy(firstPingCount = 1, pingIntervalMs = 1_000_000, deadAfterMs = 700, reconnectWindowMs = 1)
        val a = Node(idA, "Узел A", 0, quiet)
        val b = Node(idB, "Узел B", 0, quiet.copy(deadAfterMs = 60_000))
        introduce(a, b)
        pair(a, b)
        val u = runBlocking {
            withTimeout(10_000) {
                while (a.events.none { it is SyncEvent.Unlinked }) delay(20)
                a.events.filterIsInstance<SyncEvent.Unlinked>().first()
            }
        }
        assertTrue(u.dropped)
        assertTrue(u.text.contains("потеряна"))
    }

    @Test
    fun thirdDeviceGetsBusy() {
        val a = Node(idA, "Узел A", 0)
        val b = Node(idB, "Узел B", 0)
        val c = Node(idC, "Узел C", 0)
        introduce(a, b)
        introduce(c, a)
        pair(a, b)

        c.svc.connect(idA)
        val ui = c.await { it.problem != null }
        assertTrue(ui.problem?.contains("уже связано") == true, ui.problem)
        c.await { it.link is LinkState.Idle }
        assertTrue(a.svc.ui.value.link is LinkState.Linked)
        assertTrue(b.svc.ui.value.link is LinkState.Linked)
    }

    @Test
    fun slotIsReleasedAfterFailedHandshake() {
        val a = Node(idA, "Узел A", 0)
        val b = Node(idB, "Узел B", 0)
        // «Устройство», которое принимает соединение и сразу закрывает.
        val bogus = ServerSocket(0).also { extraSockets.add(it) }
        scope.launch(Dispatchers.IO) {
            while (true) {
                val s = try {
                    bogus.accept()
                } catch (_: Exception) {
                    break
                }
                s.close()
            }
        }
        val idX = "4".repeat(32)
        a.disc.emit(RawService("Призрак", mapOf("v" to "1", "id" to idX, "acc" to acc, "os" to "linux"), listOf("127.0.0.1"), bogus.localPort))
        introduce(a, b)

        a.svc.connect(idX)
        a.await { it.problem != null }
        a.await { it.link is LinkState.Idle }
        a.svc.clearProblem()

        // Отказ при сопряжении тоже снимает отметку.
        a.svc.connect(idB)
        a.await { it.link is LinkState.Pairing }
        b.await { it.link is LinkState.Pairing }
        b.svc.decide(false)
        a.await { it.problem != null }
        a.await { it.link is LinkState.Idle }
        b.await { it.link is LinkState.Idle }

        // «Отмена» после своего «да» обрывает подключение.
        a.svc.clearProblem()
        a.svc.connect(idB)
        a.await { it.link is LinkState.Pairing }
        b.await { it.link is LinkState.Pairing }
        a.svc.decide(true)
        a.svc.decide(false)
        a.await { it.link is LinkState.Idle }
        b.await { it.link is LinkState.Idle }

        // После трёх неудач связь устанавливается.
        a.svc.clearProblem()
        b.svc.clearProblem()
        pair(a, b)
    }

    @Test
    fun simultaneousConnectionsLeaveOneLink() {
        val a = Node(idA, "Узел A", 0)
        val b = Node(idB, "Узел B", 0)
        val secret = ByteArray(32) { 7 }
        a.secrets.map[idB] = secret
        b.secrets.map[idA] = secret
        a.paired.list = listOf(PairedDevice(idB, "Узел B", "android"))
        b.paired.list = listOf(PairedDevice(idA, "Узел A", "android"))
        introduce(a, b)
        val gate = CompletableDeferred<Unit>()
        scope.launch { gate.await(); a.svc.connect(idB) }
        scope.launch { gate.await(); b.svc.connect(idA) }
        gate.complete(Unit)
        a.await(20_000) { it.link is LinkState.Linked }
        b.await(20_000) { it.link is LinkState.Linked }
        sleep(300)
        assertTrue(a.svc.ui.value.link is LinkState.Linked)
        assertTrue(b.svc.ui.value.link is LinkState.Linked)
    }

    @Test
    fun stopSendsShutdown() {
        val a = Node(idA, "Узел A", 0)
        val b = Node(idB, "Узел B", 0)
        introduce(a, b)
        pair(a, b)
        runBlocking { b.svc.stop() }
        a.await { it.link is LinkState.Idle }
        val u = a.events.filterIsInstance<SyncEvent.Unlinked>().single()
        assertFalse(u.dropped)
        assertFalse(u.selfInitiated)
        assertFalse(b.svc.ui.value.running)
        sleep(300)
        assertEquals(LinkState.Idle, a.svc.ui.value.link)
    }

    @Test
    fun secretStaysInMemoryWhenStorageFails() {
        val a = Node(idA, "Узел A", 0)
        val b = Node(idB, "Узел B", 0)
        a.secrets.failSave = true
        introduce(a, b)
        pair(a, b)
        assertNotNull(a.svc.ui.value.storageWarning)
        assertTrue(a.secrets.map.isEmpty())
        assertNotNull(b.secrets.map[idA])
        assertTrue(b.svc.ui.value.storageWarning == null)
        // Секрет держится в памяти: повторная связь узнаётся без кода.
        a.svc.disconnect()
        a.await { it.link is LinkState.Idle }
        b.await { it.link is LinkState.Idle }
        a.svc.connect(idB)
        a.await { it.link is LinkState.Linked }
    }

    @Test
    fun forgetRemovesSecretAndEntry() {
        val a = Node(idA, "Узел A", 0)
        val b = Node(idB, "Узел B", 0)
        introduce(a, b)
        pair(a, b)
        a.svc.forget(idB)
        assertTrue(a.secrets.map.isEmpty())
        assertTrue(a.paired.list.isEmpty())
        assertFalse(a.svc.ui.value.visible.first().paired)
    }

    @Test
    fun discoveryFiltersForeignDevices() {
        val a = Node(idA, "Узел A", 0)
        val base = mapOf("v" to "1", "id" to idB, "acc" to acc, "os" to "linux")
        a.disc.emit(RawService("свой", base + ("id" to idA), listOf("10.0.0.5"), 47800))
        a.disc.emit(RawService("чужой акк", base + ("acc" to "ffffffffffff"), listOf("10.0.0.5"), 47800))
        a.disc.emit(RawService("версия 2", base + ("v" to "2"), listOf("10.0.0.5"), 47800))
        a.disc.emit(RawService("мусор", mapOf("id" to "zz"), listOf("10.0.0.5"), 47800))
        a.disc.emit(RawService("без адреса", base, listOf("fe80::1"), 47800))
        assertTrue(a.svc.ui.value.visible.isEmpty())
        a.disc.emit(RawService("Телефон", base, listOf("8.8.8.8", "192.168.1.5", "fe80::1"), 47801))
        val v = a.svc.ui.value.visible.single()
        assertEquals(listOf("192.168.1.5", "8.8.8.8"), v.addresses)
        assertEquals(47801, v.port)
        a.disc.lose("Телефон")
        assertTrue(a.svc.ui.value.visible.isEmpty())
        assertEquals(idA, a.disc.announcement?.id)
    }

    @Test
    fun connectByAddress() {
        val a = Node(idA, "Узел A", 0)
        val b = Node(idB, "Узел B", 0)
        a.svc.connectAddress("не адрес")
        a.await { it.problem != null }
        a.svc.clearProblem()
        a.svc.connectAddress("127.0.0.1:${b.port}")
        a.await { it.link is LinkState.Pairing }
        b.await { it.link is LinkState.Pairing }
        a.svc.decide(true)
        b.svc.decide(true)
        a.await { it.link is LinkState.Linked }
    }
}

class SyncPolicyTest {
    @Test
    fun incomingDecisions() {
        val me = "5".repeat(32)
        val lower = "3".repeat(32)
        val higher = "8".repeat(32)
        assertEquals(IncomingDecision.Accept, SyncPolicy.decideIncoming(me, lower, null, null))
        assertEquals(IncomingDecision.Busy, SyncPolicy.decideIncoming(me, lower, SlotKind.Linked, lower))
        assertEquals(IncomingDecision.Busy, SyncPolicy.decideIncoming(me, lower, SlotKind.Incoming, "x"))
        assertEquals(IncomingDecision.Busy, SyncPolicy.decideIncoming(me, lower, SlotKind.Outgoing, "other"))
        assertEquals(IncomingDecision.Busy, SyncPolicy.decideIncoming(me, lower, SlotKind.Outgoing, null))
        // Встречное: остаётся соединение инициатора с меньшим id.
        assertEquals(IncomingDecision.ReplaceOutgoing, SyncPolicy.decideIncoming(me, lower, SlotKind.Outgoing, lower))
        assertEquals(IncomingDecision.DropSilently, SyncPolicy.decideIncoming(me, higher, SlotKind.Outgoing, higher))
    }

    @Test
    fun accountLabelMatchesSha256() {
        // SHA-256("plastinka/1 account 123456"), первые 6 байт.
        val expected = SyncCrypto.sha256("plastinka/1 account 123456".toByteArray()).take(6).joinToString("") { "%02x".format(it) }
        assertEquals(expected, SyncPolicy.accountLabel(123456))
        assertEquals(12, SyncPolicy.accountLabel(1).length)
        assertTrue(SyncPolicy.accountLabel(1) != SyncPolicy.accountLabel(2))
    }

    @Test
    fun instanceNameFitsIn63Bytes() {
        assertEquals("Plastinka", SyncPolicy.instanceName(""))
        val long = "я".repeat(40) // 80 байт
        val cut = SyncPolicy.instanceName(long)
        assertTrue(cut.toByteArray().size <= 63)
        assertEquals(31, cut.length)
        assertEquals("Pixel 8", SyncPolicy.instanceName("Pixel 8"))
    }

    @Test
    fun addressHelpers() {
        assertEquals(listOf("10.0.0.1", "172.16.0.2", "192.168.0.3", "8.8.8.8"), SyncPolicy.sortAddresses(listOf("8.8.8.8", "10.0.0.1", "fe80::1", "172.16.0.2", "192.168.0.3", "10.0.0.1")))
        assertEquals("172.32.0.1", SyncPolicy.sortAddresses(listOf("172.32.0.1", "172.31.0.1")).last())
        assertEquals("192.168.1.5" to 47800, SyncPolicy.parseEndpoint("192.168.1.5"))
        assertEquals("192.168.1.5" to 1234, SyncPolicy.parseEndpoint(" 192.168.1.5:1234 "))
        assertEquals(null, SyncPolicy.parseEndpoint("192.168.1.5:0"))
        assertEquals(null, SyncPolicy.parseEndpoint("192.168.1.5:abc"))
        assertEquals(null, SyncPolicy.parseEndpoint("example.com:1"))
        assertEquals(null, SyncPolicy.parseEndpoint("300.1.1.1"))
    }

    @Test
    fun problemTexts() {
        fun t(code: SyncErrorCode, reason: String?) = SyncPolicy.problemText(SyncException(code, "x", reason), "Pixel")
        assertEquals("Обновите Plastinka на Pixel", t(SyncErrorCode.PeerBye, "version"))
        assertEquals("На Pixel другой аккаунт Яндекса", t(SyncErrorCode.PeerBye, "account"))
        assertEquals("Pixel уже связано с другим устройством", t(SyncErrorCode.PeerBye, "busy"))
        assertEquals("Сопряжение отменено", t(SyncErrorCode.Rejected, "rejected"))
        assertEquals("Pixel не узнало это устройство. Подтвердите код заново", t(SyncErrorCode.AuthFailed, "auth"))
    }
}
