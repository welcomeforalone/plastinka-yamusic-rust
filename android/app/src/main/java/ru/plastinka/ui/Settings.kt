package ru.plastinka.ui

import androidx.activity.compose.BackHandler
import androidx.annotation.DrawableRes
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import ru.plastinka.R

private const val UNOFFICIAL = "Неофициальный клиент, не связан с ООО «Яндекс»"

private enum class SettingsPage { Main, Appearance, About }

/** Раздел «Настройки»: главная и подэкраны «Оформление», «О программе». */
@Composable
fun SettingsScreen(library: LibraryUi, model: AppViewModel) {
    var page by rememberSaveable { mutableStateOf(SettingsPage.Main) }
    BackHandler(enabled = page != SettingsPage.Main) { page = SettingsPage.Main }
    when (page) {
        SettingsPage.Main -> SettingsMain(
            library = library,
            onLogout = model::logout,
            onOpen = { page = it },
        )
        SettingsPage.Appearance -> AppearanceScreen(onBack = { page = SettingsPage.Main })
        SettingsPage.About -> AboutScreen(onBack = { page = SettingsPage.Main })
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

/** Карточка-группа: surface1, радиус 20, рамка border, поля 16 по бокам. */
@Composable
private fun GroupCard(content: @Composable ColumnScope.() -> Unit) {
    val shape = RoundedCornerShape(20.dp)
    Column(
        Modifier
            .padding(horizontal = 16.dp)
            .fillMaxWidth()
            .clip(shape)
            .background(Pl.surface1, shape)
            .border(1.dp, Pl.border, shape)
            .padding(vertical = 4.dp),
        content = content,
    )
}

@Composable
private fun SettingsRow(
    @DrawableRes icon: Int,
    title: String,
    subtitle: String,
    onClick: () -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(start = 14.dp, end = 16.dp, top = 10.dp, bottom = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier.size(40.dp).clip(RoundedCornerShape(12.dp)).background(Pl.surface3),
            contentAlignment = Alignment.Center,
        ) {
            PlIcon(icon, 20.dp, Pl.text)
        }
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            PlText(title, PlType.bodyStrong, maxLines = 1)
            PlText(subtitle, PlType.caption, Pl.text3, maxLines = 1)
        }
        // Шеврона «вправо» в наборе иконок нет: поворачиваем «вниз» на −90°.
        PlIcon(R.drawable.ic_chevron_down, 16.dp, Pl.text3, Modifier.rotate(-90f))
    }
}

@Composable
private fun SettingsMain(
    library: LibraryUi,
    onLogout: () -> Unit,
    onOpen: (SettingsPage) -> Unit,
) {
    var confirmLogout by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(top = 12.dp, bottom = 16.dp)) {
        PlText("Настройки", PlType.display, modifier = Modifier.padding(horizontal = 20.dp))
        Spacer(Modifier.height(14.dp))
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
                Spacer(Modifier.width(8.dp))
                Pill("Выйти", kind = PillKind.Danger, height = 36.dp, onClick = { confirmLogout = true })
            }
        }
        Spacer(Modifier.height(12.dp))
        GroupCard {
            SettingsRow(R.drawable.ic_palette, "Оформление", "Акцент") { onOpen(SettingsPage.Appearance) }
            SettingsRow(R.drawable.ic_about, "О программе", "Версия, лицензии, авторство") {
                onOpen(SettingsPage.About)
            }
        }
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

/** Верхняя панель подэкрана: стрелка назад. */
@Composable
private fun BackBar(onBack: () -> Unit) {
    Box(
        Modifier.fillMaxWidth().height(52.dp).padding(start = 8.dp),
        contentAlignment = Alignment.CenterStart,
    ) {
        Box(
            Modifier.size(44.dp).clip(CircleShape).clickable(onClick = onBack),
            contentAlignment = Alignment.Center,
        ) {
            PlIcon(R.drawable.ic_back, 24.dp, Pl.text)
        }
    }
}

@Composable
private fun SectionTitle(text: String) {
    PlText(text, PlType.title, modifier = Modifier.padding(horizontal = 20.dp))
}

@Composable
private fun AppearanceScreen(onBack: () -> Unit) {
    val context = LocalContext.current
    val current = AccentState.current
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
        BackBar(onBack)
        PlText("Оформление", PlType.display, modifier = Modifier.padding(horizontal = 20.dp))
        Spacer(Modifier.height(18.dp))
        SectionTitle("Акцент")
        Spacer(Modifier.height(10.dp))
        GroupCard {
            Row(
                Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 14.dp, bottom = 6.dp),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                for (accent in Accent.entries) {
                    Box(
                        Modifier
                            .size(48.dp)
                            .clip(CircleShape)
                            .then(
                                if (accent == current) Modifier.border(2.dp, Pl.text, CircleShape) else Modifier,
                            )
                            .clickable { AccentState.select(context, accent) },
                        contentAlignment = Alignment.Center,
                    ) {
                        Box(Modifier.size(34.dp).background(accent.color, CircleShape))
                    }
                }
            }
            PlText(
                "${current.title} — кнопки, выбранный трек, ползунки",
                PlType.caption,
                Pl.text3,
                Modifier.padding(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 12.dp),
            )
        }
        Spacer(Modifier.height(22.dp))
        SectionTitle("Предпросмотр")
        Spacer(Modifier.height(10.dp))
        GroupCard {
            Row(
                Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 12.dp),
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Pill(
                    "Слушать",
                    Modifier.weight(1f),
                    height = 44.dp,
                    icon = R.drawable.ic_play_filled,
                )
                Pill(
                    "Перемешать",
                    Modifier.weight(1f),
                    kind = PillKind.Outline,
                    height = 44.dp,
                    icon = R.drawable.ic_shuffle,
                )
            }
        }
        Spacer(Modifier.height(16.dp))
    }
}

@Composable
private fun AboutScreen(onBack: () -> Unit) {
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
        BackBar(onBack)
        PlText("О программе", PlType.display, modifier = Modifier.padding(horizontal = 20.dp))
        Spacer(Modifier.height(18.dp))
        Column(
            Modifier.padding(horizontal = 20.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            PlText("Plastinka", PlType.title)
            PlText("Версия ${appVersion()}", PlType.secondary, Pl.text2)
            Spacer(Modifier.height(8.dp))
            PlText(UNOFFICIAL, PlType.body, Pl.text2)
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
        Spacer(Modifier.height(16.dp))
    }
}
