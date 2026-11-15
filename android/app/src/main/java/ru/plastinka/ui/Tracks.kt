package ru.plastinka.ui

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import ru.plastinka.R
import ru.plastinka.data.Track

/** Длительность `м:сс`. */
fun formatDuration(ms: Long): String {
    val total = (ms / 1000).coerceAtLeast(0)
    return "${total / 60}:${(total % 60).toString().padStart(2, '0')}"
}

/** «1 трек / 2 трека / 5 треков». */
fun tracksCount(n: Int): String {
    val mod100 = n % 100
    val mod10 = n % 10
    val word = when {
        mod100 in 11..14 -> "треков"
        mod10 == 1 -> "трек"
        mod10 in 2..4 -> "трека"
        else -> "треков"
    }
    return "$n $word"
}

/** Три столбика «играет»: качаются, пока звук идёт, на паузе стоят. */
@Composable
fun EqBars(size: Dp, color: Color, animated: Boolean, modifier: Modifier = Modifier) {
    val transition = rememberInfiniteTransition(label = "eq")
    val spec = { ms: Int ->
        infiniteRepeatable<Float>(tween(ms, easing = LinearEasing), RepeatMode.Reverse)
    }
    val a by transition.animateFloat(0.25f, 0.95f, spec(520), label = "eq-a")
    val b by transition.animateFloat(0.25f, 0.95f, spec(380), label = "eq-b")
    val c by transition.animateFloat(0.25f, 0.95f, spec(460), label = "eq-c")
    val heights = if (animated) listOf(a, b, c) else listOf(0.55f, 0.9f, 0.4f)
    Canvas(modifier.size(size)) {
        val s = this.size.minDimension
        val w = s / 6
        heights.forEachIndexed { i, h ->
            val x = s * 0.14f + i * (w + s * 0.1f)
            val top = s * (0.84f - h * 0.68f)
            drawRoundRect(
                color,
                topLeft = Offset(x, top),
                size = Size(w, s * 0.84f - top),
                cornerRadius = CornerRadius(w / 4),
            )
        }
    }
}

/**
 * Строка трека: высота 64, обложка 48. `current` — трек очереди, `soundOn` —
 * звук идёт (столбики качаются). Недоступный трек не нажимается.
 */
@Composable
fun TrackRow(
    track: Track,
    current: Boolean,
    soundOn: Boolean,
    liked: Boolean,
    onClick: () -> Unit,
    onLike: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val available = track.available
    Row(
        modifier
            .fillMaxWidth()
            .height(64.dp)
            .then(if (available) Modifier.clickable(onClick = onClick) else Modifier)
            .padding(start = 16.dp, end = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) {
            Cover(
                track.coverUrl("100x100"),
                track.id,
                48.dp,
                8.dp,
                if (available) Modifier else Modifier.alpha(0.4f),
            )
            if (current && available) {
                Box(
                    Modifier
                        .size(48.dp)
                        .clip(RoundedCornerShape(8.dp))
                        .background(Color.Black.copy(alpha = 0.6f)),
                )
                EqBars(24.dp, Pl.accent, soundOn)
            }
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            PlText(
                track.title,
                if (current && available) PlType.bodyStrong else PlType.body,
                when {
                    !available -> Pl.textDisabled
                    current -> Pl.accent
                    else -> Pl.text
                },
                maxLines = 1,
            )
            PlText(
                if (available) track.artistNames else "Недоступен",
                PlType.secondary,
                if (available) Pl.text2 else Pl.textDisabled,
                maxLines = 1,
            )
        }
        HeartButton(liked, onLike, 22.dp)
    }
}

/** Сердце с зоной нажатия не меньше 40 dp. */
@Composable
fun HeartButton(liked: Boolean, onClick: () -> Unit, size: Dp, modifier: Modifier = Modifier) {
    Box(
        modifier
            .size(maxOf(40.dp, size + 12.dp))
            .clip(RoundedCornerShape(20.dp))
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        PlIcon(
            if (liked) R.drawable.ic_heart_filled else R.drawable.ic_heart,
            size,
            if (liked) Pl.accent else Pl.text2,
        )
    }
}
