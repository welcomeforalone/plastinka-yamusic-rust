package ru.plastinka

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.res.Configuration
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.runtime.getValue
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import ru.plastinka.ui.AppViewModel
import ru.plastinka.ui.PlastinkaTheme
import ru.plastinka.ui.Pip
import ru.plastinka.ui.RootScreen

class MainActivity : ComponentActivity() {

    private val model: AppViewModel by viewModels()

    /** Кнопки окна PiP: сообщение только внутри приложения, дальше его разбирает главный экран. */
    private val pipReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            intent?.action?.let { Pip.handler?.invoke(it) }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Diagnostics.event("экран: создан (восстановление состояния: ${savedInstanceState != null})")
        enableEdgeToEdge()
        ContextCompat.registerReceiver(this, pipReceiver, Pip.intentFilter(), ContextCompat.RECEIVER_NOT_EXPORTED)
        Pip.setInPip(isInPictureInPictureMode)
        // Поворот экрана не перезапускает загрузку: библиотека уже есть или грузится.
        if (model.loggedIn) model.ensureLibrary()
        setContent {
            PlastinkaTheme {
                val login by model.login.collectAsStateWithLifecycle()
                val library by model.library.collectAsStateWithLifecycle()
                val avatarUrl by model.avatarUrl.collectAsStateWithLifecycle()
                val loggedIn by model.loggedInState.collectAsStateWithLifecycle()
                RootScreen(
                    loggedIn = loggedIn,
                    login = login,
                    library = library,
                    avatarUrl = avatarUrl,
                    model = model,
                )
            }
        }
    }

    override fun onPictureInPictureModeChanged(isInPictureInPictureMode: Boolean, newConfig: Configuration) {
        super.onPictureInPictureModeChanged(isInPictureInPictureMode, newConfig)
        Pip.setInPip(isInPictureInPictureMode)
    }

    /**
     * Место для автовхода в PiP на Android до 12 (на 12+ он идёт через параметры
     * окна). Пока [Pip.autoEnter] выключен, как на десктопе, ничего не делает.
     */
    override fun onUserLeaveHint() {
        super.onUserLeaveHint()
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S && Pip.autoEnter && Pip.ready && Pip.lastPlaying) {
            Pip.enter(this, playing = true, liked = Pip.lastLiked)
        }
    }

    override fun onDestroy() {
        Diagnostics.event("экран: уничтожен (закрыт: $isFinishing, смена конфигурации: $isChangingConfigurations)")
        unregisterReceiver(pipReceiver)
        Pip.setInPip(false)
        super.onDestroy()
    }
}
