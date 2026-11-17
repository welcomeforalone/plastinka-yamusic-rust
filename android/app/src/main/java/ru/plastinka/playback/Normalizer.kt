package ru.plastinka.playback

import androidx.media3.common.C
import androidx.media3.common.audio.AudioProcessor.AudioFormat
import androidx.media3.common.audio.BaseAudioProcessor
import java.nio.ByteBuffer
import ru.plastinka.data.Loudness

/**
 * Нормализация громкости: множитель играющего трека (из `r128`) и флаг
 * настройки. Только числа в памяти. Множитель применяет [GainProcessor] в
 * звуковом тракте, а не громкость плеера: у `ExoPlayer.volume` потолок 1,0, а
 * тихие треки надо усиливать (до +6 дБ).
 */
object Normalizer {
    @Volatile
    var enabled = false

    /** Множитель текущего трека без учёта флага: 1,0, если громкость неизвестна. */
    @Volatile
    private var trackGain = 1f

    /**
     * Текущий трек сменился или решение о его звуке уточнилось. [loudness] — громкость того,
     * что реально играет (у замены FckCensor — громкость трека замены, у прямой ссылки и
     * пока неизвестно — `null`): множитель 1.
     */
    fun onTrack(loudness: Loudness?) {
        trackGain = loudness?.gainFactor() ?: 1f
    }

    /** Множитель сейчас: 1, если нормализация выключена. */
    fun gain(): Float = if (enabled) trackGain else 1f
}

/**
 * Усиление или ослабление звука на [Normalizer.gain] (16 бит и float).
 * Множитель меняется плавно в пределах буфера, без щелчков; результат
 * зажимается в допустимый диапазон. Множитель 1 — звук проходит как есть.
 * Формат другой — процессор неактивен.
 */
class GainProcessor : BaseAudioProcessor() {

    private var current = 1f

    override fun onConfigure(inputAudioFormat: AudioFormat): AudioFormat {
        val supported = (inputAudioFormat.encoding == C.ENCODING_PCM_16BIT ||
            inputAudioFormat.encoding == C.ENCODING_PCM_FLOAT) &&
            inputAudioFormat.channelCount > 0
        return if (supported) inputAudioFormat else AudioFormat.NOT_SET
    }

    override fun queueInput(inputBuffer: ByteBuffer) {
        val size = inputBuffer.remaining()
        if (size == 0) return
        val target = Normalizer.gain()
        val out = replaceOutputBuffer(size)
        if (current == 1f && target == 1f) {
            out.put(inputBuffer)
        } else {
            apply(inputBuffer, out, target)
        }
        out.flip()
    }

    private fun apply(input: ByteBuffer, out: ByteBuffer, target: Float) {
        val channels = inputAudioFormat.channelCount
        val float = inputAudioFormat.encoding == C.ENCODING_PCM_FLOAT
        val bytes = if (float) 4 else 2
        val frames = input.remaining() / (bytes * channels)
        val step = if (frames > 0) (target - current) / frames else 0f
        var gain = current
        for (frame in 0 until frames) {
            gain += step
            for (channel in 0 until channels) {
                if (float) {
                    out.putFloat((input.getFloat() * gain).coerceIn(-1f, 1f))
                } else {
                    val scaled = (input.getShort() * gain).toInt()
                    out.putShort(scaled.coerceIn(Short.MIN_VALUE.toInt(), Short.MAX_VALUE.toInt()).toShort())
                }
            }
        }
        current = target
        // Хвост меньше кадра (не бывает на целых кадрах) копируем как есть.
        out.put(input)
    }

    override fun onReset() {
        current = Normalizer.gain()
    }
}
