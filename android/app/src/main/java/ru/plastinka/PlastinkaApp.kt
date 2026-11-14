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

    override fun onCreate() {
        super.onCreate()
        tokenStore = TokenStore.get(this)
        api = YandexApi(http)
        fckCensor = FckCensor.get(http)
        ru.plastinka.data.TrackCache.init(this, http)
    }
}
