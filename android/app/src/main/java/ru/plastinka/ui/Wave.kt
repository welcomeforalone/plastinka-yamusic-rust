package ru.plastinka.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.media3.common.Player
import ru.plastinka.R
import ru.plastinka.data.HistoryContext
import ru.plastinka.playback.QueueHolder
import ru.plastinka.playback.currentTrack

/** Вкладка «Волна»: запуск, подборки и экран играющей волны. */
@Composable
fun WaveTab(
    model: AppViewModel,
    player: PlayerController?,
    onOpenPlayer: () -> Unit,
    onOpenSettings: () -> Unit,
) {
    val queue by QueueHolder.state.collectAsStateWithLifecycle()
    val starting by model.waveStarting.collectAsStateWithLifecycle()
    val error by model.waveError.collectAsStateWithLifecycle()
    if (waveScreenShowsTrack(queue, starting, error != null)) {
        WavePlaying(queue, player, model, onOpenPlayer, onOpenSettings, foreign = !queue.isWave)
    } else {
        WaveIdle(model, player, onOpenSettings)
    }
}

/**
 * Показывает ли вкладка «Волна» играющий трек (волну или чужую очередь), а не стартовый вид.
 * Как `screen_for` на десктопе: играет волна — да; не волна — только когда волна не запускается
 * и не в ошибке (пока она грузится, остаётся стартовый вид). Общее условие для `WaveTab` и мини-плеера.
 */
internal fun waveScreenShowsTrack(queue: QueueHolder.State, starting: Boolean, hasError: Boolean): Boolean =
    queue.currentTrack() != null && (queue.isWave || (!starting && !hasError))

/**
 * Подпись источника чужой очереди (как `source_text` на десктопе). Названия берём из контекста
 * очереди; у остальных источников (поиск, «Топ месяца», подборки, история) контекст — `Other` с
 * названием, его и показываем, пустое — «Очередь».
 */
internal fun queueSourceLabel(queue: QueueHolder.State): String = when (val c = queue.context) {
    HistoryContext.Liked -> "Мне нравится"
    is HistoryContext.Album -> "Альбом «${c.title}»"
    is HistoryContext.Playlist -> "Плейлист «${c.title}»"
    is HistoryContext.Artist -> "Исполнитель ${c.name}"
    HistoryContext.Wave -> "Моя волна"
    is HistoryContext.Other -> c.title.ifBlank { "Очередь" }
}

/** Заголовок «Моя волна» и аватар справа (макеты 03 и 04): по центру одной строки, поля 20. */
@Composable
private fun WaveHeader(onOpenSettings: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 20.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        PlText("Моя волна", PlType.display, modifier = Modifier.weight(1f))
        Avatar(36.dp, Modifier.clickable(onClick = onOpenSettings))
    }
}

@Composable
private fun WaveIdle(model: AppViewModel, player: PlayerController?, onOpenSettings: () -> Unit) {
    val starting by model.waveStarting.collectAsStateWithLifecycle()
    val error by model.waveError.collectAsStateWithLifecycle()

    Box(Modifier.fillMaxSize()) {
        // Два свечения макета 03: акцент (круг 360, центр на кольцах) и фиолетовое справа сверху.
        // Контейнер лежит под строкой состояния (40 dp в макете), поэтому отсчёт от неё.
        Glow(360.dp, Modifier.align(Alignment.TopCenter).offset(y = 90.dp), alpha = 0.5f)
        Glow(
            240.dp,
            Modifier.align(Alignment.TopCenter).offset(x = 104.dp, y = 20.dp),
            color = Color(0xFF9D2DFA),
            alpha = 0.3f,
        )
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(top = 12.dp, bottom = 16.dp)) {
            WaveHeader(onOpenSettings)
            // Кольца: верх первого на 92 dp ниже заголовка (макет 03).
            Spacer(Modifier.height(92.dp))
            Box(Modifier.fillMaxWidth().height(272.dp), contentAlignment = Alignment.Center) {
                Canvas(Modifier.size(272.dp)) {
                    val stroke = Stroke(1.dp.toPx())
                    drawCircle(Color.White.copy(alpha = 0.06f), radius = 136.dp.toPx(), style = stroke)
                    drawCircle(Color.White.copy(alpha = 0.10f), radius = 104.dp.toPx(), style = stroke)
                }
                Box(Modifier.size(152.dp).background(Pl.accent.copy(alpha = 0.22f), CircleShape))
                Box(
                    Modifier
                        .size(100.dp)
                        .shadow(
                            16.dp,
                            CircleShape,
                            clip = false,
                            ambientColor = Color.Transparent,
                            spotColor = Pl.accent.copy(alpha = 0.6f),
                        )
                        .clip(CircleShape)
                        .background(Pl.accent, CircleShape)
                        .clickable(enabled = !starting) { model.startWave(player) },
                    contentAlignment = Alignment.Center,
                ) {
                    if (starting) {
                        CircularProgressIndicator(
                            Modifier.size(36.dp),
                            color = Pl.onAccent,
                            strokeWidth = 3.dp,
                        )
                    } else {
                        PlIcon(R.drawable.ic_play_filled, 42.dp, Pl.onAccent)
                    }
                }
            }
            Spacer(Modifier.height(24.dp))
            PlText(
                "Бесконечный поток музыки под ваш вкус",
                PlType.secondary,
                Pl.text2,
                Modifier.fillMaxWidth().padding(horizontal = 24.dp),
                textAlign = TextAlign.Center,
            )
            error?.let {
                Spacer(Modifier.height(12.dp))
                PlText(
                    it,
                    PlType.secondary,
                    Pl.error,
                    Modifier.fillMaxWidth().padding(horizontal = 24.dp),
                    textAlign = TextAlign.Center,
                )
            }
            // Кнопка «Моя волна» (макет 03): на 16 dp ниже подписи.
            Spacer(Modifier.height(16.dp))
            WaveSettingsButton(model, player, filled = false)
        }
    }
}

@Composable
private fun WavePlaying(
    queue: QueueHolder.State,
    player: PlayerController?,
    model: AppViewModel,
    onOpenPlayer: () -> Unit,
    onOpenSettings: () -> Unit,
    // Играет не волна: как на десктопе — вместо «Не нравится» неактивный значок, перемешивание,
    // обычный цикл повтора, источник очереди и крестик «запустить волну».
    foreign: Boolean = false,
) {
    val track = queue.currentTrack() ?: return
    val tint = rememberTint(queue, track)
    val playing = player?.isPlaying?.collectAsStateWithLifecycle()?.value ?: false
    val liked by QueueHolder.likedIds.collectAsStateWithLifecycle()
    val progress by rememberProgress(player, track)
    var preview by remember { mutableStateOf<Float?>(null) }
    val next = queue.tracks.getOrNull(queue.current + 1)
    // Повтор и перемешивание — режимы плеера: те же, что у кнопок в полноэкранном плеере.
    val repeat = player?.repeatMode?.collectAsStateWithLifecycle()?.value ?: Player.REPEAT_MODE_OFF
    val shuffle = player?.shuffle?.collectAsStateWithLifecycle()?.value ?: false

    BoxWithConstraints(Modifier.fillMaxSize()) {
        // Свечения макета 04: цвет трека (круг 420, центр в (150, 170)) и розовое справа.
        // Отсчёт от низа строки состояния (40 dp в макете).
        Glow(
            420.dp,
            Modifier.align(Alignment.TopStart).offset(x = (-60).dp, y = (-80).dp),
            color = tint,
            alpha = 0.55f,
        )
        Glow(
            300.dp,
            Modifier.align(Alignment.TopStart).offset(x = 180.dp, y = 160.dp),
            color = Color(0xFFF48FB1),
            alpha = 0.3f,
        )
        // Первая «страница» (всё до подборок) не ниже видимой области: спектр стоит у её низа.
        // В Column внутри прокрутки высота распорки считается от minHeight, а не от бесконечного максимума.
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
            Column(Modifier.heightIn(min = this@BoxWithConstraints.maxHeight).padding(top = 12.dp)) {
                WaveHeader(onOpenSettings)
                // Рамка обложки 2 dp снаружи: верх обложки на 30 dp ниже заголовка (макет: 90 → 120).
                Spacer(Modifier.height(28.dp))
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                    // Обложка 232 с рамкой 2 dp снаружи (цвет трека, 70 %) и тенью цвета трека.
                    Box(
                        Modifier
                            .size(236.dp)
                            .shadow(
                                24.dp,
                                RoundedCornerShape(26.dp),
                                clip = false,
                                ambientColor = Color.Transparent,
                                spotColor = tint.copy(alpha = 0.5f),
                            )
                            .border(2.dp, tint.copy(alpha = 0.7f), RoundedCornerShape(26.dp))
                            .padding(2.dp)
                            .clip(RoundedCornerShape(24.dp))
                            .clickable(onClick = onOpenPlayer),
                    ) {
                        Cover(track.coverUrl("600x600"), track.id, 232.dp, 24.dp)
                    }
                }
                Spacer(Modifier.height(24.dp))
                PlText(
                    track.title,
                    PlType.title,
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp),
                    textAlign = TextAlign.Center,
                    maxLines = 2,
                )
                Spacer(Modifier.height(4.dp))
                PlText(
                    track.artistNames,
                    PlType.body,
                    Pl.text2,
                    Modifier.fillMaxWidth().padding(horizontal = 24.dp),
                    textAlign = TextAlign.Center,
                    maxLines = 1,
                )
                Spacer(Modifier.height(12.dp))
                Column(Modifier.padding(horizontal = 32.dp)) {
                    // VibeColorizer красит фон, кольцо и ореол; перемотка и кнопки — акцент.
                    SeekBar(
                        fraction = progress.fraction,
                        color = Pl.accent,
                        onSeek = { f ->
                            if (progress.durationMs > 0) player?.seekTo((f * progress.durationMs).toLong())
                        },
                        onPreview = { preview = it },
                    )
                    val shown = preview?.let { (it * progress.durationMs).toLong() } ?: progress.positionMs
                    // Время на 20 dp ниже верха ползунка (макет), поэтому поднято на 4 dp.
                    TimeRow(shown, progress.durationMs, Pl.text2, Modifier.offset(y = (-4).dp))
                }
                Spacer(Modifier.height(12.dp))
                // Ряд 3 + Play + 3 в обоих режимах, половины зеркальны: «Не нравится», перемешивание,
                // «Назад», Play, «Вперёд», повтор, «Нравится». Половины равной ширины (weight 1): Play
                // точно по центру. Зоны нажатия 40 dp; при 360 dp ряд шириной 344 dp: половина 134 dp,
                // кнопки 3 × 40 + зазор у Play 6 dp, остаётся 8 dp на два промежутка (по 4 dp).
                Row(
                    Modifier.fillMaxWidth().padding(horizontal = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                        Box(
                            Modifier
                                .size(40.dp)
                                .clip(CircleShape)
                                .then(if (foreign) Modifier else Modifier.clickable { model.dislikeWaveTrack(player) }),
                            Alignment.Center,
                        ) {
                            // Вне волны «Не нравится» не работает, как в плеере на десктопе.
                            PlIcon(R.drawable.ic_dislike, 26.dp, if (foreign) Pl.textDisabled else Pl.text2)
                        }
                        Spacer(Modifier.weight(1f))
                        // В волне перемешивания нет: значок приглушён и не нажимается (как на десктопе).
                        Box(
                            Modifier
                                .size(40.dp)
                                .clip(CircleShape)
                                .then(if (foreign) Modifier.clickable { player?.toggleShuffle() } else Modifier),
                            Alignment.Center,
                        ) {
                            PlIcon(
                                R.drawable.ic_shuffle,
                                24.dp,
                                when {
                                    !foreign -> Pl.textDisabled
                                    shuffle -> Pl.accent
                                    else -> Pl.text2
                                },
                            )
                        }
                        Spacer(Modifier.weight(1f))
                        Box(Modifier.size(40.dp).clip(CircleShape).clickable { player?.previous() }, Alignment.Center) {
                            PlIcon(R.drawable.ic_previous_filled, 32.dp, Pl.text)
                        }
                        Spacer(Modifier.width(6.dp))
                    }
                    PlayButton(76.dp, playing, onClick = { player?.toggle() })
                    Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                        Spacer(Modifier.width(6.dp))
                        Box(Modifier.size(40.dp).clip(CircleShape).clickable { player?.next() }, Alignment.Center) {
                            PlIcon(R.drawable.ic_next_filled, 32.dp, Pl.text)
                        }
                        Spacer(Modifier.weight(1f))
                        // Повтор: значок без подписи, как в полноэкранном плеере. В волне «выкл ↔ трек»,
                        // у чужой очереди обычный цикл «выкл → очередь → трек».
                        Box(
                            Modifier
                                .size(40.dp)
                                .clip(CircleShape)
                                .clickable { if (foreign) player?.cycleRepeat() else player?.toggleWaveRepeat() },
                            Alignment.Center,
                        ) {
                            PlIcon(
                                if (repeat == Player.REPEAT_MODE_ONE) R.drawable.ic_repeat_one else R.drawable.ic_repeat,
                                24.dp,
                                if (repeat != Player.REPEAT_MODE_OFF) Pl.accent else Pl.text2,
                            )
                        }
                        Spacer(Modifier.weight(1f))
                        HeartButton(liked.contains(track.id), { model.toggleLike(track) }, 26.dp, Modifier.size(40.dp))
                    }
                }
                // «Далее»: место под строку занято всегда, чтобы кнопка не прыгала, когда придёт порция.
                Spacer(Modifier.height(24.dp))
                Box(Modifier.fillMaxWidth().height(16.dp), contentAlignment = Alignment.Center) {
                    // У чужой очереди строки «Далее» нет (как на десктопе): порядок зависит от перемешивания.
                    next?.takeIf { !foreign }?.let {
                        PlText(
                            "Далее: ${it.title} — ${it.artistNames}",
                            PlType.caption,
                            Pl.text3,
                            Modifier.fillMaxWidth().padding(horizontal = 24.dp),
                            textAlign = TextAlign.Center,
                            maxLines = 1,
                        )
                    }
                }
                // Кнопка «Моя волна» (макет 04: 640). Чипа «Повторять трек» из макета нет: повтор
                // перенесён в ряд кнопок по решению пользователя.
                Spacer(Modifier.height(20.dp))
                // Чужая очередь: вместо настроек — источник и крестик, что запускает волну.
                WaveSettingsButton(
                    model,
                    player,
                    filled = true,
                    label = if (foreign) queueSourceLabel(queue) else null,
                    onClose = if (foreign) ({ model.startWave(player) }) else null,
                )
                // Не меньше 12 dp до спектра; лишняя высота первой страницы уходит в распорку.
                Spacer(Modifier.height(12.dp))
                Spacer(Modifier.weight(1f))
                // Спектр под плеером у нижнего края первой страницы: высота 64, цвет волны 35 % (макет 04).
                SpectrumBars(
                    tint.copy(alpha = 0.35f),
                    Modifier.fillMaxWidth().height(64.dp),
                    barWidth = 5.dp,
                    gap = 4.dp,
                )
            }
        }
    }
}
