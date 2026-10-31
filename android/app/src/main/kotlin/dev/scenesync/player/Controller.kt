package dev.scenesync.player

import android.content.Context
import android.os.Build
import android.os.StatFs
import android.os.SystemClock
import android.util.Log
import dev.scenesync.engine.Desired
import dev.scenesync.engine.DeviceConfig
import dev.scenesync.engine.DeviceReport
import dev.scenesync.engine.MediaRef
import dev.scenesync.engine.PairRequest
import dev.scenesync.engine.PairResponse
import dev.scenesync.engine.SceneSyncJson
import dev.scenesync.engine.ServerMessage
import dev.scenesync.engine.TimeResponse
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineExceptionHandler
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.io.File
import java.io.IOException
import java.security.MessageDigest
import java.util.concurrent.TimeUnit

/**
 * Talks to the server: pairing, clock sync, config updates, downloads, and
 * status reports. Playback itself lives in [MainActivity], which evaluates
 * [config] every tick, so the player keeps running when the server is gone.
 */
class Controller(private val context: Context) {
    enum class Phase { FINDING_SERVER, NEED_SERVER_URL, PAIRING, CONNECTED, OFFLINE }

    data class Status(val phase: Phase, val message: String, val code: String? = null)

    private class Unauthorized : IOException("device token rejected")

    // An unexpected error in a background task is reported, not a crash.
    private val scope = CoroutineScope(
        SupervisorJob() + Dispatchers.IO + CoroutineExceptionHandler { _, e ->
            Log.e(TAG, "background task failed", e)
            reportError(CrashLog.describe(e, "background task").take(2_000))
        },
    )
    private val prefs = Prefs(context)
    val clock = ServerClock(prefs)

    private val http = OkHttpClient.Builder()
        .connectTimeout(5, TimeUnit.SECONDS)
        .readTimeout(60, TimeUnit.SECONDS)
        .pingInterval(30, TimeUnit.SECONDS)
        .build()

    private val mediaDir = File(context.filesDir, "media").apply { mkdirs() }
    private val configFile = File(context.filesDir, "config.json")

    private val _status = MutableStateFlow(Status(Phase.FINDING_SERVER, "Looking for the SceneSync server…"))
    val status: StateFlow<Status> = _status

    private val _config = MutableStateFlow(loadCachedConfig())
    /** Last fully downloaded config. Null until the first one arrives. */
    val config: StateFlow<DeviceConfig?> = _config

    /** What the activity is showing; drives reports. */
    val nowPlaying = MutableStateFlow<Desired>(Desired.Idle())

    @Volatile private var lastError: String? = null

    /** Average distance from the sync group's timeline, set by the activity. */
    @Volatile var syncErrorMs: Long? = null
    private val manualServerUrl = Channel<String>(Channel.CONFLATED)
    private var started = false

    private val appVersion = "android ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})"

    val updater = Updater(context, http) { reportError(it) }

    fun start() {
        if (started) return
        started = true
        CrashLog.take(context)?.let { reportError(it.take(2_000)) }
        scope.launch { keepRunning("connection") { mainLoop() } }
        scope.launch { keepRunning("reports") { reportLoop() } }
    }

    /** Runs [block] again after any unexpected error, so one bad reply can't stop the player. */
    private suspend fun keepRunning(name: String, block: suspend () -> Unit) {
        while (true) {
            try {
                block()
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Throwable) {
                Log.e(TAG, "$name failed", e)
                reportError(CrashLog.describe(e, name).take(2_000))
                _status.value = _status.value.copy(message = "Error: ${e.javaClass.simpleName}: ${e.message}; retrying")
                delay(5_000)
            }
        }
    }

    fun mediaFile(mediaId: String): File? {
        val ref = _config.value?.media?.firstOrNull { it.mediaId == mediaId } ?: return null
        return File(mediaDir, ref.sha256).takeIf { it.exists() }
    }

    fun reportError(message: String) {
        Log.w(TAG, message)
        lastError = message
    }

    /** Called when the user types a server address on the projector. */
    fun setServerUrl(url: String) {
        val clean = url.trim().trimEnd('/')
        if (clean.toHttpUrlOrNull() == null) {
            _status.value = _status.value.copy(message = "\"$clean\" isn't a valid address. Try something like http://192.168.1.50:8080")
            return
        }
        manualServerUrl.trySend(clean)
    }

    // -----------------------------------------------------------------------

    private suspend fun mainLoop() {
        while (true) {
            // A saved address the HTTP client can't use (older versions could
            // save a link-local IPv6 one) is dropped and the server found again.
            prefs.serverUrl?.let { if (it.toHttpUrlOrNull() == null) prefs.serverUrl = null }
            val server = prefs.serverUrl ?: findServer().also { prefs.serverUrl = it }
            val token = prefs.token ?: pair(server)
            try {
                connectionLoop(server, token)
            } catch (e: Unauthorized) {
                // The device was removed in the dashboard: pair again.
                prefs.token = null
            }
        }
    }

    private suspend fun findServer(): String {
        _status.value = Status(Phase.FINDING_SERVER, "Looking for the SceneSync server…")
        val result = CompletableDeferred<String>()
        val discovery = scope.launch {
            var first = true
            while (!result.isCompleted) {
                Discovery.find(context, 15_000)?.let { result.complete(it) }
                if (first && !result.isCompleted) {
                    first = false
                    _status.value = Status(
                        Phase.NEED_SERVER_URL,
                        "Server not found automatically. Type its address with the remote, or keep waiting.",
                    )
                }
                if (!result.isCompleted) delay(15_000)
            }
        }
        val manual = scope.launch { result.complete(manualServerUrl.receive()) }
        val url = result.await()
        discovery.cancel()
        manual.cancel()
        Log.i(TAG, "server: $url")
        return url
    }

    private suspend fun pair(server: String): String {
        val code = prefs.pairingCode
        _status.value = Status(Phase.PAIRING, "Enter this code in the SceneSync dashboard at $server", code)
        val req = PairRequest(
            deviceId = prefs.deviceId,
            code = code,
            name = Build.MODEL ?: "Projector",
            model = listOfNotNull(Build.MANUFACTURER, Build.MODEL).joinToString(" "),
            appVersion = appVersion,
        )
        val body = SceneSyncJson.encodeToString(PairRequest.serializer(), req).toRequestBody(JSON)
        while (true) {
            try {
                http.newCall(Request.Builder().url("$server/api/v1/pair").post(body).build()).execute().use { res ->
                    if (res.isSuccessful) {
                        val r = SceneSyncJson.decodeFromString(PairResponse.serializer(), res.body!!.string())
                        val token = r.token
                        if (r.status == "approved" && token != null) {
                            prefs.token = token
                            prefs.clearPairingCode()
                            return token
                        }
                    } else {
                        _status.value = _status.value.copy(message = "Server error ${res.code} while pairing; retrying")
                    }
                }
            } catch (e: IOException) {
                _status.value = _status.value.copy(message = "Cannot reach $server (${e.message}); retrying")
            }
            delay(3_000)
        }
    }

    private suspend fun connectionLoop(server: String, token: String) {
        var backoffMs = 1_000L
        while (true) {
            try {
                connectOnce(server, token)
                backoffMs = 1_000L
            } catch (e: Unauthorized) {
                throw e
            } catch (e: Exception) {
                Log.w(TAG, "connection: ${e.message}")
            }
            _status.value = Status(Phase.OFFLINE, "Server unreachable; playing from saved schedule")
            delay(backoffMs)
            backoffMs = (backoffMs * 2).coerceAtMost(30_000L)
        }
    }

    private suspend fun connectOnce(server: String, token: String) {
        syncClock(server)
        val messages = Channel<String>(Channel.UNLIMITED)
        val closed = CompletableDeferred<Throwable?>()
        val wsUrl = server.replaceFirst("http", "ws") + "/api/v1/device/ws"
        val ws = http.newWebSocket(
            Request.Builder().url(wsUrl).header("Authorization", "Bearer $token").build(),
            object : WebSocketListener() {
                override fun onMessage(webSocket: WebSocket, text: String) {
                    messages.trySend(text)
                }
                override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                    closed.complete(null)
                }
                override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                    closed.complete(if (response?.code == 401) Unauthorized() else t)
                }
            },
        )
        try {
            var lastSync = SystemClock.elapsedRealtime()
            while (!closed.isCompleted) {
                val text = withTimeoutOrNull(1_000) { messages.receive() }
                if (text != null) {
                    _status.value = Status(Phase.CONNECTED, "Connected to $server")
                    val msg = runCatching { SceneSyncJson.decodeFromString(ServerMessage.serializer(), text) }.getOrNull()
                    if (msg is ServerMessage.ConfigChanged && msg.revision != _config.value?.revision) {
                        try {
                            updateConfig(server, token)
                        } catch (e: Unauthorized) {
                            throw e
                        } catch (e: Exception) {
                            reportError("Update to revision ${msg.revision} failed: ${e.message}")
                        }
                    } else if (msg is ServerMessage.ConfigChanged) {
                        // Same config, but an earlier app update may still be due.
                        try {
                            updater.maybeUpdate(server, token, _config.value?.appUpdate)
                        } catch (e: Exception) {
                            reportError("App update: ${e.message}")
                        }
                    }
                }
                if (SystemClock.elapsedRealtime() - lastSync > 10 * 60_000) {
                    runCatching { syncClock(server) }
                    lastSync = SystemClock.elapsedRealtime()
                }
            }
            closed.await()?.let { throw it }
        } finally {
            ws.cancel()
        }
    }

    /** NTP-style: keep the sample with the shortest round trip. */
    private fun syncClock(server: String) {
        var best: Triple<Long, Long, Long>? = null // rtt, serverMs, sent
        var bestRecv = 0L
        repeat(8) {
            val sent = SystemClock.elapsedRealtime()
            http.newCall(Request.Builder().url("$server/api/v1/time").build()).execute().use { res ->
                val recv = SystemClock.elapsedRealtime()
                if (!res.isSuccessful) throw IOException("time: HTTP ${res.code}")
                val t = SceneSyncJson.decodeFromString(TimeResponse.serializer(), res.body!!.string())
                val rtt = recv - sent
                if (best == null || rtt < best!!.first) {
                    best = Triple(rtt, t.serverTimeMs, sent)
                    bestRecv = recv
                }
            }
        }
        best?.let { (_, serverMs, sent) -> clock.update(serverMs, sent, bestRecv) }
    }

    private fun updateConfig(server: String, token: String) {
        val cfg = http.newCall(authed("$server/api/v1/device/config", token).build()).execute().use { res ->
            if (res.code == 401) throw Unauthorized()
            if (!res.isSuccessful) throw IOException("config: HTTP ${res.code}")
            SceneSyncJson.decodeFromString(DeviceConfig.serializer(), res.body!!.string())
        }

        val missing = cfg.media.filter { !File(mediaDir, it.sha256).exists() }
        val needed = missing.sumOf { it.size }
        val free = StatFs(context.filesDir.path).availableBytes
        if (needed > free - RESERVE_BYTES) {
            throw IOException("Not enough storage: clips need ${needed / MB} MB, ${(free - RESERVE_BYTES) / MB} MB available")
        }
        for (m in missing) download(server, token, m)

        // Every clip is present and verified: apply atomically.
        val tmp = File(configFile.path + ".tmp")
        tmp.writeText(SceneSyncJson.encodeToString(DeviceConfig.serializer(), cfg))
        if (!tmp.renameTo(configFile)) throw IOException("could not save config")
        _config.value = cfg
        lastError = null
        Log.i(TAG, "applied revision ${cfg.revision}")

        try {
            updater.maybeUpdate(server, token, cfg.appUpdate)
        } catch (e: Exception) {
            reportError("App update: ${e.message}")
        }

        val keep = cfg.media.map { it.sha256 }.toSet()
        mediaDir.listFiles()?.forEach { f ->
            if (!f.name.endsWith(".part") && f.name !in keep) f.delete()
        }
    }

    /** Resumable download into `<sha>.part`, verified before it is renamed into place. */
    private fun download(server: String, token: String, m: MediaRef) {
        val part = File(mediaDir, "${m.sha256}.part")
        val have = if (part.exists()) part.length() else 0L
        val req = authed(server + m.url, token).apply { if (have > 0) header("Range", "bytes=$have-") }.build()
        http.newCall(req).execute().use { res ->
            if (res.code == 401) throw Unauthorized()
            if (!res.isSuccessful) throw IOException("download ${m.mediaId}: HTTP ${res.code}")
            val append = res.code == 206
            res.body!!.byteStream().use { input ->
                java.io.FileOutputStream(part, append).use { out -> input.copyTo(out, 64 * 1024) }
            }
        }
        val digest = MessageDigest.getInstance("SHA-256")
        part.inputStream().use { input ->
            val buf = ByteArray(64 * 1024)
            while (true) {
                val n = input.read(buf)
                if (n < 0) break
                digest.update(buf, 0, n)
            }
        }
        val got = digest.digest().joinToString("") { "%02x".format(it) }
        if (got != m.sha256) {
            part.delete()
            throw IOException("checksum mismatch for ${m.mediaId}")
        }
        if (!part.renameTo(File(mediaDir, m.sha256))) throw IOException("could not save ${m.mediaId}")
    }

    private fun authed(url: String, token: String) = Request.Builder().url(url).header("Authorization", "Bearer $token")

    private suspend fun reportLoop() {
        while (true) {
            val server = prefs.serverUrl
            val token = prefs.token
            if (server != null && token != null) {
                runCatching { sendReport(server, token) }
            }
            // Every 10 s, or sooner when playback changes.
            val current = nowPlaying.value
            withTimeoutOrNull(10_000) { nowPlaying.first { it != current } }
        }
    }

    private fun sendReport(server: String, token: String) {
        val playing = nowPlaying.value
        val stat = StatFs(context.filesDir.path)
        val report = DeviceReport(
            appliedRevision = _config.value?.revision,
            state = if (playing is Desired.Play) "playing" else "idle",
            mediaId = (playing as? Desired.Play)?.mediaId,
            storageFreeBytes = stat.availableBytes,
            storageTotalBytes = stat.totalBytes,
            mediaBytes = mediaDir.listFiles()?.sumOf { it.length() },
            clockOffsetMs = clock.wallOffsetMs(),
            appVersion = appVersion,
            appSha256 = runCatching { updater.ownSha256 }.getOrNull(),
            syncErrorMs = syncErrorMs,
            errors = listOfNotNull(lastError),
        )
        val body = SceneSyncJson.encodeToString(DeviceReport.serializer(), report).toRequestBody(JSON)
        http.newCall(authed("$server/api/v1/device/report", token).post(body).build()).execute().close()
    }

    private fun loadCachedConfig(): DeviceConfig? = runCatching {
        SceneSyncJson.decodeFromString(DeviceConfig.serializer(), configFile.readText())
    }.getOrNull()

    companion object {
        private const val TAG = "SceneSync"
        private val JSON = "application/json".toMediaType()
        private const val MB = 1024 * 1024
        /** Leave room for the system and the app itself. */
        private const val RESERVE_BYTES = 200L * MB
    }
}
