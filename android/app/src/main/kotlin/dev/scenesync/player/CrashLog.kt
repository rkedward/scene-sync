package dev.scenesync.player

import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.app.Activity
import android.graphics.Color
import android.view.Gravity
import android.widget.ScrollView
import android.widget.TextView
import java.io.File
import java.io.PrintWriter
import java.io.StringWriter

/**
 * Catches crashes, saves them in `crash.txt` and shows them on screen, so a
 * crash on a projector without adb can be photographed and fixed. The next
 * start also sends the crash to the dashboard.
 */
object CrashLog {
    private fun file(context: Context) = File(context.filesDir, "crash.txt")

    fun install(context: Context) {
        val app = context.applicationContext
        val previous = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread, error ->
            runCatching {
                val text = describe(error, thread.name)
                file(app).writeText(text)
                app.startActivity(
                    Intent(app, CrashActivity::class.java)
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
                        .putExtra("text", text),
                )
            }
            // Let Android end the process as usual.
            previous?.uncaughtException(thread, error)
        }
    }

    fun describe(error: Throwable, where: String): String {
        val sw = StringWriter()
        error.printStackTrace(PrintWriter(sw))
        return "SceneSync ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE}) crashed in $where\n" +
            "${Build.MANUFACTURER} ${Build.MODEL}, Android ${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT})\n\n" +
            sw.toString()
    }

    /** The last crash, removed once read. */
    fun take(context: Context): String? {
        val f = file(context)
        if (!f.exists()) return null
        return runCatching { f.readText() }.getOrNull().also { f.delete() }
    }
}

/**
 * Shows a crash report. Runs in its own process with no layout files, so it
 * still works when the player itself can't start.
 */
class CrashActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val text = intent.getStringExtra("text") ?: "SceneSync crashed, with no details."
        val view = TextView(this).apply {
            setTextColor(Color.WHITE)
            setBackgroundColor(Color.BLACK)
            textSize = 13f
            gravity = Gravity.START
            setPadding(32, 32, 32, 32)
            setText("SceneSync crashed. Please take a photo of this screen.\nPress Back to close.\n\n$text")
        }
        setContentView(ScrollView(this).apply { setBackgroundColor(Color.BLACK); addView(view) })
    }
}
