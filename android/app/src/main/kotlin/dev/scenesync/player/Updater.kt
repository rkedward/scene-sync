package dev.scenesync.player

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.os.Build
import android.util.Log
import dev.scenesync.engine.AppRelease
import okhttp3.OkHttpClient
import okhttp3.Request
import java.io.File
import java.io.IOException
import java.security.MessageDigest

/**
 * Installs the player APK the server offers when it differs from the running
 * one. The running APK is compared by SHA-256, so no version parsing is
 * needed. A release that failed to install is not retried until a different
 * one is offered.
 */
class Updater(
    private val context: Context,
    private val http: OkHttpClient,
    private val onError: (String) -> Unit,
) {
    private val sp = context.getSharedPreferences("scenesync_update", Context.MODE_PRIVATE)
    private val dir = File(context.filesDir, "update").apply { mkdirs() }

    /** SHA-256 of the installed APK. */
    val ownSha256: String by lazy { sha256(File(context.applicationInfo.sourceDir)) }

    @Synchronized
    fun maybeUpdate(server: String, token: String, release: AppRelease?) {
        if (release == null || release.sha256 == ownSha256) return
        if (sp.getString("failed", null) == release.sha256) return
        if (sp.getString("installing", null) == release.sha256 &&
            System.currentTimeMillis() - sp.getLong("installing_at", 0) < 10 * 60_000
        ) {
            return // Waiting for the installer (or for someone to press Install).
        }
        val apk = File(dir, "${release.sha256}.apk")
        if (!apk.exists()) download(server, token, release, apk)
        dir.listFiles()?.forEach { if (it != apk) it.delete() }
        // Never step back to an older build (Android refuses it anyway).
        val offered = archiveVersion(apk)
        if (offered != null && offered.second <= BuildConfig.VERSION_CODE) {
            sp.edit().putString("failed", release.sha256).apply()
            apk.delete()
            onError(
                "The Pi offers an older player app (${offered.first}); keeping ${BuildConfig.VERSION_NAME}. " +
                    "Click \"Get player … from GitHub\" in the dashboard to offer the latest.",
            )
            return
        }
        Log.i(TAG, "installing update ${release.sha256.take(12)}")
        sp.edit().putString("installing", release.sha256).putLong("installing_at", System.currentTimeMillis()).apply()
        install(apk)
    }

    fun installFailed(message: String) {
        val sha = sp.getString("installing", null)
        sp.edit().putString("failed", sha).remove("installing").apply()
        onError("App update failed: $message")
    }

    private fun download(server: String, token: String, release: AppRelease, dest: File) {
        val part = File(dir, "${release.sha256}.part")
        val req = Request.Builder().url(server + release.url).header("Authorization", "Bearer $token").build()
        http.newCall(req).execute().use { res ->
            if (!res.isSuccessful) throw IOException("app download: HTTP ${res.code}")
            res.body!!.byteStream().use { input -> part.outputStream().use { input.copyTo(it, 64 * 1024) } }
        }
        if (sha256(part) != release.sha256) {
            part.delete()
            throw IOException("app download: checksum mismatch")
        }
        if (!part.renameTo(dest)) throw IOException("app download: could not save")
    }

    /** (versionName, versionCode) of a downloaded APK, or null if Android can't read it. */
    @Suppress("DEPRECATION")
    private fun archiveVersion(apk: File): Pair<String, Long>? {
        val info = context.packageManager.getPackageArchiveInfo(apk.path, 0) ?: return null
        val code = if (Build.VERSION.SDK_INT >= 28) info.longVersionCode else info.versionCode.toLong()
        return (info.versionName ?: "?") to code
    }

    private fun install(apk: File) {
        val installer = context.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(context.packageName)
            if (Build.VERSION.SDK_INT >= 31) {
                setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
            }
        }
        val id = installer.createSession(params)
        installer.openSession(id).use { session ->
            session.openWrite("player.apk", 0, apk.length()).use { out ->
                apk.inputStream().use { it.copyTo(out, 64 * 1024) }
                session.fsync(out)
            }
            val flags = PendingIntent.FLAG_UPDATE_CURRENT or
                if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0
            val pending = PendingIntent.getBroadcast(context, id, Intent(context, InstallReceiver::class.java), flags)
            session.commit(pending.intentSender)
        }
    }

    companion object {
        private const val TAG = "SceneSync"

        fun sha256(file: File): String {
            val digest = MessageDigest.getInstance("SHA-256")
            file.inputStream().use { input ->
                val buf = ByteArray(64 * 1024)
                while (true) {
                    val n = input.read(buf)
                    if (n < 0) break
                    digest.update(buf, 0, n)
                }
            }
            return digest.digest().joinToString("") { "%02x".format(it) }
        }
    }
}
