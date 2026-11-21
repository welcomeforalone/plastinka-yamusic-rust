package ru.plastinka.playback

import androidx.media3.common.ForwardingPlayer
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import java.lang.reflect.InvocationHandler
import java.lang.reflect.InvocationTargetException
import java.lang.reflect.Proxy
import ru.plastinka.sync.SyncButtonRouter
import ru.plastinka.sync.SyncPlayback

/**
 * Плеер сессии: все команды контроллеров (экран приложения, уведомление, медиакнопки, PiP)
 * проходят здесь, поэтому кнопки ведомого перехватываются в одном месте.
 *
 * Ведомый играет только присланный трек и очереди не имеет:
 *  - «Следующий» и «Предыдущий» уходят ведущему командой (`next`, `previous`);
 *  - перемешивание и повтор — командой (`shuffle`, `repeat`), локально они не меняются;
 *  - выбор другого трека или списка делает это устройство ведущим;
 *  - пауза, продолжение и перемотка своего трека выполняются как обычно: их замечает
 *    [SyncPlayback] по снимку состояния и шлёт `state`.
 * Ведущему и одинокому плееру всё проходит как есть.
 *
 * Кнопки «Следующий» и «Предыдущий» объявляются доступными всегда: у ведомого одна запись
 * в очереди, и без этого их убрали бы из уведомления.
 */
@androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)
internal class SyncForwardingPlayer(
    player: Player,
    private val sync: () -> SyncPlayback?,
) : ForwardingPlayer(player) {

    private val router = SyncButtonRouter(sync)

    private fun takeOver() = router.newList()

    // ---- доступность кнопок ----

    /** Обёртки слушателей: сессия должна получать набор команд с [ALWAYS]. */
    private val wrapped = HashMap<Player.Listener, Player.Listener>()

    /**
     * MediaSession берёт набор команд для контроллеров из `onAvailableCommandsChanged`, а не из
     * [getAvailableCommands]. ExoPlayer с одним треком (у ведомого) присылает набор без «Следующего»,
     * и экран приложения, уведомление и медиакнопки молча теряли эту кнопку. Добавляем её и здесь.
     */
    override fun addListener(listener: Player.Listener) {
        val wrapper = commandsListener(listener, ::withAlways)
        synchronized(wrapped) { wrapped[listener] = wrapper }
        super.addListener(wrapper)
    }

    override fun removeListener(listener: Player.Listener) {
        val wrapper = synchronized(wrapped) { wrapped.remove(listener) } ?: listener
        super.removeListener(wrapper)
    }

    override fun getAvailableCommands(): Player.Commands = withAlways(super.getAvailableCommands())

    override fun isCommandAvailable(command: Int): Boolean =
        command in ALWAYS_SET || super.isCommandAvailable(command)

    override fun hasNextMediaItem(): Boolean = sync()?.isSlave == true || super.hasNextMediaItem()

    override fun hasPreviousMediaItem(): Boolean = sync()?.isSlave == true || super.hasPreviousMediaItem()

    // ---- переходы ----

    override fun seekToNext() {
        if (router.next()) return
        super.seekToNext()
    }

    override fun seekToNextMediaItem() {
        if (router.next()) return
        super.seekToNextMediaItem()
    }

    override fun seekToPrevious() {
        if (router.previous()) return
        super.seekToPrevious()
    }

    override fun seekToPreviousMediaItem() {
        if (router.previous()) return
        super.seekToPreviousMediaItem()
    }

    /** Выбор из очереди: у ведомого другая запись — это «Следующий» ведущего, та же — обычная перемотка. */
    override fun seekTo(mediaItemIndex: Int, positionMs: Long) {
        if (router.seekToIndex(currentMediaItemIndex, mediaItemIndex)) return
        super.seekTo(mediaItemIndex, positionMs)
    }

    // ---- режимы ----

    override fun setShuffleModeEnabled(shuffleModeEnabled: Boolean) {
        if (router.shuffle()) return
        super.setShuffleModeEnabled(shuffleModeEnabled)
    }

    override fun setRepeatMode(repeatMode: Int) {
        if (router.repeat()) return
        super.setRepeatMode(repeatMode)
    }

    // ---- новый список: ведомый становится ведущим ----

    override fun setMediaItems(mediaItems: MutableList<MediaItem>) {
        takeOver()
        super.setMediaItems(mediaItems)
    }

    override fun setMediaItems(mediaItems: MutableList<MediaItem>, resetPosition: Boolean) {
        takeOver()
        super.setMediaItems(mediaItems, resetPosition)
    }

    override fun setMediaItems(mediaItems: MutableList<MediaItem>, startIndex: Int, startPositionMs: Long) {
        takeOver()
        super.setMediaItems(mediaItems, startIndex, startPositionMs)
    }

    override fun setMediaItem(mediaItem: MediaItem) {
        takeOver()
        super.setMediaItem(mediaItem)
    }

    override fun setMediaItem(mediaItem: MediaItem, startPositionMs: Long) {
        takeOver()
        super.setMediaItem(mediaItem, startPositionMs)
    }

    override fun setMediaItem(mediaItem: MediaItem, resetPosition: Boolean) {
        takeOver()
        super.setMediaItem(mediaItem, resetPosition)
    }

    internal companion object {
        val ALWAYS = intArrayOf(
            Player.COMMAND_SEEK_TO_NEXT,
            Player.COMMAND_SEEK_TO_NEXT_MEDIA_ITEM,
            Player.COMMAND_SEEK_TO_PREVIOUS,
            Player.COMMAND_SEEK_TO_PREVIOUS_MEDIA_ITEM,
        )
        val ALWAYS_SET = ALWAYS.toSet()

        fun withAlways(commands: Player.Commands): Player.Commands =
            commands.buildUpon().addAll(*ALWAYS).build()

        /**
         * Слушатель-обёртка: все события уходят [inner] как есть, кроме набора команд — он проходит
         * через [transform]. Сделан через [Proxy], а не делегированием Kotlin (`by`): `by` не
         * пересылает методы Java-интерфейса с реализацией по умолчанию, а у [Player.Listener]
         * такие все, и сессия перестала бы получать события. Равенство — по ссылке.
         */
        fun commandsListener(inner: Player.Listener, transform: (Player.Commands) -> Player.Commands): Player.Listener {
            val handler = InvocationHandler { proxy, method, args ->
                val arguments = args ?: emptyArray()
                if (method.declaringClass == Any::class.java) {
                    // equals, hashCode, toString.
                    return@InvocationHandler when (method.name) {
                        "equals" -> proxy === arguments.firstOrNull()
                        "hashCode" -> System.identityHashCode(proxy)
                        else -> "CommandsListener($inner)"
                    }
                }
                // Единственный метод слушателя с одним `Player.Commands` — onAvailableCommandsChanged
                // (по типу, а не по имени: имя могло бы смениться при сжатии кода).
                val commands = arguments.singleOrNull() as? Player.Commands
                val forwarded = if (commands != null && method.parameterTypes.singleOrNull() == Player.Commands::class.java) {
                    arrayOf<Any?>(transform(commands))
                } else {
                    arguments
                }
                try {
                    method.invoke(inner, *forwarded)
                } catch (e: InvocationTargetException) {
                    throw e.targetException ?: e
                }
            }
            return Proxy.newProxyInstance(
                Player.Listener::class.java.classLoader,
                arrayOf(Player.Listener::class.java),
                handler,
            ) as Player.Listener
        }
    }
}
