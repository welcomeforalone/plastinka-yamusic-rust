package ru.plastinka.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import coil.compose.AsyncImage
import ru.plastinka.data.ArtistCard

/**
 * Полка коллекции (как на десктопе, в мобильном виде): заголовок и сетка из
 * `columns` колонок и `rows` рядов. Не поместилось — справа «Показать все»,
 * раскрытая полка показывает всё и предлагает «Свернуть». Состояние
 * раскрытия переживает прокрутку и пересоздание. Сама не прокручивается,
 * поэтому ложится и в `Column`, и в `LazyColumn`.
 */
@Composable
internal fun <T> CardShelf(
    title: String,
    items: List<T>,
    columns: Int,
    rows: Int,
    card: @Composable (T, Modifier) -> Unit,
) {
    if (items.isEmpty()) return
    var expanded by rememberSaveable { mutableStateOf(false) }
    val fits = rows * columns
    val shown = if (expanded) items else items.take(fits)
    Column {
        Row(
            Modifier.fillMaxWidth().padding(start = 20.dp, end = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            PlText(title, PlType.title, modifier = Modifier.weight(1f))
            if (items.size > fits) {
                Box(
                    Modifier.height(36.dp).clip(CircleShape).clickable { expanded = !expanded }
                        .padding(horizontal = 12.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    PlText(if (expanded) "Свернуть" else "Показать все", PlType.secondaryStrong, Pl.accent)
                }
            }
        }
        Spacer(Modifier.height(8.dp))
        Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            for (row in shown.chunked(columns)) {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    for (item in row) card(item, Modifier.weight(1f))
                    // Неполный ряд: пустые места держат ширину плитки.
                    repeat(columns - row.size) { Spacer(Modifier.weight(1f)) }
                }
            }
        }
    }
}

/** Заголовок списка под полками («Вам понравились»). */
@Composable
internal fun ListTitle(text: String) {
    PlText(text, PlType.title, modifier = Modifier.padding(horizontal = 20.dp))
}

/** Круглая плитка исполнителя: обложка на всю ширину `modifier`, имя под ней. */
@Composable
internal fun ArtistTile(card: ArtistCard, modifier: Modifier = Modifier, onClick: () -> Unit) {
    Column(
        modifier.clip(RoundedCornerShape(topStart = 48.dp, topEnd = 48.dp)).clickable(onClick = onClick),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Box(Modifier.fillMaxWidth().aspectRatio(1f).clip(CircleShape).background(coverPlaceholder(card.id), CircleShape)) {
            card.coverUrl("200x200")?.let { url ->
                AsyncImage(
                    model = url,
                    contentDescription = null,
                    modifier = Modifier.fillMaxSize(),
                    contentScale = ContentScale.Crop,
                )
            }
        }
        Spacer(Modifier.height(4.dp))
        PlText(card.name, PlType.secondaryStrong, maxLines = 1)
    }
}
