package ru.plastinka.ui

import androidx.compose.foundation.background
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
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import ru.plastinka.PlastinkaApp
import ru.plastinka.R
import ru.plastinka.data.Prefs
import ru.plastinka.sync.LinkState
import ru.plastinka.sync.PairedDevice
import ru.plastinka.sync.SyncService
import ru.plastinka.sync.SyncUi
import ru.plastinka.sync.VisibleDevice

// «Слушать вместе»: нижняя панель в плеере, диалог сопряжения и раздел в настройках.
// Вид — из существующих компонентов (листы, строки настроек, «таблетки»).

private const val EXPLAIN = "Одна музыка на двух устройствах сразу. Звук каждое устройство берёт у Яндекса само, " +
    "для связи нужна одна сеть Wi-Fi."

/** Служба связи приложения. */
@Composable
internal fun rememberSyncService(): SyncService {
    val context = LocalContext.current
    return remember(context) { (context.applicationContext as PlastinkaApp).sync.service }
}

private fun osTitle(os: String): String = when (os) {
    "windows" -> "Windows"
    "linux" -> "Linux"
    "macos" -> "macOS"
    "android" -> "Android"
    else -> "Устройство"
}

/** Связь готова: значок «Устройства» в плеере горит акцентом. */
@Composable
internal fun syncLinked(): Boolean {
    val service = rememberSyncService()
    val ui by service.ui.collectAsStateWithLifecycle()
    return ui.link is LinkState.Linked
}

/** Нижняя панель «Слушать вместе» из плеера. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun SyncSheet(onDismiss: () -> Unit, onOpenSettings: () -> Unit) {
    val service = rememberSyncService()
    val ui by service.ui.collectAsStateWithLifecycle()
    // Сопряжение началось: панель закрывается, чтобы не оставаться под диалогом (как на ПК).
    val pairing = ui.link is LinkState.Pairing
    LaunchedEffect(pairing) { if (pairing) onDismiss() }
    PlBottomSheet(onDismiss, rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(
            Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 20.dp)
                .navigationBarsPadding()
                .padding(bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            PlText("Слушать вместе", PlType.title)
            if (!Prefs.syncEnabled) {
                PlText(EXPLAIN, PlType.secondary, Pl.text2)
                Pill("Включить", fillWidth = true, onClick = { Prefs.changeSyncEnabled(true) })
            } else {
                LinkStatus(ui, service)
                ui.problem?.let { problem ->
                    PlText(
                        problem,
                        PlType.secondary,
                        Pl.error,
                        Modifier.fillMaxWidth().clip(CircleShape).background(Pl.errorSoft, CircleShape)
                            .padding(horizontal = 16.dp, vertical = 10.dp),
                    )
                }
                DeviceList(ui, service)
            }
            Pill(
                "Настройки",
                kind = PillKind.Tonal,
                fillWidth = true,
                onClick = {
                    onDismiss()
                    onOpenSettings()
                },
            )
        }
    }
}

@Composable
private fun LinkStatus(ui: SyncUi, service: SyncService) {
    when (val link = ui.link) {
        LinkState.Idle -> Unit
        is LinkState.Connecting -> StatusLine("Подключаемся к ${link.peerName ?: "устройству"}…", "Отменить") {
            service.disconnect()
        }
        is LinkState.Pairing -> StatusLine("Подтвердите код на ${link.request.peer.name}…", "Отменить") {
            service.decide(false)
        }
        is LinkState.Linked -> StatusLine("Вместе с ${link.peer.name}", "Отключить") { service.disconnect() }
        is LinkState.Reconnecting -> StatusLine("Восстанавливаем связь с ${link.peerName}…", "Отключить") {
            service.disconnect()
        }
    }
}

@Composable
private fun StatusLine(text: String, action: String, onAction: () -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        PlText(text, PlType.bodyStrong, modifier = Modifier.weight(1f))
        Spacer(Modifier.width(12.dp))
        Pill(action, kind = PillKind.Tonal, height = 36.dp, onClick = onAction)
    }
}

@Composable
private fun DeviceList(ui: SyncUi, service: SyncService) {
    if (ui.visible.isEmpty()) {
        PlText(
            if (ui.running) "Ищем устройства в сети…" else "Запускаем поиск…",
            PlType.secondary,
            Pl.text3,
        )
        PlText(
            "На втором устройстве должна быть открыта Plastinka с включённым «Слушать вместе» " +
                "и тем же аккаунтом. Устройство можно подключить и по адресу, в настройках.",
            PlType.caption,
            Pl.text3,
        )
        return
    }
    PlText("Устройства рядом", PlType.captionStrong, Pl.text3)
    val idle = ui.link == LinkState.Idle
    for (device in ui.visible) DeviceRow(device, idle) { service.connect(device.id) }
}

@Composable
private fun DeviceRow(device: VisibleDevice, enabled: Boolean, onConnect: () -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            PlText(device.name, PlType.bodyStrong, maxLines = 1)
            PlText(
                osTitle(device.os) + if (device.paired) " · сопряжено" else "",
                PlType.caption,
                Pl.text3,
            )
        }
        Spacer(Modifier.width(12.dp))
        Pill("Подключить", height = 36.dp, enabled = enabled, onClick = onConnect)
    }
}

/**
 * Диалог сопряжения: имя и ОС собеседника, код, кнопки. Показывается поверх любого экрана и для
 * исходящего, и для входящего подключения. Вид как у окна на ПК: карточка до 460 dp с рамкой,
 * заголовок слева и крестик справа, текст, код крупно в плашке, «таблетки» «Коды совпадают»
 * и «Отмена» (в ряд, а если не помещаются — друг под другом на всю ширину).
 */
@Composable
internal fun SyncPairingDialog() {
    val service = rememberSyncService()
    val ui by service.ui.collectAsStateWithLifecycle()
    val request = (ui.link as? LinkState.Pairing)?.request ?: return
    // После «Коды совпадают» ждём подтверждения на втором устройстве; новый запрос — заново.
    var waiting by remember(request.code, request.peer.id) { mutableStateOf(false) }
    val shape = RoundedCornerShape(20.dp)
    val cancel = { service.decide(false) }
    Dialog(
        onDismissRequest = {},
        properties = DialogProperties(
            dismissOnBackPress = false,
            dismissOnClickOutside = false,
            usePlatformDefaultWidth = false,
        ),
    ) {
        Column(
            Modifier
                .padding(horizontal = 16.dp)
                .widthIn(max = 460.dp)
                .fillMaxWidth()
                .clip(shape)
                .background(Pl.surface2, shape)
                .border(1.dp, Pl.outline, shape)
                .padding(24.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                PlText("Сопряжение", PlType.title, modifier = Modifier.weight(1f), maxLines = 1)
                Box(
                    Modifier.size(32.dp).clip(CircleShape).clickable(onClick = cancel),
                    Alignment.Center,
                ) { PlIcon(R.drawable.ic_close, 18.dp, Pl.text2) }
            }
            PlText(
                if (waiting) "Ждём подтверждения на ${request.peer.name}…"
                else "Проверьте, что на ${request.peer.name} (${osTitle(request.peer.os)}) тот же код.",
                PlType.body,
                Pl.text2,
            )
            Box(
                Modifier
                    .fillMaxWidth()
                    .background(Pl.surface3, RoundedCornerShape(10.dp))
                    .padding(vertical = 16.dp),
                Alignment.Center,
            ) {
                PlText(
                    request.displayCode,
                    PlType.display.copy(fontSize = 40.sp, lineHeight = 48.sp),
                    Pl.text,
                    maxLines = 1,
                )
            }
            DialogButtons(gap = 12.dp) {
                if (!waiting) {
                    Pill("Коды совпадают", onClick = {
                        waiting = true
                        service.decide(true)
                    })
                }
                Pill("Отмена", kind = PillKind.Outline, onClick = cancel)
            }
        }
    }
}

/**
 * Кнопки диалога одинаковой ширины на всю ширину карточки, поровну (как на ПК). Если подпись
 * какой-то кнопки не помещается в свою долю (узкий экран, крупный шрифт), кнопки встают друг под
 * другом, каждая во всю ширину. Одна кнопка — во всю ширину.
 */
@Composable
private fun DialogButtons(gap: Dp, content: @Composable () -> Unit) {
    Layout(content) { measurables, constraints ->
        val count = measurables.size
        val width = constraints.maxWidth
        if (count == 0) return@Layout layout(width, 0) {}
        val gapPx = gap.roundToPx()
        val cell = (width - gapPx * (count - 1)) / count
        val widest = measurables.maxOf { it.maxIntrinsicWidth(constraints.maxHeight) }
        if (widest <= cell) {
            val placeables = measurables.map { it.measure(Constraints.fixedWidth(cell)) }
            val height = placeables.maxOf { it.height }
            layout(width, height) {
                placeables.forEachIndexed { i, p -> p.placeRelative(i * (cell + gapPx), (height - p.height) / 2) }
            }
        } else {
            val placeables = measurables.map { it.measure(Constraints.fixedWidth(width)) }
            val height = placeables.sumOf { it.height } + gapPx * (count - 1)
            layout(width, height) {
                var y = 0
                for (p in placeables) {
                    p.placeRelative(0, y)
                    y += p.height + gapPx
                }
            }
        }
    }
}

// ----------------------------------------------------------------- настройки

/** Раздел «Слушать вместе» в настройках. */
@Composable
fun SyncSettingsScreen(onBack: () -> Unit) {
    val service = rememberSyncService()
    val ui by service.ui.collectAsStateWithLifecycle()
    SettingsPageScaffold("Слушать вместе", onBack) {
        GroupCard {
            ToggleRow(
                title = "Слушать вместе",
                subtitle = "Выключено по умолчанию",
                checked = Prefs.syncEnabled,
                onChange = Prefs::changeSyncEnabled,
                icon = R.drawable.ic_devices,
            )
        }
        SettingsNote(EXPLAIN)
        Spacer(Modifier.height(8.dp))
        SectionTitle("Это устройство")
        Spacer(Modifier.height(10.dp))
        GroupCard {
            Column(
                Modifier.fillMaxWidth().padding(start = 14.dp, end = 16.dp, top = 10.dp, bottom = 12.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                PlText("Имя", PlType.secondaryStrong)
                NameField()
                PlText("Так это устройство видят другие.", PlType.caption, Pl.text3)
            }
            Column(
                Modifier.fillMaxWidth().padding(start = 14.dp, end = 16.dp, top = 6.dp, bottom = 12.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                PlText("Задержка звука на этом устройстве", PlType.secondaryStrong)
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Pill("−10", kind = PillKind.Tonal, height = 36.dp, onClick = {
                        Prefs.changeSyncDelay(Prefs.syncDelayMs - 10)
                    })
                    PlText(
                        "${if (Prefs.syncDelayMs > 0) "+" else ""}${Prefs.syncDelayMs} мс",
                        PlType.bodyStrong,
                        modifier = Modifier.weight(1f),
                        textAlign = TextAlign.Center,
                    )
                    Pill("+10", kind = PillKind.Tonal, height = 36.dp, onClick = {
                        Prefs.changeSyncDelay(Prefs.syncDelayMs + 10)
                    })
                }
                PlText(
                    "Для Bluetooth-наушников: от −500 до +500 мс. Если звук опережает или отстаёт, подвиньте.",
                    PlType.caption,
                    Pl.text3,
                )
            }
        }
        if (Prefs.syncEnabled) {
            Spacer(Modifier.height(22.dp))
            SectionTitle("Сопряжённые устройства")
            Spacer(Modifier.height(10.dp))
            GroupCard {
                if (ui.paired.isEmpty()) {
                    PlText(
                        "Пока нет. При первой связи на обоих устройствах показывается одинаковый код.",
                        PlType.secondary,
                        Pl.text3,
                        Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
                    )
                }
                for (device in ui.paired) PairedRow(device) { service.forget(device.id) }
            }
            ui.storageWarning?.let { SettingsNote(it) }
            Spacer(Modifier.height(22.dp))
            SectionTitle("Подключение по адресу")
            Spacer(Modifier.height(10.dp))
            GroupCard {
                Column(
                    Modifier.fillMaxWidth().padding(start = 14.dp, end = 16.dp, top = 10.dp, bottom = 12.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    PlText("Адреса этого устройства", PlType.secondaryStrong)
                    PlText(
                        ui.addresses.joinToString("\n").ifEmpty { "Нет подключения к сети" },
                        PlType.secondary,
                        Pl.text2,
                    )
                    AddressConnect(service, ui)
                }
            }
            SettingsNote(
                "Если устройства не видят друг друга, введите адрес вручную как 192.168.1.5:47800. " +
                    "На компьютере проверьте, что брандмауэр пропускает Plastinka.",
            )
            ui.problem?.let { SettingsNote(it) }
        }
    }
}

@Composable
private fun PairedRow(device: PairedDevice, onForget: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(start = 16.dp, end = 12.dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            PlText(device.name.ifEmpty { "Устройство" }, PlType.bodyStrong, maxLines = 1)
            PlText(osTitle(device.os), PlType.caption, Pl.text3)
        }
        Spacer(Modifier.width(12.dp))
        Pill("Забыть", kind = PillKind.Danger, height = 36.dp, onClick = onForget)
    }
}

@Composable
private fun AddressConnect(service: SyncService, ui: SyncUi) {
    var text by remember { mutableStateOf("") }
    val focus = LocalFocusManager.current
    val connect = {
        focus.clearFocus()
        service.connectAddress(text)
    }
    TextBox(text, { text = it }, "192.168.1.5:47800", KeyboardActions(onDone = { connect() }))
    Pill(
        "Подключить",
        fillWidth = true,
        enabled = text.isNotBlank() && ui.link == LinkState.Idle,
        onClick = connect,
    )
}

@Composable
private fun NameField() {
    val focus = LocalFocusManager.current
    var text by remember { mutableStateOf(Prefs.syncDeviceName) }
    TextBox(
        text,
        { text = it },
        android.os.Build.MODEL.orEmpty(),
        KeyboardActions(onDone = { focus.clearFocus() }),
        modifier = Modifier.onFocusChanged { state ->
            // Имя сохраняется, когда поле теряет фокус; пустое возвращает имя по умолчанию.
            if (!state.isFocused && text != Prefs.syncDeviceName) {
                Prefs.changeSyncDeviceName(text)
                text = Prefs.syncDeviceName
            }
        },
    )
}

/** Однострочное поле в стиле поля формата из «Плагинов». */
@Composable
private fun TextBox(
    value: String,
    onChange: (String) -> Unit,
    hint: String,
    actions: KeyboardActions,
    modifier: Modifier = Modifier,
) {
    BasicTextField(
        value = value,
        onValueChange = { onChange(it.take(64)) },
        singleLine = true,
        textStyle = PlType.body.copy(color = Pl.text),
        cursorBrush = SolidColor(Pl.accent),
        keyboardOptions = KeyboardOptions(imeAction = androidx.compose.ui.text.input.ImeAction.Done),
        keyboardActions = actions,
        modifier = modifier.fillMaxWidth(),
        decorationBox = { inner ->
            Box(
                Modifier
                    .fillMaxWidth()
                    .height(48.dp)
                    .clip(CircleShape)
                    .background(Pl.surface2, CircleShape)
                    .border(1.dp, Pl.outline, CircleShape)
                    .padding(horizontal = 18.dp),
                contentAlignment = Alignment.CenterStart,
            ) {
                if (value.isEmpty()) PlText(hint, PlType.body, Pl.text3)
                inner()
            }
        },
    )
}
