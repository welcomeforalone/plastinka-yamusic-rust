package ru.plastinka.playback

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Связь звукового тракта и столбиков (один процесс). Отвод [SpectrumTap] кладёт
 * сюда моно-отсчёты в кольцевой буфер фиксированного размера; цикл анализа
 * (только пока столбики кто-то показывает) считает спектр ~30 раз в секунду.
 * Звук живёт только в памяти: на диск, в кэш и в лог ничего не пишется.
 */
object SpectrumHub {

    // ---------------------------------------------------------- флаг

    /** «Визуализатор включён» (по умолчанию да). Меняет и сохраняет [ru.plastinka.data.Prefs]. */
    var enabled: Boolean
        get() = enabledState
        set(value) {
            enabledState = value
            refreshCapture()
        }
    private var enabledState by mutableStateOf(true)

    // ---------------------------------------------------------- кольцо

    /** Размер кольца (степень двойки): запас на задержку звукового буфера. */
    private const val RING = 16384
    private const val MASK = RING - 1
    internal val ring = FloatArray(RING)

    /** Сколько моно-отсчётов записано всего; пишет только звуковой поток. */
    @Volatile
    private var written = 0L

    @Volatile
    private var rate = 0

    @Volatile
    private var lastFeedNanos = 0L

    /** Отвод копирует отсчёты только когда это нужно: флаг включён и есть зрители. */
    @Volatile
    var capturing = false
        private set

    /** Из звукового потока: записано до позиции [pos] (после цикла записи в [ring]). */
    internal fun publish(pos: Long, sampleRate: Int) {
        rate = sampleRate
        lastFeedNanos = System.nanoTime()
        written = pos
    }

    internal fun writePosition(): Long = written

    internal fun mask(pos: Long): Int = (pos and MASK.toLong()).toInt()

    /**
     * Копия последних [SpectrumAnalyzer.SIZE] отсчётов в [out], сдвинутая на
     * [LATENCY_MS] назад: звук доходит до динамика позже отвода (буфер
     * `AudioTrack`). Возвращает частоту, 0 — звука нет (пауза, конец трека).
     */
    private fun snapshot(out: FloatArray): Int {
        val sampleRate = rate
        if (sampleRate <= 0) return 0
        val ageMs = (System.nanoTime() - lastFeedNanos) / 1_000_000
        if (ageMs > FRESH_MS + LATENCY_MS) return 0
        val delay = (sampleRate * LATENCY_MS / 1000).toInt().coerceAtMost(RING - SpectrumAnalyzer.SIZE - 1)
        val start = written - delay - SpectrumAnalyzer.SIZE
        for (i in 0 until SpectrumAnalyzer.SIZE) {
            out[i] = ring[mask(start + i)]
        }
        return sampleRate
    }

    // ---------------------------------------------------------- зрители и цикл

    private val viewers = AtomicInteger(0)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private var job: Job? = null
    private val analyzer = SpectrumAnalyzer()

    /** Счётчик кадров: читается в фазе рисования, чтобы перерисовывать только столбики. */
    val tick = mutableLongStateOf(0L)

    /** Высоты столбиков 0..1; читать после чтения [tick]. */
    val levels: FloatArray get() = analyzer.levels

    /** Столбики показались на экране. Каждому `acquire` — свой `release`. */
    @Synchronized
    fun acquire() {
        if (viewers.getAndIncrement() == 0) {
            job = scope.launch { run() }
        }
        refreshCapture()
    }

    @Synchronized
    fun release() {
        if (viewers.updateAndGet { (it - 1).coerceAtLeast(0) } == 0) {
            job?.cancel()
            job = null
            analyzer.clear()
            tick.longValue++
        }
        refreshCapture()
    }

    private fun refreshCapture() {
        capturing = enabledState && viewers.get() > 0
    }

    private suspend fun run() {
        val buffer = FloatArray(SpectrumAnalyzer.SIZE)
        var last = System.nanoTime()
        var cleared = false
        while (true) {
            val now = System.nanoTime()
            val dt = (now - last) / 1_000_000_000f
            last = now
            var busy = false
            if (!enabledState) {
                // Выключили: столбики пропадают сразу.
                if (!cleared) {
                    analyzer.clear()
                    tick.longValue++
                    cleared = true
                }
            } else {
                cleared = false
                val sampleRate = snapshot(buffer)
                try {
                    analyzer.update(if (sampleRate > 0) buffer else null, sampleRate, dt)
                } catch (e: RuntimeException) {
                    // Анализ не должен ломать приложение: просто опускаем столбики.
                    analyzer.clear()
                }
                busy = sampleRate > 0 || analyzer.moving()
                if (busy) tick.longValue++
            }
            delay(if (busy) FRAME_MS else IDLE_MS)
        }
    }

    /** Отсчёты старше этого — уже не звук, а пауза. */
    private const val FRESH_MS = 150L

    /** Оценка задержки до динамика (буфер `AudioTrack`), мс: чтобы спектр не опережал звук. */
    private const val LATENCY_MS = 120L

    private const val FRAME_MS = 33L
    private const val IDLE_MS = 150L
}
