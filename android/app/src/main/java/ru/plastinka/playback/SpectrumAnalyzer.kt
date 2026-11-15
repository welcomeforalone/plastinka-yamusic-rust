package ru.plastinka.playback

import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.log10
import kotlin.math.log2
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.sin
import kotlin.math.sqrt

/**
 * Спектр для столбиков: БПФ на [SIZE] отсчётов с окном Ханна, свёртка в [BARS]
 * полос на логарифмической шкале и сглаживание (быстрый подъём, плавный спад).
 * Логика та же, что в десктопном `src/ui/visualizer.rs`. Чистый класс без
 * Android-зависимостей; звук только в памяти.
 */
class SpectrumAnalyzer {

    /** Сглаженные высоты столбиков 0..1. */
    val levels = FloatArray(BARS)

    private val re = FloatArray(SIZE)
    private val im = FloatArray(SIZE)
    private val window = FloatArray(SIZE) { i ->
        (0.5 - 0.5 * cos(2.0 * PI * i / (SIZE - 1))).toFloat()
    }
    private val windowSum = window.sum()
    private val cosTable = FloatArray(SIZE / 2) { k -> cos(-2.0 * PI * k / SIZE).toFloat() }
    private val sinTable = FloatArray(SIZE / 2) { k -> sin(-2.0 * PI * k / SIZE).toFloat() }
    private val targets = FloatArray(BARS)

    // Границы полос в номерах отсчётов БПФ: пересчитываются при смене частоты.
    private var bandRate = 0
    private val bandFirst = IntArray(BARS)
    private val bandLast = IntArray(BARS)
    private val bandTilt = FloatArray(BARS)

    /**
     * Новый кадр. [samples] — ровно [SIZE] моно-отсчётов (старые первыми),
     * `null` — звука нет, столбики опускаются. [dt] — секунды с прошлого кадра.
     */
    fun update(samples: FloatArray?, rate: Int, dt: Float) {
        val step = dt.coerceIn(0f, 0.1f)
        if (samples != null && samples.size >= SIZE && rate > 0) {
            spectrum(samples, rate)
        } else {
            targets.fill(0f)
        }
        for (i in 0 until BARS) {
            val level = levels[i]
            val target = targets[i]
            levels[i] = if (target > level) {
                level + (target - level) * min(step * RISE_RATE, 1f)
            } else {
                max(level - step * FALL_PER_SECOND, target)
            }
        }
    }

    /** Столбики ещё не опустились: кадры нужны. */
    fun moving(): Boolean = levels.any { it > REST }

    /** Мгновенно обнулить (визуализатор выключили). */
    fun clear() {
        levels.fill(0f)
    }

    private fun prepareBands(rate: Int) {
        if (rate == bandRate) return
        bandRate = rate
        val binHz = rate.toFloat() / SIZE
        val nyquist = SIZE / 2 - 1
        val ratio = HIGH_HZ / LOW_HZ
        for (band in 0 until BARS) {
            val low = LOW_HZ * ratio.pow(band.toFloat() / BARS)
            val high = LOW_HZ * ratio.pow((band + 1).toFloat() / BARS)
            val first = (low / binHz).toInt().coerceIn(1, nyquist)
            bandFirst[band] = first
            bandLast[band] = (high / binHz).toInt().coerceIn(first, nyquist)
            bandTilt[band] = TILT_DB_PER_OCTAVE * log2(sqrt(low * high) / 1000f)
        }
    }

    private fun spectrum(samples: FloatArray, rate: Int) {
        prepareBands(rate)
        for (i in 0 until SIZE) {
            re[i] = samples[i] * window[i]
            im[i] = 0f
        }
        fft()
        // Амплитуда синуса полной громкости даёт 1.
        val norm = 2f / max(windowSum, 1f)
        for (band in 0 until BARS) {
            var peak = 0f
            for (bin in bandFirst[band]..bandLast[band]) {
                val magnitude = sqrt(re[bin] * re[bin] + im[bin] * im[bin]) * norm
                if (magnitude > peak) peak = magnitude
            }
            val db = 20f * log10(peak + 1e-9f) + bandTilt[band]
            val level = (db - FLOOR_DB) / (CEIL_DB - FLOOR_DB)
            // Битые отсчёты (NaN, бесконечность) не должны ломать столбик.
            targets[band] = if (level.isNaN()) 0f else level.coerceIn(0f, 1f)
        }
    }

    /** БПФ по основанию 2 на месте; длина — [SIZE]. */
    private fun fft() {
        val n = SIZE
        val bits = Integer.numberOfTrailingZeros(n)
        for (i in 0 until n) {
            val j = Integer.reverse(i) ushr (32 - bits)
            if (j > i) {
                val tr = re[i]; re[i] = re[j]; re[j] = tr
                val ti = im[i]; im[i] = im[j]; im[j] = ti
            }
        }
        var len = 2
        while (len <= n) {
            val half = len / 2
            val step = n / len
            var start = 0
            while (start < n) {
                for (k in 0 until half) {
                    val wr = cosTable[k * step]
                    val wi = sinTable[k * step]
                    val a = start + k
                    val b = a + half
                    val tr = re[b] * wr - im[b] * wi
                    val ti = re[b] * wi + im[b] * wr
                    re[b] = re[a] - tr
                    im[b] = im[a] - ti
                    re[a] += tr
                    im[a] += ti
                }
                start += len
            }
            len *= 2
        }
    }

    companion object {
        /** Размер БПФ (степень двойки). */
        const val SIZE = 2048

        /** Сколько столбиков. */
        const val BARS = 48

        private const val LOW_HZ = 35f
        private const val HIGH_HZ = 16_000f

        /** Громкость полосы, дающая пустой и полный столбик, дБ. */
        private const val FLOOR_DB = -62f
        private const val CEIL_DB = -12f

        /** Высокие частоты в музыке тише: подъём на столько дБ на октаву выше 1 кГц. */
        private const val TILT_DB_PER_OCTAVE = 3f

        /** Скорость падения: доля высоты в секунду. */
        private const val FALL_PER_SECOND = 1.4f

        /** Резкость подъёма. */
        private const val RISE_RATE = 28f

        /** Ниже этой высоты столбик считается опустившимся. */
        private const val REST = 0.002f
    }
}
