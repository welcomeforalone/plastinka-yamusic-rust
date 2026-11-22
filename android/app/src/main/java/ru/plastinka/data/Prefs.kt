package ru.plastinka.data

import android.content.Context
import android.content.SharedPreferences
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.serialization.json.put
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
    private const val KEY_THEME = "theme"
    private const val KEY_THEME_MODE = "theme_mode"
    private const val KEY_VISUALIZER = "visualizer"
    private const val KEY_VIBE = "vibe_colorizer"
    private const val KEY_BLUR = "blur_cover"
    private const val KEY_PIP_AUTO = "pip_auto_enter"
    private const val KEY_QUALITY = "audio_quality"
    private const val KEY_NORMALIZE = "normalize"
    private const val KEY_LRCLIB = "lyrics_lrclib"
    private const val KEY_EQ = "equalizer"
    private const val KEY_FCKCENSOR = "plugin_fckcensor"
    private const val KEY_FCK_EXPLICIT = "plugin_fckcensor_explicit"
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
    private const val KEY_SYNC_ENABLED = "sync_enabled"
    private const val KEY_SYNC_ID = "sync_device_id"
    private const val KEY_SYNC_NAME = "sync_device_name"
    private const val KEY_SYNC_DELAY = "sync_delay_ms"
    private const val KEY_SYNC_PAIRED = "sync_paired"

    private var store: SharedPreferences? = null

    var visualizer by mutableStateOf(true)
        private set
    var vibeColorizer by mutableStateOf(false)
        private set
    var blurCover by mutableStateOf(true)
        private set

    /** Уходить в мини-плеер (PiP) при сворачивании приложения во время игры. */
    var pipAutoEnter by mutableStateOf(false)
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

    /** FckCensor: искать на Яндексе explicit-версию цензурного трека (включено по умолчанию, как на десктопе). */
    var fckExplicit by mutableStateOf(true)
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

    /** «Слушать вместе»: включено ли (по умолчанию выключено). */
    var syncEnabled by mutableStateOf(false)
        private set

    /** Идентификатор устройства (§1): 32 hex, создаётся один раз; не секрет. */
    var syncDeviceId by mutableStateOf("")
        private set

    /** Имя устройства для других устройств; по умолчанию модель телефона. */
    var syncDeviceName by mutableStateOf("")
        private set

    /** Поправка задержки, мс (−500…+500). */
    var syncDelayMs by mutableStateOf(0)
        private set

    /** Сопряжённые устройства: id, имя, ОС (без секретов, §7). */
    var syncPaired by mutableStateOf<List<ru.plastinka.sync.PairedDevice>>(emptyList())
        private set

    /** Служба «Слушать вместе» подписывается: включение и выключение запускают и останавливают её. */
    @Volatile
    var onSyncEnabledChanged: (() -> Unit)? = null

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
        pipAutoEnter = bool(KEY_PIP_AUTO, false)
        quality = AudioQuality.fromKey(all[KEY_QUALITY] as? String)
        normalize = bool(KEY_NORMALIZE, false)
        lyricsLrclib = bool(KEY_LRCLIB, true)
        eq = EqState.fromJson(all[KEY_EQ] as? String)
        fckCensor = bool(KEY_FCKCENSOR, false)
        fckExplicit = bool(KEY_FCK_EXPLICIT, true)
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
        syncEnabled = bool(KEY_SYNC_ENABLED, false)
        syncDelayMs = ((all[KEY_SYNC_DELAY] as? Int) ?: 0).coerceIn(-500, 500)
        syncDeviceName = (all[KEY_SYNC_NAME] as? String)?.takeIf { it.isNotBlank() } ?: android.os.Build.MODEL.orEmpty()
        syncDeviceId = (all[KEY_SYNC_ID] as? String)?.takeIf { Regex("[0-9a-f]{32}").matches(it) }
            ?: java.security.SecureRandom().let { rnd ->
                ByteArray(16).also { rnd.nextBytes(it) }.joinToString("") { "%02x".format(it) }
            }.also { prefs.edit().putString(KEY_SYNC_ID, it).apply() }
        syncPaired = parsePaired(all[KEY_SYNC_PAIRED] as? String)
        FckCensor.enabled = fckCensor
        FckCensor.explicitSearch = fckExplicit
        SpectrumHub.enabled = visualizer
        Normalizer.enabled = normalize
        EqControl.settings = eq.settings.sanitized()
    }

    fun changeSyncEnabled(on: Boolean) {
        syncEnabled = on
        store?.edit()?.putBoolean(KEY_SYNC_ENABLED, on)?.apply()
        onSyncEnabledChanged?.invoke()
    }

    fun changeSyncDeviceName(name: String) {
        syncDeviceName = name.trim().ifEmpty { android.os.Build.MODEL.orEmpty() }
        store?.edit()?.putString(KEY_SYNC_NAME, syncDeviceName)?.apply()
    }

    fun changeSyncDelay(ms: Int) {
        syncDelayMs = ms.coerceIn(-500, 500)
        store?.edit()?.putInt(KEY_SYNC_DELAY, syncDelayMs)?.apply()
    }

    fun saveSyncPaired(list: List<ru.plastinka.sync.PairedDevice>) {
        syncPaired = list
        val arr = kotlinx.serialization.json.buildJsonArray {
            for (d in list) {
                add(
                    kotlinx.serialization.json.buildJsonObject {
                        put("id", d.id)
                        put("name", d.name)
                        put("os", d.os)
                    },
                )
            }
        }
        store?.edit()?.putString(KEY_SYNC_PAIRED, arr.toString())?.apply()
    }

    /** Битая или пустая запись — пустой список. */
    private fun parsePaired(text: String?): List<ru.plastinka.sync.PairedDevice> {
        if (text == null) return emptyList()
        return try {
            (kotlinx.serialization.json.Json.parseToJsonElement(text) as? kotlinx.serialization.json.JsonArray)
                ?.mapNotNull { el ->
                    val o = el as? kotlinx.serialization.json.JsonObject ?: return@mapNotNull null
                    fun f(k: String) = (o[k] as? kotlinx.serialization.json.JsonPrimitive)?.takeIf { it.isString }?.content
                    val id = f("id")?.takeIf { Regex("[0-9a-f]{32}").matches(it) } ?: return@mapNotNull null
                    ru.plastinka.sync.PairedDevice(id, f("name") ?: "", f("os") ?: "unknown")
                }.orEmpty()
        } catch (_: Exception) {
            emptyList()
        }
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

    /** Ключ семейства темы (`classic`, `midnight`…) или `null`. */
    fun themeKey(): String? = try {
        store?.getString(KEY_THEME, null)
    } catch (_: Exception) {
        null
    }

    fun saveThemeKey(key: String) {
        store?.edit()?.putString(KEY_THEME, key)?.apply()
    }

    /** Режим тона (`dark`, `light`, `system`) или `null`. */
    fun themeModeKey(): String? = try {
        store?.getString(KEY_THEME_MODE, null)
    } catch (_: Exception) {
        null
    }

    fun saveThemeModeKey(key: String) {
        store?.edit()?.putString(KEY_THEME_MODE, key)?.apply()
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

    fun changePipAutoEnter(on: Boolean) {
        pipAutoEnter = on
        store?.edit()?.putBoolean(KEY_PIP_AUTO, on)?.apply()
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
     * «Сбросить всё»: звук, оформление (акцент и тему возвращают `AccentState` и `ThemeState`, см. экран настроек), текст
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
        changePipAutoEnter(false)
        changeQuality(AudioQuality.Normal)
        changeNormalize(false)
        changeLyricsLrclib(true)
        changeEq(EqState(custom = eq.custom), save = true)
        if (fckCensor) changeFckCensor(false)
        if (!fckExplicit) changeFckExplicit(true)
        if (slopless) changeSlopless(false)
        if (!slopAutoskip) changeSlopAutoskip(true)
        changeSlopDislike(false)
        changeCopyName(false)
        changeCopyFormat(CopyName.DEFAULT_FORMAT)
        if (syncEnabled) changeSyncEnabled(false)
        changeSyncDelay(0)
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

    /** Действует со следующего трека, которому понадобится поиск; играющий доиграет как есть. */
    fun changeFckExplicit(on: Boolean) {
        fckExplicit = on
        FckCensor.explicitSearch = on
        store?.edit()?.putBoolean(KEY_FCK_EXPLICIT, on)?.apply()
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
