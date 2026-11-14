package ru.plastinka.data

import android.content.Context
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey

/** Токен OAuth в зашифрованном хранилище. */
class TokenStore(context: Context) {
    private val prefs = EncryptedSharedPreferences.create(
        context,
        "plastinka_token",
        MasterKey.Builder(context).setKeyScheme(MasterKey.KeyScheme.AES256_GCM).build(),
        EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
        EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
    )

    fun load(): String? = prefs.getString("access_token", null)
    fun save(token: String) = prefs.edit().putString("access_token", token).apply()
    fun clear() = prefs.edit().clear().apply()

    companion object {
        @Volatile private var instance: TokenStore? = null
        fun get(context: Context): TokenStore =
            instance ?: synchronized(this) {
                instance ?: TokenStore(context.applicationContext).also { instance = it }
            }
    }
}
