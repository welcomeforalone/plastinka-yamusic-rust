package ru.plastinka.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import ru.plastinka.R
import ru.plastinka.playback.QueueHolder
import ru.plastinka.playback.currentTrack

/** Вкладка «Поиск»: поле и список найденных треков. */
@Composable
fun SearchTab(model: AppViewModel, player: PlayerController?) {
    val query by model.query.collectAsStateWithLifecycle()
    val tracks by model.search.collectAsStateWithLifecycle()
    val error by model.searchError.collectAsStateWithLifecycle()
    val searching by model.searching.collectAsStateWithLifecycle()
    val liked by QueueHolder.likedIds.collectAsStateWithLifecycle()
    val queue by QueueHolder.state.collectAsStateWithLifecycle()
    val soundOn = player?.isPlaying?.collectAsStateWithLifecycle()?.value ?: false
    val currentId = queue.currentTrack()?.id

    Column(Modifier.fillMaxSize().padding(top = 12.dp)) {
        PlText("Поиск", PlType.display, modifier = Modifier.padding(horizontal = 20.dp))
        Spacer(Modifier.height(14.dp))
        SearchField(query, model::setQuery)
        Spacer(Modifier.height(12.dp))
        when {
            error != null -> Message("Не удалось выполнить поиск. Проверьте соединение.", null, isError = true)
            tracks.isNotEmpty() -> LazyColumn(Modifier.fillMaxSize()) {
                items(tracks.size, key = { tracks.getOrNull(it)?.id ?: it.toString() }) { index ->
                    tracks.getOrNull(index)?.let { track ->
                        TrackRow(
                            track,
                            current = track.id == currentId,
                            soundOn = soundOn,
                            liked = liked.contains(track.id),
                            onClick = { player?.playQueue(tracks, index, "Поиск") },
                            onLike = { model.toggleLike(track) },
                        )
                    }
                }
            }
            query.isBlank() -> Message("Введите запрос", null)
            searching -> Box(Modifier.fillMaxWidth().padding(vertical = 40.dp), contentAlignment = Alignment.Center) {
                androidx.compose.material3.CircularProgressIndicator(
                    Modifier.size(28.dp),
                    color = Pl.accent,
                    strokeWidth = 2.dp,
                )
            }
            else -> Message("Ничего не нашлось", "Проверьте запрос или попробуйте другое слово")
        }
    }
}

@Composable
private fun Message(title: String, hint: String?, isError: Boolean = false) {
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 32.dp, vertical = 40.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        PlText(
            title,
            if (isError) PlType.secondary else PlType.bodyStrong,
            if (isError) Pl.error else Pl.text,
            textAlign = TextAlign.Center,
        )
        hint?.let { PlText(it, PlType.secondary, Pl.text2, textAlign = TextAlign.Center) }
    }
}

@Composable
private fun SearchField(query: String, onChange: (String) -> Unit) {
    val focus = LocalFocusManager.current
    Row(
        Modifier
            .padding(horizontal = 16.dp)
            .fillMaxWidth()
            .height(52.dp)
            .clip(CircleShape)
            .background(Pl.surface2, CircleShape)
            .border(1.dp, Pl.outline, CircleShape)
            .padding(start = 18.dp, end = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        PlIcon(R.drawable.ic_search, 20.dp, Pl.text3)
        Spacer(Modifier.width(12.dp))
        Box(Modifier.weight(1f)) {
            BasicTextField(
                value = query,
                onValueChange = onChange,
                singleLine = true,
                textStyle = PlType.body.copy(color = Pl.text),
                cursorBrush = SolidColor(Pl.accent),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                keyboardActions = KeyboardActions(onSearch = { focus.clearFocus() }),
                modifier = Modifier.fillMaxWidth(),
                decorationBox = { inner ->
                    Box(contentAlignment = Alignment.CenterStart) {
                        if (query.isEmpty()) PlText("Трек или исполнитель", PlType.body, Pl.text3)
                        inner()
                    }
                },
            )
        }
        if (query.isNotEmpty()) {
            Box(
                Modifier.size(36.dp).clip(CircleShape).clickable { onChange("") },
                contentAlignment = Alignment.Center,
            ) {
                PlIcon(R.drawable.ic_close, 18.dp, Pl.text2)
            }
        }
    }
}
