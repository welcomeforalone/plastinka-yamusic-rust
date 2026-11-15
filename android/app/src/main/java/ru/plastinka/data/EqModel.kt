package ru.plastinka.data

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put
import ru.plastinka.playback.EqSettings
import ru.plastinka.playback.EqSpec

// Модель эквалайзера для окна: пресеты, свои пресеты, проверка имени и
// хранение. Порт src/player/eq.rs (EqState) и src/storage/config.rs. Чистый
// Kotlin без Android; на диск уходят только метаданные (имя и усиления).

/** Встроенный пресет: значения полос в десятых долях дБ. */
class EqPreset(val key: String, val title: String, val gains: List<Int>)

/** Встроенные пресеты по порядку показа (те же, что на десктопе). */
val EqPresets: List<EqPreset> = listOf(
    EqPreset("flat", "Плоский", listOf(0, 0, 0, 0, 0, 0, 0, 0, 0, 0)),
    EqPreset("bass", "Бас", listOf(60, 50, 40, 20, 0, 0, 0, 0, 0, 0)),
    EqPreset("treble", "Высокие", listOf(0, 0, 0, 0, 0, 0, 20, 40, 50, 60)),
    EqPreset("v-shape", "Бас и верх", listOf(50, 40, 20, 0, -20, -20, 0, 20, 40, 50)),
    EqPreset("vocal", "Вокал", listOf(-20, -20, -10, 10, 30, 40, 40, 20, 0, -10)),
    EqPreset("rock", "Рок", listOf(40, 30, 20, 0, -10, -10, 10, 30, 40, 40)),
    EqPreset("pop", "Поп", listOf(-10, 0, 20, 30, 40, 30, 10, 0, -10, -10)),
    EqPreset("electronic", "Электроника", listOf(50, 40, 10, 0, -20, 10, 0, 20, 40, 50)),
    EqPreset("hip-hop", "Хип-хоп", listOf(50, 50, 30, 10, -10, -10, 10, 0, 20, 30)),
    EqPreset("classical", "Классика", listOf(30, 20, 10, 0, 0, 0, -10, -10, 20, 30)),
    EqPreset("jazz", "Джаз", listOf(30, 20, 10, 20, -10, -10, 0, 10, 20, 30)),
    EqPreset("acoustic", "Акустика", listOf(30, 30, 20, 10, 20, 10, 20, 30, 20, 10)),
    EqPreset("warm", "Тёплый", listOf(30, 30, 20, 10, 0, 0, -10, -20, -30, -40)),
)

fun eqPresetByKey(key: String): EqPreset? = EqPresets.firstOrNull { it.key == key }

/** Выбранный пресет: ключ встроенного или имя своего. */
sealed interface EqPresetId {
    data class Builtin(val key: String) : EqPresetId
    data class Custom(val name: String) : EqPresetId
}

/** Свой пресет пользователя: имя, предусилитель и усиления. */
data class CustomPreset(val name: String, val preamp: Int, val gains: List<Int>)

/** Почему имя не принято; [text] показывается в диалоге. */
enum class EqNameError(val text: String) {
    Empty("Введите название пресета"),
    TooLong("Название длиннее ${EqSpec.NAME_MAX} символов"),
    Duplicate("Такое название уже есть"),
    Limit("Можно сохранить не больше ${EqSpec.MAX_CUSTOM} пресетов"),
}

/**
 * Состояние эквалайзера в окне: параметры, выбранный пресет (`null` — «Свои
 * значения») и свои пресеты. Неизменяемое: каждая правка даёт новое состояние.
 */
data class EqState(
    val settings: EqSettings = EqSettings.OFF,
    val preset: EqPresetId? = null,
    val custom: List<CustomPreset> = emptyList(),
) {
    /** Ошибка имени для нового пресета или `null`, если имя годится. */
    fun nameError(name: String): EqNameError? {
        val trimmed = name.trim()
        val length = trimmed.codePointCount(0, trimmed.length)
        if (length == 0) return EqNameError.Empty
        if (length > EqSpec.NAME_MAX) return EqNameError.TooLong
        val lower = trimmed.lowercase()
        val taken = EqPresets.any { it.title.lowercase() == lower || it.key == lower } ||
            custom.any { it.name.lowercase() == lower }
        if (taken) return EqNameError.Duplicate
        if (custom.size >= EqSpec.MAX_CUSTOM) return EqNameError.Limit
        return null
    }

    fun withEnabled(on: Boolean) = copy(settings = settings.copy(enabled = on))

    fun withAutoPreamp(on: Boolean) = copy(settings = settings.copy(autoPreamp = on))

    /** Ручная правка полосы: значение в пределах, пресет перевыбирается. */
    fun withBand(index: Int, tenths: Int): EqState {
        if (index !in 0 until EqSpec.BANDS) return this
        val gains = settings.gains.toMutableList()
        gains[index] = EqSpec.clampGain(tenths)
        return copy(settings = settings.copy(gains = gains)).refreshed()
    }

    fun withPreamp(tenths: Int): EqState =
        copy(settings = settings.copy(preamp = EqSpec.clampGain(tenths))).refreshed()

    /**
     * Применяет пресет: полосы (у своего ещё и предусилитель) и выбор.
     * Выбрать пресет — значит хотеть его услышать, поэтому эквалайзер
     * включается. Неизвестный пресет ничего не меняет.
     */
    fun withPreset(id: EqPresetId): EqState = when (id) {
        is EqPresetId.Builtin -> {
            val preset = eqPresetByKey(id.key)
            if (preset == null) this else copy(settings = settings.copy(enabled = true, gains = preset.gains), preset = id)
        }
        is EqPresetId.Custom -> {
            val saved = custom.firstOrNull { it.name == id.name }
            if (saved == null) {
                this
            } else {
                copy(settings = settings.copy(enabled = true, gains = saved.gains, preamp = saved.preamp), preset = id)
            }
        }
    }

    /** «Сбросить»: полосы и предусилитель в ноль, пресет «Плоский»; включённость не трогаем. */
    fun reset(): EqState = copy(
        settings = settings.copy(preamp = 0, gains = EqSettings.OFF.gains),
        preset = EqPresetId.Builtin("flat"),
    )

    /** Полосы и предусилитель уже в нуле, выбран «Плоский». */
    fun isFlat(): Boolean =
        settings.gains.all { it == 0 } && settings.preamp == 0 && preset == EqPresetId.Builtin("flat")

    /**
     * Сохраняет текущие предусилитель и полосы под именем и выбирает его.
     * Имя должно пройти [nameError]; иначе состояние не меняется.
     */
    fun savedAs(name: String): EqState {
        if (nameError(name) != null) return this
        val trimmed = name.trim()
        val saved = CustomPreset(trimmed, settings.preamp, settings.gains)
        return copy(custom = custom + saved, preset = EqPresetId.Custom(trimmed))
    }

    /** Удаляет свой пресет. Если он был выбран — «Свои значения», значения остаются. */
    fun withoutCustom(name: String): EqState = copy(
        custom = custom.filterNot { it.name == name },
        preset = if (preset == EqPresetId.Custom(name)) null else preset,
    )

    /** Название выбранного пресета. */
    fun presetTitle(): String = when (val id = preset) {
        is EqPresetId.Builtin -> eqPresetByKey(id.key)?.title ?: "Свои значения"
        is EqPresetId.Custom -> id.name
        null -> "Свои значения"
    }

    /**
     * После ручной правки выбранный пресет остаётся, пока значения с ним
     * совпадают; иначе берём тот, с которым совпали (или «Свои значения»).
     */
    private fun refreshed(): EqState {
        val current = preset
        val keep = current != null && matches(current)
        return if (keep) this else copy(preset = matching())
    }

    /** Пресет с этим идентификатором существует и совпадает с текущими полосами (у своего ещё и с предусилителем). */
    private fun matches(id: EqPresetId): Boolean = when (id) {
        is EqPresetId.Builtin -> eqPresetByKey(id.key)?.gains == settings.gains
        is EqPresetId.Custom ->
            custom.any { it.name == id.name && it.gains == settings.gains && it.preamp == settings.preamp }
    }

    /** Какому пресету соответствуют значения: сначала свои (полосы и предусилитель), потом встроенные (только полосы). */
    private fun matching(): EqPresetId? {
        val own = custom.firstOrNull { it.gains == settings.gains && it.preamp == settings.preamp }
        if (own != null) return EqPresetId.Custom(own.name)
        return EqPresets.firstOrNull { it.gains == settings.gains }?.let { EqPresetId.Builtin(it.key) }
    }

    // ----------------------------------------------------------- хранение

    /** Компактный JSON для SharedPreferences. */
    fun toJson(): String {
        val s = settings.sanitized()
        val gainsJson = { list: List<Int> -> buildJsonArray { list.forEach { add(JsonPrimitive(it)) } } }
        return buildJsonObject {
            put("enabled", s.enabled)
            put("autoPreamp", s.autoPreamp)
            put("preamp", s.preamp)
            put("gains", gainsJson(s.gains))
            when (val id = preset) {
                is EqPresetId.Builtin -> put("preset", id.key)
                is EqPresetId.Custom -> put("preset", id.name)
                null -> Unit
            }
            put(
                "presets",
                buildJsonArray {
                    for (item in custom) {
                        add(
                            buildJsonObject {
                                put("name", item.name)
                                put("preamp", EqSpec.clampGain(item.preamp))
                                put("gains", gainsJson(item.gains))
                            },
                        )
                    }
                },
            )
        }.toString()
    }

    companion object {
        /**
         * Читает состояние. Битая строка или поле — значение по умолчанию,
         * значения вне пределов зажимаются, плохие свои пресеты отбрасываются,
         * выбранный пресет, не совпадающий с полосами, снимается.
         */
        fun fromJson(text: String?): EqState {
            if (text.isNullOrBlank()) return EqState()
            val root = try {
                Json.parseToJsonElement(text) as? JsonObject
            } catch (_: Exception) {
                null
            } ?: return EqState()
            val defaults = EqSettings()
            val settings = EqSettings(
                enabled = root.bool("enabled") ?: defaults.enabled,
                autoPreamp = root.bool("autoPreamp") ?: defaults.autoPreamp,
                preamp = root.int("preamp")?.let(EqSpec::clampGain) ?: defaults.preamp,
                gains = root["gains"].gains() ?: defaults.gains,
            )
            var state = EqState(settings = settings)
            val presets = root["presets"] as? JsonArray
            if (presets != null) {
                for (item in presets) {
                    val obj = item as? JsonObject ?: continue
                    val name = obj.str("name") ?: continue
                    val gains = obj["gains"].gains() ?: continue
                    val preamp = obj.int("preamp")?.let(EqSpec::clampGain) ?: 0
                    // Свой пресет добавляется как обычный: имя проверяется, повторы и лишнее отбрасываются.
                    if (state.nameError(name) == null) {
                        state = state.copy(custom = state.custom + CustomPreset(name.trim(), preamp, gains))
                    }
                }
            }
            val chosen = root.str("preset")?.let { key ->
                eqPresetByKey(key)?.let { EqPresetId.Builtin(it.key) }
                    ?: state.custom.firstOrNull { it.name == key }?.let { EqPresetId.Custom(it.name) }
            }
            return state.copy(preset = chosen).let { if (chosen != null && !it.matches(chosen)) it.copy(preset = null) else it }
        }

        private fun JsonObject.bool(key: String): Boolean? = (this[key] as? JsonPrimitive)?.booleanOrNull

        private fun JsonObject.int(key: String): Int? = (this[key] as? JsonPrimitive)?.intOrNull

        private fun JsonObject.str(key: String): String? =
            (this[key] as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull

        /** Десять целых в пределах; другой размер или нечисло — `null`. */
        private fun JsonElement?.gains(): List<Int>? {
            val array = this as? JsonArray ?: return null
            if (array.size != EqSpec.BANDS) return null
            val values = array.map { ((it as? JsonPrimitive)?.intOrNull ?: return null) }
            return values.map(EqSpec::clampGain)
        }
    }
}
