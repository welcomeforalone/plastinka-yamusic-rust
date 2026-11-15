package ru.plastinka.ui

import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import ru.plastinka.data.AudioQuality
import ru.plastinka.data.Prefs

/** «Звук»: качество потока и нормализация громкости. */
@Composable
fun SoundScreen(onBack: () -> Unit) {
    SettingsPageScaffold("Звук", onBack) {
        SoundSettingsContent()
    }
}

/**
 * Содержимое страницы «Звук»: его же показывает нижний лист «Настройки звука»
 * в плеере, поэтому в одном месте.
 */
@Composable
fun SoundSettingsContent() {
    SectionTitle("Качество звука")
    Spacer(Modifier.height(10.dp))
    GroupCard {
        for (quality in AudioQuality.entries) {
            ChoiceRow(
                title = quality.title,
                hint = quality.hint,
                selected = Prefs.quality == quality,
                onSelect = { Prefs.changeQuality(quality) },
            )
        }
    }
    SettingsNote("Смена качества перезапускает играющий трек с той же позиции; пауза сохраняется.")
    Spacer(Modifier.height(22.dp))
    SectionTitle("Громкость")
    Spacer(Modifier.height(10.dp))
    GroupCard {
        ToggleRow(
            title = "Нормализация громкости",
            subtitle = "Выравнивает громкость треков",
            checked = Prefs.normalize,
            onChange = Prefs::changeNormalize,
        )
    }
    SettingsNote("Действует, когда сервис сообщил громкость трека; для остальных звук не меняется.")
}

/** «Текст песни»: запасной источник LRCLib. */
@Composable
fun LyricsSettingsScreen(onBack: () -> Unit) {
    SettingsPageScaffold("Текст песни", onBack) {
        GroupCard {
            ToggleRow(
                title = "Искать текст в LRCLib",
                subtitle = "Если у Яндекса текста нет. В lrclib.net уходят название и исполнитель трека",
                checked = Prefs.lyricsLrclib,
                onChange = Prefs::changeLyricsLrclib,
            )
        }
    }
}
