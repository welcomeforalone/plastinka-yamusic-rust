package ru.plastinka.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import ru.plastinka.data.ArtistRef
import ru.plastinka.data.Track

/** Модификаторы нажатия для названия трека и строки исполнителей; пустые, если переходить некуда. */
class TrackLinks(val title: Modifier, val artists: Modifier)

/** Нажатие без подсветки: как у соседних текстовых элементов плеера. */
@Composable
internal fun Modifier.plainClickable(onClick: () -> Unit): Modifier = clickable(
    interactionSource = remember { MutableInteractionSource() },
    indication = null,
    onClick = onClick,
)

/**
 * Переход к странице (альбом, исполнитель) из любого места интерфейса; задаёт `MainScreen`.
 * По умолчанию `null`: ссылок нет.
 */
val LocalOpenRoute = staticCompositionLocalOf<((CollectionRoute) -> Unit)?> { null }

/**
 * Общая логика ссылок трека: название ведёт к альбому, исполнитель — к своей странице
 * (несколько — через лист «Исполнители», который показывается отсюда же).
 * Без [LocalOpenRoute] ссылок нет (модификаторы пустые).
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun rememberTrackLinks(track: Track): TrackLinks {
    val onOpenRoute = LocalOpenRoute.current
    var showArtists by remember { mutableStateOf(false) }
    if (onOpenRoute == null) return TrackLinks(Modifier, Modifier)
    val albumRoute = albumRouteOf(track)
    val linkable = linkableArtists(track)
    if (showArtists) {
        ArtistsSheet(linkable, onDismiss = { showArtists = false }) { route ->
            showArtists = false
            onOpenRoute(route)
        }
    }
    return TrackLinks(
        title = if (albumRoute != null) Modifier.plainClickable { onOpenRoute(albumRoute) } else Modifier,
        artists = if (linkable.isEmpty()) {
            Modifier
        } else {
            Modifier.plainClickable {
                // Один исполнитель — сразу страница, несколько — лист выбора.
                val only = linkable.singleOrNull()?.let(::artistRouteOf)
                if (only != null) onOpenRoute(only) else showArtists = true
            }
        },
    )
}

/** Лист «Исполнители»: строки как в меню трека (56 dp, `body`). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ArtistsSheet(artists: List<ArtistRef>, onDismiss: () -> Unit, onOpen: (CollectionRoute) -> Unit) {
    PlBottomSheet(onDismiss) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().padding(bottom = 8.dp)) {
            PlText(
                "Исполнители",
                PlType.headline,
                modifier = Modifier.padding(start = 20.dp, end = 20.dp, bottom = 12.dp),
                maxLines = 1,
            )
            for (artist in artists) {
                val route = artistRouteOf(artist) ?: continue
                Row(
                    Modifier
                        .fillMaxWidth()
                        .height(56.dp)
                        .clickable { onOpen(route) }
                        .padding(horizontal = 20.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    PlText(artist.name, PlType.body, maxLines = 1)
                }
            }
        }
    }
}
