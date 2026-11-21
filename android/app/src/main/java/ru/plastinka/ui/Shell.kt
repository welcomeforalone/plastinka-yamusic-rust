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
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.spring
import androidx.compose.foundation.text.BasicText
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextOverflow
import kotlin.math.abs
import kotlin.math.roundToInt
import kotlinx.coroutines.flow.collectLatest
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
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
        LoginScreen(login, onLogin = model::startLogin, onRetrySave = model::retrySaveToken)
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
    // Вкладки листаются свайпом; `tab` — устоявшаяся страница (по ней прячется мини-плеер).
    val pagerState = rememberPagerState(initialPage = tab) { NAV_ITEMS.size }
    // Ключ появления страницы растёт при выборе вкладки меню или кодом, но не при свайпе:
    // после свайпа `PageAppear` второй раз не играет.
    var appearKey by remember { mutableIntStateOf(0) }
    val selectTab = { target: Int ->
        if (target != tab) {
            appearKey += 1
            tab = target
        }
    }
    // Нажатие в меню переключает сразу, без пролёта через промежуточные вкладки.
    LaunchedEffect(tab) {
        if (pagerState.currentPage != tab) pagerState.scrollToPage(tab)
    }
    // Свайп закончен: новая вкладка становится выбранной.
    LaunchedEffect(pagerState) {
        snapshotFlow { pagerState.settledPage }.collect { page -> if (page != tab) tab = page }
    }
    // «Настройки» при возвращении открываются с главного экрана: после ухода меняем ключ
    // состояния, и сохранённый подэкран не восстанавливается.
    var settingsEpoch by remember { mutableIntStateOf(0) }
    LaunchedEffect(pagerState) {
        snapshotFlow { pagerState.settledPage }.collect { page -> if (page != 3) settingsEpoch += 1 }
    }
    // Стек экранов «Поиска» и «Коллекции» (и прокрутка в них) переживает уход на другую
    // вкладку: без держателя `rememberSaveable` внутри вкладки сбрасывается при её смене.
    // «Волна» и «Настройки» открываются как раньше, с главного экрана.
    val tabStates = rememberSaveableStateHolder()
    // Запрос «открыть маршрут» из плеера: вкладка кладёт маршрут на стек и сбрасывает запрос.
    var routeRequest by remember { mutableStateOf<RouteRequest?>(null) }
    var routeSeq by remember { mutableIntStateOf(0) }
    val liked by QueueHolder.likedIds.collectAsStateWithLifecycle()
    val playing = player?.playWhenReady?.collectAsStateWithLifecycle()?.value ?: false
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

    // Переход из любого места (плеер, мини-плеер, строки треков): как на десктопе,
    // страница открывается, полноэкранный плеер сворачивается.
    val openRoute = { route: CollectionRoute ->
        val target = tabForRouteOpen(tab)
        routeSeq += 1
        routeRequest = RouteRequest(route, target, routeSeq)
        selectTab(target)
        showPlayer = false
    }
    androidx.compose.runtime.CompositionLocalProvider(LocalOpenRoute provides openRoute) {
        Box(Modifier.fillMaxSize().background(Pl.bg)) {
            Column(Modifier.fillMaxSize()) {
                Box(Modifier.weight(1f).fillMaxWidth().statusBarsPadding()) {
                    PageAppear(appearKey) {
                        HorizontalPager(
                            state = pagerState,
                            beyondViewportPageCount = 0,
                            // Горизонтальные списки, перемотка, ползунки и обложки забирают свои жесты сами.
                            modifier = Modifier.fillMaxSize(),
                        ) { page ->
                            androidx.compose.runtime.CompositionLocalProvider(
                                LocalTabActive provides (page == pagerState.settledPage),
                            ) {
                                when (page) {
                                    0 -> WaveTab(
                                        model,
                                        player,
                                        onOpenPlayer = { showPlayer = true },
                                        onOpenSettings = { selectTab(3) },
                                    )
                                    1 -> tabStates.SaveableStateProvider(1) {
                                        SearchTab(
                                            model,
                                            player,
                                            request = routeRequest?.takeIf { it.tab == 1 },
                                            onRequestHandled = { routeRequest = null },
                                        )
                                    }
                                    2 -> tabStates.SaveableStateProvider(2) {
                                        CollectionTab(
                                            library,
                                            model,
                                            player,
                                            onOpenSettings = { selectTab(3) },
                                            request = routeRequest?.takeIf { it.tab == 2 },
                                            onRequestHandled = { routeRequest = null },
                                        )
                                    }
                                    else -> androidx.compose.runtime.key(settingsEpoch) { SettingsScreen(library, model) }
                                }
                            }
                        }
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
                BottomNav(
                    // Положение таблетки: страница плюс доля свайпа, читается только при отрисовке.
                    rawPosition = { pagerState.currentPage + pagerState.currentPageOffsetFraction },
                    onSelect = selectTab,
                )
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
}

private class NavItem(val label: String, @DrawableRes val icon: Int)

private val NAV_ITEMS = listOf(
    NavItem("Волна", R.drawable.ic_wave),
    NavItem("Поиск", R.drawable.ic_search),
    NavItem("Коллекция", R.drawable.ic_collection),
    NavItem("Настройки", R.drawable.ic_settings),
)

/** Пружина переезда таблетки при нажатии в меню (пейджер прыгает сразу): мягкая, с лёгким перелётом. */
private const val PILL_JUMP_DAMPING = 0.75f
private const val PILL_JUMP_STIFFNESS = 450f

/** Разница положения, после которой изменение считается прыжком (нажатие), а не движением пальца. */
private const val PILL_JUMP_THRESHOLD = 0.6f

/** Таблетка на ходу вытягивается по ширине максимум на столько (доля от 60 dp). */
private const val PILL_STRETCH = 0.4f

private val PILL_WIDTH = 60.dp
private val PILL_HEIGHT = 32.dp

/** Состояние переезда таблетки: откуда и куда идёт прыжок (читается только при отрисовке). */
private class PillMotion {
    var jumping = false
    var from = 0f
    var to = 0f
}

/**
 * Нижнее меню: линия сверху и одна «таблетка» 60×32 под значком активного пункта.
 * Таблетка едет за пальцем при свайпе вкладок ([rawPosition] — страница плюс доля свайпа),
 * а по нажатию в меню плавно переезжает пружиной. Положение и цвета читаются в фазе
 * отрисовки, меню не перекомпонуется на каждом кадре.
 */
@Composable
private fun BottomNav(rawPosition: () -> Float, onSelect: (Int) -> Unit) {
    val rawNow by rememberUpdatedState(rawPosition)
    val pos = remember { Animatable(rawNow()) }
    val motion = remember { PillMotion() }
    LaunchedEffect(Unit) {
        snapshotFlow { rawNow() }.collectLatest { raw ->
            if (abs(raw - pos.targetValue) > PILL_JUMP_THRESHOLD) {
                // Нажатие в меню: пейджер перескочил, таблетка переезжает плавно.
                motion.from = pos.value
                motion.to = raw
                motion.jumping = true
                pos.animateTo(raw, spring(PILL_JUMP_DAMPING, PILL_JUMP_STIFFNESS))
                motion.jumping = false
            } else {
                // Палец ведёт вкладки: таблетка точно под ним, без отставания.
                motion.jumping = false
                pos.snapTo(raw)
            }
        }
    }
    // Жирная подпись у ближайшего пункта, без смешивания: смешанная жирность дёргает текст.
    val nearest by remember { derivedStateOf { pos.value.roundToInt() } }
    val pillColor = Pl.surface3
    val activeIcon = Pl.accent
    val idleIcon = Pl.text2
    val activeText = Pl.text
    val idleText = Pl.text3

    Box(Modifier.fillMaxWidth().background(Pl.bg).navigationBarsPadding()) {
        Box(Modifier.fillMaxWidth().height(68.dp).padding(start = 8.dp, end = 8.dp, top = 8.dp)) {
            // Одна таблетка на всё меню, под значками.
            Box(
                Modifier.fillMaxWidth().height(PILL_HEIGHT).drawBehind {
                    val p = pos.value
                    // Чем дальше от пунктов, тем шире: эффект «капли». В покое ровно 60 dp.
                    val away = if (motion.jumping) {
                        minOf(abs(p - motion.from), abs(p - motion.to))
                    } else {
                        abs(p - p.roundToInt())
                    }
                    val width = PILL_WIDTH.toPx() * (1f + PILL_STRETCH * minOf(1f, away * 2f))
                    val centerX = (p + 0.5f) * size.width / NAV_ITEMS.size
                    drawRoundRect(
                        pillColor,
                        topLeft = Offset(centerX - width / 2f, 0f),
                        size = Size(width, PILL_HEIGHT.toPx()),
                        cornerRadius = CornerRadius(PILL_HEIGHT.toPx() / 2f),
                    )
                },
            )
            Row(Modifier.fillMaxSize()) {
                NAV_ITEMS.forEachIndexed { index, item ->
                    val painter = painterResource(item.icon)
                    // Близость положения к пункту: 1 — таблетка точно под ним, 0 — за пункт и дальше.
                    val closeness = { (1f - abs(pos.value - index)).coerceIn(0f, 1f) }
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
                        Box(Modifier.width(PILL_WIDTH).height(PILL_HEIGHT), contentAlignment = Alignment.Center) {
                            Box(
                                Modifier.size(22.dp).drawBehind {
                                    val tint = lerp(idleIcon, activeIcon, closeness())
                                    with(painter) { draw(size, colorFilter = ColorFilter.tint(tint)) }
                                },
                            )
                        }
                        BasicText(
                            item.label,
                            style = if (nearest == index) PlType.captionStrong else PlType.caption,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            color = { lerp(idleText, activeText, closeness()) },
                        )
                    }
                }
            }
        }
        // Разделитель лежит поверх первой строки, как в макете: пункты начинаются от верха меню.
        Box(Modifier.fillMaxWidth().height(1.dp).background(Pl.border))
    }
}
