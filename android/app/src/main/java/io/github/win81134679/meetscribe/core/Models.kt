package io.github.win81134679.meetscribe.core

import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/** Rust 端 bridge.rs 的 summary_view／full_view 轉成 Kotlin。欄位名稱是兩邊的契約。 */
data class ProjectSummary(
    val id: String,
    val title: String,
    val createdAt: String,
    val duration: Double,
    /** queued／running／done／failed／cancelled */
    val status: String,
    val statusLabel: String,
    val error: String?,
    val stageLabel: String,
    val resumable: Boolean,
    val busy: Boolean,
    val speakers: Int,
    val hasSummary: Boolean,
    val segments: Int,
    val totalUsd: Double,
) {
    companion object {
        fun from(o: JSONObject) = ProjectSummary(
            id = o.getString("id"),
            title = o.optString("title"),
            createdAt = o.optString("created_at"),
            duration = o.optDouble("duration", 0.0),
            status = o.optString("status"),
            statusLabel = o.optString("status_label"),
            error = o.optStringOrNull("error"),
            stageLabel = o.optString("stage_label"),
            resumable = o.optBoolean("resumable"),
            busy = o.optBoolean("busy"),
            speakers = o.optInt("speakers"),
            hasSummary = o.optBoolean("has_summary"),
            segments = if (o.opt("segments") is JSONArray) o.getJSONArray("segments").length() else o.optInt("segments"),
            totalUsd = o.optDouble("total_usd", 0.0),
        )
    }
}

data class Speaker(val id: String, val name: String, val color: Int, val segments: Int)

data class Segment(
    val id: Long,
    val start: Double,
    val end: Double,
    val speaker: String?,
    val speakerName: String,
    val text: String,
    val edited: Boolean,
    val estimated: Boolean,
)

data class Summary(val markdown: String, val model: String, val createdAt: String, val instructions: String)

/** 摘要模型下拉選單的一項（OpenRouter）。`price`：「免費」或「付費 · 輸入 $…／輸出 $…（每百萬 tokens）」；`url`：OpenRouter 官網的模型頁。 */
data class SummaryModel(val id: String, val name: String, val free: Boolean, val recommended: Boolean, val price: String, val url: String)

data class ProjectDetail(
    val summary: ProjectSummary,
    val audioPath: String?,
    val chunksTotal: Int,
    val chunksDone: Int,
    val speakers: List<Speaker>,
    val segments: List<Segment>,
    val summaryDoc: Summary?,
    val sttUsd: Double,
    val llmUsd: Double,
    val modelStt: String,
    val modelAttribution: String,
    val modelSummary: String,
    val warnings: List<String>,
    val exported: List<String>,
    val options: JSONObject,
    /** 這場會議的匯出資料夾（已存在才有）。 */
    val folder: String?,
    /** 匯出到手機、還在的檔（刪除會議時可一起刪）。 */
    val exportFiles: List<String>,
    /** 原始檔是錄音管理裡的錄音（還沒處理完）：刪掉會議後錄音還在。 */
    val keepsRecording: Boolean,
    /** 收過別台的會議內容更新，可以「還原上一版」。 */
    val hasPreviousContent: Boolean = false,
) {
    val id get() = summary.id

    companion object {
        fun from(o: JSONObject): ProjectDetail {
            val usage = o.optJSONObject("usage") ?: JSONObject()
            val models = o.optJSONObject("models") ?: JSONObject()
            return ProjectDetail(
                summary = ProjectSummary.from(o),
                audioPath = o.optStringOrNull("audio_path"),
                chunksTotal = o.optInt("chunks_total"),
                chunksDone = o.optInt("chunks_done"),
                speakers = o.optJSONArray("speakers").objects().map {
                    Speaker(it.getString("id"), it.optString("name"), it.optInt("color"), it.optInt("segments"))
                },
                segments = o.optJSONArray("segments").objects().map {
                    Segment(
                        id = it.getLong("id"),
                        start = it.optDouble("start", 0.0),
                        end = it.optDouble("end", 0.0),
                        speaker = it.optStringOrNull("speaker"),
                        speakerName = it.optString("speaker_name"),
                        text = it.optString("text"),
                        edited = it.optBoolean("edited"),
                        estimated = it.optBoolean("estimated"),
                    )
                },
                summaryDoc = o.optJSONObject("summary")?.let {
                    Summary(it.optString("markdown"), it.optString("model"), it.optString("created_at"), it.optString("instructions"))
                },
                sttUsd = usage.optDouble("stt_usd", 0.0),
                llmUsd = usage.optDouble("llm_usd", 0.0),
                modelStt = models.optString("stt"),
                modelAttribution = models.optString("attribution"),
                modelSummary = models.optString("summary"),
                warnings = o.optJSONArray("warnings").strings(),
                exported = o.optJSONArray("exported").strings(),
                options = o.optJSONObject("options") ?: JSONObject(),
                folder = o.optStringOrNull("folder"),
                exportFiles = o.optJSONArray("export_files").strings(),
                keepsRecording = o.optBoolean("keeps_recording"),
                hasPreviousContent = o.optBoolean("has_previous_content"),
            )
        }
    }
}

/**
 * 錄音管理頁的一列（bridge.rs 的 recording_view／meeting_recording_view）：
 * kind = recording（App 錄的、還沒轉錄）或 meeting（一場會議的錄音）。
 */
data class RecordingItem(
    val kind: String,
    val id: String,
    /** 已建會議的話是那場會議的 ID。 */
    val projectId: String?,
    val title: String,
    val createdAt: String,
    val duration: Double,
    val size: Long,
    val path: String?,
    val ext: String?,
    /** untranscribed／queued／running／done／failed／cancelled */
    val status: String,
    val statusLabel: String,
    val busy: Boolean,
    /** 檔案不完整（錄到一半 App 被系統關掉），播不了。 */
    val broken: Boolean,
    val segments: Int,
    val hasSummary: Boolean,
    val speakers: Int,
) {
    val isMeeting get() = kind == "meeting"
    /** 畫面導覽用的鍵（兩種的 ID 可能撞號，所以帶種類）。 */
    val key get() = "$kind:$id"
    val file: File? get() = path?.let(::File)?.takeIf { it.isFile }
    val playable get() = !broken && file != null

    companion object {
        fun from(o: JSONObject) = RecordingItem(
            kind = o.optString("kind"),
            id = o.getString("id"),
            projectId = o.optStringOrNull("project_id"),
            title = o.optString("title"),
            createdAt = o.optString("created_at"),
            duration = o.optDouble("duration", 0.0),
            size = o.optLong("size"),
            path = o.optStringOrNull("path"),
            ext = o.optStringOrNull("ext"),
            status = o.optString("status"),
            statusLabel = o.optString("status_label"),
            busy = o.optBoolean("busy"),
            broken = o.optBoolean("broken"),
            segments = o.optInt("segments"),
            hasSummary = o.optBoolean("has_summary"),
            speakers = o.optInt("speakers"),
        )
    }
}

data class Progress(val stageLabel: String, val fraction: Float, val message: String)

data class LogLine(val level: String, val message: String)

data class Exporter(val id: String, val label: String, val extension: String, val hasSummary: Boolean)

data class Choice(val id: String, val label: String)

data class KeyTest(val ok: Boolean, val text: String)

fun JSONObject.optStringOrNull(name: String): String? =
    if (isNull(name)) null else optString(name).takeIf { it.isNotEmpty() }

fun JSONArray?.objects(): List<JSONObject> =
    if (this == null) emptyList() else (0 until length()).mapNotNull { optJSONObject(it) }

fun JSONArray?.strings(): List<String> =
    if (this == null) emptyList() else (0 until length()).map { optString(it) }

/** 檔案大小：「14.5 MB」。 */
fun formatSize(bytes: Long): String = when {
    bytes >= 1L shl 30 -> "%.1f GB".format(bytes / (1L shl 30).toDouble())
    bytes >= 1L shl 20 -> "%.1f MB".format(bytes / (1L shl 20).toDouble())
    bytes >= 1L shl 10 -> "%.0f KB".format(bytes / (1L shl 10).toDouble())
    else -> "$bytes B"
}

fun formatClock(secs: Double): String {
    val s = secs.toLong().coerceAtLeast(0)
    val h = s / 3600
    val m = (s % 3600) / 60
    val sec = s % 60
    return if (h > 0) "%d:%02d:%02d".format(h, m, sec) else "%d:%02d".format(m, sec)
}
