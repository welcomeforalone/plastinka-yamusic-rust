package ru.plastinka.ui

import androidx.annotation.DrawableRes
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.EnterExitState
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import ru.plastinka.R
import ru.plastinka.data.Prefs
import ru.plastinka.playback.QueueHolder
import ru.plastinka.playback.currentTrack

/**
 * Корневой экран. Порядок строгий: вход, затем ворота подписки (правило 2),
 * и только после них музыка и плеер.
 */
@Composable
fun RootScreen(
    loggedIn: Boolean,
    login: LoginUi,
    library: LibraryUi,
    avatarUrl: String?,
    model: AppViewModel,
) {
    // Выход из аккаунта или пропавший Плюс закрывают окно PiP: показывать в нём нечего.
    val context = LocalContext.current
    val inPip by Pip.inPip.collectAsStateWithLifecycle()
    val allowed = loggedIn && library.account?.hasPlus == true
    LaunchedEffect(inPip, allowed) {
        if (inPip && !allowed) (context as? android.app.Activity)?.let(Pip::close)
    }
    if (!loggedIn) {
        LoginScreen(login, onLogin = model::startLogin)
        return
    }

    val account = library.account
    when {
        account != null && account.hasPlus -> {
            // Аватар и буква-запас во всех местах основного интерфейса.
            val letter = account.displayName.firstOrNull()?.uppercase().orEmpty()
            androidx.compose.runtime.CompositionLocalProvider(
                LocalAvatar provides AvatarInfo(avatarUrl, letter),
            ) { MainScreen(library, model) }
        }
        account != null -> SubscriptionRequiredScreen(
            account = account,
            checking = library.loading,
            onRecheck = model::loadLibrary,
            onLogout = model::logout,
        )
        // Ошибка запроса статуса не считается подпиской.
        library.error != null -> SubscriptionErrorScreen(
            error = library.error,
            onRetry = model::loadLibrary,
            onLogout = model::logout,
        )
        else -> SubscriptionCheckingScreen()
    }
}

/** Основной интерфейс: создаётся только при подтверждённом Плюсе. */
@Composable
private fun MainScreen(library: LibraryUi, model: AppViewModel) {
    val context = LocalContext.current
    var player by remember { mutableStateOf<PlayerController?>(null) }
    // Плеер строится только здесь, а при уходе с экрана (выход, ошибка
    // проверки) воспроизведение останавливается и плеер освобождается.
    val alive = remember { booleanArrayOf(true) }
    LaunchedEffect(Unit) {
        PlayerController.build(context) { built ->
            if (alive[0]) {
                player = built
            } else {
                built?.stopAndRelease()
            }
        }
    }
    val currentPlayer by rememberUpdatedState(player)
    androidx.compose.runtime.DisposableEffect(Unit) {
        onDispose {
            alive[0] = false
            // Активность закрывается или пересоздаётся (поворот, «назад», система
            // забрала память): музыка играет дальше в сервисе, отпускаем только
            // контроллер. Останавливаем звук, лишь когда закрылись ворота при
            // живой активности: выход из аккаунта или пропавшая подписка.
            val activity = context as? android.app.Activity
            val leavingApp = activity == null || activity.isChangingConfigurations ||
                activity.isFinishing || activity.isDestroyed
            if (leavingApp) currentPlayer?.release() else currentPlayer?.stopAndRelease()
        }
    }

    val queue by QueueHolder.state.collectAsStateWithLifecycle()
    var showPlayer by remember { mutableStateOf(false) }
    val hasTrack = queue.currentTrack() != null
    // Очередь опустела: полноэкранный плеер закрывается, а не остаётся пустым слоем.
    LaunchedEffect(hasTrack) { if (!hasTrack) showPlayer = false }
    var tab by rememberSaveable { mutableIntStateOf(0) }
    val liked by QueueHolder.likedIds.collectAsStateWithLifecycle()
    val playing = player?.isPlaying?.collectAsStateWithLifecycle()?.value ?: false
    val likedNow = queue.currentTrack()?.let { liked.contains(it.id) } ?: false

    // Мини-плеер PiP: только с треком (а значит, после проверки Плюса).
    val activity = context as? android.app.Activity
    val pipSupported = activity != null && Pip.isSupported(context)
    androidx.compose.runtime.DisposableEffect(hasTrack) {
        Pip.ready = hasTrack
        onDispose { Pip.ready = false }
    }
    // Кнопки окна идут в тот же плеер и те же действия, что и кнопки приложения;
    // «Следующий» в волне — тот же пропуск, что в нижнем плеере.
    androidx.compose.runtime.DisposableEffect(player) {
        Pip.handler = { action ->
            when (action) {
                Pip.ACTION_PREVIOUS -> player?.previous()
                Pip.ACTION_TOGGLE -> player?.toggle()
                Pip.ACTION_NEXT -> player?.next()
                Pip.ACTION_LIKE -> QueueHolder.state.value.currentTrack()?.let(model::toggleLike)
            }
        }
        onDispose { Pip.handler = null }
    }
    // Кнопки окна и параметры обновляются при смене трека, паузе и лайке.
    LaunchedEffect(queue.currentTrack()?.id, playing, likedNow, hasTrack, Prefs.pipAutoEnter) {
        if (activity != null && hasTrack) Pip.update(activity, playing, likedNow)
    }

    Box(Modifier.fillMaxSize().background(Pl.bg)) {
        Column(Modifier.fillMaxSize()) {
            Box(Modifier.weight(1f).fillMaxWidth().statusBarsPadding()) {
                when (tab) {
                    0 -> WaveTab(
                        model,
                        player,
                        onOpenPlayer = { showPlayer = true },
                        onOpenSettings = { tab = 3 },
                    )
                    1 -> SearchTab(model, player)
                    2 -> CollectionTab(library, model, player, onOpenSettings = { tab = 3 })
                    else -> SettingsScreen(library, model)
                }
            }
            // Мини-плеер над нижним меню. Под полноэкранным плеером его не прячем:
            // плеер закрывает всё окно, а список под ним не должен менять высоту.
            // На вкладке «Волна» мини-плеер заменяет сам экран волны, когда он показывает трек.
            val waveStarting by model.waveStarting.collectAsStateWithLifecycle()
            val waveError by model.waveError.collectAsStateWithLifecycle()
            queue.currentTrack()?.let { track ->
                AnimatedVisibility(visible = !(tab == 0 && waveScreenShowsTrack(queue, waveStarting, waveError != null))) {
                    MiniPlayer(
                        track = track,
                        player = player,
                        liked = liked.contains(track.id),
                        onLike = { model.toggleLike(track) },
                        onOpen = { showPlayer = true },
                    )
                }
            }
            BottomNav(selected = tab, onSelect = { tab = it })
        }
        // Плеер выезжает снизу и проявляется, 320 мс (как FULLSCREEN на десктопе).
        AnimatedVisibility(
            visible = showPlayer && hasTrack,
            enter = slideInVertically(tween(320, easing = FastOutSlowInEasing)) { it } +
                fadeIn(tween(320, easing = FastOutSlowInEasing)),
            exit = slideOutVertically(tween(320, easing = FastOutSlowInEasing)) { it } +
                fadeOut(tween(320, easing = FastOutSlowInEasing)),
        ) {
            FullPlayer(
                queue = queue,
                player = player,
                model = model,
                onClose = { showPlayer = false },
                backEnabled = transition.targetState == EnterExitState.Visible,
                onMiniPlayer = if (pipSupported) {
                    { Pip.enter(activity, playing, likedNow) }
                } else {
                    null
                },
            )
        }
        // В режиме PiP вместо интерфейса — компактный мини-плеер. Основной экран под ним
        // остаётся в композиции, чтобы выбранная вкладка и плеер сохранились до возврата.
        val inPip by Pip.inPip.collectAsStateWithLifecycle()
        if (inPip) queue.currentTrack()?.let { PipScreen(it, player) }
    }
}

private class NavItem(val label: String, @DrawableRes val icon: Int)

private val NAV_ITEMS = listOf(
    NavItem("Волна", R.drawable.ic_wave),
    NavItem("Поиск", R.drawable.ic_search),
    NavItem("Коллекция", R.drawable.ic_collection),
    NavItem("Настройки", R.drawable.ic_settings),
)

/** Нижнее меню: линия сверху, «таблетка» 60×32 под значком активного пункта. */
@Composable
private fun BottomNav(selected: Int, onSelect: (Int) -> Unit) {
    Box(Modifier.fillMaxWidth().background(Pl.bg).navigationBarsPadding()) {
        Row(
            Modifier.fillMaxWidth().height(68.dp).padding(start = 8.dp, end = 8.dp, top = 8.dp),
        ) {
            NAV_ITEMS.forEachIndexed { index, item ->
                val on = index == selected
                Column(
                    Modifier
                        .weight(1f)
                        .clickable(
                            interactionSource = remember { MutableInteractionSource() },
                            indication = null,
                        ) { onSelect(index) },
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = androidx.compose.foundation.layout.Arrangement.spacedBy(4.dp),
                ) {
                    Box(
                        Modifier
                            .width(60.dp)
                            .height(32.dp)
                            .clip(RoundedCornerShape(16.dp))
                            .background(if (on) Pl.surface3 else androidx.compose.ui.graphics.Color.Transparent),
                        contentAlignment = Alignment.Center,
                    ) {
                        PlIcon(item.icon, 22.dp, if (on) Pl.accent else Pl.text2, Modifier.size(22.dp))
                    }
                    PlText(
                        item.label,
                        if (on) PlType.captionStrong else PlType.caption,
                        if (on) Pl.text else Pl.text3,
                        maxLines = 1,
                    )
                }
            }
        }
        // Разделитель лежит поверх первой строки, как в макете: пункты начинаются от верха меню.
        Box(Modifier.fillMaxWidth().height(1.dp).background(Pl.border))
    }
}
