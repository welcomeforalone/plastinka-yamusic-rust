package ru.plastinka.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
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
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.flow.MutableStateFlow
import ru.plastinka.PlastinkaApp
import ru.plastinka.R
import ru.plastinka.data.ArtistRef
import ru.plastinka.data.CopyName
import ru.plastinka.data.FckState
import ru.plastinka.data.Prefs
import ru.plastinka.data.SlopStatus
import ru.plastinka.data.Slopless
import ru.plastinka.data.Track
import ru.plastinka.playback.QueueHolder
import ru.plastinka.playback.currentTrack

/** «Плагины»: FckCensor, Slopless, CopyTrackName. Все выключены по умолчанию. */
@Composable
fun PluginsScreen(onBack: () -> Unit) {
    SettingsPageScaffold("Плагины", onBack) {
        FckCensorCard()
        Spacer(Modifier.height(12.dp))
        SloplessCard()
        Spacer(Modifier.height(12.dp))
        CopyNameCard()
    }
}

/** Строка состояния плагина: текст и, если нужно, небольшая кнопка справа. */
@Composable
private fun StatusRow(title: String, text: String, action: String? = null, onAction: () -> Unit = {}) {
    Row(
        Modifier.fillMaxWidth().padding(start = 14.dp, end = 16.dp, top = 6.dp, bottom = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            PlText(title, PlType.secondaryStrong)
            PlText(text, PlType.caption, Pl.text3)
        }
        if (action != null) {
            Spacer(Modifier.width(12.dp))
            Pill(action, kind = PillKind.Tonal, height = 36.dp, onClick = onAction)
        }
    }
}

// ----------------------------------------------------------------- FckCensor

@Composable
private fun FckCensorCard() {
    GroupCard {
        ToggleRow(
            title = "FckCensor",
            subtitle = "Названия, исполнители, обложки и звук без цензуры из списка FckCensorData " +
                "для треков, которые в нём есть. Звук играет потоком, на устройство сохраняется только список",
            checked = Prefs.fckCensor,
            onChange = Prefs::changeFckCensor,
        )
        if (Prefs.fckCensor) {
            val app = LocalContext.current.applicationContext as? PlastinkaApp
            val state by (app?.fckCensor?.state ?: remember { MutableStateFlow<FckState>(FckState.Off) })
                .collectAsStateWithLifecycle()
            when (val s = state) {
                FckState.Off, FckState.Loading -> StatusRow("Список", "Загружаем список…")
                is FckState.Ready -> StatusRow(
                    "Список",
                    if (s.count == 0) "Список загружен, замен в нём нет" else "Замен в списке: ${s.count}",
                    "Обновить",
                ) { app?.fckCensor?.retry() }
                is FckState.Failed -> StatusRow(
                    "Список не загружен",
                    "${s.message.replaceFirstChar { it.uppercase() }}. Замены не действуют, пока список не получен",
                    "Повторить",
                ) { app?.fckCensor?.retry() }
            }
        }
    }
    SettingsNote("Названия и обложки меняются сразу. Звук заменяется со следующего трека, играющий доиграет как есть.")
}

// ----------------------------------------------------------------- Slopless

/** «1 исполнитель / 2 исполнителя / 5 исполнителей». */
private fun artistsCount(n: Int): String {
    val mod100 = n % 100
    val mod10 = n % 10
    val word = when {
        mod100 in 11..14 -> "исполнителей"
        mod10 == 1 -> "исполнитель"
        mod10 in 2..4 -> "исполнителя"
        else -> "исполнителей"
    }
    return "$n $word"
}

@Composable
private fun SloplessCard() {
    var askDislike by remember { mutableStateOf(false) }
    GroupCard {
        ToggleRow(
            title = "Slopless",
            subtitle = "Метки ИИ-исполнителей и ИИ-треков по базе slopless.art. " +
                "В сервис уходят только номера исполнителей, не ваш аккаунт",
            checked = Prefs.slopless,
            onChange = Prefs::changeSlopless,
        )
        if (Prefs.slopless) {
            ToggleRow(
                title = "Пропускать ИИ-треки",
                subtitle = "В очереди. Трек, включённый вручную, играет",
                checked = Prefs.slopAutoskip,
                onChange = Prefs::changeSlopAutoskip,
            )
            ToggleRow(
                title = "Ставить «Не нравится»",
                subtitle = "Пропущенным ИИ-трекам. Это запись в ваш аккаунт: включается после подтверждения",
                checked = Prefs.slopDislike,
                onChange = { on -> if (on) askDislike = true else Prefs.changeSlopDislike(false) },
            )
            val slop = (LocalContext.current.applicationContext as? PlastinkaApp)?.slopless
            val status by (slop?.status ?: remember { MutableStateFlow(SlopStatus()) })
                .collectAsStateWithLifecycle()
            val uri = LocalUriHandler.current
            val error = status.error
            if (error != null) {
                StatusRow(
                    "База",
                    "Не удалось получить данные: $error. Метки появятся, когда связь восстановится. " +
                        "Проверено: ${artistsCount(status.known)}",
                    "slopless.art",
                ) { uri.openUri(Slopless.SITE_URL) }
            } else {
                StatusRow("База", "Проверено: ${artistsCount(status.known)}", "slopless.art") {
                    uri.openUri(Slopless.SITE_URL)
                }
            }
        }
    }
    if (askDislike) {
        AlertDialog(
            onDismissRequest = { askDislike = false },
            containerColor = Pl.surface2,
            title = { PlText("Ставить «Не нравится» ИИ-трекам?", PlType.headline) },
            text = {
                PlText(
                    "Slopless будет отмечать пропущенные ИИ-треки как «Не нравится» в вашем аккаунте. " +
                        "Это меняет рекомендации и видно на всех устройствах. Снять отметку можно в списке " +
                        "«Не нравится» на сайте музыки.",
                    PlType.body,
                    Pl.text2,
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    askDislike = false
                    Prefs.changeSlopDislike(true)
                }) { PlText("Разрешить", PlType.secondaryStrong, Pl.accent) }
            },
            dismissButton = {
                TextButton(onClick = { askDislike = false }) {
                    PlText("Отмена", PlType.secondaryStrong, Pl.text2)
                }
            },
        )
    }
}

// ----------------------------------------------------------------- CopyTrackName

private val sampleTrack = Track(
    id = "0",
    sourceTitle = "Название",
    sourceArtists = listOf(ArtistRef(null, "Исполнитель")),
    albumId = null,
    albumTitle = "Альбом",
    durationMs = null,
    available = true,
    sourceCoverUri = null,
)

@Composable
private fun CopyNameCard() {
    GroupCard {
        ToggleRow(
            title = "CopyTrackName",
            subtitle = "Пункт «Скопировать название» в меню трека (кнопка «…») и в плеере",
            checked = Prefs.copyName,
            onChange = Prefs::changeCopyName,
        )
        if (Prefs.copyName) {
            val queue by QueueHolder.state.collectAsStateWithLifecycle()
            val example = CopyName.format(Prefs.copyFormat, queue.currentTrack() ?: sampleTrack)
            Column(
                Modifier.fillMaxWidth().padding(start = 14.dp, end = 16.dp, top = 6.dp, bottom = 12.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                PlText("Формат", PlType.secondaryStrong)
                FormatField()
                PlText(
                    "{artist} — исполнители, {title} — название, {album} — альбом. Сейчас: $example",
                    PlType.caption,
                    Pl.text3,
                )
            }
        }
    }
}

/**
 * Поле формата. Пустое — формат по умолчанию (виден как подсказка). Пока
 * пользователь печатает, значение хранится как есть; когда поле теряет фокус,
 * пустое или из одних пробелов возвращается к формату по умолчанию.
 */
@Composable
private fun FormatField() {
    val focus = LocalFocusManager.current
    val shape = CircleShape
    BasicTextField(
        value = Prefs.copyFormat,
        onValueChange = Prefs::changeCopyFormat,
        singleLine = true,
        textStyle = PlType.body.copy(color = Pl.text),
        cursorBrush = SolidColor(Pl.accent),
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { focus.clearFocus() }),
        modifier = Modifier
            .fillMaxWidth()
            .onFocusChanged { state ->
                if (!state.isFocused && Prefs.copyFormat.isBlank()) {
                    Prefs.changeCopyFormat(CopyName.DEFAULT_FORMAT)
                }
            },
        decorationBox = { inner ->
            Box(
                Modifier
                    .fillMaxWidth()
                    .height(48.dp)
                    .clip(shape)
                    .background(Pl.surface2, shape)
                    .border(1.dp, Pl.outline, shape)
                    .padding(horizontal = 18.dp),
                contentAlignment = Alignment.CenterStart,
            ) {
                if (Prefs.copyFormat.isEmpty()) PlText(CopyName.DEFAULT_FORMAT, PlType.body, Pl.text3)
                inner()
            }
        },
    )
}
