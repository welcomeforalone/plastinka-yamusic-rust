package ru.plastinka.ui

import androidx.annotation.DrawableRes
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import ru.plastinka.R

// Общие элементы экранов настроек: на них опираются все подэкраны.

/** Карточка-группа: surface1, радиус 20, рамка border, поля 16 по бокам. */
@Composable
fun GroupCard(content: @Composable ColumnScope.() -> Unit) {
    val shape = RoundedCornerShape(20.dp)
    Column(
        Modifier
            .padding(horizontal = 16.dp)
            .fillMaxWidth()
            .clip(shape)
            .background(Pl.surface1, shape)
            .border(1.dp, Pl.border, shape)
            .padding(vertical = 4.dp),
        content = content,
    )
}

/** Заголовок раздела внутри подэкрана («Акцент», «Плеер»). */
@Composable
fun SectionTitle(text: String) {
    PlText(text, PlType.title, modifier = Modifier.padding(horizontal = 20.dp))
}

/** Подэкран настроек: стрелка назад, крупный заголовок и прокручиваемое содержимое. */
@Composable
fun SettingsPageScaffold(title: String, onBack: () -> Unit, content: @Composable ColumnScope.() -> Unit) {
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
        BackBar(onBack)
        PlText(title, PlType.display, modifier = Modifier.padding(horizontal = 20.dp))
        Spacer(Modifier.height(18.dp))
        content()
        Spacer(Modifier.height(16.dp))
    }
}

/** Значок 40×40 на плашке surface3. */
@Composable
private fun RowIcon(@DrawableRes icon: Int) {
    Box(
        Modifier.size(40.dp).clip(RoundedCornerShape(12.dp)).background(Pl.surface3),
        contentAlignment = Alignment.Center,
    ) {
        PlIcon(icon, 20.dp, Pl.text)
    }
}

/** Строка-переход: значок, заголовок, подпись и шеврон «вправо». */
@Composable
fun SettingsRow(
    @DrawableRes icon: Int,
    title: String,
    subtitle: String,
    onClick: () -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(start = 14.dp, end = 16.dp, top = 10.dp, bottom = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RowIcon(icon)
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            PlText(title, PlType.bodyStrong, maxLines = 1)
            PlText(subtitle, PlType.caption, Pl.text3, maxLines = 1)
        }
        Chevron()
    }
}

/** Переключатель макета: 52×32, ручка 24. Включён — акцент и белая ручка, выключен — surface4. */
@Composable
fun PlSwitch(checked: Boolean, modifier: Modifier = Modifier) {
    val track by animateColorAsState(if (checked) Pl.accent else Pl.surface4, tween(160), label = "switch-track")
    val thumb by animateColorAsState(if (checked) Color.White else Pl.text2, tween(160), label = "switch-thumb")
    val offset by animateDpAsState(if (checked) 24.dp else 4.dp, tween(160), label = "switch-offset")
    Box(modifier.size(width = 52.dp, height = 32.dp).clip(CircleShape).background(track, CircleShape)) {
        Box(
            Modifier
                .offset(x = offset, y = 4.dp)
                .size(24.dp)
                .background(thumb, CircleShape),
        )
    }
}

/** Строка с переключателем: заголовок, подпись и [PlSwitch]; нажимается вся строка. */
@Composable
fun ToggleRow(
    title: String,
    subtitle: String,
    checked: Boolean,
    onChange: (Boolean) -> Unit,
    @DrawableRes icon: Int? = null,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .toggleable(value = checked, role = Role.Switch, onValueChange = onChange)
            .padding(start = 14.dp, end = 16.dp, top = 10.dp, bottom = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (icon != null) {
            RowIcon(icon)
            Spacer(Modifier.width(14.dp))
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            PlText(title, PlType.bodyStrong)
            PlText(subtitle, PlType.caption, Pl.text3)
        }
        Spacer(Modifier.width(14.dp))
        PlSwitch(checked)
    }
}

/** Строка выбора одного из вариантов: заголовок, пояснение и кольцо-«радио» справа. */
@Composable
fun ChoiceRow(
    title: String,
    hint: String,
    selected: Boolean,
    onSelect: () -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .selectable(selected = selected, role = Role.RadioButton, onClick = onSelect)
            .padding(start = 14.dp, end = 16.dp, top = 10.dp, bottom = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            PlText(title, PlType.bodyStrong)
            PlText(hint, PlType.caption, Pl.text3)
        }
        Spacer(Modifier.width(14.dp))
        Box(
            Modifier
                .size(22.dp)
                .border(2.dp, if (selected) Pl.accent else Pl.textDisabled, CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            if (selected) Box(Modifier.size(10.dp).background(Pl.accent, CircleShape))
        }
    }
}

/** Пояснение под группой строк: мелкий серый текст с полями как у заголовков. */
@Composable
fun SettingsNote(text: String) {
    PlText(text, PlType.caption, Pl.text3, Modifier.padding(horizontal = 20.dp, vertical = 8.dp))
}
