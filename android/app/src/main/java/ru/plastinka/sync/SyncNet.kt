package ru.plastinka.sync

import java.io.IOException
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine

private val netScope = CoroutineScope(Dispatchers.IO + SupervisorJob())

/**
 * TCP-подключение с таймаутом. Отмена закрывает сокет, поэтому заблокированный
 * `connect` тут же прерывается. Адрес — только числовой IPv4 (DNS не нужен).
 */
internal suspend fun connectTcp(host: String, port: Int, timeoutMs: Int): Socket =
    suspendCancellableCoroutine { cont ->
        val socket = Socket()
        cont.invokeOnCancellation { closeQuietly(socket) }
        netScope.launch {
            try {
                socket.connect(InetSocketAddress(InetAddress.getByName(host), port), timeoutMs)
                socket.tcpNoDelay = true
                socket.keepAlive = true
                if (cont.isActive) cont.resume(socket) else closeQuietly(socket)
            } catch (e: IOException) {
                closeQuietly(socket)
                if (cont.isActive) cont.resumeWithException(e)
            } catch (e: IllegalArgumentException) {
                closeQuietly(socket)
                if (cont.isActive) cont.resumeWithException(IOException("Неверный адрес", e))
            }
        }
    }

internal fun closeQuietly(socket: Socket) {
    try {
        socket.close()
    } catch (_: IOException) {
    }
}

internal fun transportOf(socket: Socket): FrameTransport =
    StreamFrameTransport(socket.getInputStream(), socket.getOutputStream()) { closeQuietly(socket) }
