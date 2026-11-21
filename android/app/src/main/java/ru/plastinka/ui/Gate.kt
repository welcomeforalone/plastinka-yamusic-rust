package ru.plastinka.ui

import android.os.SystemClock
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import ru.plastinka.R
import ru.plastinka.data.AccountInfo

private val SCREEN_PADDING = 24.dp

/** Общая подложка ворот: чёрный фон и свечение акцентом сверху (glowTop — верх круга от края экрана). */
@Composable
private fun GateBackground(glowAlpha: Float, glowTop: Dp = (-190).dp, content: @Composable () -> Unit) {
    Box(Modifier.fillMaxSize().background(Pl.bg)) {
        // Макеты 01 и 02: круг 320 dp, центр выше верхнего края экрана.
        Glow(
            size = 320.dp,
            alpha = glowAlpha,
            modifier = Modifier.align(Alignment.TopCenter).offset(y = glowTop),
        )
        content()
    }
}

/** Индикатор акцентом. */
@Composable
private fun Spinner(size: Int = 18) {
    CircularProgressIndicator(
        modifier = Modifier.size(size.dp),
        color = Pl.accent,
        strokeWidth = 2.dp,
    )
}

// ------------------------------------------------------------------ вход

@Composable
fun LoginScreen(login: LoginUi, onLogin: () -> Unit, onRetrySave: () -> Unit) {
    GateBackground(glowAlpha = 0.5f) {
        Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding()) {
            Column(
                Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = SCREEN_PADDING)
                    .padding(top = 56.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Vinyl(112.dp)
                Spacer(Modifier.height(28.dp))
                PlText("Вход в Plastinka", PlType.display, textAlign = TextAlign.Center)
                Spacer(Modifier.height(10.dp))
                login.notice?.let {
                    PlText(it, PlType.secondary, Pl.text2, textAlign = TextAlign.Center)
                    Spacer(Modifier.height(10.dp))
                }
                LoginBody(login, onLogin, onRetrySave)
            }
            PlText(
                "Пароль вводится только на странице Яндекса.\n" +
                    "Неофициальный клиент, не связан с ООО «Яндекс».",
                PlType.caption,
                Pl.text3,
                Modifier.fillMaxWidth().padding(horizontal = SCREEN_PADDING, vertical = 16.dp),
                textAlign = TextAlign.Center,
            )
        }
    }
}

@Composable
private fun LoginBody(login: LoginUi, onLogin: () -> Unit, onRetrySave: () -> Unit) {
    val context = LocalContext.current
    when (login.phase) {
        LoginPhase.Idle -> {
            if (login.retrySave) {
                // Токен получен, но хранилище ключей его не приняло: повторяем только сохранение.
                Spacer(Modifier.height(16.dp))
                PlText("Не удалось сохранить вход", PlType.bodyStrong, Pl.error, textAlign = TextAlign.Center)
                Spacer(Modifier.height(8.dp))
                PlText(login.error.orEmpty(), PlType.secondary, Pl.text2, textAlign = TextAlign.Center)
                Spacer(Modifier.height(24.dp))
                Pill("Повторить", height = 52.dp, fillWidth = true, onClick = onRetrySave)
                return
            }
            PlText(
                "Войдите по коду устройства: пароль вы введёте на странице Яндекса.",
                PlType.body,
                Pl.text2,
                textAlign = TextAlign.Center,
            )
            login.error?.let {
                Spacer(Modifier.height(16.dp))
                PlText(it, PlType.secondary, Pl.error, textAlign = TextAlign.Center)
            }
            Spacer(Modifier.height(24.dp))
            Pill(
                if (login.error != null) "Получить новый код" else "Войти по коду устройства",
                kind = if (login.error != null) PillKind.Outline else PillKind.Primary,
                height = 52.dp,
                fillWidth = true,
                onClick = onLogin,
            )
        }

        LoginPhase.Requesting -> {
            Spacer(Modifier.height(24.dp))
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Spinner()
                PlText("Получаем код…", PlType.secondary, Pl.text2)
            }
        }

        LoginPhase.Waiting, LoginPhase.Done -> {
            val code = login.code
            if (code == null) {
                // Состояние без кода не должно возникать; даём выйти из тупика.
                Spacer(Modifier.height(24.dp))
                Pill("Получить новый код", kind = PillKind.Outline, height = 52.dp, fillWidth = true, onClick = onLogin)
                return
            }
            val host = code.verificationUrl.removePrefix("https://").removePrefix("http://")
            PlText(
                "Откройте $host на любом устройстве\nи введите код:",
                PlType.body,
                Pl.text2,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(24.dp))
            CodeCard(code.userCode, code.expiresInSeconds)
            Spacer(Modifier.height(20.dp))
            Pill(
                "Открыть страницу",
                height = 52.dp,
                icon = R.drawable.ic_external_link,
                fillWidth = true,
                onClick = { openUrl(context, code.verificationUrl) },
            )
            Spacer(Modifier.height(10.dp))
            CopyCodeButton(code.userCode)
            Spacer(Modifier.height(24.dp))
            if (login.error != null) {
                PlText(login.error, PlType.secondary, Pl.error, textAlign = TextAlign.Center)
                Spacer(Modifier.height(16.dp))
                Pill(
                    "Получить новый код",
                    kind = PillKind.Outline,
                    height = 52.dp,
                    fillWidth = true,
                    onClick = onLogin,
                )
            } else {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Spinner()
                    PlText("Ждём подтверждения…", PlType.secondary, Pl.text2)
                }
            }
            Spacer(Modifier.height(24.dp))
        }
    }
}

/** Карточка с кодом и обратным отсчётом срока действия. */
@Composable
private fun CodeCard(userCode: String, expiresInSeconds: Long) {
    // Отсчёт от момента показа карточки, по монотонным часам. Момент переживает
    // поворот экрана, иначе срок после поворота начинался бы заново.
    val startedAt = androidx.compose.runtime.saveable.rememberSaveable(userCode) {
        SystemClock.elapsedRealtime()
    }
    var left by remember(userCode) { mutableLongStateOf(expiresInSeconds) }
    LaunchedEffect(userCode) {
        while (left > 0) {
            val elapsed = (SystemClock.elapsedRealtime() - startedAt) / 1000
            left = (expiresInSeconds - elapsed).coerceAtLeast(0)
            delay(1000)
        }
    }
    val shape = RoundedCornerShape(24.dp)
    Column(
        Modifier
            .fillMaxWidth()
            .clip(shape)
            .background(Pl.surface1, shape)
            .border(1.dp, Pl.border, shape)
            .padding(start = 16.dp, end = 16.dp, top = 24.dp, bottom = 20.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        PlText(userCode, PlType.code, textAlign = TextAlign.Center)
        if (expiresInSeconds > 0 && left > 0) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                PlIcon(R.drawable.ic_clock, 14.dp, Pl.text3)
                PlText(
                    "Код действует ${left / 60}:${(left % 60).toString().padStart(2, '0')}",
                    PlType.caption.copy(fontFeatureSettings = "tnum"),
                    Pl.text3,
                )
            }
        }
    }
}

@Composable
private fun CopyCodeButton(userCode: String) {
    val clipboard = LocalClipboardManager.current
    var copied by remember(userCode) { mutableStateOf(false) }
    LaunchedEffect(copied) {
        if (copied) {
            delay(2000)
            copied = false
        }
    }
    Pill(
        if (copied) "Скопировано" else "Скопировать код",
        kind = PillKind.Outline,
        height = 52.dp,
        icon = if (copied) R.drawable.ic_check else R.drawable.ic_copy,
        fillWidth = true,
        onClick = {
            clipboard.setText(AnnotatedString(userCode))
            copied = true
        },
    )
}

// ------------------------------------------------------- проверка подписки

/** Идёт проверка подписки: музыки и плеера ещё нет. */
@Composable
fun SubscriptionCheckingScreen() {
    GateBackground(glowAlpha = 0.4f) {
        Column(
            Modifier.fillMaxSize(),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Spinner(28)
            Spacer(Modifier.height(16.dp))
            PlText("Проверяем подписку…", PlType.body, Pl.text2)
        }
    }
}

/** Проверить подписку не удалось: ошибка не считается подпиской. */
@Composable
fun SubscriptionErrorScreen(error: String, onRetry: () -> Unit, onLogout: () -> Unit) {
    GateBackground(glowAlpha = 0.3f) {
        Column(
            Modifier
                .fillMaxSize()
                .statusBarsPadding()
                .navigationBarsPadding()
                .verticalScroll(rememberScrollState())
                .padding(SCREEN_PADDING),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            PlText("Не удалось проверить подписку", PlType.display, textAlign = TextAlign.Center)
            Spacer(Modifier.height(12.dp))
            PlText(error, PlType.body, Pl.error, textAlign = TextAlign.Center)
            Spacer(Modifier.height(24.dp))
            Pill("Повторить", height = 52.dp, icon = R.drawable.ic_retry, fillWidth = true, onClick = onRetry)
            Spacer(Modifier.height(10.dp))
            Pill(
                "Выйти",
                kind = PillKind.Ghost,
                ghostColor = Pl.error,
                height = 52.dp,
                icon = R.drawable.ic_logout,
                fillWidth = true,
                onClick = onLogout,
            )
        }
    }
}

/** Экран «Нужна подписка Плюс»: плеер не создаётся, пока Плюса нет. */
@Composable
fun SubscriptionRequiredScreen(
    account: AccountInfo,
    checking: Boolean,
    onRecheck: () -> Unit,
    onLogout: () -> Unit,
) {
    val context = LocalContext.current
    GateBackground(glowAlpha = 0.4f, glowTop = (-210).dp) {
        Column(
            Modifier
                .fillMaxSize()
                .statusBarsPadding()
                .navigationBarsPadding()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = SCREEN_PADDING)
                .padding(top = 92.dp, bottom = 24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Box(
                Modifier.size(96.dp).clip(CircleShape).background(Pl.accentSoft, CircleShape),
                contentAlignment = Alignment.Center,
            ) {
                PlIcon(R.drawable.ic_lock, 40.dp, Pl.accent)
            }
            Spacer(Modifier.height(28.dp))
            PlText("Нужна подписка Плюс", PlType.display, textAlign = TextAlign.Center)
            Spacer(Modifier.height(12.dp))
            PlText(
                "Plastinka играет музыку только при активной подписке. " +
                    "Только что оформили? Подождите минуту и проверьте снова.",
                PlType.body,
                Pl.text2,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(24.dp))
            val shape = RoundedCornerShape(20.dp)
            Row(
                Modifier
                    .fillMaxWidth()
                    .clip(shape)
                    .background(Pl.surface1, shape)
                    .border(1.dp, Pl.border, shape)
                    .padding(start = 12.dp, end = 16.dp, top = 12.dp, bottom = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Avatar(44.dp)
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    PlText(account.displayName, PlType.bodyStrong, maxLines = 1)
                    PlText("Подписка Плюс не подтверждена", PlType.caption, Pl.error, maxLines = 1)
                }
            }
            Spacer(Modifier.height(24.dp))
            Pill(
                if (checking) "Проверяем…" else "Проверить снова",
                height = 52.dp,
                icon = R.drawable.ic_retry,
                fillWidth = true,
                enabled = !checking,
                onClick = onRecheck,
            )
            Spacer(Modifier.height(10.dp))
            Pill(
                "Оформить подписку",
                kind = PillKind.Outline,
                height = 52.dp,
                icon = R.drawable.ic_external_link,
                fillWidth = true,
                onClick = { openUrl(context, "https://plus.yandex.ru") },
            )
            Spacer(Modifier.height(10.dp))
            Pill(
                "Выйти",
                kind = PillKind.Ghost,
                ghostColor = Pl.error,
                height = 52.dp,
                icon = R.drawable.ic_logout,
                fillWidth = true,
                onClick = onLogout,
            )
        }
    }
}
