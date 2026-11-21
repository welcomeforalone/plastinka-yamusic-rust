package ru.plastinka.sync

import kotlin.math.abs
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assume.assumeTrue
import ru.plastinka.data.ArtistRef

/**
 * Живая сверка с десктопом через 127.0.0.1. Пропускается, если не задана переменная окружения
 * `PLASTINKA_INTEROP` (например, `127.0.0.1:47999`). Десктопная сторона (игнорируемый Rust-тест)
 * должна быть уже запущена: она ждёт подключения, подтверждает код и шлёт начальный `state`
 * с треком `12345`.
 */
class InteropTest {
    private val timeoutMs = 30_000L

    private fun hexId(): String = SyncCrypto.randomBytes(16).joinToString("") { "%02x".format(it) }

    @Test
    fun desktopInterop() {
        val target = System.getenv("PLASTINKA_INTEROP")
        assumeTrue("PLASTINKA_INTEROP не задана, живая сверка пропущена", !target.isNullOrBlank())
        requireNotNull(target)
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
        try {
            runBlocking { withTimeout(timeoutMs) { run(scope, target) } }
        } finally {
            scope.cancel()
        }
    }

    private suspend fun run(scope: CoroutineScope, target: String) {
        val id = hexId()
        val clock: () -> Long = { System.nanoTime() / 1_000_000 }
        val svc = SyncService(
            secrets = MemSecrets(),
            pairedStore = MemPaired(),
            discovery = FakeDiscovery(),
            scope = scope,
            clock = clock,
            timing = SyncTiming(frameTimeoutMs = 10_000, confirmTimeoutMs = 20_000, connectTimeoutMs = 3_000),
            preferredPort = 0,
        )
        val events = java.util.concurrent.CopyOnWriteArrayList<SyncEvent>()
        scope.launch(Dispatchers.Unconfined, start = kotlinx.coroutines.CoroutineStart.UNDISPATCHED) {
            svc.events.collect { events.add(it) }
        }
        svc.start(DeviceInfo(id, "Android interop", "android", SyncPolicy.accountLabel(42)))
        try {
            svc.connectAddress(target)

            // 1. Код сопряжения.
            val pairing = waitUi(svc, "запрос кода сопряжения после connectAddress($target)") {
                it.link is LinkState.Pairing || it.problem != null
            }
            val link = pairing.link
            check(link is LinkState.Pairing) { "ждали запрос кода, а пришла проблема: ${pairing.problem}" }
            println("INTEROP: код сопряжения ${link.request.displayCode}, собеседник ${link.request.peer.name} (${link.request.peer.os})")
            svc.decide(true)

            // 2. Связь готова.
            val linked = waitUi(svc, "связь готова (Linked) после подтверждения кода") {
                it.link is LinkState.Linked || it.problem != null
            }
            check(linked.link is LinkState.Linked) { "ждали Linked, а пришла проблема: ${linked.problem}" }

            // 3. Свой начальный state: без трека, join = true.
            val initial = SyncState(
                rev = 0, by = id, owner = id, track = null, playing = false, positionMs = 0,
                at = clock(), initial = true, join = true,
            )
            check(svc.send(SyncMessage.State(initial))) { "начальный state не отправился: связь не готова" }

            // 4. Начальный state десктопа.
            val desktop = waitMessage<SyncMessage.State>(events, "начальный state десктопа") { it.state.initial }.state
            val receivedAt = clock()
            val track = desktop.track
            checkNotNull(track) { "в начальном state десктопа нет трека: $desktop" }
            assertEquals("12345", track.id, "id трека")
            assertEquals("678", track.albumId, "albumId")
            assertEquals("Interop", track.sourceTitle, "title")
            assertEquals(listOf(ArtistRef("9", "Тест")), track.sourceArtists, "artists")
            assertEquals(200_000L, track.durationMs, "durationMs")
            assertTrue(desktop.playing, "playing")
            assertEquals(10_000L, desktop.positionMs, "positionMs")
            assertEquals(false, desktop.join, "join у ответчика")
            // `at` переведён в свои часы: момент отправки не в будущем и не старше 500 мс.
            val age = receivedAt - desktop.at
            assertTrue(age in -50..500, "at отстоит от получения на $age мс (ждали 0..500)")
            val now = SyncLogic.positionNow(desktop.positionMs, desktop.at, desktop.playing, 0, receivedAt)
            assertTrue(abs(now - (10_000 + age)) <= 1, "позиция сейчас $now")
            println("INTEROP: state десктопа разобран верно, позиция сейчас $now мс")

            // 5. Мелкие сообщения.
            check(svc.send(SyncMessage.Liked("12345", true))) { "liked не отправился" }
            check(svc.send(SyncMessage.Command("next"))) { "command next не отправился" }
            delay(500)

            // 6. Отключение: наша сторона закрывает с bye user, ошибок нет.
            svc.disconnect()
            waitUi(svc, "закрытие связи после disconnect()") { it.link is LinkState.Idle }
            val unlinked = events.filterIsInstance<SyncEvent.Unlinked>().firstOrNull()
            checkNotNull(unlinked) { "нет события Unlinked после disconnect()" }
            assertTrue(unlinked.selfInitiated, "Unlinked.selfInitiated")
            assertEquals(false, unlinked.dropped, "Unlinked.dropped")
            assertNull(svc.ui.value.problem, "после disconnect() проблема не ожидалась")
            println("INTEROP: готово")
        } finally {
            svc.stop()
        }
    }

    private suspend fun waitUi(svc: SyncService, what: String, pred: (SyncUi) -> Boolean): SyncUi = try {
        withTimeout(timeoutMs) { svc.ui.first(pred) }
    } catch (e: kotlinx.coroutines.TimeoutCancellationException) {
        error("Не дождались: $what. Состояние: ${svc.ui.value.link}, проблема: ${svc.ui.value.problem}")
    }

    private suspend inline fun <reified M : SyncMessage> waitMessage(
        events: List<SyncEvent>,
        what: String,
        crossinline pred: (M) -> Boolean,
    ): M {
        return try {
            withTimeout(timeoutMs) {
                var found: M? = null
                while (found == null) {
                    found = events.filterIsInstance<SyncEvent.Message>().map { it.msg }.filterIsInstance<M>()
                        .firstOrNull { pred(it) }
                    if (found == null) delay(20)
                }
                found
            }
        } catch (e: kotlinx.coroutines.TimeoutCancellationException) {
            error("Не дождались: $what. Пришло: ${events.joinToString { it.toString().take(120) }}")
        }
    }
}
