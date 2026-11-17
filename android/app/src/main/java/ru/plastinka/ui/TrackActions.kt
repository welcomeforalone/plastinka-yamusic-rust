package ru.plastinka.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import android.widget.Toast
import androidx.annotation.DrawableRes
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import ru.plastinka.PlastinkaApp
import ru.plastinka.R
import ru.plastinka.data.CopyName
import ru.plastinka.data.FckCensor
import ru.plastinka.data.ReplaceOutcome
import ru.plastinka.data.Prefs
import ru.plastinka.data.Track

/** Что действие меню может сделать: контекст, открыть лист замены, выбор файла, убрать замену. */
class TrackActionHost(
    val context: Context,
    val openReplace: () -> Unit,
    val pickFile: () -> Unit,
    val clearReplacement: () -> Unit,
)

/** Одно действие меню трека. */
class TrackAction(
    @DrawableRes val icon: Int,
    val title: String,
    val run: (TrackActionHost) -> Unit,
)

/**
 * Действия меню трека. Новые действия добавляются здесь, в одном месте:
 * меню и кнопка «…» появляются, только если список не пуст (пустых меню нет).
 * Читает настройки плагинов, поэтому при смене переключателя интерфейс
 * перерисовывается сам.
 */
fun trackActions(track: Track): List<TrackAction> = buildList {
    if (Prefs.copyName) {
        add(TrackAction(R.drawable.ic_copy, "Скопировать название") { host -> copyTrackName(host.context, track) })
    }
    // FckCensor: ручные замены звука (как в меню трека на десктопе).
    if (Prefs.fckCensor) {
        add(TrackAction(R.drawable.ic_search, "Заменить на…") { host -> host.openReplace() })
        add(TrackAction(R.drawable.ic_plus, "Свой файл…") { host -> host.pickFile() })
        FckCensor.current?.replacementLabel(track.id)?.let { label ->
            add(TrackAction(R.drawable.ic_close, "Убрать замену ($label)") { host -> host.clearReplacement() })
        }
    }
}

/** CopyTrackName: «Исполнитель — Название» (или свой формат) в буфер обмена. */
private fun copyTrackName(context: Context, track: Track) {
    val text = CopyName.format(Prefs.copyFormat, track)
    val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager ?: return
    try {
        clipboard.setPrimaryClip(ClipData.newPlainText("Название трека", text))
    } catch (_: Exception) {
        Toast.makeText(context, "Не удалось скопировать", Toast.LENGTH_SHORT).show()
        return
    }
    // Начиная с Android 13 система сама показывает подсказку о копировании.
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
        Toast.makeText(context, "Скопировано", Toast.LENGTH_SHORT).show()
    }
}

/**
 * Кнопка «…» с нижним листом действий трека. Если действий нет, не рисуется
 * вовсе (и места не занимает).
 */
@Composable
fun TrackMoreButton(
    track: Track,
    modifier: Modifier = Modifier,
    iconSize: Dp = 22.dp,
    tint: Color = Pl.text2,
) {
    val actions = trackActions(track)
    if (actions.isEmpty()) return
    var open by remember { mutableStateOf(false) }
    var replacing by remember { mutableStateOf(false) }
    var picking by remember { mutableStateOf(false) }
    val context = LocalContext.current
    val host = remember(track.id, context) {
        TrackActionHost(
            context = context,
            openReplace = { replacing = true },
            pickFile = { picking = true },
            clearReplacement = {
                val fck = (context.applicationContext as? PlastinkaApp)?.fckCensor
                fck?.replaceAsync({ fck.clearReplacement(track.id) }) { outcome: ReplaceOutcome ->
                    Toast.makeText(context.applicationContext, outcome.message, Toast.LENGTH_LONG).show()
                }
            },
        )
    }
    Box(
        modifier
            .size(40.dp)
            .clip(RoundedCornerShape(20.dp))
            .clickable { open = true },
        contentAlignment = Alignment.Center,
    ) {
        PlIcon(R.drawable.ic_more, iconSize, tint)
    }
    if (open) {
        TrackMenuSheet(track, actions, host, onDismiss = { open = false })
    }
    if (replacing) ReplaceSheet(track, onDismiss = { replacing = false })
    if (picking) FilePickHost(track.id, onFinished = { picking = false })
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun TrackMenuSheet(track: Track, actions: List<TrackAction>, host: TrackActionHost, onDismiss: () -> Unit) {
    PlBottomSheet(onDismiss) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().padding(bottom = 8.dp)) {
            Column(
                Modifier.padding(start = 20.dp, end = 20.dp, bottom = 12.dp),
                verticalArrangement = Arrangement.spacedBy(2.dp),
            ) {
                PlText(track.title, PlType.headline, maxLines = 1)
                PlText(track.artistNames, PlType.secondary, Pl.text2, maxLines = 1)
            }
            for (action in actions) {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .height(56.dp)
                        .clickable {
                            action.run(host)
                            onDismiss()
                        }
                        .padding(horizontal = 20.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    PlIcon(action.icon, 22.dp, Pl.text)
                    Spacer(Modifier.width(16.dp))
                    PlText(action.title, PlType.body)
                }
            }
        }
    }
}

// ----------------------------------------------------------------- Slopless

/**
 * Трек отмечен как ИИ (Slopless включён и данные об исполнителях получены).
 * Видимой строке нужны данные об исполнителях: запрос ставится в очередь при
 * показе. Выключенный плагин ничего не спрашивает и не помечает.
 */
@Composable
fun rememberIsAi(track: Track): Boolean {
    if (!Prefs.slopless) return false
    val slop = (LocalContext.current.applicationContext as? PlastinkaApp)?.slopless ?: return false
    val version by slop.version.collectAsStateWithLifecycle()
    LaunchedEffect(track.id) { slop.wantTracks(listOf(track)) }
    return version >= 0 && slop.isAi(track)
}

/** Компактная метка «ИИ». */
@Composable
fun AiTag(modifier: Modifier = Modifier, color: Color = Pl.text2) {
    val shape = RoundedCornerShape(4.dp)
    Box(
        modifier
            .height(16.dp)
            .border(1.dp, color.copy(alpha = 0.6f), shape)
            .padding(horizontal = 5.dp),
        contentAlignment = Alignment.Center,
    ) {
        PlText("ИИ", PlType.micro, color, maxLines = 1)
    }
}
