package ru.plastinka

import android.app.ActivityManager
import android.app.Application
import android.app.ApplicationExitInfo
import android.content.Context
import android.content.pm.ApplicationInfo
import android.os.Build
import androidx.annotation.RequiresApi
import androidx.core.content.pm.PackageInfoCompat
import java.io.File
import java.io.PrintWriter
import java.io.StringWriter
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.concurrent.ConcurrentLinkedDeque
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Журнал сбоев и причина прошлого выхода. Всё только на устройстве, в личной папке
 * приложения `filesDir/diagnostics/` (это текст, не звук), и никуда не отправляется:
 * отчёт уходит лишь через буфер обмена по нажатию пользователя. Токена и звука здесь нет:
 * хранилище токена диагностика не читает, а весь текст перед записью проходит [sanitize].
 */
object Diagnostics {
    private const val DIR = "diagnostics"
    private const val CRASH = "crash.txt"
    private const val CRASH_PREV = "crash-prev.txt"
    private const val EVENTS = "events.txt"
    private const val EVENTS_PREV = "events-prev.txt"
    private const val EXITS = "exits.txt"
    private const val PREFS = "diagnostics"
    private const val PREF_LAST_EXIT = "lastExitTime"

    private const val MAX_CRASH_CHARS = 64 * 1024
    private const val MAX_EVENTS = 50
    private const val MAX_EVENT_CHARS = 300
    private const val MAX_EXITS = 5
    private const val MAX_TRACE_LINES = 200
    private const val MAX_TRACE_CHARS = 64 * 1024
    private const val WRITE_DELAY_MS = 1000L
    private const val EXIT_SEPARATOR = "#### запись ####"

    private val timeFormat = DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm:ss")

    /** Корень личных файлов и версия: сам Context не храним (утечка), только то, что из него нужно. */
    @Volatile
    private var filesRoot: File? = null

    @Volatile
    private var versionText = "?"

    /** Кольцо последних событий. Без блокировок: обработчик сбоя не должен ничего ждать. */
    private val ring = ConcurrentLinkedDeque<String>()
    private val writeScheduled = AtomicBoolean(false)

    private val writer: ScheduledExecutorService by lazy {
        Executors.newSingleThreadScheduledExecutor { r ->
            Thread(r, "diagnostics-writer").apply { isDaemon = true }
        }
    }

    /** Краткая сводка для экрана «О программе»: строки уже готовы к показу. */
    data class Summary(val lastCrash: String?, val lastExit: String?)

    /**
     * Самой первой строкой `Application.onCreate`: ставит обработчик необработанных сбоев.
     * Обработчик пишет файл синхронно и зовёт прежний, так что система закрывает приложение как обычно.
     */
    fun install(app: Application) {
        try {
            filesRoot = app.filesDir
            versionText = readVersion(app)
            val previous = Thread.getDefaultUncaughtExceptionHandler()
            Thread.setDefaultUncaughtExceptionHandler { thread, error ->
                try {
                    writeCrash(thread, error)
                } catch (_: Throwable) {
                    // Обработчик сбоя сам не падает.
                }
                try {
                    previous?.uncaughtException(thread, error)
                } catch (_: Throwable) {
                    // Прежний обработчик закрывает процесс сам.
                }
            }
            // Журнал прошлого запуска остаётся рядом, пока не придёт следующий.
            val dir = dir() ?: return
            val events = File(dir, EVENTS)
            if (events.exists()) {
                val prev = File(dir, EVENTS_PREV)
                prev.delete()
                events.renameTo(prev)
            }
        } catch (_: Throwable) {
            // Диагностика не должна мешать запуску.
        }
    }

    /** Строка со временем в кольцо событий и (не чаще раза в секунду, в фоне) в `events.txt`. */
    fun event(text: String) {
        try {
            val line = "${now()}  ${sanitize(text).take(MAX_EVENT_CHARS)}"
            ring.addLast(line)
            while (ring.size > MAX_EVENTS) ring.pollFirst()
            if (writeScheduled.compareAndSet(false, true)) {
                writer.schedule({
                    writeScheduled.set(false)
                    try {
                        writeEvents("events.tmp")
                    } catch (_: Throwable) {
                        // Не записалось: повторим при следующем событии.
                    }
                }, WRITE_DELAY_MS, TimeUnit.MILLISECONDS)
            }
        } catch (_: Throwable) {
            // Журнал не должен ломать то, что он описывает.
        }
    }

    /** Сборка с отладочной подписью: тогда в «О программе» работает проверочное долгое нажатие. */
    fun isDebuggable(context: Context): Boolean =
        context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0

    /** Читает причины выхода из системы (Android 11+) и дописывает новые в `exits.txt`. Вызывать не с главного потока. */
    fun collectExits(app: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return
        try {
            collectExitsApi30(app)
        } catch (_: Throwable) {
            // Система не ответила: попробуем при следующем запуске.
        }
    }

    @RequiresApi(Build.VERSION_CODES.R)
    private fun collectExitsApi30(app: Context) {
        val dir = dir() ?: return
        val manager = app.getSystemService(ActivityManager::class.java) ?: return
        val prefs = app.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val lastTime = prefs.getLong(PREF_LAST_EXIT, 0L)
        // Система отдаёт сначала самые новые.
        val fresh = manager.getHistoricalProcessExitReasons(app.packageName, 0, MAX_EXITS)
            .filter { it.timestamp > lastTime }
        if (fresh.isEmpty()) return
        val records = fresh.map { sanitize(exitRecord(it)) }
        val old = readExitRecords(File(dir, EXITS))
        val all = (records + old).take(MAX_EXITS)
        File(dir, EXITS).writeText(all.joinToString("\n$EXIT_SEPARATOR\n"))
        prefs.edit().putLong(PREF_LAST_EXIT, fresh.maxOf { it.timestamp }).apply()
    }

    @RequiresApi(Build.VERSION_CODES.R)
    private fun exitRecord(info: ApplicationExitInfo): String = buildString {
        appendLine("Время: ${format(info.timestamp)}")
        appendLine("Причина: ${exitReasonText(info.reason)}")
        appendLine("Состояние: ${importanceText(info.importance)}")
        appendLine("Описание: ${info.description.orEmpty()}")
        appendLine("Статус: ${info.status}")
        append("Память: PSS ${info.pss / 1024} МБ, RSS ${info.rss / 1024} МБ")
        when (info.reason) {
            ApplicationExitInfo.REASON_ANR -> {
                append("\nТрассировка ANR:\n")
                append(readTrace(info))
            }
            ApplicationExitInfo.REASON_CRASH_NATIVE ->
                append("\nСбой в нативном коде, подробности системы не разбираются")
            else -> Unit // остальным причинам хватает строк выше
        }
    }

    @RequiresApi(Build.VERSION_CODES.R)
    private fun readTrace(info: ApplicationExitInfo): String {
        return try {
            val stream = info.traceInputStream ?: return "(трассировки нет)"
            stream.bufferedReader().use { reader ->
                val out = StringBuilder()
                var lines = 0
                while (lines < MAX_TRACE_LINES && out.length < MAX_TRACE_CHARS) {
                    val line = reader.readLine() ?: break
                    out.append(line).append('\n')
                    lines++
                }
                out.toString().take(MAX_TRACE_CHARS).trimEnd()
            }
        } catch (_: Throwable) {
            "(трассировку прочитать не удалось)"
        }
    }

    // Константы выше minSdk сравниваются числами: так lint не требует проверок версии.
    private fun exitReasonText(reason: Int): String = when (reason) {
        6 -> "приложение зависло (ANR)"
        4 -> "сбой приложения"
        5 -> "сбой в системной библиотеке"
        3 -> "остановлено системой: мало памяти"
        9 -> "остановлено системой: большой расход ресурсов (батарея)"
        14 -> "заморожено системой"
        10 -> "закрыто пользователем (смахнули или «Остановить»)"
        11 -> "остановлено пользователем (профиль или пользователь отключён)"
        16 -> "приложение обновлено"
        15 -> "изменено состояние пакета"
        8 -> "изменено разрешение"
        12 -> "завершилась связанная служба"
        7 -> "сбой при запуске"
        2 -> "завершено сигналом"
        1 -> "приложение вышло само"
        13 -> "остановлено системой (другая причина)"
        else -> "неизвестно ($reason)"
    }

    private fun importanceText(importance: Int): String {
        val text = when {
            importance <= 100 -> "на экране"
            importance <= 125 -> "служба на переднем плане (играла музыка)"
            importance <= 200 -> "видно на экране"
            importance <= 230 -> "заметно пользователю"
            importance <= 300 -> "служба в фоне"
            importance < 400 -> "в фоне"
            else -> "в кэше (давно в фоне)"
        }
        return "$text (код $importance)"
    }

    private fun readExitRecords(file: File): List<String> {
        val text = try {
            if (file.exists()) file.readText() else return emptyList()
        } catch (_: Throwable) {
            return emptyList()
        }
        return text.split("\n$EXIT_SEPARATOR\n").filter { it.isNotBlank() }
    }

    /** Сводка для «О программе». Читает файлы: не вызывать с главного потока. */
    fun summary(): Summary {
        val dir = dir()
        val crash = dir?.let { readOrNull(File(it, CRASH)) }?.let { text ->
            val time = field(text, "Время") ?: return@let null
            val type = field(text, "Исключение")?.substringAfterLast('.')
            "$time — ${type ?: "ошибка"}"
        }
        val exit = dir?.let { readExitRecords(File(it, EXITS)).firstOrNull() }?.let { record ->
            val time = field(record, "Время") ?: return@let null
            "$time — ${field(record, "Причина") ?: "неизвестно"}"
        }
        return Summary(crash, exit)
    }

    /** Полный отчёт для буфера обмена. Читает файлы: не вызывать с главного потока. */
    fun report(): String {
        val dir = dir()
        return buildString {
            appendLine("Отчёт Plastinka")
            append(header())
            appendLine("Отчёт собран: ${now()}")
            section("Последний сбой", dir?.let { readOrNull(File(it, CRASH)) })
            section("Предыдущий сбой", dir?.let { readOrNull(File(it, CRASH_PREV)) })
            appendLine()
            appendLine("== Причины выхода ==")
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
                appendLine("Причину завершения система сообщает только с Android 11")
            } else {
                val records = dir?.let { readExitRecords(File(it, EXITS)) }.orEmpty()
                if (records.isEmpty()) {
                    appendLine("(нет)")
                } else {
                    records.forEachIndexed { index, record ->
                        if (index > 0) appendLine()
                        appendLine(record.trimEnd())
                    }
                }
            }
            section("Журнал прошлого запуска", dir?.let { readOrNull(File(it, EVENTS_PREV)) })
            section("Журнал этого запуска", ring.toList().joinToString("\n"))
        }.trimEnd() + "\n"
    }

    private fun StringBuilder.section(title: String, body: String?) {
        appendLine()
        appendLine("== $title ==")
        appendLine(if (body.isNullOrBlank()) "(нет)" else body.trimEnd())
    }

    // Ниже: запись сбоя и общие помощники.

    private fun writeCrash(thread: Thread, error: Throwable) {
        val dir = dir() ?: return
        val trace = StringWriter().also { error.printStackTrace(PrintWriter(it)) }.toString()
        val text = buildString {
            appendLine("Время: ${now()}")
            append(header())
            appendLine("Поток: ${thread.name}")
            appendLine("Исключение: ${error.javaClass.name}")
            appendLine("Стек:")
            append(sanitize(trace).take(MAX_CRASH_CHARS))
            appendLine()
            appendLine()
            appendLine("События перед сбоем:")
            append(ring.toList().joinToString("\n"))
        }
        val crash = File(dir, CRASH)
        if (crash.exists()) {
            val prev = File(dir, CRASH_PREV)
            prev.delete()
            crash.renameTo(prev)
        }
        crash.writeText(text)
        writeEvents("events-crash.tmp")
    }

    private fun writeEvents(tmpName: String) {
        val dir = dir() ?: return
        val tmp = File(dir, tmpName)
        tmp.writeText(ring.toList().joinToString("\n"))
        val target = File(dir, EVENTS)
        if (!tmp.renameTo(target)) {
            target.writeText(tmp.readText())
            tmp.delete()
        }
    }

    private fun readVersion(app: Context): String = try {
        val info = app.packageManager.getPackageInfo(app.packageName, 0)
        "${info.versionName ?: "?"} (${PackageInfoCompat.getLongVersionCode(info)})"
    } catch (_: Throwable) {
        // Версию узнать не вышло: в отчёте останется «?».
        "?"
    }

    private fun header(): String = buildString {
        appendLine("Версия: $versionText")
        appendLine("Android: ${Build.VERSION.RELEASE} (SDK ${Build.VERSION.SDK_INT})")
        appendLine("Устройство: ${Build.MANUFACTURER} ${Build.MODEL}")
    }

    private fun dir(): File? {
        val root = filesRoot ?: return null
        val dir = File(root, DIR)
        return if (dir.isDirectory || dir.mkdirs()) dir else null
    }

    private fun readOrNull(file: File): String? = try {
        if (file.exists()) file.readText() else null
    } catch (_: Throwable) {
        null
    }

    /** Значение строки «Имя: значение» из текста записи. */
    private fun field(text: String, name: String): String? =
        text.lineSequence().firstOrNull { it.startsWith("$name: ") }?.substringAfter(": ")?.trim()

    private fun now(): String = format(System.currentTimeMillis())

    private fun format(millis: Long): String =
        timeFormat.format(Instant.ofEpochMilli(millis).atZone(ZoneId.systemDefault()))

    private val tokenRules: List<Pair<Regex, String>> = listOf(
        Regex("(?i)\\bOAuth\\s+\\S+") to "OAuth [скрыто]",
        Regex("(?i)Authorization\\s*[:=][^\\r\\n]*") to "Authorization: [скрыто]",
        Regex("\\by0_[A-Za-z0-9_.\\-]+") to "[скрыто]",
        Regex("\\bAQAAAA[A-Za-z0-9_.\\-]*") to "[скрыто]",
        Regex("(?i)\\b((?:access_|refresh_)?token)=[^&\\s\"']+") to "$1=[скрыто]",
        // У адреса всё после «?» (параметры, в том числе токены) отбрасывается.
        Regex("(https?://[^\\s?#\"'<>)]*)\\?\\S*") to "$1?[скрыто]",
    )

    /** Длинная последовательность символов base64 или hex. */
    private val longRun = Regex("[A-Za-z0-9+/_\\-]{32,}={0,2}")

    /** Убирает из текста всё, похожее на токен или адрес с параметрами. */
    fun sanitize(text: String): String {
        var result = text
        for ((regex, replacement) in tokenRules) result = regex.replace(result, replacement)
        // Длинные последовательности скрываем, только если в них есть цифра: длинные имена
        // остаются. Цифру ищем в найденном куске, а не опережающей проверкой в самом
        // выражении: так время линейное и на длинной строке из одних букв.
        return longRun.replace(result) { m -> if (m.value.any(Char::isDigit)) "[скрыто]" else m.value }
    }
}
