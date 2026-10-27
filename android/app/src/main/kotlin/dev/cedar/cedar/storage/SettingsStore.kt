package dev.cedar.cedar.storage

import android.content.Context
import android.content.SharedPreferences
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import dev.cedar.core.ServerSettings
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Where the app keeps its settings, as iOS/Cedar/Storage/ServerSettings.swift does: the access token
 * encrypted with an AES key that never leaves the Android Keystore (the iOS app's Keychain), the
 * rest in plain preferences.
 */
class SettingsStore(context: Context) {
    private val prefs: SharedPreferences = context.getSharedPreferences("cedar", Context.MODE_PRIVATE)

    fun load(): ServerSettings = ServerSettings(
        serverURL = prefs.getString(SERVER_URL, "") ?: "",
        accessToken = prefs.getString(ACCESS_TOKEN, null)?.let(TokenCipher::decrypt) ?: "",
        model = prefs.getString(MODEL, "") ?: "",
        reasoning = prefs.getString(REASONING, "") ?: "",
        plan = prefs.getString(PLAN, "") ?: "",
    )

    fun save(settings: ServerSettings) {
        val token = settings.accessToken.trim()
        prefs.edit()
            .putString(SERVER_URL, settings.serverURL.trim())
            .putString(MODEL, settings.model.trim())
            .putString(REASONING, settings.reasoning.trim())
            .putString(PLAN, settings.plan.trim())
            .apply { if (token.isEmpty()) remove(ACCESS_TOKEN) else putString(ACCESS_TOKEN, TokenCipher.encrypt(token)) }
            .apply()
    }

    /** The language picked: a tag, "" for Automatic, or null before the user has chosen one. */
    var language: String?
        get() = prefs.getString(LANGUAGE, null)
        set(value) {
            prefs.edit().apply { if (value == null) remove(LANGUAGE) else putString(LANGUAGE, value) }.apply()
        }

    /** Whether the server still has to be told the language (`PUT /api/settings/locale`). */
    var localePending: Boolean
        get() = prefs.getBoolean(LOCALE_PENDING, false)
        set(value) {
            prefs.edit().putBoolean(LOCALE_PENDING, value).apply()
        }

    /** Small values the push registrars keep: the token they last sent, and where to. */
    fun string(key: String): String? = prefs.getString(key, null)

    fun setString(key: String, value: String?) {
        prefs.edit().apply { if (value == null) remove(key) else putString(key, value) }.apply()
    }

    private companion object {
        const val SERVER_URL = "chatServerURL"
        const val ACCESS_TOKEN = "accessToken"
        const val MODEL = "model"
        const val REASONING = "reasoning"
        const val PLAN = "plan"
        const val LANGUAGE = "language"
        const val LOCALE_PENDING = "localePending"
    }
}

/** AES-GCM with a key in the Android Keystore; the stored text is base64 of IV + ciphertext. */
private object TokenCipher {
    private const val ALIAS = "cedar-access-token"
    private const val TRANSFORMATION = "AES/GCM/NoPadding"

    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getEntry(ALIAS, null) as? KeyStore.SecretKeyEntry)?.let { return it.secretKey }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return generator.generateKey()
    }

    fun encrypt(text: String): String {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val sealed = cipher.iv + cipher.doFinal(text.toByteArray(Charsets.UTF_8))
        return Base64.encodeToString(sealed, Base64.NO_WRAP)
    }

    /** `null` when the stored value can't be read (a key lost with a restore, say): ask again. */
    fun decrypt(stored: String): String? = try {
        val sealed = Base64.decode(stored, Base64.NO_WRAP)
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, sealed, 0, 12))
        String(cipher.doFinal(sealed, 12, sealed.size - 12), Charsets.UTF_8)
    } catch (_: Exception) {
        null
    }
}
