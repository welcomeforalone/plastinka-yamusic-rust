package ru.plastinka.ui

import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.annotation.DrawableRes
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.width
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import coil.compose.AsyncImage
import ru.plastinka.R

/** Открыть ссылку во внешнем браузере; если открыть нечем — тихо ничего не делаем. */
fun openUrl(context: Context, url: String) {
    try {
        val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        context.startActivity(intent)
    } catch (_: Exception) {
        // Нет браузера или ссылка не открылась: приложение это не роняет.
    }
}

/** Текст со стилем и цветом из шкалы макета. */
@Composable
fun PlText(
    text: String,
    style: TextStyle,
    color: Color = Pl.text,
    modifier: Modifier = Modifier,
    textAlign: TextAlign? = null,
    maxLines: Int = Int.MAX_VALUE,
) {
    Text(
        text = text,
        style = style,
        color = color,
        modifier = modifier,
        textAlign = textAlign,
        maxLines = maxLines,
        overflow = if (maxLines == Int.MAX_VALUE) TextOverflow.Clip else TextOverflow.Ellipsis,
    )
}

/** Иконка Lucide из `res/drawable/ic_*.xml`: белая, красится через tint. */
@Composable
fun PlIcon(
    @DrawableRes id: Int,
    size: Dp = 24.dp,
    tint: Color = Pl.text,
    modifier: Modifier = Modifier,
) {
    Icon(painterResource(id), contentDescription = null, modifier = modifier.size(size), tint = tint)
}

enum class PillKind { Primary, Outline, Tonal, Danger, Ghost }

/**
 * Кнопка-«таблетка». Высота 48/52 — обычная, 36–44 — малая.
 * `onClick == null` — некликабельная (предпросмотр).
 */
@Composable
fun Pill(
    label: String,
    modifier: Modifier = Modifier,
    kind: PillKind = PillKind.Primary,
    height: Dp = 48.dp,
    @DrawableRes icon: Int? = null,
    fillWidth: Boolean = false,
    enabled: Boolean = true,
    ghostColor: Color = Pl.text2,
    onClick: (() -> Unit)? = null,
) {
    val big = height >= 48.dp
    val (fill, textColor) = when (kind) {
        PillKind.Primary -> Pl.accent to Pl.onAccent
        PillKind.Outline -> Color.Transparent to Pl.text
        PillKind.Tonal -> Pl.surface3 to Pl.text
        PillKind.Danger -> Pl.errorSoft to Pl.error
        PillKind.Ghost -> Color.Transparent to ghostColor
    }
    var m = modifier
    if (fillWidth) m = m.fillMaxWidth()
    m = m.height(height).clip(CircleShape).background(fill, CircleShape)
    if (kind == PillKind.Outline) m = m.border(1.dp, Pl.outline, CircleShape)
    if (onClick != null) m = m.clickable(enabled = enabled, onClick = onClick)
    Row(
        m.padding(start = if (icon != null) (if (big) 18.dp else 14.dp) else (if (big) 22.dp else 16.dp),
            end = if (big) 22.dp else 16.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val c = if (enabled) textColor else textColor.copy(alpha = 0.5f)
        if (icon != null) PlIcon(icon, if (big) 18.dp else 16.dp, c)
        PlText(label, if (big) PlType.bodyStrong else PlType.secondaryStrong, c, maxLines = 1)
    }
}

/**
 * Чип: 36 dp, выбранный — заливка text и тёмный текст. `fill` — полупрозрачная
 * заливка вместо рамки (чипы на цветном фоне, макет 04); `icon` — значок 16 dp
 * слева (поле слева 12); `trailingIcon` — значок 16 dp справа (подпись при нехватке
 * места обрезается, значок остаётся); `color` — цвет текста и значка невыбранного чипа.
 */
@Composable
fun Chip(
    label: String,
    selected: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    @DrawableRes icon: Int? = null,
    @DrawableRes trailingIcon: Int? = null,
    fill: Color? = null,
    color: Color = Pl.text,
) {
    val shape = CircleShape
    var m = modifier.height(36.dp).clip(shape)
    m = when {
        selected -> m.background(Pl.text, shape)
        fill != null -> m.background(fill, shape)
        else -> m.border(1.dp, Pl.outline, shape)
    }
    val content = if (selected) Pl.darkText else color
    Row(
        m.clickable(onClick = onClick).padding(start = if (icon != null) 12.dp else 14.dp, end = 14.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (icon != null) PlIcon(icon, 16.dp, content)
        PlText(
            label,
            PlType.secondaryStrong,
            content,
            modifier = if (trailingIcon != null) Modifier.weight(1f, fill = false) else Modifier,
            maxLines = 1,
        )
        if (trailingIcon != null) PlIcon(trailingIcon, 16.dp, content)
    }
}

/** Круглая кнопка Play/Pause; иконка — 42 % размера. */
@Composable
fun PlayButton(
    size: Dp,
    playing: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    color: Color = Pl.accent,
    onColor: Color = Pl.onAccent,
) {
    Box(
        modifier.size(size).clip(CircleShape).background(color, CircleShape).clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        PlIcon(
            if (playing) R.drawable.ic_pause_filled else R.drawable.ic_play_filled,
            size * 0.42f,
            onColor,
        )
    }
}

/**
 * Аватар: с адресом — картинка с бека (пока грузится — градиент-подложка),
 * без — серый градиент и иконка пользователя.
 */
@Composable
fun Avatar(size: Dp, url: String? = null, modifier: Modifier = Modifier) {
    Box(
        modifier
            .size(size)
            .clip(CircleShape)
            .background(
                Brush.linearGradient(
                    listOf(Color(0xFF3A3A3A), Color(0xFF6A6A6A)),
                    start = Offset.Zero,
                    end = Offset.Infinite,
                ),
            ),
        contentAlignment = Alignment.Center,
    ) {
        if (url != null) {
            AsyncImage(
                model = url,
                contentDescription = null,
                modifier = Modifier.size(size),
                contentScale = androidx.compose.ui.layout.ContentScale.Crop,
            )
        } else {
            PlIcon(R.drawable.ic_user, size * 0.5f, Pl.text)
        }
    }
}

/** Логотип-пластинка: диск, дорожки, «яблоко» акцентом и отверстие. */
@Composable
fun Vinyl(size: Dp, modifier: Modifier = Modifier) {
    val accent = Pl.accent
    Canvas(modifier.size(size)) {
        val s = this.size.minDimension
        val center = Offset(s / 2, s / 2)
        val ringStroke = 1.dp.toPx()
        val edge = 1.5.dp.toPx()
        drawCircle(Color(0xFF111111), radius = s / 2, center = center)
        drawCircle(Color(0xFF2E2E2E), radius = s / 2 - edge / 2, center = center, style = Stroke(edge))
        for (k in listOf(0.86f, 0.72f, 0.58f, 0.46f)) {
            drawCircle(Color(0xFF262626), radius = s * k / 2, center = center, style = Stroke(ringStroke))
        }
        drawCircle(accent, radius = s * 0.344f / 2, center = center)
        drawCircle(Color.Black, radius = s * 0.055f / 2, center = center)
    }
}

/** Мягкое свечение: радиальный градиент от акцента к прозрачному (без blur, он только с API 31). */
@Composable
fun Glow(size: Dp, modifier: Modifier = Modifier, color: Color = Pl.accent, alpha: Float = 0.5f) {
    Box(
        modifier
            .size(size)
            .background(
                Brush.radialGradient(
                    listOf(color.copy(alpha = alpha), Color.Transparent),
                ),
            ),
    )
}

/** Пары цветов заглушек обложек, как в `src/ui/theme.rs`. */
private val COVER_GRADIENTS = listOf(
    0xFF5B4BDB to 0xFFC15BD8, 0xFF1F8A70 to 0xFF9AD94B, 0xFFE2574C to 0xFFF2A541,
    0xFF2A3B8F to 0xFF4FB3E8, 0xFF8E2D5A to 0xFFF06C9B, 0xFF3C3C46 to 0xFF8C8C99,
    0xFF0E7C86 to 0xFF57E0C6, 0xFFB5651D to 0xFFF4D35E, 0xFF6A1B9A to 0xFFF48FB1,
    0xFF263238 to 0xFF4DB6AC, 0xFFC62828 to 0xFF6A1B9A, 0xFF1565C0 to 0xFF00ACC1,
)

/** Градиент-заглушка: одна и та же для одного id, из левого верхнего угла в правый нижний. */
fun coverPlaceholder(seed: String): Brush {
    // Беззнаковые 64 бита с переполнением, как `usize` на десктопе: один и тот
    // же id даёт одну и ту же пару цветов в обоих клиентах.
    var sum = 0UL
    for (byte in seed.toByteArray(Charsets.UTF_8)) {
        sum = sum * 31UL + (byte.toInt() and 0xFF).toULong()
    }
    val index = (sum % COVER_GRADIENTS.size.toULong()).toInt()
    val pair = COVER_GRADIENTS.getOrElse(index) { 0xFF3C3C46 to 0xFF8C8C99 }
    return Brush.linearGradient(
        listOf(Color(pair.first), Color(pair.second)),
        start = Offset.Zero,
        end = Offset.Infinite,
    )
}

/** Обложка: картинка по ссылке поверх градиента-заглушки. */
@Composable
fun Cover(
    url: String?,
    seed: String,
    size: Dp,
    radius: Dp,
    modifier: Modifier = Modifier,
) {
    val shape = RoundedCornerShape(radius)
    Box(modifier.size(size).clip(shape).background(coverPlaceholder(seed), shape)) {
        if (url != null) {
            AsyncImage(
                model = url,
                contentDescription = null,
                modifier = Modifier.size(size),
                contentScale = androidx.compose.ui.layout.ContentScale.Crop,
            )
        }
    }
}

/**
 * Верхняя панель подэкрана: стрелка назад (значок 24 dp на расстоянии 16 dp от
 * края, зона нажатия 44 dp). Высота 52 dp; у экранов с шапкой-обложкой — 56 dp.
 */
@Composable
fun BackBar(onBack: () -> Unit, height: Dp = 52.dp) {
    Box(
        Modifier.fillMaxWidth().height(height).padding(start = 6.dp),
        contentAlignment = Alignment.CenterStart,
    ) {
        Box(
            Modifier.size(44.dp).clip(CircleShape).clickable(onClick = onBack),
            contentAlignment = Alignment.Center,
        ) {
            PlIcon(R.drawable.ic_back, 24.dp, Pl.text)
        }
    }
}

/** Шеврон «дальше» из макета: 16×16, контур (6,3)-(11,8)-(6,13), линия 1,6, скруглённые концы. */
@Composable
fun Chevron(modifier: Modifier = Modifier, color: Color = Pl.text3) {
    Canvas(modifier.size(16.dp)) {
        val k = size.width / 16f
        val path = Path().apply {
            moveTo(6f * k, 3f * k)
            lineTo(11f * k, 8f * k)
            lineTo(6f * k, 13f * k)
        }
        drawPath(path, color, style = Stroke(1.6.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round))
    }
}

/** Строка списка: обложка 48 (радиус 8 — квадрат, 24 — круг), название и необязательная подпись. */
@Composable
fun MediaRow(
    coverUrl: String?,
    seed: String,
    radius: Dp,
    title: String,
    subtitle: String?,
    onClick: () -> Unit,
) {
    Row(
        Modifier.fillMaxWidth().height(64.dp).clickable(onClick = onClick).padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Cover(coverUrl, seed, 48.dp, radius)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            PlText(title, PlType.body, maxLines = 1)
            if (subtitle != null) PlText(subtitle, PlType.secondary, Pl.text2, maxLines = 1)
        }
    }
}

/**
 * Плитка: квадратная обложка с радиусом 16 на всю ширину `modifier`, под ней
 * название (`secondaryStrong`) и подпись (`caption`).
 */
@Composable
fun MediaTile(
    coverUrl: String?,
    seed: String,
    title: String,
    subtitle: String?,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    val shape = RoundedCornerShape(16.dp)
    // Зона нажатия скруглена только сверху: скругление снизу срезало бы начало подписи.
    val touchShape = RoundedCornerShape(topStart = 16.dp, topEnd = 16.dp)
    Column(modifier.clip(touchShape).clickable(onClick = onClick), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Box(Modifier.fillMaxWidth().aspectRatio(1f).clip(shape).background(coverPlaceholder(seed), shape)) {
            if (coverUrl != null) {
                AsyncImage(
                    model = coverUrl,
                    contentDescription = null,
                    modifier = Modifier.fillMaxSize(),
                    contentScale = androidx.compose.ui.layout.ContentScale.Crop,
                )
            }
        }
        Spacer(Modifier.height(6.dp))
        PlText(title, PlType.secondaryStrong, maxLines = 1)
        if (!subtitle.isNullOrEmpty()) PlText(subtitle, PlType.caption, Pl.text3, maxLines = 1)
    }
}

@Composable
fun Centered(content: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 32.dp, vertical = 48.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) { content() }
}

@Composable
fun LoadingBox() {
    Centered { CircularProgressIndicator(Modifier.size(28.dp), color = Pl.accent, strokeWidth = 2.dp) }
}

/** Ошибка загрузки: понятный текст и «Повторить». */
@Composable
fun ErrorState(message: String, onRetry: () -> Unit) {
    Centered {
        PlText(message, PlType.secondary, Pl.text2, textAlign = TextAlign.Center)
        Spacer(Modifier.height(8.dp))
        Pill("Повторить", kind = PillKind.Tonal, height = 44.dp, icon = R.drawable.ic_retry, onClick = onRetry)
    }
}
