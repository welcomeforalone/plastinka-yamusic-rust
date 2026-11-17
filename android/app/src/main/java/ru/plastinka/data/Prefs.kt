package ru.plastinka.data

import android.content.Context
import android.content.SharedPreferences
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import ru.plastinka.playback.EqControl
import ru.plastinka.playback.Normalizer
import ru.plastinka.playback.SpectrumHub

/**
 * Качество звука. Ключи и значения запроса те же, что на десктопе
 * (src/api/stream.rs). HE-AAC не просим: декодер читает только AAC-LC.
 */
enum class AudioQuality(
    val key: String,
    val title: String,
    val hint: String,
    /** Параметр `quality` запроса `get-file-info`. */
    val level: String,
    /** Параметр `codecs` запроса `get-file-info`. */
    val codecs: String,
) {
    Lossless(
        "lossless",
        "Превосходное",
        "Музыка в lossless и других высококачественных форматах для быстрого интернета и хорошей акустики",
        "lossless",
        "flac,flac-mp4,aac-mp4,aac,mp3",
    ),
    Normal(
        "normal",
        "Оптимальное",
        "Сбалансированный звук для большинства устройств",
        "nq",
        "aac-mp4,aac,mp3",
    ),
    Low(
        "low",
        "Экономичное",
        "Стабильное звучание при медленном интернете",
        "lq",
        "aac-mp4,aac,mp3",
    ),
    ;

    companion object {
        /** Непонятное или пустое значение — «Оптимальное» по умолчанию. */
        fun fromKey(key: String?): AudioQuality = entries.firstOrNull { it.key == key } ?: Normal
    }
}

/**
 * Настройки приложения: SharedPreferences `plastinka`, только метаданные
 * (ничего про звук и токен). Поля — состояние Compose: смена сразу видна в
 * интерфейсе; сервис воспроизведения живёт в том же процессе и читает те же
 * поля. Значения по умолчанию — как на десктопе (src/ui/mod.rs), кроме
 * цвета волны: он выключен, как все плагины.
 */
object Prefs {
    private const val FILE = "plastinka"

    private const val KEY_ACCENT = "accent"
    private const val KEY_VISUALIZER = "visualizer"
    private const val KEY_VIBE = "vibe_colorizer"
    private const val KEY_BLUR = "blur_cover"
    private const val KEY_QUALITY = "audio_quality"
    private const val KEY_NORMALIZE = "normalize"
    private const val KEY_LRCLIB = "lyrics_lrclib"
    private const val KEY_EQ = "equalizer"
    private const val KEY_FCKCENSOR = "plugin_fckcensor"
    private const val KEY_SLOPLESS = "plugin_slopless"
    private const val KEY_SLOP_AUTOSKIP = "plugin_slopless_autoskip"
    private const val KEY_SLOP_DISLIKE = "plugin_slopless_dislike"
    private const val KEY_COPY_NAME = "plugin_copy_name"
    private const val KEY_COPY_FORMAT = "plugin_copy_name_format"
    private const val KEY_WAVE_ACTIVITY = "wave_activity"
    private const val KEY_WAVE_ACTIVITY_NAME = "wave_activity_name"
    private const val KEY_WAVE_DIVERSITY = "wave_diversity"
    private const val KEY_WAVE_MOOD = "wave_mood"
    private const val KEY_WAVE_LANGUAGE = "wave_language"

    private var store: SharedPreferences? = null

    var visualizer by mutableStateOf(true)
        private set
    var vibeColorizer by mutableStateOf(false)
        private set
    var blurCover by mutableStateOf(true)
        private set
    var quality by mutableStateOf(AudioQuality.Normal)
        private set
    var normalize by mutableStateOf(false)
        private set
    var lyricsLrclib by mutableStateOf(true)
        private set

    // Плагины: все выключены по умолчанию (автопропуск Slopless включён вместе с плагином, как на десктопе).
    var fckCensor by mutableStateOf(false)
        private set
    var slopless by mutableStateOf(false)
        private set
    var slopAutoskip by mutableStateOf(true)
        private set

    /** Автоматическое «Не нравится» — запись в аккаунт, только после согласия пользователя. */
    var slopDislike by mutableStateOf(false)
        private set
    var copyName by mutableStateOf(false)
        private set

    /** Сервис воспроизведения подписывается, чтобы сразу перепроверить играющий трек. */
    @Volatile
    var onSlopChanged: (() -> Unit)? = null

    var copyFormat by mutableStateOf(CopyName.DEFAULT_FORMAT)
        private set

    /** Выбор настроек «Моей волны» (не запись в аккаунт: только на устройстве). */
    var waveSettings by mutableStateOf(WaveSettings())
        private set

    /** Эквалайзер: параметры, выбранный пресет и свои пресеты (только метаданные). */
    var eq by mutableStateOf(EqState())
        private set

    /** Загрузка при старте приложения. Битое или неизвестное значение — по умолчанию. */
    fun load(context: Context) {
        val prefs = context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE)
        store = prefs
        val all = try {
            prefs.all
        } catch (_: Exception) {
            emptyMap<String, Any?>()
        }
        fun bool(key: String, default: Boolean) = all[key] as? Boolean ?: default
        visualizer = bool(KEY_VISUALIZER, true)
        vibeColorizer = bool(KEY_VIBE, false)
        blurCover = bool(KEY_BLUR, true)
        quality = AudioQuality.fromKey(all[KEY_QUALITY] as? String)
        normalize = bool(KEY_NORMALIZE, false)
        lyricsLrclib = bool(KEY_LRCLIB, true)
        eq = EqState.fromJson(all[KEY_EQ] as? String)
        fckCensor = bool(KEY_FCKCENSOR, false)
        slopless = bool(KEY_SLOPLESS, false)
        slopAutoskip = bool(KEY_SLOP_AUTOSKIP, true)
        slopDislike = bool(KEY_SLOP_DISLIKE, false)
        copyName = bool(KEY_COPY_NAME, false)
        copyFormat = CopyName.sanitize(all[KEY_COPY_FORMAT] as? String)
        waveSettings = WaveSettings.fromStored(
            all[KEY_WAVE_ACTIVITY] as? String,
            all[KEY_WAVE_ACTIVITY_NAME] as? String,
            all[KEY_WAVE_DIVERSITY] as? String,
            all[KEY_WAVE_MOOD] as? String,
            all[KEY_WAVE_LANGUAGE] as? String,
        )
        FckCensor.enabled = fckCensor
        SpectrumHub.enabled = visualizer
        Normalizer.enabled = normalize
        EqControl.settings = eq.settings.sanitized()
    }

    /** Строка акцента (`red` или `#RRGGBB`) или `null`. */
    fun accentKey(): String? = try {
        store?.getString(KEY_ACCENT, null)
    } catch (_: Exception) {
        null
    }

    fun saveAccentKey(key: String) {
        store?.edit()?.putString(KEY_ACCENT, key)?.apply()
    }

    fun changeVisualizer(on: Boolean) {
        visualizer = on
        SpectrumHub.enabled = on
        store?.edit()?.putBoolean(KEY_VISUALIZER, on)?.apply()
    }

    fun changeVibeColorizer(on: Boolean) {
        vibeColorizer = on
        store?.edit()?.putBoolean(KEY_VIBE, on)?.apply()
    }

    fun changeBlurCover(on: Boolean) {
        blurCover = on
        store?.edit()?.putBoolean(KEY_BLUR, on)?.apply()
    }

    /** Качество сменилось: сервис перезапускает играющий трек с той же позиции. */
    @Volatile
    var onQualityChanged: (() -> Unit)? = null

    fun changeQuality(value: AudioQuality) {
        // То же значение выбрано повторно: ничего не меняется, трек не перезапускаем.
        if (value == quality) return
        quality = value
        store?.edit()?.putString(KEY_QUALITY, value.key)?.apply()
        onQualityChanged?.invoke()
    }

    fun changeNormalize(on: Boolean) {
        normalize = on
        Normalizer.enabled = on
        store?.edit()?.putBoolean(KEY_NORMALIZE, on)?.apply()
    }

    fun changeLyricsLrclib(on: Boolean) {
        lyricsLrclib = on
        store?.edit()?.putBoolean(KEY_LRCLIB, on)?.apply()
    }

    /**
     * Новое состояние эквалайзера. Звуковой поток получает параметры сразу
     * (слышно во время перетаскивания); на диск — только при [save], то есть
     * по окончании жеста, а не на каждый пиксель.
     */
    fun changeEq(state: EqState, save: Boolean) {
        eq = state
        EqControl.settings = state.settings.sanitized()
        if (save) saveEq()
    }

    /** Записывает текущее состояние эквалайзера (конец жеста, смена пресетов). */
    fun saveEq() {
        store?.edit()?.putString(KEY_EQ, eq.toJson())?.apply()
    }

    /** Новый выбор настроек волны; пустой выбор стирает записи. */
    fun changeWaveSettings(value: WaveSettings) {
        waveSettings = value
        store?.edit()
            ?.putOrRemove(KEY_WAVE_ACTIVITY, value.activity?.tag)
            ?.putOrRemove(KEY_WAVE_ACTIVITY_NAME, value.activity?.name)
            ?.putOrRemove(KEY_WAVE_DIVERSITY, value.diversity?.key)
            ?.putOrRemove(KEY_WAVE_MOOD, value.mood?.key)
            ?.putOrRemove(KEY_WAVE_LANGUAGE, value.language?.key)
            ?.apply()
    }

    /**
     * «Сбросить всё»: звук, оформление (акцент возвращает `AccentState`, см. экран настроек), текст
     * песни, эквалайзер и плагины возвращаются к значениям по умолчанию. Остаются
     * свои пресеты эквалайзера и настройки «Моей волны» (как на десктопе). Согласие на
     * «Не нравится» Slopless сбрасывается вместе с плагином. Каждое значение меняется
     * тем же методом, что при ручной смене, поэтому плеер, спектр, нормализатор и
     * плагины сразу получают новое; качество перезапускает трек, только если менялось.
     */
    fun resetAll() {
        changeVisualizer(true)
        changeVibeColorizer(false)
        changeBlurCover(true)
        changeQuality(AudioQuality.Normal)
        changeNormalize(false)
        changeLyricsLrclib(true)
        changeEq(EqState(custom = eq.custom), save = true)
        if (fckCensor) changeFckCensor(false)
        if (slopless) changeSlopless(false)
        if (!slopAutoskip) changeSlopAutoskip(true)
        changeSlopDislike(false)
        changeCopyName(false)
        changeCopyFormat(CopyName.DEFAULT_FORMAT)
    }

    private fun SharedPreferences.Editor.putOrRemove(key: String, value: String?): SharedPreferences.Editor =
        if (value == null) remove(key) else putString(key, value)

    // ------------------------------------------------------------ плагины

    fun changeFckCensor(on: Boolean) {
        fckCensor = on
        FckCensor.enabled = on
        store?.edit()?.putBoolean(KEY_FCKCENSOR, on)?.apply()
        // Включили — список подгружается сразу, замены действуют со следующего трека.
        FckCensor.current?.onToggle()
    }

    fun changeSlopless(on: Boolean) {
        slopless = on
        store?.edit()?.putBoolean(KEY_SLOPLESS, on)?.apply()
        onSlopChanged?.invoke()
    }

    fun changeSlopAutoskip(on: Boolean) {
        slopAutoskip = on
        store?.edit()?.putBoolean(KEY_SLOP_AUTOSKIP, on)?.apply()
        onSlopChanged?.invoke()
    }

    /** Вызывать только после явного согласия (диалог на странице плагинов). */
    fun changeSlopDislike(on: Boolean) {
        slopDislike = on
        store?.edit()?.putBoolean(KEY_SLOP_DISLIKE, on)?.apply()
    }

    fun changeCopyName(on: Boolean) {
        copyName = on
        store?.edit()?.putBoolean(KEY_COPY_NAME, on)?.apply()
    }

    /** Формат копирования: пока пользователь печатает, храним как есть (пустой — формат по умолчанию при копировании). */
    fun changeCopyFormat(value: String) {
        copyFormat = value.take(CopyName.MAX_FORMAT)
        store?.edit()?.putString(KEY_COPY_FORMAT, copyFormat)?.apply()
    }
}
