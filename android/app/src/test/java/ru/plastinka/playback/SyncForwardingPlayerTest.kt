package ru.plastinka.playback

import androidx.media3.common.Player
import java.lang.reflect.Proxy
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotEquals
import kotlin.test.assertTrue

class SyncForwardingPlayerTest {

    /** Значение по умолчанию для аргумента типа [type]. */
    private fun defaultOf(type: Class<*>): Any? = when (type) {
        java.lang.Boolean.TYPE -> false
        java.lang.Integer.TYPE -> 0
        java.lang.Long.TYPE -> 0L
        java.lang.Float.TYPE -> 0f
        java.lang.Double.TYPE -> 0.0
        else -> null
    }

    /**
     * Обёртка слушателя передаёт каждое событие [Player.Listener] — все они методы Java-интерфейса
     * с реализацией по умолчанию, их и терять было бы легче всего. Набор команд (`Player.Commands`)
     * на JVM не собрать (он держится на `SparseBooleanArray` из Android), поэтому его подмена здесь
     * не проверяется: `null` проходит как есть.
     */
    @Test
    fun listenerForwardsEveryEvent() {
        val calls = mutableListOf<String>()
        val inner = Proxy.newProxyInstance(
            Player.Listener::class.java.classLoader,
            arrayOf(Player.Listener::class.java),
        ) { _, method, _ ->
            calls.add(method.name + method.parameterTypes.joinToString(",", "(", ")") { it.simpleName })
            null
        } as Player.Listener
        val wrapper = SyncForwardingPlayer.commandsListener(inner) { it }

        val methods = Player.Listener::class.java.methods.filter { it.declaringClass == Player.Listener::class.java }
        assertTrue(methods.size > 20, "методов слушателя: ${methods.size}")
        for (method in methods) {
            method.invoke(wrapper, *method.parameterTypes.map(::defaultOf).toTypedArray())
        }
        val expected = methods.map { m -> m.name + m.parameterTypes.joinToString(",", "(", ")") { it.simpleName } }
        assertEquals(expected.sorted(), calls.sorted())
    }

    @Test
    fun listenerEqualityIsByReference() {
        val inner = object : Player.Listener {}
        val a = SyncForwardingPlayer.commandsListener(inner) { it }
        val b = SyncForwardingPlayer.commandsListener(inner) { it }
        assertTrue(a == a)
        assertFalse(a == b)
        assertEquals(a.hashCode(), a.hashCode())
        assertNotEquals(a.toString(), "")
    }
}
