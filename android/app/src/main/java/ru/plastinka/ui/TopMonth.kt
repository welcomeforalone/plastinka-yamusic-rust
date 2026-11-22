package ru.plastinka.ui

import androidx.annotation.DrawableRes
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import ru.plastinka.R
import ru.plastinka.data.HistoryContext
import ru.plastinka.data.TopArtist
import ru.plastinka.data.Trend
import ru.plastinka.data.listenedText

/** Название источника в истории и очереди для треков топа (как на десктопе). */
private const val TOP_SOURCE_TITLE = "Топ месяца"

/** Размер круглой обложки в полке и в сетке страницы. */
private val SHELF_COVER = 104.dp
private val GRID_COVER = 140.dp

/**
 * Полка «Ваш топ месяца»: заголовок со ссылкой «Весь топ» и ряд карточек
 * исполнителей. Общая для хаба и экрана «Исполнители»; сама она не
 * прокручивается по вертикали, поэтому ложится и в `Column`, и в `LazyColumn`.
 */
@Composable
internal fun TopMonthShelf(
    artists: List<TopArtist>,
    onAll: () -> Unit,
    onOpenArtist: (TopArtist) -> Unit,
) {
    if (artists.isEmpty()) return
    Column {
        Row(
            Modifier.fillMaxWidth().padding(start = 20.dp, end = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            PlText("Ваш топ месяца", PlType.title, modifier = Modifier.weight(1f))
            Box(
                Modifier.height(36.dp).clip(CircleShape).clickable(onClick = onAll).padding(horizontal = 12.dp),
                contentAlignment = Alignment.Center,
            ) {
                PlText("Весь топ", PlType.secondaryStrong, Pl.accent)
            }
        }
        Spacer(Modifier.height(8.dp))
        LazyRow(
            contentPadding = PaddingValues(horizontal = 20.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            items(artists, key = { it.card.id }) { top ->
                TopArtistCard(top, SHELF_COVER, onClick = { onOpenArtist(top) })
            }
        }
    }
}

/** Карточка исполнителя из топа: круглая обложка, имя, «место ▲ · время». */
@Composable
private fun TopArtistCard(top: TopArtist, size: Dp, onClick: () -> Unit) {
    // Зона нажатия скруглена только сверху, по кругу обложки: скругление снизу
    // срезало бы первую цифру места, она стоит в самом углу.
    Column(
        Modifier
            .width(size)
            .clip(RoundedCornerShape(topStart = size / 2, topEnd = size / 2))
            .clickable(onClick = onClick),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Cover(top.card.coverUrl("200x200"), top.card.id, size, size / 2)
        Spacer(Modifier.height(4.dp))
        PlText(top.card.name, PlType.bodyStrong, maxLines = 1)
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            top.position?.let { PlText(it.toString(), PlType.secondaryStrong, maxLines = 1) }
            trendMark(top.trend)?.let { (icon, color) -> PlIcon(icon, 10.dp, color) }
            listenedText(top.listenedSeconds)?.let {
                PlText("· $it", PlType.secondary, Pl.text2, maxLines = 1, modifier = Modifier.weight(1f, fill = false))
            }
        }
    }
}

/** Значок и цвет тренда (как на десктопе); без значения тренда значка нет. */
@DrawableRes
private fun trendIcon(trend: Trend): Int = when (trend) {
    Trend.Up -> R.drawable.ic_trend_up
    Trend.Down -> R.drawable.ic_trend_down
    Trend.New -> R.drawable.ic_trend_new
    Trend.Same -> R.drawable.ic_trend_same
}

@Composable
private fun trendMark(trend: Trend?): Pair<Int, androidx.compose.ui.graphics.Color>? = trend?.let {
    trendIcon(it) to when (it) {
        Trend.Up -> Pl.growth
        Trend.Down -> Pl.error
        Trend.New -> Pl.accent
        Trend.Same -> Pl.text3
    }
}

/** Страница «Топ месяца»: десять треков и исполнители одним списком. */
@Composable
internal fun TopMonthScreen(
    model: AppViewModel,
    player: PlayerController?,
    onBack: () -> Unit,
    onOpenArtist: (TopArtist) -> Unit,
) {
    val state by model.monthTop.collectAsStateWithLifecycle()
    val playing = rememberPlayingState(player)
    LaunchedEffect(Unit) { model.loadMonthTop() }
    val top = state.data
    val source = HistoryContext.Other(TOP_SOURCE_TITLE)

    Column(Modifier.fillMaxSize()) {
        BackBar(onBack)
        when {
            top == null && state.error != null -> ErrorState(state.error.orEmpty()) { model.loadMonthTop(force = true) }
            top == null -> LoadingBox()
            else -> LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 16.dp)) {
                item(key = "head") {
                    Column(Modifier.padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        PlText("Топ месяца", PlType.display)
                        PlText(
                            "Исполнители и треки, которые вы слушали больше всего за месяц",
                            PlType.body,
                            Pl.text2,
                        )
                    }
                    Spacer(Modifier.height(18.dp))
                }
                if (top.tracks.isEmpty() && top.artists.isEmpty()) {
                    item(key = "empty") {
                        Centered {
                            PlText(
                                "Послушайте музыку — здесь появится ваш топ за месяц.",
                                PlType.secondary,
                                Pl.text2,
                                textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                            )
                        }
                    }
                }
                if (top.tracks.isNotEmpty()) {
                    item(key = "h-tracks") { PageSection("Треки месяца") }
                    items(top.tracks.size, key = { "t$it:${top.tracks.getOrNull(it)?.id}" }) { index ->
                        top.tracks.getOrNull(index)?.let { track ->
                            TrackRow(
                                track,
                                current = track.id == playing.currentId,
                                soundOn = playing.soundOn,
                                liked = playing.liked.contains(track.id),
                                onClick = { player?.playQueue(top.tracks, index, TOP_SOURCE_TITLE, source = source) },
                                onLike = { model.toggleLike(track) },
                                number = index + 1,
                            )
                        }
                    }
                }
                if (top.artists.isNotEmpty()) {
                    item(key = "h-artists") { PageSection("Исполнители") }
                    val pairs = top.artists.chunked(2)
                    items(pairs.size, key = { "a$it" }) { index ->
                        Row(
                            Modifier.fillMaxWidth().padding(horizontal = 20.dp),
                            horizontalArrangement = Arrangement.spacedBy(12.dp),
                        ) {
                            for (artist in pairs.getOrNull(index).orEmpty()) {
                                Box(Modifier.weight(1f), contentAlignment = Alignment.TopCenter) {
                                    TopArtistCard(artist, GRID_COVER, onClick = { onOpenArtist(artist) })
                                }
                            }
                            // Нечётный исполнитель занимает одну колонку.
                            if (pairs.getOrNull(index)?.size == 1) Spacer(Modifier.weight(1f))
                        }
                        Spacer(Modifier.height(16.dp))
                    }
                }
            }
        }
    }
}

@Composable
private fun PageSection(title: String) {
    PlText(title, PlType.title, modifier = Modifier.padding(start = 20.dp, end = 20.dp, top = 18.dp, bottom = 12.dp))
}
