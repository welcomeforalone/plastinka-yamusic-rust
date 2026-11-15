package ru.plastinka.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import ru.plastinka.data.PlayLog
import ru.plastinka.data.Track
import ru.plastinka.data.dayLabel
import ru.plastinka.data.mergeHistory
import ru.plastinka.data.todayLocal

/** Строка экрана истории: подпись дня или трек. */
private sealed interface HistoryRow {
    data class Header(val label: String) : HistoryRow
    data class Item(val track: Track, val queueIndex: Int) : HistoryRow
}

/**
 * Экран «История»: серверная история и локальный журнал, по дням, свежие
 * сверху. Сервер не ответил, а журнал не пуст — показываем журнал без ошибки.
 */
@Composable
fun HistoryScreen(model: AppViewModel, player: PlayerController?, onBack: () -> Unit) {
    val server by model.history.collectAsStateWithLifecycle()
    val local by PlayLog.plays.collectAsStateWithLifecycle()
    val playing = rememberPlayingState(player)
    LaunchedEffect(Unit) { if (server.days == null && !server.loading) model.loadHistory() }

    val days = remember(server.days, local) { mergeHistory(server.days.orEmpty(), local) }
    // Очередь «История» — все треки списка подряд; нажатие играет с выбранного.
    val (rows, queue) = remember(days) {
        val today = todayLocal()
        val rows = mutableListOf<HistoryRow>()
        val queue = mutableListOf<Track>()
        for (day in days) {
            rows += HistoryRow.Header(dayLabel(day.date, today))
            for (track in day.groups.flatMap { it.tracks }) {
                rows += HistoryRow.Item(track, queue.size)
                queue += track
            }
        }
        rows.toList() to queue.toList()
    }

    Column(Modifier.fillMaxSize()) {
        BackBar(onBack)
        PlText("История", PlType.display, modifier = Modifier.padding(horizontal = 20.dp))
        val error = server.error
        when {
            rows.isEmpty() && server.days == null && error != null -> ErrorState(error) { model.loadHistory() }
            rows.isEmpty() && server.days == null -> LoadingBox()
            rows.isEmpty() -> Centered {
                PlText("Пока ничего нет", PlType.bodyStrong, textAlign = TextAlign.Center)
                PlText(
                    "Здесь появятся треки, которые вы слушали",
                    PlType.secondary,
                    Pl.text2,
                    textAlign = TextAlign.Center,
                )
            }
            else -> LazyColumn(Modifier.fillMaxSize()) {
                items(rows.size, key = { index ->
                    when (val row = rows.getOrNull(index)) {
                        is HistoryRow.Header -> "h$index"
                        is HistoryRow.Item -> "t$index:${row.track.id}"
                        null -> "n$index"
                    }
                }) { index ->
                    when (val row = rows.getOrNull(index)) {
                        is HistoryRow.Header -> PlText(
                            row.label,
                            PlType.title,
                            modifier = Modifier.padding(start = 20.dp, end = 20.dp, top = 18.dp, bottom = 6.dp),
                        )
                        is HistoryRow.Item -> TrackRow(
                            row.track,
                            current = row.track.id == playing.currentId,
                            soundOn = playing.soundOn,
                            liked = playing.liked.contains(row.track.id),
                            onClick = { player?.playQueue(queue, row.queueIndex, "История") },
                            onLike = { model.toggleLike(row.track) },
                        )
                        null -> Unit
                    }
                }
            }
        }
    }
}
