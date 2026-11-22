package ru.plastinka.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import ru.plastinka.R
import ru.plastinka.data.Prefs
import ru.plastinka.data.WaveActivity
import ru.plastinka.data.WaveDiversity
import ru.plastinka.data.WaveLanguage
import ru.plastinka.data.WaveMood
import ru.plastinka.data.WaveSettings

/** Четыре раздела меню «Моя волна» в порядке макета; подписи — как на десктопе. */
private enum class WaveSetting(val title: String) {
    Activity("Занятие"),
    Diversity("Характер"),
    Mood("Настроение"),
    Language("Язык"),
}

/** Вариант в листе выбора: подпись, выбран ли и какими станут настройки, если его нажать. */
private class WaveOption(val title: String, val selected: Boolean, val result: WaveSettings)

private fun WaveSetting.options(settings: WaveSettings, activities: List<WaveActivity>): List<WaveOption> =
    when (this) {
        WaveSetting.Activity -> listOf(WaveOption("Любое", settings.activity == null, settings.copy(activity = null))) +
            activities.map { WaveOption(it.name, settings.activity?.tag == it.tag, settings.copy(activity = it)) }
        WaveSetting.Diversity -> listOf(
            WaveOption("По умолчанию", settings.diversity == null, settings.copy(diversity = null)),
        ) + WaveDiversity.entries.map { WaveOption(it.title, settings.diversity == it, settings.copy(diversity = it)) }
        WaveSetting.Mood -> listOf(WaveOption("Любое", settings.mood == null, settings.copy(mood = null))) +
            WaveMood.entries.map { WaveOption(it.title, settings.mood == it, settings.copy(mood = it)) }
        WaveSetting.Language -> listOf(WaveOption("Любой", settings.language == null, settings.copy(language = null))) +
            WaveLanguage.entries.map { WaveOption(it.title, settings.language == it, settings.copy(language = it)) }
    }

/**
 * Кнопка-«таблетка» «Моя волна» (макеты 03 и 04): подпись — выбранное кратко, справа
 * стрелка вниз. Нажатие открывает одно меню со всеми настройками. `filled` — кнопка
 * на цветном фоне играющей волны (заливка вместо рамки, макет 04).
 */
@Composable
internal fun WaveSettingsButton(
    model: AppViewModel,
    player: PlayerController?,
    filled: Boolean,
    modifier: Modifier = Modifier,
    // Чужая очередь: вместо подписи настроек — источник, рядом круглая кнопка «Запустить волну».
    label: String? = null,
    onClose: (() -> Unit)? = null,
) {
    val settings = Prefs.waveSettings
    // Список занятий грузится при показе кнопки, чтобы меню открывалось сразу полным;
    // не загрузился — повторяется при следующем показе.
    LaunchedEffect(Unit) { model.loadWaveStations() }
    var open by remember { mutableStateOf(false) }

    Row(
        modifier.fillMaxWidth().padding(horizontal = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        // Длинная подпись обрезается в одну строку и не выдавливает крестик.
        Chip(
            label = label ?: settings.label(),
            selected = false,
            onClick = { open = true },
            modifier = if (onClose != null) Modifier.weight(1f, fill = false) else Modifier,
            trailingIcon = R.drawable.ic_chevron_down,
            fill = if (filled) Pl.text.copy(alpha = 0.10f) else null,
        )
        if (onClose != null) {
            Box(
                Modifier
                    .size(36.dp)
                    .clip(CircleShape)
                    .background(Pl.text.copy(alpha = 0.10f), CircleShape)
                    .clickable(onClick = onClose),
                contentAlignment = Alignment.Center,
            ) {
                PlIcon(R.drawable.ic_close, 16.dp, Pl.text)
            }
        }
    }
    if (open) WaveSettingsSheet(settings, model, player, onDismiss = { open = false })
}

/**
 * Меню «Моя волна»: нижний лист со всеми разделами подряд. Выбор сразу запускает новую
 * волну, а лист остаётся открытым (как меню на десктопе): можно сменить несколько настроек.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
private fun WaveSettingsSheet(
    settings: WaveSettings,
    model: AppViewModel,
    player: PlayerController?,
    onDismiss: () -> Unit,
) {
    val stations by model.waveStations.collectAsStateWithLifecycle()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val choose = { next: WaveSettings -> model.changeWaveSettings(player, next) }
    val loading = stations is AppViewModel.WaveStations.Loading
    val activities = when (val s = stations) {
        is AppViewModel.WaveStations.Ready -> s.list
        // Занятия не загрузились, но одно было выбрано раньше: его можно хотя бы увидеть.
        else -> listOfNotNull(settings.activity)
    }
    // Раздел «Занятие» скрыт, если список не загрузился, а выбора в нём нет.
    val activityShown = loading || activities.isNotEmpty()
    PlBottomSheet(onDismiss, sheetState) {
        Column(
            Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .navigationBarsPadding()
                .padding(start = 20.dp, end = 20.dp, bottom = 16.dp),
        ) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                PlText("Моя волна", PlType.title, modifier = Modifier.weight(1f))
                // Один сброс на все настройки: обычная «Моя волна».
                Pill(
                    "Сбросить",
                    kind = PillKind.Tonal,
                    height = 36.dp,
                    icon = R.drawable.ic_retry,
                    enabled = !settings.isDefault,
                    onClick = { choose(WaveSettings()) },
                )
            }
            for (setting in WaveSetting.entries) {
                if (setting == WaveSetting.Activity && !activityShown) continue
                Spacer(Modifier.height(18.dp))
                PlText(setting.title, PlType.secondaryStrong, Pl.text2)
                Spacer(Modifier.height(10.dp))
                if (setting == WaveSetting.Activity && loading) {
                    PlText("Загружаем занятия…", PlType.secondary, Pl.text2)
                } else {
                    FlowRow(
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        for (option in setting.options(settings, activities)) {
                            WaveOptionChip(option.title, option.selected) { choose(option.result) }
                        }
                    }
                }
            }
            Spacer(Modifier.height(18.dp))
            PlText("Смена настроек запускает новую волну", PlType.caption, Pl.text3)
        }
    }
}

/** Вариант выбора: выбранный — акцентом, как «таблетки» в настройках и на десктопе. */
@Composable
private fun WaveOptionChip(title: String, selected: Boolean, onClick: () -> Unit) {
    Box(
        Modifier
            .height(36.dp)
            .clip(CircleShape)
            .background(if (selected) Pl.accent else Pl.surface3, CircleShape)
            .clickable(onClick = onClick)
            .padding(horizontal = 14.dp),
        contentAlignment = Alignment.Center,
    ) {
        PlText(title, PlType.secondaryStrong, if (selected) Pl.onAccent else Pl.text2, maxLines = 1)
    }
}
