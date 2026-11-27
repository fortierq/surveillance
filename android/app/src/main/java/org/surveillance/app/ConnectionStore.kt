package org.surveillance.app

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

class ConnectionStore(context: Context) {
    private val preferences = context.getSharedPreferences("host_connection", Context.MODE_PRIVATE)

    fun load(): Connection? {
        val values = preferences.all
        val origin = values["origin"] as? String ?: return null
        val ciphertext = values["credential"] as? String ?: return null
        val iv = values["iv"] as? String ?: return null
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, Base64.decode(iv, Base64.NO_WRAP)))
        cipher.updateAAD(origin.toByteArray(Charsets.UTF_8))
        val credential = cipher.doFinal(Base64.decode(ciphertext, Base64.NO_WRAP))
        return Connection(origin, credential.toString(Charsets.UTF_8))
    }

    fun save(connection: Connection) {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key())
        cipher.updateAAD(connection.origin.toByteArray(Charsets.UTF_8))
        val ciphertext = cipher.doFinal(connection.credential.toByteArray(Charsets.UTF_8))
        check(
            preferences.edit()
                .putString("origin", connection.origin)
                .putString("credential", Base64.encodeToString(ciphertext, Base64.NO_WRAP))
                .putString("iv", Base64.encodeToString(cipher.iv, Base64.NO_WRAP))
                .commit(),
        ) { "Could not save the paired connection" }
    }

    fun clear() {
        check(preferences.edit().clear().commit()) { "Could not clear the saved connection" }
    }

    private fun key(): SecretKey = synchronized(keyLock) {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(alias, null) as? SecretKey) ?: KeyGenerator
            .getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
                init(
                    KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                        .build(),
                )
            }.generateKey()
    }

    private companion object {
        const val alias = "surveillance_host_credential"
        val keyLock = Any()
    }
}
