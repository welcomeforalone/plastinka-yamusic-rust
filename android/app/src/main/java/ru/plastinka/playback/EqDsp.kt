package ru.plastinka.playback

import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.pow
import kotlin.math.sin

// Графический эквалайзер: 10 полос, пиковые фильтры второго порядка (формулы
// из общеизвестного «Audio EQ Cookbook», добротность в одну октаву). Порт
// src/player/eq.rs. Чистый Kotlin без Android: всё считается в памяти на лету.

/** Постоянные эквалайзера: те же, что на десктопе. */
object EqSpec {
    const val BANDS = 10

    /** Центральные частоты полос, Гц. */
    val FREQS = intArrayOf(32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000)

    /** Предел усиления полосы и предусилителя, десятые доли дБ. */
    const val GAIN_LIMIT = 120

    /** Не больше стольких своих пресетов. */
    const val MAX_CUSTOM = 32

    /** Длина имени своего пресета, символов. */
    const val NAME_MAX = 32

    /** Шаг ползунков, десятые доли дБ (0,5 дБ). */
    const val STEP = 5

    fun clampGain(value: Int): Int = value.coerceIn(-GAIN_LIMIT, GAIN_LIMIT)
}

/**
 * Параметры эквалайзера. Усиления — целые десятые доли дБ. Объект
 * неизменяемый: звуковой поток берёт его как снимок из [EqControl].
 */
data class EqSettings(
    val enabled: Boolean = false,
    val autoPreamp: Boolean = true,
    val preamp: Int = 0,
    val gains: List<Int> = List(EqSpec.BANDS) { 0 },
) {
    /** Все значения в пределах и ровно десять полос (лишнее отбрасывается, недостающее — нули). */
    fun sanitized(): EqSettings = copy(
        preamp = EqSpec.clampGain(preamp),
        gains = List(EqSpec.BANDS) { EqSpec.clampGain(gains.getOrElse(it) { 0 }) },
    )

    /**
     * Предусилитель, который реально применяется: при [autoPreamp] ещё минус
     * наибольшее положительное усиление полосы, чтобы пики не упирались в 0 dBFS.
     */
    fun effectivePreamp(): Int {
        if (!autoPreamp) return preamp
        var peak = 0
        for (gain in gains) if (gain > peak) peak = gain
        return preamp - peak
    }

    companion object {
        /** Выключенный эквалайзер с нулями. */
        val OFF = EqSettings()
    }
}

/**
 * Эквалайзер одного источника: коэффициенты и память фильтров по каналам.
 * Усиления идут к цели плавно, не быстрее [STEP_DB_PER_SEC]. После создания
 * ничего не выделяет: все массивы заранее.
 *
 * Порядок вызова на каждый блок: [prepare]; если ответ `false` — для каждого
 * кадра [nextPreamp], затем для каждого канала [run].
 */
class Equalizer(val channels: Int, val rate: Int, initial: EqSettings) {

    private val targetGains = FloatArray(EqSpec.BANDS)
    private var targetPreamp = 0f

    /** Текущие (сглаженные) усиления полос и предусилитель, дБ. */
    private val gains = FloatArray(EqSpec.BANDS)
    private var preamp = 0f

    /** Множитель предусилителя в конце прошлого блока. */
    private var preampLinear = 1.0
    private var preampNow = 1.0
    private var preampStep = 0.0

    private val b0 = DoubleArray(EqSpec.BANDS)
    private val b1 = DoubleArray(EqSpec.BANDS)
    private val b2 = DoubleArray(EqSpec.BANDS)
    private val a1 = DoubleArray(EqSpec.BANDS)
    private val a2 = DoubleArray(EqSpec.BANDS)

    /** Нулевое усиление или частота слишком высокая — фильтр пропускается. */
    private val active = BooleanArray(EqSpec.BANDS)

    /** Память фильтров (транспонированная прямая форма II): канал, полоса, две ячейки. */
    private val memory = DoubleArray(channels.coerceAtLeast(1) * EqSpec.BANDS * 2)

    init {
        reset(initial)
    }

    /** Новые цели по настройкам: выключен — все нули. Без выделений памяти. */
    fun setTargets(settings: EqSettings) {
        if (!settings.enabled) {
            targetGains.fill(0f)
            targetPreamp = 0f
            return
        }
        for (band in 0 until EqSpec.BANDS) {
            targetGains[band] = EqSpec.clampGain(settings.gains.getOrElse(band) { 0 }) / 10f
        }
        targetPreamp = settings.effectivePreamp() / 10f
    }

    /** Сразу в цель и чистая память: новый трек или перемотка не «въезжают» в эквалайзер. */
    fun reset(settings: EqSettings) {
        setTargets(settings)
        for (band in 0 until EqSpec.BANDS) {
            gains[band] = targetGains[band]
            setPeaking(band, gains[band])
        }
        preamp = targetPreamp
        preampLinear = dbToLinear(preamp)
        preampNow = preampLinear
        preampStep = 0.0
        memory.fill(0.0)
    }

    /**
     * Готовит блок из [frames] кадров: шаг усилений к цели и расчёт множителя
     * предусилителя. `true` — всё в нуле, блок надо пропустить побайтно.
     */
    fun prepare(frames: Int): Boolean {
        // Быстрее этого усиление к цели не идёт; считаем по числу кадров, чтобы
        // скорость не зависела от размера буфера декодера.
        val maxStep = STEP_DB_PER_SEC * frames / rate.coerceAtLeast(1)
        for (band in 0 until EqSpec.BANDS) {
            val next = slew(gains[band], targetGains[band], maxStep)
            if (next != gains[band]) {
                gains[band] = next
                setPeaking(band, next)
                // Полоса выключилась: память не должна дождаться включения и щёлкнуть.
                if (!active[band]) clearBand(band)
            }
        }
        preamp = slew(preamp, targetPreamp, maxStep)
        // Всё в нуле, и прошлый блок закончился на единице (иначе на стыке была
        // бы ступенька): точный обход, память забываем.
        if (preamp == 0f && preampLinear == 1.0 && gains.all { it == 0f }) {
            memory.fill(0.0)
            return true
        }
        val to = dbToLinear(preamp)
        // Предусилитель меняется по кадрам плавно, без ступеньки на стыке блоков.
        preampStep = if (frames > 0) (to - preampLinear) / frames else 0.0
        preampNow = preampLinear
        preampLinear = to
        return false
    }

    /** Множитель предусилителя для следующего кадра. */
    fun nextPreamp(): Double {
        preampNow += preampStep
        return preampNow
    }

    /**
     * Один отсчёт канала [channel] через все фильтры и предусилитель. Нечисло
     * на входе или в памяти: память канала обнуляется, отсчёт — ноль, звук не
     * «залипает».
     */
    fun run(channel: Int, input: Float, preampGain: Double): Float {
        var value = input.toDouble()
        val base = channel * EqSpec.BANDS * 2
        if (value.isFinite() && base >= 0 && base + EqSpec.BANDS * 2 <= memory.size) {
            for (band in 0 until EqSpec.BANDS) {
                if (!active[band]) continue
                val z = base + band * 2
                val out = b0[band] * value + memory[z]
                memory[z] = b1[band] * value - a1[band] * out + memory[z + 1]
                memory[z + 1] = b2[band] * value - a2[band] * out
                value = out
            }
            val result = (value * preampGain).toFloat()
            if (result.isFinite()) return result
        }
        if (base >= 0 && base + EqSpec.BANDS * 2 <= memory.size) {
            memory.fill(0.0, base, base + EqSpec.BANDS * 2)
        }
        return 0f
    }

    /** Коэффициенты пикового фильтра полосы [band] (a0 уже поделён). */
    private fun setPeaking(band: Int, gainDb: Float) {
        val freq = EqSpec.FREQS[band]
        if (gainDb == 0f || freq >= MAX_RATIO * rate) {
            active[band] = false
            return
        }
        val a = 10.0.pow(gainDb.toDouble() / 40.0)
        val w0 = 2.0 * PI * freq / rate
        val alpha = sin(w0) / (2.0 * Q)
        val c = cos(w0)
        val a0 = 1.0 + alpha / a
        b0[band] = (1.0 + alpha * a) / a0
        b1[band] = -2.0 * c / a0
        b2[band] = (1.0 - alpha * a) / a0
        a1[band] = -2.0 * c / a0
        a2[band] = (1.0 - alpha / a) / a0
        active[band] = true
    }

    private fun clearBand(band: Int) {
        for (channel in 0 until channels.coerceAtLeast(1)) {
            val z = (channel * EqSpec.BANDS + band) * 2
            memory.fill(0.0, z, z + 2)
        }
    }

    private fun slew(current: Float, target: Float, maxStep: Float): Float {
        val delta = target - current
        return if (abs(delta) <= maxStep) target else current + if (delta > 0f) maxStep else -maxStep
    }

    private fun dbToLinear(db: Float): Double = 10.0.pow(db.toDouble() / 20.0)

    companion object {
        /** Добротность пикового фильтра (октава). */
        const val Q = 1.41

        /** Полоса не применяется, если её частота не ниже этой доли частоты дискретизации. */
        const val MAX_RATIO = 0.45

        /**
         * Скорость сдвига усиления: на десктопе 1,5 дБ за порцию в 1024 кадра
         * при 44,1 кГц (около 23 мс), то есть около 65 дБ в секунду.
         */
        const val STEP_DB_PER_SEC = 65f

        /** Сколько кадров обрабатывается между шагами усиления (ровность перехода). */
        const val BLOCK_FRAMES = 512
    }
}
