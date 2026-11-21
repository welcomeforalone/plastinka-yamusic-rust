package ru.plastinka.sync

import java.io.IOException
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Служба «Слушать вместе»: приём и исходящие подключения, сопряжение, готовая связь с
 * пингами и часами, восстановление, объявление и поиск устройств. Без Android: сеть
 * идёт через сокеты JVM, обнаружение — через [Discovery], секреты — через [SecretStore],
 * часы подаются снаружи ([clock]: монотонные миллисекунды, на Android
 * `SystemClock.elapsedRealtime()`). Всё сетевое работает на `Dispatchers.IO`.
 *
 * У устройства не больше одной связи или одного рукопожатия за раз (слот). Отметка
 * «занят» снимается при любой ошибке и отмене. Времени собеседника снаружи не видно:
 * `at` во входящих `state`/`tick` уже в своих часах, в исходящих — подаётся в своих и уходит как есть.
 */
class SyncService(
    private val secrets: SecretStore,
    private val pairedStore: PairedStore,
    private val discovery: Discovery,
    private val scope: CoroutineScope,
    private val clock: () -> Long,
    private val timing: SyncTiming = SyncTiming(),
    private val preferredPort: Int = SyncPolicy.DEFAULT_PORT,
) {
    private val _ui = MutableStateFlow(SyncUi())

    /** Состояние для экранов. */
    val ui: StateFlow<SyncUi> = _ui.asStateFlow()

    private val _events = MutableSharedFlow<SyncEvent>(extraBufferCapacity = 512)

    /** События связи и входящие сообщения. Подписываться до [start]: без подписчика события не хранятся. */
    val events: SharedFlow<SyncEvent> = _events.asSharedFlow()

    private class Slot(var kind: SlotKind, var peerId: String?, val job: Job?)

    private val lock = Any()
    private var slot: Slot? = null
    private var link: Link? = null
    private var reconnectJob: Job? = null
    private var reconnectName: String = ""

    @Volatile
    private var self: DeviceInfo? = null

    @Volatile
    private var running = false
    private var server: ServerSocket? = null
    private var serverJob: Job? = null
    private val seen = ConcurrentHashMap<String, VisibleDevice>()
    private val memorySecrets = ConcurrentHashMap<String, ByteArray>()

    @Volatile
    private var pendingDecision: CompletableDeferred<Boolean>? = null

    @Volatile
    private var pairedList: List<PairedDevice> = emptyList()

    // ---- Запуск и остановка ----

    /** Запустить приём, объявление и поиск. Повторный вызов при работающей службе ничего не делает. */
    fun start(self: DeviceInfo) {
        synchronized(lock) {
            if (running) return
            running = true
        }
        this.self = self
        pairedList = try {
            pairedStore.load()
        } catch (_: Exception) {
            emptyList()
        }
        val srv = bind()
        if (srv == null) {
            running = false
            _ui.update { it.copy(problem = "Не удалось открыть порт для подключений других устройств") }
            return
        }
        server = srv
        _ui.update {
            it.copy(
                running = true,
                paired = pairedList,
                listenPort = srv.localPort,
                addresses = SyncPolicy.localAddresses(srv.localPort),
                link = LinkState.Idle,
                problem = null,
            )
        }
        serverJob = scope.launch(Dispatchers.IO) { acceptLoop(srv) }
        try {
            discovery.start(
                Announcement(SyncPolicy.instanceName(self.name), srv.localPort, self.id, self.acc, self.os),
                ::onFound,
                ::onLost,
            )
        } catch (e: Exception) {
            // Без поиска подключение по адресу и приём работают.
            _ui.update { it.copy(problem = "Поиск устройств в сети недоступен: ${e.javaClass.simpleName}") }
        }
    }

    /**
     * Остановить службу: связь закрывается с `bye shutdown` (короткий таймаут), приём,
     * объявление и поиск выключаются. Сопряжённые устройства остаются в списке.
     */
    suspend fun stop() {
        synchronized(lock) {
            if (!running) return
            running = false
        }
        cancelReconnect(null)
        pendingDecision?.complete(false)
        val l = synchronized(lock) { link }
        l?.closeBy("shutdown", selfInitiated = true)
        synchronized(lock) { slot?.job }?.cancel()
        try {
            discovery.stop()
        } catch (_: Exception) {
        }
        try {
            server?.close()
        } catch (_: IOException) {
        }
        server = null
        serverJob?.cancel()
        seen.clear()
        synchronized(lock) { slot = null; link = null }
        _ui.update {
            it.copy(running = false, visible = emptyList(), addresses = emptyList(), listenPort = 0, link = LinkState.Idle)
        }
    }

    /** Обновить список своих адресов (сеть могла смениться). */
    fun refreshAddresses() {
        val port = _ui.value.listenPort
        if (running && port != 0) _ui.update { it.copy(addresses = SyncPolicy.localAddresses(port)) }
    }

    private fun bind(): ServerSocket? {
        for (port in listOf(preferredPort, 0).distinct()) {
            try {
                val s = ServerSocket()
                s.reuseAddress = true
                s.bind(InetSocketAddress(port))
                return s
            } catch (_: IOException) {
            }
        }
        return null
    }

    // ---- Управление ----

    /** Подключиться к найденному устройству («Подключить»): адреса по очереди, без кода, если сопряжены. */
    fun connect(id: String) {
        scope.launch(Dispatchers.IO) {
            guard {
                cancelReconnect(null)
                val v = seen.values.firstOrNull { it.id == id }
                if (v == null) {
                    problem("Устройство не найдено в сети")
                } else {
                    connectOutgoing(v.addresses.map { it to v.port }, v.id, v.name, join = true, auto = false)
                }
            }
        }
    }

    /** Подключиться по адресу `ip:port` (запасной путь, §2). */
    fun connectAddress(text: String) {
        scope.launch(Dispatchers.IO) {
            guard {
                val ep = SyncPolicy.parseEndpoint(text)
                if (ep == null) {
                    problem("Введите адрес в виде 192.168.1.5:47800")
                } else {
                    cancelReconnect(null)
                    connectOutgoing(listOf(ep), null, null, join = true, auto = false)
                }
            }
        }
    }

    /** Решение пользователя о коде сопряжения: `true` — «Коды совпадают», `false` — «Отмена». */
    fun decide(ok: Boolean) {
        val decision = pendingDecision ?: return
        // Своё «да» уже ушло: «Отмена» обрывает подключение, второй ответ не примется.
        if (!decision.complete(ok) && !ok) disconnect()
    }

    /** «Отключить»: `bye user`, автоматического восстановления не будет. Отменяет и подключение в процессе. */
    fun disconnect() {
        scope.launch(Dispatchers.IO) {
            guard {
                cancelReconnect(null)
                pendingDecision?.complete(false)
                val l = synchronized(lock) { link }
                if (l != null) {
                    l.closeBy("user", selfInitiated = true)
                } else {
                    synchronized(lock) { slot?.job }?.cancel()
                    idleIfFree()
                }
            }
        }
    }

    /** «Забыть»: убрать запись и секрет на этом устройстве. Текущая связь не рвётся. */
    fun forget(id: String) {
        memorySecrets.remove(id)
        try {
            secrets.delete(id)
        } catch (_: Exception) {
        }
        pairedList = pairedList.filter { it.id != id }
        savePaired()
        _ui.update { it.copy(paired = pairedList) }
        publishVisible()
    }

    /**
     * Отправить сообщение собеседнику: `state`, `tick`, `command`, `liked`. `at` — в своих
     * часах ([clock]); на провод оно уходит как есть, перевод в свои часы делает получатель (§4.3). `false` — связи нет, она ещё не готова или сообщение не того вида.
     */
    fun send(msg: SyncMessage): Boolean {
        val l = synchronized(lock) { link } ?: return false
        return l.enqueue(msg)
    }

    /** Убрать показанную проблему. */
    fun clearProblem() {
        _ui.update { it.copy(problem = null) }
    }

    /** Только для тестов: оборвать соединение без `bye`, как обрыв сети. */
    internal fun dropConnectionForTest() {
        synchronized(lock) { link }?.dropForTest()
    }

    // ---- Приём ----

    private suspend fun acceptLoop(srv: ServerSocket) {
        while (running) {
            val socket = try {
                srv.accept()
            } catch (_: IOException) {
                return
            }
            try {
                socket.tcpNoDelay = true
                socket.keepAlive = true
            } catch (_: IOException) {
            }
            scope.launch(Dispatchers.IO) { guard { handleIncoming(socket) } }
        }
    }

    private suspend fun handleIncoming(socket: Socket) {
        val me = self
        val transport = try {
            transportOf(socket)
        } catch (_: IOException) {
            closeQuietly(socket)
            return
        }
        if (me == null) {
            transport.close()
            return
        }
        val job = currentCoroutineContext()[Job]
        var mySlot: Slot? = null
        var peerName: String? = null
        val cfg = HandshakeConfig(
            self = me,
            secretFor = ::secretFor,
            isBusy = { id -> admit(me, id, job) { mySlot = it } },
            frameTimeoutMs = timing.frameTimeoutMs,
            confirmTimeoutMs = timing.confirmTimeoutMs,
        )
        var established = false
        try {
            val res = Handshake.respond(transport, cfg) { p, m ->
                peerName = p.name
                onMode(p, m, Role.Responder, auto = false)
            }
            val s = mySlot ?: throw SyncException(SyncErrorCode.Protocol, "Нет слота связи")
            establish(res, s, null, join = false)
            established = true
        } catch (e: SyncException) {
            val code = e.code
            val quiet = e.reason == "simultaneous" || e.reason == "busy" || peerName == null ||
                code == SyncErrorCode.Closed || code == SyncErrorCode.Timeout ||
                code == SyncErrorCode.Frame || code == SyncErrorCode.Protocol
            if (!quiet) problem(SyncPolicy.problemText(e, peerName))
        } finally {
            if (!established) {
                mySlot?.let { releaseSlot(it) }
                pendingDecision = null
                idleIfFree()
            }
        }
    }

    /** Встречные и занятые случаи (§6): слот под входящее рукопожатие. */
    private fun admit(me: DeviceInfo, initiatorId: String, job: Job?, set: (Slot) -> Unit): HandshakeBusy = synchronized(lock) {
        if (!running) return@synchronized HandshakeBusy.Busy
        val s = slot
        when (SyncPolicy.decideIncoming(me.id, initiatorId, s?.kind, s?.peerId)) {
            IncomingDecision.Accept -> {
                val n = Slot(SlotKind.Incoming, initiatorId, job)
                slot = n
                set(n)
                HandshakeBusy.Free
            }
            IncomingDecision.ReplaceOutgoing -> {
                s?.job?.cancel()
                val n = Slot(SlotKind.Incoming, initiatorId, job)
                slot = n
                set(n)
                HandshakeBusy.Free
            }
            IncomingDecision.Busy -> HandshakeBusy.Busy
            IncomingDecision.DropSilently -> HandshakeBusy.DropSilently
        }
    }

    private fun releaseSlot(s: Slot) {
        synchronized(lock) {
            if (slot === s && s.kind != SlotKind.Linked) slot = null
        }
    }

    // ---- Исходящее подключение ----

    private enum class Outcome { Linked, Failed, Busy, Fatal }

    private suspend fun connectOutgoing(
        endpoints: List<Pair<String, Int>>,
        expectedId: String?,
        name: String?,
        join: Boolean,
        auto: Boolean,
    ): Outcome {
        val me = self ?: return Outcome.Failed
        val job = currentCoroutineContext()[Job]
        val mySlot = synchronized(lock) {
            if (!running || slot != null) null else Slot(SlotKind.Outgoing, expectedId, job).also { slot = it }
        }
        if (mySlot == null) {
            if (!auto) problem("Уже есть связь или подключение. Сначала отключитесь")
            return Outcome.Busy
        }
        var peerName = name
        var linked = false
        try {
            if (!auto) setLink(LinkState.Connecting(name))
            var socket: Socket? = null
            var used: Pair<String, Int>? = null
            for (ep in endpoints) {
                try {
                    socket = connectTcp(ep.first, ep.second, timing.connectTimeoutMs)
                    used = ep
                    break
                } catch (_: IOException) {
                }
            }
            if (socket == null || used == null) {
                if (!auto) {
                    problem(
                        "Не удалось подключиться к ${name ?: "устройству"}. Проверьте, что оба устройства в одной сети " +
                            "и брандмауэр пропускает подключение",
                    )
                }
                return Outcome.Failed
            }
            val transport = try {
                transportOf(socket)
            } catch (_: IOException) {
                closeQuietly(socket)
                return Outcome.Failed
            }
            val cfg = HandshakeConfig(
                self = me,
                secretFor = ::secretFor,
                frameTimeoutMs = timing.frameTimeoutMs,
                confirmTimeoutMs = timing.confirmTimeoutMs,
            )
            val res = Handshake.initiate(transport, cfg, expectedId, join) { p, m ->
                peerName = p.name
                onMode(p, m, Role.Initiator, auto)
            }
            establish(res, mySlot, used, join)
            linked = true
            return Outcome.Linked
        } catch (e: SyncException) {
            val fatal = e.code == SyncErrorCode.AuthFailed || e.code == SyncErrorCode.Rejected ||
                (e.code == SyncErrorCode.PeerBye && e.reason != "busy")
            if (!auto || fatal) problem(SyncPolicy.problemText(e, peerName))
            return if (auto && fatal) Outcome.Fatal else Outcome.Failed
        } finally {
            if (!linked) {
                releaseSlot(mySlot)
                pendingDecision = null
                if (!auto) idleIfFree()
            }
        }
    }

    private fun onMode(peer: PeerInfo, mode: HandshakeMode, role: Role, auto: Boolean) {
        when (mode) {
            HandshakeMode.Recognition -> if (!auto) setLink(LinkState.Connecting(peer.name))
            is HandshakeMode.Pairing -> if (auto) {
                // Автовосстановление не должно показывать код: нужен ручной вход.
                mode.decision.complete(false)
            } else {
                pendingDecision = mode.decision
                setLink(LinkState.Pairing(PairingRequest(peer, role, mode.code, mode.displayCode)))
            }
        }
    }

    // ---- Секреты и сопряжённые ----

    private fun secretFor(peerId: String): ByteArray? = memorySecrets[peerId] ?: try {
        secrets.load(peerId)?.also { memorySecrets[peerId] = it }
    } catch (_: Exception) {
        null
    }

    private fun savePaired() {
        try {
            pairedStore.save(pairedList)
        } catch (_: Exception) {
            // Список сопряжённых — удобство: без него устройства всё равно узнаются по секрету.
        }
    }

    private fun upsertPaired(peer: PeerInfo) {
        val entry = PairedDevice(peer.id, peer.name, peer.os)
        val old = pairedList.firstOrNull { it.id == peer.id }
        if (old == entry) return
        pairedList = pairedList.filter { it.id != peer.id } + entry
        savePaired()
        _ui.update { it.copy(paired = pairedList) }
        publishVisible()
    }

    private fun savePairing(peer: PeerInfo, secret: ByteArray) {
        memorySecrets[peer.id] = secret
        val ok = try {
            secrets.save(peer.id, secret)
        } catch (_: Exception) {
            false
        }
        if (!ok) {
            _ui.update {
                it.copy(
                    storageWarning = "Не удалось записать ключ сопряжения в защищённое хранилище: " +
                        "он сохранён только до закрытия приложения, код придётся подтверждать заново",
                )
            }
        }
        upsertPaired(peer)
        _events.tryEmit(SyncEvent.Paired(peer))
    }

    // ---- Готовая связь ----

    private fun establish(res: HandshakeResult, s: Slot, endpoint: Pair<String, Int>?, join: Boolean) {
        val secret = res.newSecret
        if (secret != null) savePairing(res.peer, secret) else upsertPaired(res.peer)
        val l = Link(res, s, endpoint, join)
        synchronized(lock) {
            s.kind = SlotKind.Linked
            s.peerId = res.peer.id
            link = l
        }
        pendingDecision = null
        cancelReconnect(null)
        l.start()
    }

    private inner class Link(
        val res: HandshakeResult,
        val slot: Slot,
        val endpoint: Pair<String, Int>?,
        val join: Boolean,
    ) {
        val peer: PeerInfo = res.peer
        private val est = ClockEstimator()
        private val outbox = Channel<SyncMessage>(Channel.UNLIMITED)
        private val ended = AtomicBoolean(false)

        @Volatile
        private var lastRecv = clock()

        @Volatile
        private var ready = false
        private val pending = ArrayList<SyncMessage>()

        @Volatile
        private var jobs: List<Job> = emptyList()

        fun start() {
            val writer = scope.launch(Dispatchers.IO) {
                try {
                    for (m in outbox) res.channel.send(m)
                } catch (e: SyncException) {
                    endByError(e)
                }
            }
            val reader = scope.launch(Dispatchers.IO) {
                try {
                    while (true) {
                        val m = res.channel.receive()
                        lastRecv = clock()
                        if (m != null) handle(m)
                    }
                } catch (e: SyncException) {
                    endByError(e)
                }
            }
            val pinger = scope.launch(Dispatchers.IO) {
                var n = 0
                while (true) {
                    outbox.trySend(est.nextPing(clock()))
                    n++
                    delay(if (n < timing.firstPingCount) timing.firstPingIntervalMs else timing.pingIntervalMs)
                }
            }
            val watchdog = scope.launch(Dispatchers.IO) {
                val step = (timing.deadAfterMs / 3).coerceIn(20, 1000)
                while (true) {
                    delay(step)
                    if (clock() - lastRecv > timing.deadAfterMs) {
                        end("Связь с ${peer.name} потеряна", dropped = true, selfInitiated = false)
                    }
                }
            }
            jobs = listOf(writer, reader, pinger, watchdog)
            if (ended.get()) jobs.forEach { it.cancel() }
        }

        /** Разбор входящего сообщения: только из корутины чтения, поэтому порядок сохраняется. */
        private fun handle(m: SyncMessage) {
            when (m) {
                is SyncMessage.Ping -> outbox.trySend(est.answer(m, clock()))
                is SyncMessage.Pong -> {
                    est.onPong(m, clock())
                    if (!ready && est.offsetMs != null) becomeReady()
                }
                is SyncMessage.State, is SyncMessage.Tick, is SyncMessage.Command, is SyncMessage.Liked ->
                    if (ready) deliver(m) else pending.add(m)
                is SyncMessage.Bye -> endByBye(m.reason)
                else -> Unit
            }
        }

        private fun becomeReady() {
            ready = true
            setLink(LinkState.Linked(peer, res.role))
            _events.tryEmit(SyncEvent.Linked(peer, res.role, join))
            val queued = pending.toList()
            pending.clear()
            queued.forEach { deliver(it) }
        }

        private fun deliver(m: SyncMessage) {
            val off = est.offsetMs ?: 0L
            val out = when (m) {
                is SyncMessage.State -> SyncMessage.State(m.state.copy(at = m.state.at - off))
                is SyncMessage.Tick -> m.copy(at = m.at - off)
                else -> m
            }
            _events.tryEmit(SyncEvent.Message(out))
        }

        fun enqueue(msg: SyncMessage): Boolean {
            if (!ready || ended.get()) return false
            // `at` уходит в своих часах как есть: в свои часы его переводит получатель (§4.3).
            return when (msg) {
                is SyncMessage.State, is SyncMessage.Tick, is SyncMessage.Command, is SyncMessage.Liked ->
                    outbox.trySend(msg).isSuccess
                else -> false
            }
        }

        /** Закрыть связь своей стороной с `bye` и коротким таймаутом. */
        suspend fun closeBy(reason: String, selfInitiated: Boolean) {
            withTimeoutOrNull(timing.shutdownByeTimeoutMs) {
                try {
                    res.channel.send(SyncMessage.Bye(reason))
                } catch (_: SyncException) {
                }
            }
            end("Связь с ${peer.name} разорвана", dropped = false, selfInitiated = selfInitiated)
        }

        fun dropForTest() = res.channel.close()

        private fun endByBye(reason: String) {
            val text = when (reason) {
                "user", "shutdown" -> "Связь с ${peer.name} разорвана"
                else -> SyncPolicy.problemText(
                    SyncException(SyncErrorCode.PeerBye, "bye", reason),
                    peer.name,
                )
            }
            end(text, dropped = false, selfInitiated = false)
        }

        private fun endByError(e: SyncException) {
            if (ended.get()) return
            end("Связь с ${peer.name} разорвана: ${e.message ?: "соединение прервано"}", dropped = true, selfInitiated = false)
        }

        fun end(text: String, dropped: Boolean, selfInitiated: Boolean) {
            if (!ended.compareAndSet(false, true)) return
            jobs.forEach { it.cancel() }
            outbox.close()
            res.channel.close()
            synchronized(lock) {
                if (link === this) link = null
                if (this@SyncService.slot === slot) this@SyncService.slot = null
            }
            _events.tryEmit(SyncEvent.Unlinked(peer, text, dropped, selfInitiated))
            val canRestore = dropped && running && pairedList.any { it.id == peer.id }
            if (canRestore) {
                startReconnect(peer, endpoint)
            } else {
                setLink(LinkState.Idle)
                if (dropped) problem(text)
            }
        }
    }

    // ---- Восстановление (§5.3) ----

    private fun startReconnect(peer: PeerInfo, last: Pair<String, Int>?) {
        val job = scope.launch(Dispatchers.IO, start = CoroutineStart.LAZY) { guard { reconnectLoop(peer, last) } }
        synchronized(lock) {
            reconnectJob?.cancel()
            reconnectJob = job
            reconnectName = peer.name
        }
        setLink(LinkState.Reconnecting(peer.name))
        job.start()
    }

    private suspend fun reconnectLoop(peer: PeerInfo, last: Pair<String, Int>?) {
        val me = currentCoroutineContext()[Job]
        val deadline = clock() + timing.reconnectWindowMs
        var lastEndpoint = last
        try {
            while (running && clock() < deadline) {
                delay(timing.reconnectIntervalMs)
                val visible = seen.values.firstOrNull { it.id == peer.id }
                val eps = visible?.addresses?.map { it to visible.port } ?: lastEndpoint?.let { listOf(it) } ?: continue
                when (connectOutgoing(eps, peer.id, peer.name, join = false, auto = true)) {
                    Outcome.Linked, Outcome.Fatal -> return
                    else -> if (visible != null) lastEndpoint = eps.firstOrNull()
                }
            }
            if (running) problem("Не удалось восстановить связь с ${peer.name}")
        } finally {
            synchronized(lock) {
                if (reconnectJob === me) reconnectJob = null
            }
            idleIfFree()
        }
    }

    private fun cancelReconnect(except: Job?) {
        val j = synchronized(lock) {
            val r = reconnectJob
            if (r == null || r === except) null else r.also { reconnectJob = null }
        }
        j?.cancel()
    }

    // ---- Обнаружение ----

    private fun onFound(raw: RawService) {
        val me = self ?: return
        val txt = raw.txt
        val id = txt["id"]
        if (txt["v"] != "1" || id == null || !ID_RE.matches(id) || id == me.id || txt["acc"] != me.acc) {
            if (seen.remove(raw.serviceName) != null) publishVisible()
            return
        }
        val addresses = SyncPolicy.sortAddresses(raw.addresses)
        if (addresses.isEmpty() || raw.port !in 1..65535) return
        val os = txt["os"]?.takeIf { OS_RE.matches(it) } ?: "unknown"
        seen[raw.serviceName] = VisibleDevice(id, raw.serviceName.take(64), os, addresses, raw.port, paired = false)
        publishVisible()
    }

    private fun onLost(serviceName: String) {
        if (seen.remove(serviceName) != null) publishVisible()
    }

    private fun publishVisible() {
        val pairedIds = pairedList.map { it.id }.toSet()
        val list = seen.values.groupBy { it.id }.values.map { it.last() }
            .map { it.copy(paired = it.id in pairedIds) }
            .sortedBy { it.name.lowercase() }
        _ui.update { it.copy(visible = list) }
    }

    // ---- Помощники ----

    private fun setLink(state: LinkState) {
        _ui.update { it.copy(link = state) }
    }

    private fun problem(text: String) {
        _ui.update { it.copy(problem = text) }
    }

    /** Если никто не занимает слот: состояние «нет связи» или «восстанавливаем». */
    private fun idleIfFree() {
        val free = synchronized(lock) { slot == null && link == null }
        if (!free) return
        val rec = synchronized(lock) { reconnectJob?.takeIf { it.isActive } != null }
        setLink(if (rec) LinkState.Reconnecting(reconnectName) else LinkState.Idle)
    }

    /** Ошибки на сетевых путях не роняют приложение: неожиданное исключение превращается в проблему. */
    private suspend fun guard(block: suspend () -> Unit) {
        try {
            block()
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            problem("Ошибка «Слушать вместе»: ${e.javaClass.simpleName}")
        }
    }

    private companion object {
        val ID_RE = Regex("[0-9a-f]{32}")
        val OS_RE = Regex("[a-z0-9]{1,16}")
    }
}
