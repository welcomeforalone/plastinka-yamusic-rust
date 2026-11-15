package ru.plastinka.ui

import androidx.annotation.DrawableRes
import androidx.compose.animation.AnimatedVisibility
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
    model: AppViewModel,
) {
    if (!loggedIn) {
        LoginScreen(login, onLogin = model::startLogin)
        return
    }

    val account = library.account
    when {
        account != null && account.hasPlus -> MainScreen(library, model)
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
    var tab by rememberSaveable { mutableIntStateOf(0) }
    val liked by QueueHolder.likedIds.collectAsStateWithLifecycle()

    Box(Modifier.fillMaxSize().background(Pl.bg)) {
        Column(Modifier.fillMaxSize()) {
            Box(Modifier.weight(1f).fillMaxWidth().statusBarsPadding()) {
                when (tab) {
                    0 -> WaveTab(model, player, onOpenPlayer = { showPlayer = true })
                    1 -> SearchTab(model, player)
                    2 -> CollectionTab(library, model, player, onOpenSettings = { tab = 3 })
                    else -> SettingsScreen(library, model)
                }
            }
            // Мини-плеер над нижним меню.
            // На вкладке «Волна» играющая волна сама заменяет мини-плеер.
            queue.currentTrack()?.let { track ->
                AnimatedVisibility(visible = !showPlayer && !(tab == 0 && queue.isWave)) {
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
        if (showPlayer) {
            FullPlayer(
                queue = queue,
                player = player,
                model = model,
                onClose = { showPlayer = false },
            )
        }
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
    Column(Modifier.fillMaxWidth().background(Pl.bg).navigationBarsPadding()) {
        Box(Modifier.fillMaxWidth().height(1.dp).background(Pl.border))
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
    }
}
