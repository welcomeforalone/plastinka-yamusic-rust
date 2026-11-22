package ru.plastinka.ui

import androidx.compose.foundation.Canvas
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
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.selection.selectable
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.dp
import ru.plastinka.R
import ru.plastinka.data.Prefs

/** «Оформление»: тема (режим и 12 семейств), акцент (готовый или свой), плеер, предпросмотр. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun AppearanceScreen(onBack: () -> Unit) {
    var picking by remember { mutableStateOf(false) }
    val tone = rememberWindowTone()
    SettingsPageScaffold("Оформление", onBack) {
        SectionTitle("Тема")
        Spacer(Modifier.height(10.dp))
        GroupCard {
            // Режим тона: три равные «таблетки»; выбранная залита цветом текста (как чипы).
            Row(
                Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, top = 12.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                for (mode in ThemeMode.entries) {
                    Chip(
                        label = mode.title,
                        selected = ThemeState.mode == mode,
                        onClick = { ThemeState.selectMode(mode) },
                        modifier = Modifier.weight(1f),
                        compact = true,
                    )
                }
            }
            PlText(
                themeSummary(ThemeState.family, ThemeState.mode, tone),
                PlType.caption,
                Pl.text3,
                Modifier.padding(start = 16.dp, end = 16.dp, top = 8.dp, bottom = 12.dp),
            )
            // Сетка 12 семейств по три в ряд; миниатюры в текущем тоне.
            Column(
                Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, bottom = 14.dp),
                verticalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                for (line in Family.entries.chunked(THEME_COLUMNS)) {
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        for (family in line) {
                            ThemeCard(family, tone, Modifier.weight(1f))
                        }
                        // Неполный последний ряд не растягивает карточки.
                        repeat(THEME_COLUMNS - line.size) { Spacer(Modifier.weight(1f)) }
                    }
                }
            }
        }
        Spacer(Modifier.height(22.dp))
        SectionTitle("Акцент")
        Spacer(Modifier.height(10.dp))
        GroupCard {
            // 8 готовых и «свой»: девять кружков по 48 dp переходят на вторую строку.
            FlowRow(
                Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 14.dp, bottom = 6.dp),
                horizontalArrangement = Arrangement.spacedBy(10.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                for (accent in Accent.entries) {
                    Swatch(
                        fill = accent.color,
                        selected = AccentState.custom == null && AccentState.preset == accent,
                        onClick = { AccentState.selectPreset(accent) },
                    )
                }
                CustomSwatch(onClick = { picking = true })
            }
            PlText(
                "${AccentState.title} — кнопки, выбранный трек, ползунки",
                PlType.caption,
                Pl.text3,
                Modifier.padding(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 12.dp),
            )
        }
        Spacer(Modifier.height(22.dp))
        SectionTitle("Плеер")
        Spacer(Modifier.height(10.dp))
        GroupCard {
            ToggleRow(
                title = "Визуализатор",
                subtitle = "Спектр звука в плеере",
                checked = Prefs.visualizer,
                onChange = Prefs::changeVisualizer,
            )
            if (Prefs.visualizer) {
                Box(
                    Modifier
                        .padding(start = 16.dp, end = 16.dp, top = 2.dp, bottom = 12.dp)
                        .fillMaxWidth()
                        .height(56.dp)
                        .clip(RoundedCornerShape(14.dp))
                        .background(Pl.bg),
                ) {
                    SpectrumBars(
                        color = Pl.accent.copy(alpha = 0.7f),
                        modifier = Modifier.fillMaxWidth().height(56.dp).padding(start = 12.dp, end = 12.dp, top = 10.dp),
                        barWidth = 4.dp,
                        gap = 3.dp,
                        idleDemo = true,
                    )
                }
            }
            ToggleRow(
                title = "Цвет волны под обложку",
                subtitle = "Цвет «Моей волны» и спектра под обложку",
                checked = Prefs.vibeColorizer,
                onChange = Prefs::changeVibeColorizer,
            )
            ToggleRow(
                title = "Размытая обложка в плеере",
                subtitle = "Фон полноэкранного плеера",
                checked = Prefs.blurCover,
                onChange = Prefs::changeBlurCover,
            )
            if (Pip.isSupported(LocalContext.current)) {
                ToggleRow(
                    title = "Мини-плеер при сворачивании",
                    subtitle = "Во время игры приложение уходит в «картинку в картинке»",
                    checked = Prefs.pipAutoEnter,
                    onChange = Prefs::changePipAutoEnter,
                )
            }
        }
        Spacer(Modifier.height(22.dp))
        SectionTitle("Предпросмотр")
        Spacer(Modifier.height(10.dp))
        GroupCard { PreviewCard() }
    }

    if (picking) {
        AccentPickerDialog(
            initial = AccentState.rgb,
            onDismiss = { picking = false },
            onDone = {
                AccentState.selectCustom(it)
                picking = false
            },
        )
    }
}

/** Карточек темы в ряду. */
private const val THEME_COLUMNS = 3

/** Подпись выбранной темы: «Полночь, светлая»; в режиме системы с пометкой «(по системе)». */
private fun themeSummary(family: Family, mode: ThemeMode, tone: Tone): String {
    val toneTitle = if (tone == Tone.Dark) "тёмная" else "светлая"
    val suffix = if (mode == ThemeMode.System) " (по системе)" else ""
    return "${family.title}, $toneTitle$suffix"
}

/**
 * Карточка темы: миниатюра в текущем тоне и название под ней. Выбранная — рамка
 * акцентом 2 dp, остальные — контур 1 dp.
 */
@Composable
private fun ThemeCard(family: Family, tone: Tone, modifier: Modifier = Modifier) {
    val selected = ThemeState.family == family
    Column(
        modifier
            .clip(RoundedCornerShape(12.dp))
            .selectable(selected = selected, role = Role.RadioButton, onClick = { ThemeState.selectFamily(family) }),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        ThemePreview(family.colors(tone), tone, selected)
        Spacer(Modifier.height(6.dp))
        PlText(
            family.title,
            if (selected) PlType.captionStrong else PlType.caption,
            if (selected) Pl.text else Pl.text2,
            textAlign = TextAlign.Center,
            maxLines = 1,
        )
    }
}

private fun paletteColor(rgb: Int) = Color(0xFF000000.toInt() or (rgb and 0xFFFFFF))

/**
 * Миниатюра окна в палитре [colors]: фон, полоски текста, мини-плеер на surface2 с акцентным
 * прогрессом, нижнее меню на surface1 и рамка контуром (выбранная — акцентом 2 dp).
 * Цвета берутся из палитры, а не из текущей темы окна.
 */
@Composable
private fun ThemePreview(colors: PlColors, tone: Tone, selected: Boolean) {
    val shape = RoundedCornerShape(12.dp)
    val accent = paletteColor(resolveFor(AccentState.rgb, colors.bg, tone))
    val strong = paletteColor(colors.textPrimary)
    val soft = paletteColor(colors.textTertiary)
    Box(
        Modifier
            .fillMaxWidth()
            .aspectRatio(0.72f)
            .clip(shape)
            .background(paletteColor(colors.bg)),
    ) {
        Column(Modifier.fillMaxSize()) {
            Column(
                Modifier.weight(1f).fillMaxWidth().padding(start = 8.dp, end = 8.dp, top = 10.dp),
                verticalArrangement = Arrangement.spacedBy(5.dp),
            ) {
                Box(Modifier.fillMaxWidth(0.55f).height(6.dp).background(strong, CircleShape))
                for (width in listOf(0.9f, 0.7f, 0.8f)) {
                    Box(Modifier.fillMaxWidth(width).height(4.dp).background(soft, CircleShape))
                }
            }
            // Мини-плеер: карточка surface2 с акцентным прогрессом снизу.
            Box(
                Modifier
                    .padding(horizontal = 5.dp)
                    .fillMaxWidth()
                    .height(14.dp)
                    .clip(RoundedCornerShape(5.dp))
                    .background(paletteColor(colors.surface2)),
            ) {
                Box(
                    Modifier
                        .align(Alignment.BottomStart)
                        .fillMaxWidth(0.55f)
                        .height(2.dp)
                        .background(accent),
                )
            }
            Spacer(Modifier.height(5.dp))
            // Нижнее меню: четыре пункта на surface1, первый активный.
            Row(
                Modifier.fillMaxWidth().height(16.dp).background(paletteColor(colors.surface1)),
                horizontalArrangement = Arrangement.SpaceEvenly,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                for (index in 0 until 4) {
                    Box(Modifier.size(5.dp).background(if (index == 0) accent else soft, CircleShape))
                }
            }
        }
        // Рамка поверх содержимого: полоса меню не перекрывает нижний край.
        Box(
            Modifier
                .matchParentSize()
                .border(if (selected) 2.dp else 1.dp, if (selected) accent else paletteColor(colors.outline), shape),
        )
    }
}

/** Кружок акцента 48 dp: исходный цвет 34 dp (не затемнённый под светлый тон), у выбранного кольцо цвета текста. */
@Composable
private fun Swatch(fill: Color, selected: Boolean, onClick: () -> Unit) {
    Box(
        Modifier
            .size(48.dp)
            .clip(CircleShape)
            .then(if (selected) Modifier.border(2.dp, Pl.text, CircleShape) else Modifier)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.size(34.dp).background(fill, CircleShape))
    }
}

/**
 * Последний кружок: пунктирный «+», пока свой цвет не выбран; потом закрашен
 * своим цветом и выбран. Нажатие всегда открывает выбор цвета.
 */
@Composable
private fun CustomSwatch(onClick: () -> Unit) {
    val custom = AccentState.custom
    if (custom != null) {
        Swatch(fill = AccentState.color, selected = true, onClick = onClick)
        return
    }
    val dash = Pl.outline
    Box(
        Modifier.size(48.dp).clip(CircleShape).clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Canvas(Modifier.size(48.dp)) {
            val stroke = 1.dp.toPx()
            drawCircle(
                color = dash,
                radius = (size.minDimension - stroke) / 2f,
                style = Stroke(stroke, pathEffect = PathEffect.dashPathEffect(floatArrayOf(4.dp.toPx(), 4.dp.toPx()))),
            )
        }
        PlIcon(R.drawable.ic_plus, 20.dp, Pl.text2)
    }
}

/** Образец: строка трека «играет» и две «таблетки» с текущим акцентом. */
@Composable
private fun PreviewCard() {
    Row(
        Modifier.fillMaxWidth().height(64.dp).padding(start = 16.dp, end = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier.size(48.dp).clip(RoundedCornerShape(8.dp)).background(coverPlaceholder("preview")),
            contentAlignment = Alignment.Center,
        ) {
            Box(Modifier.size(48.dp).background(Color.Black.copy(alpha = 0.6f)))
            EqBars(24.dp, Pl.accent, animated = false)
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            PlText("Ночной трамвай", PlType.bodyStrong, Pl.accent, maxLines = 1)
            PlText("Северный Ветер", PlType.secondary, Pl.text2, maxLines = 1)
        }
        Spacer(Modifier.width(12.dp))
        PlIcon(R.drawable.ic_heart_filled, 22.dp, Pl.accent)
        Spacer(Modifier.width(12.dp))
        PlIcon(R.drawable.ic_more, 20.dp, Pl.text2)
    }
    Row(
        Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 6.dp, bottom = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Pill("Слушать", Modifier.weight(1f), height = 44.dp, icon = R.drawable.ic_play_filled)
        Pill(
            "Перемешать",
            Modifier.weight(1f),
            kind = PillKind.Outline,
            height = 44.dp,
            icon = R.drawable.ic_shuffle,
        )
    }
}
