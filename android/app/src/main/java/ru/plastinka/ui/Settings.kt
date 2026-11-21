package ru.plastinka.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.widget.Toast
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import ru.plastinka.Diagnostics
import ru.plastinka.R
import ru.plastinka.data.Prefs

private const val UNOFFICIAL = "Неофициальный клиент, не связан с ООО «Яндекс»"

/** Раздел «Настройки»: главная и подэкраны. Системное «назад» с подэкрана — на главную. */
@Composable
fun SettingsScreen(library: LibraryUi, model: AppViewModel) {
    var page by rememberSaveable { mutableStateOf(SettingsPage.Main) }
    BackHandler(enabled = page != SettingsPage.Main && LocalTabActive.current) { page = SettingsPage.Main }
    val back = { page = SettingsPage.Main }
    PageAppear(page) {
        when (page) {
            SettingsPage.Main -> SettingsMain(
                library = library,
                onLogout = model::logout,
                onOpen = { page = it },
            )
            SettingsPage.Appearance -> AppearanceScreen(onBack = back)
            SettingsPage.Sound -> SoundScreen(onBack = back)
            SettingsPage.Equalizer -> EqualizerScreen(onBack = back)
            SettingsPage.Lyrics -> LyricsSettingsScreen(onBack = back)
            SettingsPage.Plugins -> PluginsScreen(onBack = back)
            SettingsPage.SyncTogether -> SyncSettingsScreen(onBack = back)
            SettingsPage.About -> AboutScreen(onBack = back)
        }
    }
}

@Composable
private fun appVersion(): String {
    val context = LocalContext.current
    return remember(context) {
        try {
            context.packageManager.getPackageInfo(context.packageName, 0).versionName ?: ""
        } catch (_: Exception) {
            ""
        }
    }
}

@Composable
private fun SettingsMain(
    library: LibraryUi,
    onLogout: () -> Unit,
    onOpen: (SettingsPage) -> Unit,
) {
    var confirmLogout by remember { mutableStateOf(false) }
    var confirmReset by remember { mutableStateOf(false) }
    var query by rememberSaveable { mutableStateOf("") }
    val searching = isSearching(query)
    // «Назад» при непустом запросе сначала сбрасывает его.
    BackHandler(enabled = query.isNotEmpty() && LocalTabActive.current) { query = "" }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(top = 12.dp, bottom = 16.dp)) {
        PlText("Настройки", PlType.display, modifier = Modifier.padding(horizontal = 20.dp))
        Spacer(Modifier.height(14.dp))
        SearchField(query, { query = it }, placeholder = "Найти настройку")
        Spacer(Modifier.height(12.dp))
        if (searching) {
            SettingsSearchResults(query, onOpen)
            return@Column
        }
        GroupCard {
            Row(
                Modifier.fillMaxWidth().padding(14.dp, 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Avatar(48.dp)
                Spacer(Modifier.width(14.dp))
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                    PlText(library.account?.displayName ?: "", PlType.headline, maxLines = 1)
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Box(Modifier.size(8.dp).background(Pl.growth, CircleShape))
                        Spacer(Modifier.width(6.dp))
                        PlText("Плюс активен", PlType.caption, Pl.text2)
                    }
                }
                Spacer(Modifier.width(14.dp))
                Pill("Выйти", kind = PillKind.Danger, height = 36.dp, onClick = { confirmLogout = true })
            }
        }
        Spacer(Modifier.height(12.dp))
        GroupCard {
            SettingsRow(R.drawable.ic_palette, "Оформление", "Акцент и визуализатор") {
                onOpen(SettingsPage.Appearance)
            }
            SettingsRow(R.drawable.ic_volume, "Звук", "Качество потока и громкость") {
                onOpen(SettingsPage.Sound)
            }
            SettingsRow(R.drawable.ic_equalizer, "Эквалайзер", "Десять полос, пресеты и предусилитель") {
                onOpen(SettingsPage.Equalizer)
            }
            SettingsRow(
                R.drawable.ic_lyrics,
                "Текст песни",
                if (Prefs.lyricsLrclib) "Яндекс, затем LRCLib" else "Только Яндекс",
            ) { onOpen(SettingsPage.Lyrics) }
        }
        Spacer(Modifier.height(12.dp))
        GroupCard {
            SettingsRow(R.drawable.ic_devices, "Слушать вместе", if (Prefs.syncEnabled) "Включено" else "Одна музыка на двух устройствах") {
                onOpen(SettingsPage.SyncTogether)
            }
            SettingsRow(R.drawable.ic_plug, "Плагины", "FckCensor, Slopless, CopyTrackName") {
                onOpen(SettingsPage.Plugins)
            }
            SettingsRow(R.drawable.ic_about, "О программе", "Версия, лицензии, отчёт о сбое") {
                onOpen(SettingsPage.About)
            }
        }
        Spacer(Modifier.height(16.dp))
        Pill(
            "Сбросить всё",
            Modifier.padding(horizontal = 16.dp),
            kind = PillKind.Danger,
            fillWidth = true,
            onClick = { confirmReset = true },
        )
        Spacer(Modifier.height(16.dp))
        Column(
            Modifier.fillMaxWidth().padding(horizontal = 24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            PlText("Plastinka ${appVersion()}", PlType.captionStrong, Pl.text3, textAlign = TextAlign.Center)
            PlText(UNOFFICIAL, PlType.caption, Pl.text3, textAlign = TextAlign.Center)
        }
    }

    if (confirmReset) {
        AlertDialog(
            onDismissRequest = { confirmReset = false },
            containerColor = Pl.surface2,
            title = { PlText("Сбросить все настройки?", PlType.headline) },
            text = {
                PlText(
                    "Оформление, звук, эквалайзер, текст песни и плагины вернутся к значениям по умолчанию. " +
                        "Аккаунт, свои пресеты эквалайзера и настройки «Моей волны» останутся.",
                    PlType.secondary,
                    Pl.text2,
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    confirmReset = false
                    resetAllSettings()
                }) { PlText("Сбросить", PlType.secondaryStrong, Pl.error) }
            },
            dismissButton = {
                TextButton(onClick = { confirmReset = false }) {
                    PlText("Отмена", PlType.secondaryStrong, Pl.text2)
                }
            },
        )
    }

    if (confirmLogout) {
        AlertDialog(
            onDismissRequest = { confirmLogout = false },
            containerColor = Pl.surface2,
            title = { PlText("Выйти из аккаунта?", PlType.headline) },
            confirmButton = {
                TextButton(onClick = {
                    confirmLogout = false
                    onLogout()
                }) { PlText("Выйти", PlType.secondaryStrong, Pl.error) }
            },
            dismissButton = {
                TextButton(onClick = { confirmLogout = false }) {
                    PlText("Отмена", PlType.secondaryStrong, Pl.text2)
                }
            },
        )
    }
}

/** Результаты поиска: найденные настройки по разделам, название раздела открывает его. */
@Composable
private fun SettingsSearchResults(query: String, onOpen: (SettingsPage) -> Unit) {
    val found = remember(query) { searchSettings(query) }
    if (found.isEmpty()) {
        Column(
            Modifier.fillMaxWidth().padding(horizontal = 32.dp, vertical = 40.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            PlText("Ничего не нашлось", PlType.bodyStrong, textAlign = TextAlign.Center)
            PlText(
                "Проверьте запрос или попробуйте другое слово",
                PlType.secondary,
                Pl.text2,
                textAlign = TextAlign.Center,
            )
        }
        return
    }
    for ((page, entries) in found.groupBy { it.page }) {
        PlText(
            page.title,
            PlType.headline,
            modifier = Modifier
                .fillMaxWidth()
                .clickable { onOpen(page) }
                .padding(horizontal = 20.dp, vertical = 10.dp),
        )
        GroupCard {
            for (entry in entries) {
                Column(
                    Modifier
                        .fillMaxWidth()
                        .clickable { onOpen(page) }
                        .padding(horizontal = 16.dp, vertical = 10.dp),
                    verticalArrangement = Arrangement.spacedBy(2.dp),
                ) {
                    PlText(entry.title, PlType.bodyStrong)
                    PlText(entry.hint, PlType.caption, Pl.text3)
                }
            }
        }
        Spacer(Modifier.height(8.dp))
    }
}

/** Отчёт о сбое в буфер обмена (только по нажатию пользователя). */
private fun copyReport(context: Context, text: String) {
    val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
    try {
        clipboard?.setPrimaryClip(ClipData.newPlainText("Отчёт о сбое Plastinka", text)) ?: return
    } catch (_: Exception) {
        Toast.makeText(context, "Не удалось скопировать", Toast.LENGTH_SHORT).show()
        return
    }
    Toast.makeText(context, "Отчёт скопирован", Toast.LENGTH_SHORT).show()
}

/** «Сбросить всё»: акцент и настройки возвращаются к значениям по умолчанию. */
private fun resetAllSettings() {
    AccentState.selectPreset(Accent.Red)
    Prefs.resetAll()
}

@Composable
@OptIn(ExperimentalFoundationApi::class)
private fun AboutScreen(onBack: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    // null — файлы ещё читаются (не на главном потоке).
    var summary by remember { mutableStateOf<Diagnostics.Summary?>(null) }
    LaunchedEffect(Unit) { summary = withContext(Dispatchers.IO) { Diagnostics.summary() } }
    val debuggable = remember(context) { Diagnostics.isDebuggable(context) }
    SettingsPageScaffold("О программе", onBack) {
        Column(
            Modifier.padding(horizontal = 20.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            PlText("Plastinka", PlType.title)
            // Проверочный хук только для отладочной сборки: долгое нажатие роняет приложение.
            PlText(
                "Версия ${appVersion()}",
                PlType.secondary,
                Pl.text2,
                if (debuggable) {
                    Modifier.combinedClickable(
                        onClick = {},
                        onLongClick = { throw IllegalStateException("Проверка отчёта о сбое") },
                    )
                } else {
                    Modifier
                },
            )
            Spacer(Modifier.height(8.dp))
            PlText(UNOFFICIAL, PlType.body, Pl.text2)
        }
        Spacer(Modifier.height(22.dp))
        SectionTitle("Отчёт о сбое")
        Spacer(Modifier.height(10.dp))
        GroupCard {
            val data = summary
            val lines = listOfNotNull(
                data?.lastCrash?.let { "Последний сбой: $it" },
                data?.lastExit?.let { "Прошлый выход: $it" },
            )
            if (data != null) {
                for (line in lines.ifEmpty { listOf("Сбоев не было") }) {
                    PlText(
                        line,
                        PlType.secondary,
                        Pl.text2,
                        Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp),
                    )
                }
            }
            Pill(
                "Скопировать",
                Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
                kind = PillKind.Tonal,
                fillWidth = true,
                onClick = {
                    scope.launch {
                        // Файлы читаем в фоне, в буфер кладём уже на главном потоке.
                        val text = withContext(Dispatchers.IO) { Diagnostics.report() }
                        copyReport(context, text)
                    }
                },
            )
        }
        Spacer(Modifier.height(22.dp))
        SectionTitle("Лицензии")
        Spacer(Modifier.height(10.dp))
        GroupCard {
            for (line in listOf(
                "Inter — шрифт (SIL OFL 1.1)",
                "Lucide — иконки (ISC)",
                "Jetpack Compose, Media3, OkHttp, Coil, kotlinx.serialization (Apache-2.0)",
            )) {
                PlText(
                    line,
                    PlType.secondary,
                    Pl.text2,
                    Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp),
                )
            }
        }
    }
}
