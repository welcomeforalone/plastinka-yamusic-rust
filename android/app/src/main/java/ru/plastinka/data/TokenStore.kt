package ru.plastinka.data

import android.annotation.SuppressLint
import android.content.Context
import android.content.SharedPreferences
import android.util.Log
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import java.security.KeyStore
import ru.plastinka.Diagnostics

/**
 * Токен OAuth в зашифрованном хранилище. Сломанный Keystore (часть прошивок, перенос
 * данных на новый телефон) приложение не роняет: хранилище создаётся лениво, одна
 * попытка восстановления, потом оно считается недоступным. Запасного места для токена
 * нет (правило 5): без хранилища токен не сохраняется.
 *
 * Держится только `applicationContext` (живёт столько же, сколько процесс), а `commit()` нужен,
 * чтобы знать, записан ли токен.
 */
@SuppressLint("StaticFieldLeak", "ApplySharedPref")
class TokenStore(context: Context) {
    private val context = context.applicationContext
    private var prefs: SharedPreferences? = null
    private var failed = false

    private fun create(): SharedPreferences = EncryptedSharedPreferences.create(
        context,
        FILE,
        MasterKey.Builder(context).setKeyScheme(MasterKey.KeyScheme.AES256_GCM).build(),
        EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
        EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
    )

    /** В журнал и диагностику уходит только тип исключения: текст может нести лишнее. */
    private fun report(what: String, e: Exception) {
        Log.w(TAG, "$what: ${e.javaClass.simpleName}")
        Diagnostics.event("хранилище токена: $what: ${e.javaClass.simpleName}")
    }

    private fun open(): SharedPreferences? {
        // Две обычные попытки: сбой Keystore бывает и разовым, а восстановление ниже стирает вход.
        repeat(2) { attempt ->
            try {
                return create()
            } catch (e: Exception) {
                report(if (attempt == 0) "не открылось" else "не открылось повторно", e)
            }
        }
        // Одна попытка восстановления: бьются файл настроек или ключ, пересоздаём оба.
        return try {
            context.deleteSharedPreferences(FILE)
            val keyStore = KeyStore.getInstance("AndroidKeyStore")
            keyStore.load(null)
            keyStore.deleteEntry(MasterKey.DEFAULT_MASTER_KEY_ALIAS)
            create().also { Diagnostics.event("хранилище токена: пересоздано") }
        } catch (e: Exception) {
            report("не пересоздалось", e)
            null
        }
    }

    /** Хранилище или `null`, если оно недоступно. После неудачи заново пробует только [save]. */
    private fun store(retry: Boolean = false): SharedPreferences? = synchronized(this) {
        prefs?.let { return@synchronized it }
        if (failed && !retry) return@synchronized null
        open().also {
            prefs = it
            failed = it == null
        }
    }

    fun load(): String? = try {
        store()?.getString(KEY, null)
    } catch (e: Exception) {
        report("не прочиталось", e)
        null
    }

    /** `true` — токен записан. Любой сбой — `false`, токен нигде больше не хранится. */
    fun save(token: String): Boolean = try {
        store(retry = true)?.edit()?.putString(KEY, token)?.commit() == true
    } catch (e: Exception) {
        report("не записалось", e)
        false
    }

    fun clear() {
        try {
            store()?.edit()?.clear()?.commit()
        } catch (e: Exception) {
            report("не очистилось", e)
        }
    }

    companion object {
        private const val TAG = "TokenStore"
        private const val FILE = "plastinka_token"
        private const val KEY = "access_token"

        @Volatile private var instance: TokenStore? = null
        fun get(context: Context): TokenStore =
            instance ?: synchronized(this) {
                instance ?: TokenStore(context.applicationContext).also { instance = it }
            }
    }
}
