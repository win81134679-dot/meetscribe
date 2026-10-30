package io.github.win81134679.meetscribe.core

import android.content.Context
import android.content.Intent
import androidx.core.content.ContextCompat
import io.github.win81134679.meetscribe.service.JobService
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
    val busy = MutableStateFlow<Set<String>>(emptySet())
    val progress = MutableStateFlow<Map<String, Progress>>(emptyMap())
    val logs = MutableStateFlow<Map<String, List<LogLine>>>(emptyMap())
    val keyTests = MutableStateFlow<Map<String, KeyTest>>(emptyMap())
    /** provider → 模型清單（或錯誤訊息，放在 Result 裡）。 */
    val models = MutableStateFlow<Map<String, Result<List<String>>>>(emptyMap())
    /** 專案頁訂閱：某個專案有新內容。 */
    val projectUpdates = MutableSharedFlow<ProjectDetail>(extraBufferCapacity = 64)
    val toasts = MutableSharedFlow<Pair<Boolean, String>>(extraBufferCapacity = 16)

    var exporters: List<Exporter> = emptyList(); private set
    var contents: List<Choice> = emptyList(); private set
    var filenamePresets: List<String> = emptyList(); private set
    var exportsDir: File = File("."); private set
    var importsDir: File = File("."); private set
    var version: String = ""; private set
    var initError: String? = null; private set

    fun init(context: Context) {
        if (started) return
        started = true
        app = context.applicationContext
        val data = File(app.filesDir, "data")
        val exports = app.getExternalFilesDir("exports") ?: File(app.filesDir, "exports")
        val r = JSONObject(Native.init(data.absolutePath, exports.absolutePath))
        if (!r.optBoolean("ok")) {
            initError = r.optString("error")
            return
        }
        runCatching { loadState(callSync("state") as JSONObject) }.onFailure { initError = it.message }
        scope.launch { pollLoop() }
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
        version = st.optString("version")
    }

    private fun parseKeys(o: JSONObject) = mapOf(
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
                if (e.optString("type") == "finished") progress.update { it - d.id }
            }
            "toast" -> toasts.tryEmit(e.optBoolean("error") to e.optString("text"))
            "busy" -> {
                val ids = e.optJSONArray("ids").strings().toSet()
                busy.value = ids
                // 清單上的「排隊中／處理中」跟著變
                projects.update { list -> list.map { it.copy(busy = it.id in ids) } }
                if (ids.isNotEmpty()) startService()
            }
            "key_test" -> keyTests.update { it + (e.getString("kind") to KeyTest(e.optBoolean("ok"), e.optString("text"))) }
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
