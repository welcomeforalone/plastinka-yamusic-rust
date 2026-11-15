package ru.plastinka.data

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/** Занятие: станция `activity:{tag}`. Подпись приходит от сервиса. */
data class WaveActivity(val tag: String, val name: String)

/** «Характер» музыки. Ключ — значение в seed и в настройках (как `Diversity` на десктопе). */
enum class WaveDiversity(val key: String, val title: String) {
    Favorite("favorite", "Любимое"),
    Discover("discover", "Незнакомое"),
    Popular("popular", "Популярное"),
    ;

    companion object {
        fun fromKey(key: String?): WaveDiversity? = entries.firstOrNull { it.key == key }
    }
}

/** «Настроение». */
enum class WaveMood(val key: String, val title: String) {
    Active("active", "Бодрое"),
    Fun("fun", "Весёлое"),
    Calm("calm", "Спокойное"),
    Sad("sad", "Грустное"),
    ;

    companion object {
        fun fromKey(key: String?): WaveMood? = entries.firstOrNull { it.key == key }
    }
}

/** «Язык» треков. */
enum class WaveLanguage(val key: String, val title: String) {
    Russian("russian", "Русский"),
    NotRussian("not-russian", "Иностранный"),
    WithoutWords("without-words", "Без слов"),
    ;

    companion object {
        fun fromKey(key: String?): WaveLanguage? = entries.firstOrNull { it.key == key }
    }
}

/**
 * Выбор настроек «Моей волны». Пусто — «по умолчанию»: seed не передаётся.
 * Это не запись в аккаунт: выбор лежит только на устройстве, а на сервер уходит
 * seeds нового сеанса (`POST /rotor/session/new`) — как на десктопе
 * (`src/api/wave_settings.rs`).
 */
data class WaveSettings(
    val activity: WaveActivity? = null,
    val diversity: WaveDiversity? = null,
    val mood: WaveMood? = null,
    val language: WaveLanguage? = null,
) {
    /** Ничего не выбрано: обычная «Моя волна». */
    val isDefault: Boolean get() = this == WaveSettings()

    /**
     * Подпись кнопки «Моя волна»: выбранное кратко через « · » в порядке занятие,
     * настроение, характер, язык (как `WaveSettings::label` на десктопе). Ничего не выбрано — «Моя волна».
     */
    fun label(): String =
        listOfNotNull(activity?.name, mood?.title, diversity?.title, language?.title)
            .joinToString(" · ")
            .ifEmpty { "Моя волна" }

    /**
     * Seeds для `session/new`. Без занятия — `user:onyourwave`, с занятием —
     * `activity:{tag}` вместо него; настройки добавляются в обоих случаях.
     */
    fun seeds(): List<String> = buildList {
        add(if (activity != null) "activity:${activity.tag}" else YandexApi.WAVE_SEED)
        diversity?.let { add("settingDiversity:${it.key}") }
        mood?.let { add("settingMoodEnergy:${it.key}") }
        language?.let { add("settingLanguage:${it.key}") }
    }

    companion object {
        /** Сколько занятий берём из списка станций. */
        private const val MAX_ACTIVITIES = 60

        /** Длина названия занятия: длиннее на чипе не нужно. */
        private const val MAX_NAME = 60

        /** Тег идёт в тело запроса: буквы, цифры, `-`, `_`, `.`, до 64 знаков. */
        fun safeTag(tag: String): Boolean =
            tag.length in 1..64 && tag.all { it.isLetterOrDigit() && it.code < 128 || it == '-' || it == '_' || it == '.' }

        /** Название без управляющих знаков и лишних пробелов; пустое — `null`. */
        fun cleanName(name: String?): String? =
            name?.filter { !it.isISOControl() }?.trim()?.take(MAX_NAME)?.takeIf { it.isNotEmpty() }

        /**
         * Из сохранённых строк. Неизвестное или небезопасное значение — как будто
         * его нет (остальные настройки сохраняются).
         */
        fun fromStored(
            activityTag: String?,
            activityName: String?,
            diversity: String?,
            mood: String?,
            language: String?,
        ): WaveSettings {
            val activity = activityTag?.takeIf { safeTag(it) }?.let { tag ->
                WaveActivity(tag, cleanName(activityName) ?: tag)
            }
            return WaveSettings(
                activity = activity,
                diversity = WaveDiversity.fromKey(diversity),
                mood = WaveMood.fromKey(mood),
                language = WaveLanguage.fromKey(language),
            )
        }

        /**
         * Занятия из ответа `GET /rotor/stations/list`: станции типа `activity`.
         * Непонятный элемент пропускается, повторы отбрасываются. Ответ не того вида — `null`.
         */
        fun parseActivities(envelope: JsonObject): List<WaveActivity>? {
            val items = envelope["result"] as? JsonArray ?: return null
            val out = ArrayList<WaveActivity>()
            for (item in items) {
                val station = (item as? JsonObject)?.get("station") as? JsonObject ?: continue
                val id = station["id"] as? JsonObject
                val kind = text(id?.get("type"))
                val tag = text(id?.get("tag"))
                val name = cleanName(text(station["name"]))
                if (kind != "activity" || tag == null || name == null) continue
                if (safeTag(tag) && out.none { it.tag == tag }) out += WaveActivity(tag, name)
                if (out.size >= MAX_ACTIVITIES) break
            }
            return out
        }

        private fun text(element: JsonElement?): String? =
            (element as? JsonPrimitive)?.takeIf { it.isString }?.content
    }
}
