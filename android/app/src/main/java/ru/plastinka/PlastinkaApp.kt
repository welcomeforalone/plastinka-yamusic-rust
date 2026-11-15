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

    val http: okhttp3.OkHttpClient by lazy { YandexApi.defaultHttp() }

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
        tokenStore = TokenStore.get(this)
        api = YandexApi(http)
        fckCensor = FckCensor.get(http)
        removeLegacyAudio()
        ru.plastinka.data.PlayLog.init(this)
    }
}
