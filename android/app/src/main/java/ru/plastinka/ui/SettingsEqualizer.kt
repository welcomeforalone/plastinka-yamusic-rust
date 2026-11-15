package ru.plastinka.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
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
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import ru.plastinka.R
import ru.plastinka.data.EqNameError
import ru.plastinka.data.EqPresetId
import ru.plastinka.data.EqPresets
import ru.plastinka.data.Prefs
import ru.plastinka.playback.EqSpec

/**
 * «Эквалайзер»: переключатель, пресеты, десять полос, предусилитель. Каждое
 * изменение сразу уходит в звуковой тракт; на диск — по окончании жеста и при
 * выборе пресета. Состояние всегда берём из [Prefs.eq] в момент действия, а не из
 * снимка композиции: два быстрых касания не затрут друг друга.
 */
@Composable
fun EqualizerScreen(onBack: () -> Unit) {
    val state = Prefs.eq
    val dimmed = !state.settings.enabled
    var saving by remember { mutableStateOf(false) }
    var removing by remember { mutableStateOf<String?>(null) }

    SettingsPageScaffold("Эквалайзер", onBack) {
        GroupCard {
            ToggleRow(
                title = "Эквалайзер",
                subtitle = "Меняет звук на лету, на качество потока не влияет",
                checked = state.settings.enabled,
                onChange = { Prefs.changeEq(Prefs.eq.withEnabled(it), save = true) },
            )
        }
        Spacer(Modifier.height(22.dp))
        SectionTitle("Пресет")
        Spacer(Modifier.height(10.dp))
        GroupCard {
            Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                PlText(state.presetTitle(), PlType.secondary, Pl.text3, maxLines = 1)
                PresetChips(
                    selected = state.preset,
                    custom = state.custom.map { it.name },
                    onPick = { Prefs.changeEq(Prefs.eq.withPreset(it), save = true) },
                    onRemove = { removing = it },
                    onSave = { saving = true },
                )
            }
        }
        Spacer(Modifier.height(22.dp))
        SectionTitle("Полосы")
        Spacer(Modifier.height(10.dp))
        GroupCard {
            Column(Modifier.padding(vertical = 12.dp)) {
                EqBands(
                    gains = state.settings.gains,
                    dimmed = dimmed,
                    onBand = { index, tenths -> Prefs.changeEq(Prefs.eq.withBand(index, tenths), save = false) },
                    onCommit = Prefs::saveEq,
                )
            }
        }
        SettingsNote("От −12 до +12 дБ, шаг 0,5 дБ. Двигайте полосы пальцем: звук меняется сразу.")
        Spacer(Modifier.height(6.dp))
        GroupCard {
            Column(Modifier.padding(horizontal = 14.dp, vertical = 12.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    PlText("Предусилитель", PlType.bodyStrong, modifier = Modifier.weight(1f))
                    PlText(formatEqDb(state.settings.preamp), PlType.secondaryStrong, if (dimmed) Pl.text3 else Pl.text)
                }
                PlText(
                    if (state.settings.autoPreamp) {
                        "Общий уровень до полос. С учётом автоматического: ${formatEqDb(state.settings.effectivePreamp())}"
                    } else {
                        "Общий уровень до полос"
                    },
                    PlType.caption,
                    Pl.text3,
                )
                EqPreampSlider(
                    tenths = state.settings.preamp,
                    dimmed = dimmed,
                    onChange = { Prefs.changeEq(Prefs.eq.withPreamp(it), save = false) },
                    onCommit = Prefs::saveEq,
                )
            }
        }
        Spacer(Modifier.height(12.dp))
        GroupCard {
            ToggleRow(
                title = "Автоматический предусилитель",
                subtitle = "Снижает общий уровень на величину наибольшего усиления, чтобы звук не искажался",
                checked = state.settings.autoPreamp,
                onChange = { Prefs.changeEq(Prefs.eq.withAutoPreamp(it), save = true) },
            )
        }
        Spacer(Modifier.height(16.dp))
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), horizontalArrangement = Arrangement.End) {
            Pill(
                "Сбросить",
                kind = PillKind.Tonal,
                height = 44.dp,
                enabled = !state.isFlat(),
                onClick = { Prefs.changeEq(Prefs.eq.reset(), save = true) },
            )
        }
    }

    if (saving) SavePresetDialog(onDismiss = { saving = false })
    removing?.let { name ->
        AlertDialog(
            onDismissRequest = { removing = null },
            containerColor = Pl.surface2,
            title = { PlText("Удалить пресет «$name»?", PlType.headline) },
            confirmButton = {
                TextButton(onClick = {
                    removing = null
                    Prefs.changeEq(Prefs.eq.withoutCustom(name), save = true)
                }) { PlText("Удалить", PlType.secondaryStrong, Pl.error) }
            },
            dismissButton = {
                TextButton(onClick = { removing = null }) { PlText("Отмена", PlType.secondaryStrong, Pl.text2) }
            },
        )
    }
}

/** «Таблетки» пресетов с переносом строк: встроенные, свои (с крестиком) и «Сохранить как…». */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun PresetChips(
    selected: EqPresetId?,
    custom: List<String>,
    onPick: (EqPresetId) -> Unit,
    onRemove: (String) -> Unit,
    onSave: () -> Unit,
) {
    FlowRow(
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        for (preset in EqPresets) {
            val id = EqPresetId.Builtin(preset.key)
            PresetChip(preset.title, selected == id, onClick = { onPick(id) })
        }
        for (name in custom) {
            val id = EqPresetId.Custom(name)
            PresetChip(name, selected == id, onClick = { onPick(id) }, onRemove = { onRemove(name) })
        }
        Row(
            Modifier
                .height(36.dp)
                .clip(CircleShape)
                .border(1.dp, Pl.outline, CircleShape)
                .clickable(onClick = onSave)
                .padding(horizontal = 14.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            PlIcon(R.drawable.ic_plus, 14.dp, Pl.text2)
            PlText("Сохранить как…", PlType.secondaryStrong, Pl.text2, maxLines = 1)
        }
    }
}

/** Одна «таблетка»: выбранная — акцентом; у своего пресета справа крестик «удалить». */
@Composable
private fun PresetChip(title: String, selected: Boolean, onClick: () -> Unit, onRemove: (() -> Unit)? = null) {
    val textColor = if (selected) Pl.onAccent else Pl.text2
    Row(
        Modifier
            .height(36.dp)
            .clip(CircleShape)
            .background(if (selected) Pl.accent else Pl.surface3, CircleShape),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier
                .height(36.dp)
                .clickable(onClick = onClick)
                .padding(start = 14.dp, end = if (onRemove == null) 14.dp else 6.dp),
            contentAlignment = Alignment.Center,
        ) {
            PlText(title, PlType.secondaryStrong, textColor, maxLines = 1)
        }
        if (onRemove != null) {
            Box(
                Modifier.size(36.dp).clip(CircleShape).clickable(onClick = onRemove),
                contentAlignment = Alignment.Center,
            ) {
                PlIcon(R.drawable.ic_close, 14.dp, textColor)
            }
        }
    }
}

/** «Сохранить как…»: имя своего пресета. Ошибка имени показывается в диалоге, поле остаётся открытым. */
@Composable
private fun SavePresetDialog(onDismiss: () -> Unit) {
    var name by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<EqNameError?>(null) }
    val save = {
        val problem = Prefs.eq.nameError(name)
        if (problem == null) {
            Prefs.changeEq(Prefs.eq.savedAs(name), save = true)
            onDismiss()
        } else {
            error = problem
        }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = Pl.surface2,
        title = { PlText("Новый пресет", PlType.headline) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                PlText("Сохранятся текущие полосы и предусилитель", PlType.secondary, Pl.text2)
                val shape = RoundedCornerShape(50)
                BasicTextField(
                    value = name,
                    onValueChange = {
                        name = it
                        error = null
                    },
                    singleLine = true,
                    textStyle = PlType.secondaryStrong.copy(color = Pl.text),
                    cursorBrush = SolidColor(Pl.accent),
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { save() }),
                    decorationBox = { field ->
                        Box {
                            if (name.isEmpty()) PlText("Название пресета", PlType.secondaryStrong, Pl.text3)
                            field()
                        }
                    },
                    modifier = Modifier
                        .fillMaxWidth()
                        .border(1.dp, if (error != null) Pl.error else Pl.outline, shape)
                        .padding(horizontal = 16.dp, vertical = 12.dp),
                )
                error?.let { PlText(it.text, PlType.caption, Pl.error) }
            }
        },
        confirmButton = {
            TextButton(onClick = { save() }) { PlText("Сохранить", PlType.secondaryStrong, Pl.accent) }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { PlText("Отмена", PlType.secondaryStrong, Pl.text2) }
        },
    )
}
