package ru.plastinka.playback

import androidx.media3.common.C
import androidx.media3.common.audio.AudioProcessor.AudioFormat
import androidx.media3.common.audio.BaseAudioProcessor
import java.nio.ByteBuffer

/**
 * Параметры эквалайзера для звукового потока: неизменяемый снимок за
 * `@Volatile`. Окно подменяет его целиком, процессор заметит смену по ссылке на
 * следующем буфере. Только числа в памяти.
 */
object EqControl {
    @Volatile
    var settings: EqSettings = EqSettings.OFF
}

/**
 * Эквалайзер в звуковом тракте (16 бит и float, любое число каналов и любая
 * частота дискретизации). Выключенный или нулевой эквалайзер пропускает звук
 * побайтно. Без выделений памяти на буфер и без блокировок; ошибка расчёта не
 * глушит звук: буфер уходит как есть. Результат зажат в допустимый диапазон.
 */
@androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)
class EqProcessor : BaseAudioProcessor() {

    private var eq: Equalizer? = null

    /** Снимок настроек, который уже учтён в [eq]. */
    private var seen: EqSettings = EqSettings.OFF

    override fun onConfigure(inputAudioFormat: AudioFormat): AudioFormat {
        val supported = (inputAudioFormat.encoding == C.ENCODING_PCM_16BIT ||
            inputAudioFormat.encoding == C.ENCODING_PCM_FLOAT) &&
            inputAudioFormat.channelCount > 0 &&
            inputAudioFormat.sampleRate > 0
        return if (supported) inputAudioFormat else AudioFormat.NOT_SET
    }

    /** Перемотка, новый трек, смена формата: фильтры чистые, усиления сразу в цель. */
    override fun onFlush() {
        val format = inputAudioFormat
        if (format.channelCount <= 0 || format.sampleRate <= 0) return
        val settings = EqControl.settings
        seen = settings
        val current = eq
        if (current != null && current.channels == format.channelCount && current.rate == format.sampleRate) {
            current.reset(settings)
        } else {
            eq = Equalizer(format.channelCount, format.sampleRate, settings)
        }
    }

    override fun onReset() {
        eq = null
        seen = EqSettings.OFF
    }

    override fun queueInput(inputBuffer: ByteBuffer) {
        val size = inputBuffer.remaining()
        if (size == 0) return
        val out = replaceOutputBuffer(size)
        val inPos = inputBuffer.position()
        val outPos = out.position()
        try {
            process(inputBuffer, out)
        } catch (e: RuntimeException) {
            // Сбой расчёта не должен глушить звук: отдаём буфер как есть.
            inputBuffer.position(inPos)
            out.position(outPos)
            out.put(inputBuffer)
        }
        out.flip()
    }

    private fun process(input: ByteBuffer, out: ByteBuffer) {
        val format = inputAudioFormat
        val channels = format.channelCount
        var equalizer = eq
        if (equalizer == null || equalizer.channels != channels || equalizer.rate != format.sampleRate) {
            // onFlush не был вызван (редкость): создаём один раз, не в горячем пути.
            equalizer = Equalizer(channels, format.sampleRate, EqControl.settings)
            eq = equalizer
            seen = EqControl.settings
        }
        val settings = EqControl.settings
        if (settings !== seen) {
            seen = settings
            equalizer.setTargets(settings)
        }
        val float = format.encoding == C.ENCODING_PCM_FLOAT
        val bytes = if (float) 4 else 2
        val frameBytes = bytes * channels
        var frames = input.remaining() / frameBytes
        while (frames > 0) {
            val block = minOf(frames, Equalizer.BLOCK_FRAMES)
            if (equalizer.prepare(block)) {
                copy(input, out, block * frameBytes)
            } else {
                for (frame in 0 until block) {
                    val preamp = equalizer.nextPreamp()
                    for (channel in 0 until channels) {
                        if (float) {
                            val y = equalizer.run(channel, input.getFloat(), preamp)
                            out.putFloat(y.coerceIn(-1f, 1f))
                        } else {
                            val y = equalizer.run(channel, input.getShort() / 32768f, preamp)
                            val v = Math.round(y * 32768f).coerceIn(Short.MIN_VALUE.toInt(), Short.MAX_VALUE.toInt())
                            out.putShort(v.toShort())
                        }
                    }
                }
            }
            frames -= block
        }
        // Хвост меньше кадра (на целых кадрах не бывает) копируем как есть.
        out.put(input)
    }

    /** Переносит [count] байт без изменений (ограничивая лимит входа на время копирования). */
    private fun copy(input: ByteBuffer, out: ByteBuffer, count: Int) {
        val limit = input.limit()
        input.limit(input.position() + count)
        out.put(input)
        input.limit(limit)
    }
}
