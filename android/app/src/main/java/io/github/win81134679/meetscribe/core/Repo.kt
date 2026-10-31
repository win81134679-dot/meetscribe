package io.github.win81134679.meetscribe.core

import android.content.Context
import android.content.Intent
import androidx.core.content.ContextCompat
import io.github.win81134679.meetscribe.service.JobService
import io.github.win81134679.meetscribe.service.Recording
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

class BridgeError(message: String) : Exception(message)

/** 更新的狀態（設定頁「關於與更新」、新增轉錄頁頂端的提示）。 */
sealed interface UpdateUi {
    data object Idle : UpdateUi
    data object Checking : UpdateUi
    data class Downloading(val version: String, val done: Long, val total: Long) : UpdateUi
    data object Latest : UpdateUi
    data class Ready(val version: String, val notes: String, val apk: File, val targeted: Boolean) : UpdateUi
    data class Failed(val message: String) : UpdateUi
}

/**
 * App 與 Rust 核心之間唯一的出入口：同步請求走 [call]，背景事件由 [pollLoop] 收進各個 StateFlow，
 * 畫面只讀這些 Flow。跟桌面的 worker.rs／app.rs 分工相同。
 */
object Repo {
    private lateinit var app: Context
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var started = false

    val settings = MutableStateFlow(JSONObject())
    /** kind（openrouter／google）→ 遮罩後的金鑰；沒設定是 null。 */
    val keys = MutableStateFlow<Map<String, String?>>(emptyMap())
    val projects = MutableStateFlow<List<ProjectSummary>>(emptyList())
    /** 錄音管理頁：App 錄的音檔＋每場會議的錄音。 */
    val recordings = MutableStateFlow<List<RecordingItem>>(emptyList())
    val busy = MutableStateFlow<Set<String>>(emptySet())
    val progress = MutableStateFlow<Map<String, Progress>>(emptyMap())
    val logs = MutableStateFlow<Map<String, List<LogLine>>>(emptyMap())
    val keyTests = MutableStateFlow<Map<String, KeyTest>>(emptyMap())
    /** provider → 模型清單（或錯誤訊息，放在 Result 裡）。 */
    val models = MutableStateFlow<Map<String, Result<List<String>>>>(emptyMap())
    /** 摘要模型的下拉選單（排好序：免費在前、各組推薦的在前）；null＝還沒載入。 */
    val summaryModels = MutableStateFlow<Result<List<SummaryModel>>?>(null)
    /** 專案頁訂閱：某個專案有新內容。 */
    val projectUpdates = MutableSharedFlow<ProjectDetail>(extraBufferCapacity = 64)
    val toasts = MutableSharedFlow<Pair<Boolean, String>>(extraBufferCapacity = 16)
    val update = MutableStateFlow<UpdateUi>(UpdateUi.Idle)
    /** 這台的識別碼（發佈者用它單獨推送更新）。 */
    var deviceId: String = ""; private set

    var exporters: List<Exporter> = emptyList(); private set
    var contents: List<Choice> = emptyList(); private set
    var filenamePresets: List<String> = emptyList(); private set
    var exportsDir: File = File("."); private set
    var importsDir: File = File("."); private set
    /** 錄音機把檔寫在這裡（`<uuid>/<名稱>.m4a`），永久保存。 */
    var recordingsDir: File = File("."); private set
    var version: String = ""; private set
    /** App 的版本（versionName；Rust 的版本號跟它一樣，但測試版可以只改 APK 的）。 */
    var appVersion: String = ""; private set
    var initError: String? = null; private set

    fun init(context: Context) {
        if (started) return
        started = true
        app = context.applicationContext
        val data = File(app.filesDir, "data")
        val exports = Storage.exportsDir(app)
        appVersion = runCatching { app.packageManager.getPackageInfo(app.packageName, 0).versionName }.getOrNull().orEmpty()
        val r = JSONObject(Native.init(data.absolutePath, exports.absolutePath))
        if (!r.optBoolean("ok")) {
            initError = r.optString("error")
            return
        }
        runCatching { loadState(callSync("state") as JSONObject) }.onFailure { initError = it.message }
        scope.launch { pollLoop() }
        scope.launch { startUpdates() }
        refreshRecordingsLater()
        scope.launch { Transfers.load() }
    }

    // ---------- 更新 ----------

    private suspend fun startUpdates() {
        val info = runCatching { call("update_info", "version" to appVersion) as JSONObject }.getOrNull() ?: return
        deviceId = info.optString("device_id")
        info.optStringOrNull("updated_from")?.let { toasts.emit(false to "已更新到 $appVersion（原本 $it）") }
        info.optJSONObject("ready")?.let { r ->
            update.value = UpdateUi.Ready(r.optString("version"), r.optString("notes"), File(r.optString("path")), r.optBoolean("targeted"))
        }
        if (info.optBoolean("due") && settings.value.optBoolean("auto_update", true)) checkUpdate(manual = false)
    }

    fun checkUpdate(manual: Boolean) {
        if (update.value is UpdateUi.Checking || update.value is UpdateUi.Downloading) return
        scope.launch { tryCall("check_update", "version" to appVersion, "manual" to manual) }
    }

    /** 按「更新」：第一次先請使用者允許安裝；之後交給 PackageInstaller。 */
    fun installUpdate(ctx: Context) {
        val r = update.value as? UpdateUi.Ready ?: return
        if (!Updater.canInstall(ctx)) {
            toasts.tryEmit(false to "請允許「語音轉錄」安裝應用程式，回來再按一次「更新」")
            Updater.openInstallPermission(ctx)
            return
        }
        if (busy.value.isNotEmpty()) {
            toasts.tryEmit(true to "還有會議在處理，做完再更新")
            return
        }
        scope.launch {
            runCatching { Updater.install(app, r.apk) }
                .onFailure { toasts.emit(true to "無法安裝新版：${it.message ?: it.javaClass.simpleName}") }
        }
    }

    fun updateFailed(text: String) {
        toasts.tryEmit(true to text)
    }

    private fun handleUpdate(e: JSONObject) {
        val manual = e.optBoolean("manual")
        val next = when (e.optString("status")) {
            "checking" -> UpdateUi.Checking
            "downloading" -> UpdateUi.Downloading(e.optString("version"), e.optLong("done"), e.optLong("total"))
            "latest" -> UpdateUi.Latest.also { if (manual) toasts.tryEmit(false to "目前已是最新版：$appVersion") }
            "ready" -> UpdateUi.Ready(e.optString("version"), e.optString("notes"), File(e.optString("path")), e.optBoolean("targeted"))
            else -> UpdateUi.Failed(e.optString("error")).also { if (manual) toasts.tryEmit(true to "檢查更新失敗：${it.message}") }
        }
        val before = update.value
        update.value = next
        if (next is UpdateUi.Ready && (before as? UpdateUi.Ready)?.version != next.version) {
            Notifier.updateReady(app, next.version)
        }
    }

    private fun loadState(st: JSONObject) {
        settings.value = st.getJSONObject("settings")
        keys.value = parseKeys(st.getJSONObject("keys"))
        projects.value = st.optJSONArray("projects").objects().map(ProjectSummary::from).sortedByDescending { it.createdAt }
        busy.value = st.optJSONArray("busy").strings().toSet()
        exporters = st.optJSONArray("exporters").objects().map {
            Exporter(it.getString("id"), it.optString("label"), it.optString("extension"), it.optBoolean("has_summary"))
        }
        contents = st.optJSONArray("contents").objects().map { Choice(it.getString("id"), it.optString("label")) }
        filenamePresets = st.optJSONArray("filename_presets").strings()
        exportsDir = File(st.optString("exports_dir"))
        importsDir = File(st.optString("imports_dir"))
        recordingsDir = File(st.optString("recordings_dir"))
        version = st.optString("version")
    }

    fun parseKeys(o: JSONObject) = mapOf(
        "openrouter" to o.optStringOrNull("openrouter"),
        "google" to o.optStringOrNull("google"),
    )

    /** 同步呼叫 Rust；失敗丟 [BridgeError]。只能在背景執行緒用。 */
    fun callSync(op: String, args: Map<String, Any?> = emptyMap()): Any? {
        val req = JSONObject().put("op", op)
        for ((k, v) in args) req.put(k, v ?: JSONObject.NULL)
        val res = JSONObject(Native.call(req.toString()))
        if (!res.optBoolean("ok")) throw BridgeError(res.optString("error", "未知錯誤"))
        return res.opt("data")
    }

    suspend fun call(op: String, vararg args: Pair<String, Any?>): Any? =
        withContext(Dispatchers.IO) { callSync(op, args.toMap()) }

    /** 呼叫並把錯誤變成提示訊息；成功回結果，失敗回 null。 */
    suspend fun tryCall(op: String, vararg args: Pair<String, Any?>): Any? =
        try {
            call(op, *args)
        } catch (e: BridgeError) {
            toasts.emit(true to (e.message ?: "失敗"))
            null
        }

    suspend fun refreshProjects() {
        val list = (call("list_projects") as JSONArray).objects().map(ProjectSummary::from)
        projects.value = list
    }

    suspend fun refreshRecordings() {
        recordings.value = (call("list_recordings") as JSONArray).objects().map(RecordingItem::from)
    }

    fun refreshRecordingsLater() {
        scope.launch { runCatching { refreshRecordings() } }
    }

    /**
     * 錄音機停止後：登記進錄音庫（讀長度、寫 info.json），之後一定在錄音管理找得到。
     * 從「新增轉錄」開始錄的交給那頁當成選好的錄音；從錄音管理錄的只提示已儲存。
     */
    fun recordingStopped(file: File, title: String, transcribe: Boolean) {
        scope.launch {
            val r = try {
                RecordingItem.from(call("add_recording", "path" to file.absolutePath, "title" to title) as JSONObject)
            } catch (e: BridgeError) {
                toasts.emit(true to "錄音存檔失敗：${e.message}")
                null
            }
            runCatching { refreshRecordings() }
            when {
                r == null -> {}
                r.broken -> toasts.emit(true to "錄音檔不完整，無法使用；可到「錄音管理」刪除")
                transcribe -> Recording.finished.emit(r)
                else -> toasts.emit(false to "已存到「錄音管理」：${r.title}")
            }
        }
    }

    suspend fun loadProject(id: String): ProjectDetail? =
        (tryCall("load_project", "id" to id) as? JSONObject)?.let(ProjectDetail::from)

    suspend fun saveSettings(s: JSONObject): Boolean {
        val saved = tryCall("save_settings", "settings" to s) as? JSONObject ?: return false
        settings.value = saved
        return true
    }

    suspend fun saveKey(kind: String, key: String): Boolean {
        val k = tryCall("save_key", "kind" to kind, "key" to key) as? JSONObject ?: return false
        keys.value = parseKeys(k)
        keyTests.update { it - kind }
        return true
    }

    suspend fun deleteKey(kind: String) {
        val k = tryCall("delete_key", "kind" to kind) as? JSONObject ?: return
        keys.value = parseKeys(k)
        keyTests.update { it - kind }
    }

    /** 專案編輯（改名、合併說話者、改字…）：成功就推一份新內容給專案頁。 */
    suspend fun edit(op: String, vararg args: Pair<String, Any?>): ProjectDetail? {
        val v = tryCall(op, *args) as? JSONObject ?: return null
        val d = ProjectDetail.from(v)
        projectUpdates.emit(d)
        replaceSummary(d.summary)
        return d
    }

    private fun replaceSummary(s: ProjectSummary) {
        projects.update { list ->
            if (list.any { it.id == s.id }) list.map { if (it.id == s.id) s else it } else listOf(s) + list
        }
    }

    // ---------- 事件 ----------

    private fun pollLoop() {
        while (true) {
            val arr = runCatching { JSONArray(Native.poll(1000)) }.getOrNull() ?: continue
            for (e in arr.objects()) runCatching { handle(e) }
        }
    }

    private fun handle(e: JSONObject) {
        when (e.optString("type")) {
            "progress" -> progress.update {
                it + (e.getString("id") to Progress(
                    e.optString("stage_label"), e.optDouble("fraction", 0.0).toFloat(), e.optString("message"),
                ))
            }
            "log" -> logs.update {
                val id = e.getString("id")
                val line = LogLine(e.optString("level"), e.optString("message"))
                it + (id to ((it[id] ?: emptyList()) + line).takeLast(300))
            }
            "updated", "finished" -> {
                val d = ProjectDetail.from(e.getJSONObject("project"))
                projectUpdates.tryEmit(d)
                replaceSummary(d.summary)
                if (e.optString("type") == "finished") {
                    progress.update { it - d.id }
                    refreshRecordingsLater()
                    Storage.scan(app, d.exported.map(::File))
                    // 排隊跑完的才通知（重新產生摘要也會送 finished，但那時人就在畫面上）
                    if (d.summary.status == "done" && d.id in busy.value) Notifier.projectDone(app, d.id, d.summary.title)
                }
            }
            "update" -> handleUpdate(e)
            "toast" -> toasts.tryEmit(e.optBoolean("error") to e.optString("text"))
            "busy" -> {
                val ids = e.optJSONArray("ids").strings().toSet()
                busy.value = ids
                // 清單上的「排隊中／處理中」跟著變
                projects.update { list -> list.map { it.copy(busy = it.id in ids) } }
                if (ids.isNotEmpty()) startService()
                refreshRecordingsLater()
            }
            "transfer_inbox", "transfer_error", "transfer_progress", "transfer_done", "transfer_failed" -> Transfers.handle(e)
            "transfer_received" -> {
                Transfers.handle(e)
                when (e.optString("kind")) {
                    "file" -> Storage.scan(app, e.optJSONArray("paths").strings().map(::File))
                    "recording" -> refreshRecordingsLater()
                    else -> scope.launch { runCatching { refreshProjects() }; refreshRecordingsLater() }
                }
            }
            "key_test" -> keyTests.update { it + (e.getString("kind") to KeyTest(e.optBoolean("ok"), e.optString("text"))) }
            "summary_models" -> summaryModels.value =
                if (e.optBoolean("ok")) Result.success(e.optJSONArray("models").objects().map {
                    SummaryModel(it.optString("id"), it.optString("name"), it.optBoolean("free"), it.optBoolean("recommended"), it.optString("price"), it.optString("url"))
                }) else Result.failure(Exception(e.optString("error")))
            "models" -> models.update {
                val r = if (e.optBoolean("ok")) Result.success(e.optJSONArray("models").strings())
                else Result.failure(Exception(e.optString("error")))
                it + (e.getString("provider") to r)
            }
        }
    }

    private fun startService() {
        runCatching { ContextCompat.startForegroundService(app, Intent(app, JobService::class.java)) }
    }
}
