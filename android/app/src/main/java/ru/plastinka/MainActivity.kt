package ru.plastinka

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import ru.plastinka.ui.AppViewModel
import ru.plastinka.ui.PlastinkaTheme
import ru.plastinka.ui.RootScreen

class MainActivity : ComponentActivity() {

    private val model: AppViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        if (model.loggedIn) model.loadLibrary()
        setContent {
            PlastinkaTheme {
                val login by model.login.collectAsStateWithLifecycle()
                val library by model.library.collectAsStateWithLifecycle()
                val loggedIn by model.loggedInState.collectAsStateWithLifecycle()
                RootScreen(
                    loggedIn = loggedIn,
                    login = login,
                    library = library,
                    model = model,
                )
            }
        }
    }
}
