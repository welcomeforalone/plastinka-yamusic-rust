package ru.plastinka.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
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

/** «Оформление»: акцент (готовый или свой), плеер, предпросмотр. */
@Composable
fun AppearanceScreen(onBack: () -> Unit) {
    var picking by remember { mutableStateOf(false) }
    SettingsPageScaffold("Оформление", onBack) {
        SectionTitle("Акцент")
        Spacer(Modifier.height(10.dp))
        GroupCard {
            Row(
                Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 14.dp, bottom = 6.dp),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
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
            initial = AccentState.color.toRgbInt(),
            onDismiss = { picking = false },
            onDone = {
                AccentState.selectCustom(it)
                picking = false
            },
        )
    }
}

private fun Color.toRgbInt(): Int =
    (Math.round(red * 255f) shl 16) or (Math.round(green * 255f) shl 8) or Math.round(blue * 255f)

/** Кружок акцента 48 dp: цвет 34 dp, у выбранного кольцо цвета текста. */
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
 * Шестой кружок: пунктирный «+», пока свой цвет не выбран; потом закрашен
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
