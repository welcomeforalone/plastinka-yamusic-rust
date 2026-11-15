package ru.plastinka.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
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
import androidx.compose.foundation.text.BasicTextField
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
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp

/**
 * Выбор своего акцента: поле «насыщенность и яркость», полоса оттенка, поле
 * HEX и предпросмотр. Цвет хранится как HSV, чтобы на сером и чёрном оттенок
 * не терялся при перетаскивании. Применяется кнопкой «Готово».
 */
@Composable
fun AccentPickerDialog(initial: Int, onDismiss: () -> Unit, onDone: (Int) -> Unit) {
    var hsv by remember { mutableStateOf(Hsv.fromRgb(initial, 0f)) }
    var text by remember { mutableStateOf(ColorMath.formatHex(initial)) }

    val raw = hsv.toRgb()
    // Слишком тёмный цвет осветляется: показываем и применяем уже читаемый.
    val shown = ColorMath.readable(raw)
    val parsed = ColorMath.parseHex(text)
    val invalid = parsed == null

    fun setHsv(next: Hsv) {
        hsv = next
        text = ColorMath.formatHex(next.toRgb())
    }

    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = Pl.surface2,
        title = { PlText("Свой цвет", PlType.headline) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(14.dp)) {
                DragSurface(
                    modifier = Modifier.fillMaxWidth().height(168.dp),
                    onPick = { x, y -> setHsv(hsv.copy(s = x, v = 1f - y)) },
                ) { size ->
                    drawRect(Brush.horizontalGradient(listOf(Color.White, hsv.pure())))
                    drawRect(Brush.verticalGradient(listOf(Color.Transparent, Color.Black)))
                    marker(Offset(hsv.s * size.width, (1f - hsv.v) * size.height), Color(0xFF000000.toInt() or raw))
                }
                DragSurface(
                    modifier = Modifier.fillMaxWidth().height(18.dp),
                    onPick = { x, _ -> setHsv(hsv.copy(h = x * 360f)) },
                ) { size ->
                    val stops = (0..6).map { Hsv(it * 60f, 1f, 1f).pure() }
                    drawRect(Brush.horizontalGradient(stops))
                    marker(Offset(hsv.h / 360f * size.width, size.height / 2f), hsv.pure())
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(
                        Modifier
                            .size(36.dp)
                            .background(Color(0xFF000000.toInt() or shown), CircleShape)
                            .border(1.dp, Pl.outline, CircleShape),
                    )
                    Spacer(Modifier.width(12.dp))
                    HexField(
                        text = text,
                        invalid = invalid,
                        onChange = { value ->
                            text = value
                            ColorMath.parseHex(value)?.let { hsv = Hsv.fromRgb(it, hsv.h) }
                        },
                    )
                }
                when {
                    invalid -> PlText("Нужен цвет вида #RRGGBB", PlType.caption, Pl.error)
                    shown != raw ->
                        PlText("Цвет осветлён, чтобы быть видным на чёрном фоне", PlType.caption, Pl.text3)
                }
            }
        },
        confirmButton = {
            TextButton(enabled = !invalid, onClick = { onDone(shown) }) {
                PlText("Готово", PlType.secondaryStrong, if (invalid) Pl.textDisabled else Pl.accent)
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { PlText("Отмена", PlType.secondaryStrong, Pl.text2) }
        },
    )
}

/** Область, где касание и перетаскивание дают точку 0..1 по обеим осям. */
@Composable
private fun DragSurface(
    modifier: Modifier,
    onPick: (Float, Float) -> Unit,
    draw: androidx.compose.ui.graphics.drawscope.DrawScope.(androidx.compose.ui.geometry.Size) -> Unit,
) {
    val pick = { offset: Offset, size: IntSize ->
        if (size.width > 0 && size.height > 0) {
            onPick(
                (offset.x / size.width).coerceIn(0f, 1f),
                (offset.y / size.height).coerceIn(0f, 1f),
            )
        }
    }
    Canvas(
        modifier
            .clip(RoundedCornerShape(14.dp))
            .pointerInput(Unit) { detectTapGestures { pick(it, size) } }
            .pointerInput(Unit) {
                detectDragGestures(
                    onDragStart = { pick(it, size) },
                    onDrag = { change, _ ->
                        change.consume()
                        pick(change.position, size)
                    },
                )
            },
    ) { draw(size) }
}

/** Маркер: белое кольцо с чёрной окантовкой снаружи и цветом выбора внутри. */
private fun androidx.compose.ui.graphics.drawscope.DrawScope.marker(at: Offset, fill: Color) {
    drawCircle(fill, 8.dp.toPx(), at)
    drawCircle(Color.Black.copy(alpha = 0.4f), 10.dp.toPx(), at, style = Stroke(1.dp.toPx()))
    drawCircle(Color.White, 8.dp.toPx(), at, style = Stroke(2.dp.toPx()))
}

/** Поле `#RRGGBB`: рамка красная, пока запись неверна. */
@Composable
private fun HexField(text: String, invalid: Boolean, onChange: (String) -> Unit) {
    val shape = RoundedCornerShape(50)
    BasicTextField(
        value = text,
        onValueChange = { onChange(it.take(9)) },
        singleLine = true,
        textStyle = PlType.secondaryStrong.copy(color = Pl.text),
        cursorBrush = SolidColor(Pl.accent),
        keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Characters),
        modifier = Modifier
            .width(130.dp)
            .border(1.dp, if (invalid) Pl.error else Pl.outline, shape)
            .padding(horizontal = 16.dp, vertical = 10.dp),
    )
}
