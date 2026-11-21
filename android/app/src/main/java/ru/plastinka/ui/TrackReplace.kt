package ru.plastinka.ui

import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import ru.plastinka.PlastinkaApp
import ru.plastinka.data.AUDIO_MIME_TYPES
import ru.plastinka.data.ReplaceOutcome
import ru.plastinka.data.SearchKind
import ru.plastinka.data.Track
import ru.plastinka.data.TrackLink

/** Сколько результатов показывать в «Заменить на…» (как на десктопе). */
private const val REPLACE_RESULTS = 8

/** Что показано в «Заменить на…» под полем. */
private sealed interface ReplaceFound {
    data object Idle : ReplaceFound
    data object Loading : ReplaceFound
    data class Failed(val text: String) : ReplaceFound
    data class Ready(val tracks: List<Track>) : ReplaceFound
}

private fun toast(context: android.content.Context, outcome: ReplaceOutcome) {
    Toast.makeText(context.applicationContext, outcome.message, Toast.LENGTH_LONG).show()
}

/** Метка «Замена» в строке трека: у трека ручная замена FckCensor. Вид как у [AiTag]. */
@Composable
fun ReplacedTag(modifier: Modifier = Modifier, color: Color = Pl.accent) {
    val shape = RoundedCornerShape(4.dp)
    Box(
        modifier
            .height(16.dp)
            .border(1.dp, color.copy(alpha = 0.6f), shape)
            .padding(horizontal = 5.dp),
        contentAlignment = Alignment.Center,
    ) {
        PlText("Замена", PlType.micro, color, maxLines = 1)
    }
}

/**
 * «Свой файл…»: открывает системный выбор файла один раз и сообщает, когда всё закончено
 * ([onFinished]). Выбранный файл не копируется: сохраняется адрес и постоянное разрешение на
 * чтение. Композиция держит обработчик результата, пока выбор открыт.
 */
@Composable
fun FilePickHost(trackId: String, onFinished: () -> Unit) {
    val context = LocalContext.current
    val fck = (context.applicationContext as? PlastinkaApp)?.fckCensor
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri == null || fck == null) {
            onFinished()
        } else {
            fck.replaceAsync({ fck.replaceWithFile(trackId, uri) }) { outcome ->
                toast(context, outcome)
                onFinished()
            }
        }
    }
    LaunchedEffect(Unit) {
        try {
            launcher.launch(AUDIO_MIME_TYPES)
        } catch (_: Exception) {
            Toast.makeText(context.applicationContext, "На устройстве нет приложения для выбора файлов", Toast.LENGTH_LONG).show()
            onFinished()
        }
    }
}

/**
 * «Заменить на…»: нижний лист с поиском трека Яндекса или ссылкой на него
 * (`music.yandex.ru/album/…/track/…`). Выбор сохраняет замену и закрывает лист.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ReplaceSheet(track: Track, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val app = context.applicationContext as? PlastinkaApp
    var query by remember { mutableStateOf("${track.artistNames} ${track.displayTitle}".trim()) }
    var found by remember { mutableStateOf<ReplaceFound>(ReplaceFound.Idle) }

    LaunchedEffect(query) {
        val text = query.trim()
        if (text.isEmpty()) {
            found = ReplaceFound.Idle
            return@LaunchedEffect
        }
        // Пока печатают, не спрашиваем на каждую букву.
        delay(350)
        found = ReplaceFound.Loading
        val token = app?.tokenStore?.load()
        if (app == null || token == null) {
            found = ReplaceFound.Failed("Нет входа в аккаунт")
            return@LaunchedEffect
        }
        found = try {
            val link = TrackLink.trackId(text)
            val tracks = if (link != null) {
                app.api.tracks(token, listOf(link), placeholders = false)
            } else {
                app.api.search(token, SearchKind.Track, text).tracks
            }
            ReplaceFound.Ready(tracks.take(REPLACE_RESULTS))
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            ReplaceFound.Failed("Не удалось выполнить поиск. Проверьте соединение")
        }
    }

    PlBottomSheet(onDismiss) {
        Column(Modifier.fillMaxWidth().imePadding().navigationBarsPadding().padding(bottom = 8.dp)) {
            Column(
                Modifier.padding(start = 20.dp, end = 20.dp, bottom = 14.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                PlText("Заменить звук", PlType.title)
                PlText(
                    "Вместо «${track.displayTitle}» будет играть выбранный трек Яндекса. " +
                        "Замена действует везде, в том числе в очереди.",
                    PlType.secondary,
                    Pl.text2,
                )
            }
            SearchField(query, { query = it }, "Название трека или ссылка на Яндекс Музыку")
            Spacer(Modifier.height(8.dp))
            Box(Modifier.fillMaxWidth().heightIn(min = 120.dp, max = 420.dp)) {
                when (val state = found) {
                    ReplaceFound.Idle -> ReplaceNote("Найдите трек на Яндексе или вставьте ссылку на него.")
                    ReplaceFound.Loading -> ReplaceNote("Ищем…")
                    is ReplaceFound.Failed -> ReplaceNote(state.text)
                    is ReplaceFound.Ready -> if (state.tracks.isEmpty()) {
                        ReplaceNote("Ничего не нашлось.")
                    } else {
                        Column(Modifier.verticalScroll(rememberScrollState())) {
                            for (item in state.tracks) {
                                ReplaceRow(item, original = track.id) {
                                    val fck = app?.fckCensor
                                    if (fck != null) {
                                        fck.replaceAsync({ fck.replaceWithTrack(track.id, item) }) { outcome ->
                                            toast(context, outcome)
                                        }
                                    }
                                    onDismiss()
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun ReplaceNote(text: String) {
    Box(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 24.dp), contentAlignment = Alignment.TopCenter) {
        PlText(text, PlType.secondary, Pl.text3)
    }
}

/** Строка результата. Сам трек и недоступные не выбрать. */
@Composable
private fun ReplaceRow(item: Track, original: String, onPick: () -> Unit) {
    val usable = item.available && item.id != original
    Row(
        Modifier
            .fillMaxWidth()
            .height(60.dp)
            .then(if (usable) Modifier.clickable(onClick = onPick) else Modifier)
            .padding(horizontal = 20.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Cover(item.coverUrl("100x100"), item.id, 44.dp, 8.dp, if (usable) Modifier else Modifier.alpha(0.4f))
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                if (item.explicit) ExplicitBadge()
                PlText(
                    item.displayTitle,
                    PlType.body,
                    if (usable) Pl.text else Pl.textDisabled,
                    maxLines = 1,
                    modifier = Modifier.weight(1f, fill = false),
                )
            }
            PlText(item.artistNames, PlType.secondary, if (usable) Pl.text2 else Pl.textDisabled, maxLines = 1)
        }
        item.durationMs?.let {
            Spacer(Modifier.width(12.dp))
            PlText(formatDuration(it), PlType.secondary.copy(fontFeatureSettings = "tnum"), Pl.text3)
        }
    }
}
