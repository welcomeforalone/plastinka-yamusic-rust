package ru.plastinka.playback

import android.content.Context
import androidx.media3.common.C
import androidx.media3.common.audio.AudioProcessor
import androidx.media3.common.audio.AudioProcessor.AudioFormat
import androidx.media3.common.audio.BaseAudioProcessor
import androidx.media3.exoplayer.DefaultRenderersFactory
import androidx.media3.exoplayer.audio.AudioSink
import androidx.media3.exoplayer.audio.DefaultAudioSink
import java.nio.ByteBuffer

/**
 * Цепочка звуковых процессоров плеера — единственное место сборки.
 * Нормализация громкости идёт первой, затем эквалайзер. Спектр показывает
 * звук после эквалайзера, поэтому отвод идёт последним.
 */
object AudioChain {
    fun processors(): Array<AudioProcessor> = arrayOf(
        GainProcessor(),
        EqProcessor(),
        SpectrumTap(),
    )
}

/** Фабрика рендереров с нашей цепочкой процессоров в аудиотракте. */
class PlastinkaRenderersFactory(context: Context) : DefaultRenderersFactory(context) {
    override fun buildAudioSink(
        context: Context,
        enableFloatOutput: Boolean,
        enableAudioTrackPlaybackParams: Boolean,
    ): AudioSink =
        DefaultAudioSink.Builder(context)
            .setEnableFloatOutput(enableFloatOutput)
            .setEnableAudioTrackPlaybackParams(enableAudioTrackPlaybackParams)
            .setAudioProcessors(AudioChain.processors())
            .build()
}

/**
 * Отвод звука для визуализатора. Пропускает звук без изменений; если
 * [SpectrumHub.capturing], копирует моно-отсчёты в кольцо в памяти. Без
 * выделений памяти на буфер и без блокировок. Формат не 16 бит и не float —
 * процессор неактивен (звук идёт мимо него).
 */
class SpectrumTap : BaseAudioProcessor() {

    override fun onConfigure(inputAudioFormat: AudioFormat): AudioFormat {
        val supported = (inputAudioFormat.encoding == C.ENCODING_PCM_16BIT ||
            inputAudioFormat.encoding == C.ENCODING_PCM_FLOAT) &&
            inputAudioFormat.channelCount > 0 &&
            inputAudioFormat.sampleRate > 0
        // NOT_SET делает процессор неактивным: звук не ломается, анализ молчит.
        return if (supported) inputAudioFormat else AudioFormat.NOT_SET
    }

    override fun queueInput(inputBuffer: ByteBuffer) {
        val size = inputBuffer.remaining()
        if (size == 0) return
        if (SpectrumHub.capturing) {
            try {
                capture(inputBuffer)
            } catch (e: RuntimeException) {
                // Анализ не должен задеть звук.
            }
        }
        val out = replaceOutputBuffer(size)
        out.put(inputBuffer)
        out.flip()
    }

    /** Сводит кадры в моно и пишет в кольцо; [input] не сдвигается (читаем по индексу). */
    private fun capture(input: ByteBuffer) {
        val format = inputAudioFormat
        val channels = format.channelCount
        val float = format.encoding == C.ENCODING_PCM_FLOAT
        val bytes = if (float) 4 else 2
        val frames = input.remaining() / (bytes * channels)
        val base = input.position()
        val scale = 1f / channels
        var pos = SpectrumHub.writePosition()
        for (frame in 0 until frames) {
            var sum = 0f
            val at = base + frame * channels * bytes
            for (c in 0 until channels) {
                sum += if (float) {
                    input.getFloat(at + c * 4)
                } else {
                    input.getShort(at + c * 2) / 32768f
                }
            }
            SpectrumHub.ring[SpectrumHub.mask(pos)] = sum * scale
            pos++
        }
        SpectrumHub.publish(pos, format.sampleRate)
    }
}
