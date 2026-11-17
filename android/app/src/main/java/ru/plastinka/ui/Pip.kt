package ru.plastinka.ui

import android.app.Activity
import android.app.PendingIntent
import android.app.PictureInPictureParams
import android.app.RemoteAction
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.graphics.drawable.Icon
import android.os.Build
import android.util.Rational
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import ru.plastinka.R
import ru.plastinka.data.Track

/**
 * Мини-плеер «картинка в картинке» (аналог мини-плеера десктопа). Окно PiP
 * не принимает касаний по содержимому, поэтому кнопки — действия окна
 * ([RemoteAction]): они приходят широковещательным сообщением внутри
 * приложения и попадают в тот же [PlayerController], что и кнопки плеера.
 */
object Pip {
    const val ACTION_PREVIOUS = "ru.plastinka.pip.PREVIOUS"
    const val ACTION_TOGGLE = "ru.plastinka.pip.TOGGLE"
    const val ACTION_NEXT = "ru.plastinka.pip.NEXT"
    const val ACTION_LIKE = "ru.plastinka.pip.LIKE"

    private val _inPip = MutableStateFlow(false)

    /** Окно сейчас в режиме PiP: [ru.plastinka.MainActivity] обновляет значение. */
    val inPip: StateFlow<Boolean> = _inPip

    fun setInPip(value: Boolean) {
        _inPip.value = value
    }

    /**
     * Обработчик действий окна. Ставится главным экраном, пока есть плеер
     * (то есть после проверки подписки), и снимается при уходе с экрана.
     */
    @Volatile
    var handler: ((String) -> Unit)? = null

    /** Последнее известное состояние кнопок: нужно автовходу из `onUserLeaveHint`. */
    @Volatile
    var lastPlaying: Boolean = false

    @Volatile
    var lastLiked: Boolean = false

    /** Есть ли трек и подтверждённый Плюс: только тогда PiP можно открыть. */
    @Volatile
    var ready: Boolean = false

    /**
     * Автовход в PiP при сворачивании приложения во время игры (настройка
     * «Мини-плеер при сворачивании», по умолчанию выключена). Работает через
     * `setAutoEnterEnabled` на Android 12+ и через `onUserLeaveHint` на более
     * старых; на паузе автовхода нет.
     */
    val autoEnter: Boolean get() = ru.plastinka.data.Prefs.pipAutoEnter

    /**
     * Соотношение сторон окна. 16:9: слева квадратная обложка на всю высоту,
     * справа название и исполнитель, внизу тонкая полоса прогресса. Квадрат
     * сделал бы текст слишком мелким, а 16:9 — привычный размер PiP.
     */
    private val aspect = Rational(16, 9)

    fun isSupported(context: Context): Boolean =
        context.packageManager.hasSystemFeature(PackageManager.FEATURE_PICTURE_IN_PICTURE)

    fun intentFilter(): IntentFilter = IntentFilter().apply {
        addAction(ACTION_PREVIOUS)
        addAction(ACTION_TOGGLE)
        addAction(ACTION_NEXT)
        addAction(ACTION_LIKE)
    }

    private fun action(context: Context, code: Int, name: String, icon: Int, title: String): RemoteAction {
        // Явный пакет: сообщение уходит только в это приложение.
        val intent = Intent(name).setPackage(context.packageName)
        val pending = PendingIntent.getBroadcast(
            context,
            code,
            intent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return RemoteAction(Icon.createWithResource(context, icon), title, title, pending)
    }

    /**
     * Параметры окна и его кнопки. Системе нужно не больше
     * `maxNumPictureInPictureActions` действий (обычно три): при нехватке места
     * первым уходит «Нравится», затем «Предыдущий» и «Следующий»; play/pause
     * остаётся всегда.
     */
    fun params(activity: Activity, playing: Boolean, liked: Boolean): PictureInPictureParams {
        lastPlaying = playing
        lastLiked = liked
        val all = listOf(
            "prev" to action(activity, 1, ACTION_PREVIOUS, R.drawable.ic_previous_filled, "Предыдущий"),
            "toggle" to action(
                activity,
                2,
                ACTION_TOGGLE,
                if (playing) R.drawable.ic_pause_filled else R.drawable.ic_play_filled,
                if (playing) "Пауза" else "Играть",
            ),
            "next" to action(activity, 3, ACTION_NEXT, R.drawable.ic_next_filled, "Следующий"),
            "like" to action(
                activity,
                4,
                ACTION_LIKE,
                if (liked) R.drawable.ic_heart_filled else R.drawable.ic_heart,
                if (liked) "Убрать из «Мне нравится»" else "Нравится",
            ),
        )
        val max = activity.maxNumPictureInPictureActions
        var kept = all
        for (key in listOf("like", "prev", "next")) {
            if (kept.size <= max) break
            kept = kept.filter { it.first != key }
        }
        val builder = PictureInPictureParams.Builder()
            .setAspectRatio(aspect)
            .setActions(kept.take(maxOf(max, 0)).map { it.second })
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setAutoEnterEnabled(autoEnter && ready && playing)
        }
        return builder.build()
    }

    /** Открыть окно PiP; `false`, если нельзя (нет трека, не поддерживается, запрещено системой). */
    fun enter(activity: Activity, playing: Boolean, liked: Boolean): Boolean {
        if (!ready || !isSupported(activity)) return false
        return try {
            activity.enterPictureInPictureMode(params(activity, playing, liked))
        } catch (_: Exception) {
            false
        }
    }

    /** Обновить кнопки уже открытого (или готового к автовходу) окна. */
    fun update(activity: Activity, playing: Boolean, liked: Boolean) {
        if (!isSupported(activity)) return
        try {
            activity.setPictureInPictureParams(params(activity, playing, liked))
        } catch (_: Exception) {
            // Окно могло закрыться между проверкой и вызовом: обновлять нечего.
        }
    }

    /** Закрыть окно PiP вместе с приложением (выход из аккаунта, пропала подписка). */
    fun close(activity: Activity) {
        activity.finishAndRemoveTask()
    }
}

/** Содержимое окна PiP: обложка, название, исполнитель, полоса прогресса на приглушённом фоне. */
@Composable
fun PipScreen(track: Track, player: PlayerController?) {
    val progress by rememberProgress(player, track)
    val average by rememberCoverColor(track.coverUrl("600x600"))
    // Фон как у мини-плеера десктопа: средний цвет обложки, насыщенность до 20 %, яркость 18 %.
    val target = average?.let { Color(0xFF000000.toInt() or ColorMath.muted(it)) } ?: Pl.surface2
    val background by animateColorAsState(target, tween(300), label = "pipBackground")
    Box(Modifier.fillMaxSize().background(background)) {
        BoxWithConstraints(Modifier.fillMaxSize()) {
            val side = maxHeight
            Row(Modifier.fillMaxSize(), verticalAlignment = Alignment.CenterVertically) {
                Cover(track.coverUrl("600x600"), track.id, side, 0.dp)
                Column(
                    Modifier.weight(1f).fillMaxHeight().padding(horizontal = 10.dp),
                    verticalArrangement = Arrangement.Center,
                ) {
                    PlText(track.title, PlType.secondaryStrong, maxLines = 2)
                    PlText(track.artistNames, PlType.caption, Pl.text2, maxLines = 1)
                }
            }
        }
        Box(
            Modifier
                .align(Alignment.BottomStart)
                .fillMaxWidth()
                .height(3.dp)
                .background(Color.White.copy(alpha = 0.12f)),
        ) {
            Box(Modifier.fillMaxWidth(progress.fraction).fillMaxHeight().background(Pl.accent))
        }
    }
}
