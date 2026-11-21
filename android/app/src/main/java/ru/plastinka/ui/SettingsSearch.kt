package ru.plastinka.ui

/** Разделы настроек Android. Порядок — как в списке на главном экране. */
enum class SettingsPage(val title: String) {
    Main("Настройки"),
    Appearance("Оформление"),
    Sound("Звук"),
    Equalizer("Эквалайзер"),
    Lyrics("Текст песни"),
    Plugins("Плагины"),
    About("О программе"),
}

/**
 * Запись для поиска по настройкам: одна строка настройки и то, по чему её ищут.
 * Список записей живёт здесь, без Compose, чтобы сопоставление проверялось отдельно.
 */
class SettingsEntry(
    val page: SettingsPage,
    val title: String,
    val hint: String,
    /** Дополнительные слова через пробел. */
    val keywords: String = "",
) {
    /** Подходит ли запись под запрос; название раздела тоже участвует в поиске. */
    fun foundBy(query: String): Boolean = matchesQuery(query, title, hint, keywords, page.title)
}

/** Приведение текста к виду для сравнения: без регистра, «ё» = «е». */
fun foldText(text: String): String = text.lowercase().replace('ё', 'е')

/** Ищут ли сейчас: запрос из одних пробелов — пустой запрос. */
fun isSearching(query: String): Boolean = query.isNotBlank()

/**
 * Все слова запроса входят в один из `parts` (без учёта регистра, «ё» = «е»).
 * Пустой запрос подходит всему.
 */
fun matchesQuery(query: String, vararg parts: String): Boolean {
    val haystack = parts.joinToString(" ") { foldText(it) }
    return foldText(query).split(Regex("""\s+""")).filter { it.isNotEmpty() }.all { haystack.contains(it) }
}

/** Найденные записи в порядке разделов. */
fun searchSettings(query: String): List<SettingsEntry> = SETTINGS_INDEX.filter { it.foundBy(query) }

/** Настройки, которые реально есть на Android; тексты те же, что на страницах. */
private val SETTINGS_INDEX: List<SettingsEntry> = listOf(
    SettingsEntry(
        SettingsPage.Appearance, "Акцент", "Кнопки, выбранный трек, ползунки",
        "цвет тема красный свой палитра оформление внешний вид",
    ),
    SettingsEntry(
        SettingsPage.Appearance, "Визуализатор", "Спектр звука в плеере",
        "спектр анимация эффекты плеер",
    ),
    SettingsEntry(
        SettingsPage.Appearance, "Цвет волны под обложку", "Цвет «Моей волны» и спектра под обложку",
        "vibecolorizer vibe colorizer вайб колорайзер моя волна",
    ),
    SettingsEntry(
        SettingsPage.Appearance, "Размытая обложка в плеере", "Фон полноэкранного плеера",
        "блюр размытие фон обложка полноэкранный",
    ),
    SettingsEntry(
        SettingsPage.Appearance, "Мини-плеер при сворачивании", "Во время игры приложение уходит в «картинку в картинке»",
        "мини-плеер pip картинка в картинке сворачивание автовход",
    ),
    SettingsEntry(
        SettingsPage.Sound, "Качество звука", "Превосходное, оптимальное, экономичное",
        "lossless flac mp3 aac битрейт поток трафик интернет",
    ),
    SettingsEntry(
        SettingsPage.Sound, "Нормализация громкости", "Выравнивает громкость треков",
        "громкость уровень replaygain",
    ),
    SettingsEntry(
        SettingsPage.Equalizer, "Эквалайзер", "Меняет звук на лету, на качество потока не влияет",
        "эквалайзер eq полосы бас частоты звук",
    ),
    SettingsEntry(
        SettingsPage.Equalizer, "Пресет", "Готовые и свои пресеты эквалайзера",
        "пресеты рок поп классика плоский сохранить удалить",
    ),
    SettingsEntry(
        SettingsPage.Equalizer, "Полосы", "От −12 до +12 дБ, шаг 0,5 дБ",
        "десять 10 децибел усиление сброс",
    ),
    SettingsEntry(
        SettingsPage.Equalizer, "Предусилитель", "Общий уровень до полос",
        "preamp усиление",
    ),
    SettingsEntry(
        SettingsPage.Equalizer, "Автоматический предусилитель",
        "Снижает общий уровень на величину наибольшего усиления, чтобы звук не искажался",
        "искажения клиппинг preamp",
    ),
    SettingsEntry(
        SettingsPage.Lyrics, "Искать текст в LRCLib",
        "Если у Яндекса текста нет. В lrclib.net уходят название и исполнитель трека",
        "текст песни слова lrclib lrc субтитры караоке запасной источник",
    ),
    SettingsEntry(
        SettingsPage.Plugins, "FckCensor",
        "Названия, исполнители, обложки и звук без цензуры из списка FckCensorData для треков, которые в нём есть",
        "цензура explicit замена трека название обложка плагин свой файл заменить на",
    ),
    SettingsEntry(
        SettingsPage.Plugins, "Искать версию без цензуры",
        "Если на Яндексе есть explicit-версия трека, играет она. В поиск Яндекса уходят исполнитель и название трека",
        "fckcensor explicit цензура мат поиск версия плагин",
    ),
    SettingsEntry(
        SettingsPage.Plugins, "Slopless", "Метки ИИ-исполнителей и ИИ-треков по базе slopless.art",
        "ии ai нейросеть сгенерированные плагин",
    ),
    SettingsEntry(
        SettingsPage.Plugins, "Пропускать ИИ-треки", "В очереди. Трек, включённый вручную, играет",
        "автопропуск slopless ии",
    ),
    SettingsEntry(
        SettingsPage.Plugins, "Ставить «Не нравится»",
        "Пропущенным ИИ-трекам. Это запись в ваш аккаунт: включается после подтверждения",
        "дизлайк slopless согласие ии",
    ),
    SettingsEntry(
        SettingsPage.Plugins, "CopyTrackName",
        "Пункт «Скопировать название» в меню трека (кнопка «…») и в плеере",
        "копировать название формат artist title album буфер обмена плагин",
    ),
    SettingsEntry(
        SettingsPage.About, "О программе", "Версия, лицензии, отчёт о сбое",
        "plastinka пластинка неофициальный клиент яндекс лицензии версия сбой отчёт ошибка краш завис",
    ),
)
