package ru.plastinka.sync

import android.app.Application
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.os.SystemClock
import java.util.Base64
import java.util.concurrent.Executors
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import ru.plastinka.data.Prefs
import ru.plastinka.data.TokenStore
import ru.plastinka.playback.AccountHolder

/**
 * Секреты сопряжения: `EncryptedSharedPreferences`, как токен, но свой файл и свой ключ
 * Keystore (сбой этого хранилища не должен стереть вход). Запасного файла нет: при сбое
 * `save` возвращает `false`, а [SyncService] держит секрет в памяти до конца процесса.
 */
class SyncSecretStore(app: Application) : SecretStore {
    private val store = TokenStore(app, "plastinka_sync", "plastinka_sync_key")

    override fun load(peerId: String): ByteArray? = try {
        store.loadKey(KEY + peerId)?.let { Base64.getDecoder().decode(it) }
    } catch (_: IllegalArgumentException) {
        null
    }

    override fun save(peerId: String, secret: ByteArray): Boolean =
        store.saveKey(KEY + peerId, Base64.getEncoder().encodeToString(secret))

    override fun delete(peerId: String) = store.removeKey(KEY + peerId)

    private companion object {
        const val KEY = "sync-"
    }
}

/** Список сопряжённых (без секретов) лежит в [Prefs]. */
class PrefsPairedStore : PairedStore {
    override fun load(): List<PairedDevice> = Prefs.syncPaired
    override fun save(list: List<PairedDevice>) = Prefs.saveSyncPaired(list)
}

/**
 * Обнаружение через `NsdManager` (§2): объявление `_plastinka._tcp` с TXT и поиск. Адреса:
 * API 34+ — `registerServiceInfoCallback`, ниже — `resolveService` строго по одному (очередью).
 * Фильтрация (`v`, `acc`, `id`) — в [SyncService].
 */
class NsdDiscovery(app: Application) : Discovery {
    private val nsd = app.getSystemService(Application.NSD_SERVICE) as? NsdManager
    private val executor = Executors.newSingleThreadExecutor()
    private val lock = Any()

    private var registration: NsdManager.RegistrationListener? = null
    private var discovery: NsdManager.DiscoveryListener? = null
    private var onFound: (RawService) -> Unit = {}
    private var onLost: (String) -> Unit = {}

    private val queue = ArrayDeque<NsdServiceInfo>()
    private var resolving = false
    private val watchers = HashMap<String, Any>()

    override fun start(announcement: Announcement, onFound: (RawService) -> Unit, onLost: (serviceName: String) -> Unit) {
        val mgr = nsd ?: throw IllegalStateException("NSD недоступен")
        synchronized(lock) {
            this.onFound = onFound
            this.onLost = onLost
        }
        val info = NsdServiceInfo().apply {
            serviceName = announcement.instanceName
            serviceType = TYPE
            port = announcement.port
            setAttribute("v", "1")
            setAttribute("id", announcement.id)
            setAttribute("acc", announcement.acc)
            setAttribute("os", announcement.os)
        }
        val reg = object : NsdManager.RegistrationListener {
            override fun onRegistrationFailed(serviceInfo: NsdServiceInfo?, errorCode: Int) = Unit
            override fun onUnregistrationFailed(serviceInfo: NsdServiceInfo?, errorCode: Int) = Unit
            override fun onServiceRegistered(serviceInfo: NsdServiceInfo?) = Unit
            override fun onServiceUnregistered(serviceInfo: NsdServiceInfo?) = Unit
        }
        val disc = object : NsdManager.DiscoveryListener {
            override fun onStartDiscoveryFailed(serviceType: String?, errorCode: Int) = Unit
            override fun onStopDiscoveryFailed(serviceType: String?, errorCode: Int) = Unit
            override fun onDiscoveryStarted(serviceType: String?) = Unit
            override fun onDiscoveryStopped(serviceType: String?) = Unit
            override fun onServiceFound(serviceInfo: NsdServiceInfo) {
                if (serviceInfo.serviceType?.startsWith(TYPE_PREFIX) != true) return
                handleFound(serviceInfo)
            }

            override fun onServiceLost(serviceInfo: NsdServiceInfo) {
                handleLost(serviceInfo.serviceName)
            }
        }
        registration = reg
        discovery = disc
        mgr.registerService(info, NsdManager.PROTOCOL_DNS_SD, reg)
        mgr.discoverServices(TYPE, NsdManager.PROTOCOL_DNS_SD, disc)
    }

    override fun stop() {
        val mgr = nsd ?: return
        try {
            registration?.let { mgr.unregisterService(it) }
        } catch (_: IllegalArgumentException) {
        }
        try {
            discovery?.let { mgr.stopServiceDiscovery(it) }
        } catch (_: IllegalArgumentException) {
        }
        registration = null
        discovery = null
        synchronized(lock) {
            queue.clear()
            resolving = false
            if (Build.VERSION.SDK_INT >= 34) {
                watchers.values.forEach { unregisterCallback(mgr, it) }
            }
            watchers.clear()
        }
    }

    private fun handleFound(info: NsdServiceInfo) {
        val mgr = nsd ?: return
        if (Build.VERSION.SDK_INT >= 34) {
            watch(mgr, info)
        } else {
            synchronized(lock) { queue.addLast(info) }
            resolveNext()
        }
    }

    private fun handleLost(name: String) {
        val mgr = nsd
        synchronized(lock) {
            queue.removeAll { it.serviceName == name }
            if (mgr != null && Build.VERSION.SDK_INT >= 34) {
                watchers.remove(name)?.let { unregisterCallback(mgr, it) }
            }
        }
        onLost(name)
    }

    // API 34+: адреса и TXT приходят через callback, повторные обновления — тем же callback.
    @Suppress("NewApi")
    private fun watch(mgr: NsdManager, info: NsdServiceInfo) {
        val name = info.serviceName
        synchronized(lock) {
            if (watchers.containsKey(name)) return
            val cb = object : NsdManager.ServiceInfoCallback {
                override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {
                    synchronized(lock) { watchers.remove(name) }
                }

                override fun onServiceUpdated(serviceInfo: NsdServiceInfo) {
                    emit(serviceInfo, serviceInfo.hostAddresses.mapNotNull { it.hostAddress })
                }

                override fun onServiceLost() {
                    synchronized(lock) { watchers.remove(name) }
                    onLost(name)
                }

                override fun onServiceInfoCallbackUnregistered() = Unit
            }
            watchers[name] = cb
            try {
                mgr.registerServiceInfoCallback(info, executor, cb)
            } catch (_: IllegalArgumentException) {
                watchers.remove(name)
            }
        }
    }

    @Suppress("NewApi")
    private fun unregisterCallback(mgr: NsdManager, cb: Any) {
        try {
            (cb as? NsdManager.ServiceInfoCallback)?.let { mgr.unregisterServiceInfoCallback(it) }
        } catch (_: IllegalArgumentException) {
        }
    }

    // Ниже API 34: `resolveService` принимает только один запрос за раз.
    @Suppress("DEPRECATION")
    private fun resolveNext() {
        val mgr = nsd ?: return
        val next = synchronized(lock) {
            if (resolving) return
            val n = queue.removeFirstOrNull() ?: return
            resolving = true
            n
        }
        val finish = {
            synchronized(lock) { resolving = false }
            resolveNext()
        }
        try {
            mgr.resolveService(
                next,
                object : NsdManager.ResolveListener {
                    override fun onResolveFailed(serviceInfo: NsdServiceInfo?, errorCode: Int) = finish()
                    override fun onServiceResolved(serviceInfo: NsdServiceInfo) {
                        emit(serviceInfo, listOfNotNull(serviceInfo.host?.hostAddress))
                        finish()
                    }
                },
            )
        } catch (_: IllegalArgumentException) {
            finish()
        }
    }

    private fun emit(info: NsdServiceInfo, addresses: List<String>) {
        val txt = HashMap<String, String>()
        for ((k, v) in info.attributes) {
            if (v != null) txt[k] = v.toString(Charsets.US_ASCII)
        }
        onFound(RawService(info.serviceName, txt, addresses, info.port))
    }

    private companion object {
        const val TYPE = "_plastinka._tcp"
        const val TYPE_PREFIX = "_plastinka._tcp"
    }
}

/**
 * Склейка службы с приложением: решает, когда держать сеть (правило 2 и экономия).
 *
 * Служба работает, только если синхронизация включена, подписка подтверждена
 * ([AccountHolder.uid] появляется лишь после проверки Плюса) и приложение на экране либо
 * что-то играет. Ушли с экрана и ничего не играет — через 15 с (не гасим сеть на каждом
 * повороте экрана или переходе) служба останавливается с `bye shutdown`. Выход из аккаунта и
 * выключение функции останавливают её сразу.
 */
class SyncRuntime(private val app: Application) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val mutex = Mutex()
    private var stopJob: Job? = null
    private var started: DeviceInfo? = null

    @Volatile
    private var foreground = false

    @Volatile
    private var playing = false

    val service: SyncService = SyncService(
        secrets = SyncSecretStore(app),
        pairedStore = PrefsPairedStore(),
        discovery = NsdDiscovery(app),
        scope = scope,
        clock = { SystemClock.elapsedRealtime() },
    )

    /** Подписаться на смену аккаунта и настройки. Вызывается один раз при старте приложения. */
    fun attach() {
        AccountHolder.onChanged = { update() }
        Prefs.onSyncEnabledChanged = { update() }
        update()
    }

    /** Экран приложения виден (`onStart`/`onStop` активности). */
    fun setForeground(value: Boolean) {
        foreground = value
        update()
    }

    /** Что-то играет (плеер сообщает на шаге интерфейса): связь не гасится при уходе с экрана. */
    fun setPlaying(value: Boolean) {
        playing = value
        update()
    }

    fun update() {
        scope.launch { mutex.withLock { apply() } }
    }

    private suspend fun apply() {
        val uid = AccountHolder.uid
        if (!Prefs.syncEnabled || uid == null) {
            stopJob?.cancel()
            stopService()
            return
        }
        val device = DeviceInfo(Prefs.syncDeviceId, Prefs.syncDeviceName, "android", SyncPolicy.accountLabel(uid))
        if (foreground || playing) {
            stopJob?.cancel()
            stopJob = null
            if (started != null && started != device) stopService()
            if (started == null) {
                started = device
                service.start(device)
            }
        } else if (started != null && stopJob?.isActive != true) {
            stopJob = scope.launch {
                delay(BACKGROUND_GRACE_MS)
                mutex.withLock {
                    if (!foreground && !playing) stopService()
                }
            }
        }
    }

    private suspend fun stopService() {
        if (started == null) return
        started = null
        service.stop()
    }

    private companion object {
        const val BACKGROUND_GRACE_MS = 15_000L
    }
}
