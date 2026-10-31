package dev.scenesync.player

import android.app.Application
import java.io.File

class SceneSyncApp : Application() {
    lateinit var controller: Controller
        private set

    override fun onCreate() {
        super.onCreate()
        // The crash screen runs in its own process and needs none of this.
        if (processName().endsWith(":crash")) return
        CrashLog.install(this)
        controller = Controller(this)
        controller.start()
    }

    private fun processName(): String =
        runCatching { File("/proc/self/cmdline").readText().trimEnd('\u0000') }.getOrDefault("")
}
