package ru.plastinka

import android.app.Application
import ru.plastinka.data.FckCensor
import ru.plastinka.data.TokenStore
import ru.plastinka.data.YandexApi

class PlastinkaApp : Application() {
    lateinit var tokenStore: TokenStore
        private set
    lateinit var api: YandexApi
        private set
    lateinit var fckCensor: FckCensor
        private set

    /** Slopless: метки ИИ-треков (работает, только если плагин включён). */
    lateinit var slopless: ru.plastinka.data.Slopless
        private set

    val http: okhttp3.OkHttpClient by lazy { YandexApi.defaultHttp() }

    /** Тексты песен: кэш только в памяти, токен берётся из хранилища при каждом запросе. */
    val lyrics: ru.plastinka.data.LyricsRepository by lazy {
        ru.plastinka.data.LyricsRepository(http) { tokenStore.load() }
    }

    /**
     * Звук на диске быть не должен (правило проекта: только потоковое
     * воспроизведение). Прежняя версия складывала треки в `filesDir/tracks`:
     * один раз в фоне удаляем этот каталог со всем содержимым. Сбой удаления
     * не страшен.
     */
    private fun removeLegacyAudio() {
        Thread {
            try {
                java.io.File(filesDir, "tracks").takeIf { it.exists() }?.deleteRecursively()
            } catch (_: Exception) {
                // Не удалось удалить: приложение это не роняет, повторим при следующем запуске.
            }
        }.apply { isDaemon = true }.start()
    }

    override fun onCreate() {
        super.onCreate()
        // Настройки и акцент грузим до первого кадра и до запуска сервиса воспроизведения.
        ru.plastinka.data.Prefs.load(this)
        ru.plastinka.ui.AccentState.load()
        tokenStore = TokenStore.get(this)
        api = YandexApi(http)
        fckCensor = FckCensor.get(http)
        slopless = ru.plastinka.data.Slopless.get(http, cacheDir)
        removeLegacyAudio()
        ru.plastinka.data.PlayLog.init(this)
    }
}
