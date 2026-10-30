package io.github.win81134679.meetscribe.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Checkbox
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import io.github.win81134679.meetscribe.core.Repo
import io.github.win81134679.meetscribe.core.strings
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.roundToInt

/** 跟桌面 config.rs 的預設一致（Google 免費額度優先；沒 Google Key 時核心自動退到 OpenRouter :free）。 */
private val PROVIDERS = listOf("Google" to "Google AI Studio（免費額度）", "OpenRouter" to "OpenRouter")
private val DEFAULT_MODELS = mapOf(
    "attribution" to mapOf("Google" to "gemini-3.8-flash", "OpenRouter" to "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free"),
    "summary" to mapOf("Google" to "gemini-3.8-flash", "OpenRouter" to "nvidia/nemotron-3-ultra-550b-a55b:free"),
)
private val OUTPUT_LANGUAGES = listOf(
    "zh-TW" to "繁體中文（台灣用語）", "auto" to "跟會議語言相同", "zh-CN" to "简体中文", "en" to "English", "ja" to "日本語",
)

@Composable
fun SettingsScreen() {
    val scope = rememberCoroutineScope()
    val settings by Repo.settings.collectAsState()

    /** 改一個欄位就存（設定是整份 JSON 回寫，Rust 端會 sanitized）。 */
    fun save(f: JSONObject.() -> Unit) {
        val s = JSONObject(settings.toString()).apply(f)
        Repo.settings.value = s
        scope.launch { Repo.saveSettings(s) }
    }

    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Text("設定", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.SemiBold)

        SectionCard("API 金鑰", "金鑰只存在這支手機的 App 私有空間，其他 App 讀不到；解除安裝就清掉。") {
            KeyBox(
                "openrouter", "OpenRouter（逐字稿，付費）", "sk-or-v1-…",
                listOf("儲值" to Links.OPENROUTER_CREDITS, "API Keys" to Links.OPENROUTER_KEYS),
            )
            Spacer(Modifier.height(6.dp))
            KeyBox(
                "google", "Google AI Studio（說話者與摘要，免費額度）", "AIza…",
                listOf("取得金鑰" to Links.GOOGLE_AI_STUDIO_KEYS),
            )
        }

        SectionCard("模型", "只有逐字稿收費；說話者分配與摘要預設走免費模型。") {
            ModelPicker("說話者分配（要能聽音訊）", "attribution", settings) { p, m ->
                save { put("attribution_provider", p); put("attribution_model", m) }
            }
            ModelPicker("說話者整合與摘要", "summary", settings) { p, m ->
                save { put("summary_provider", p); put("summary_model", m) }
            }
            SelectRow("摘要語言", OUTPUT_LANGUAGES, settings.optString("output_language", "zh-TW")) {
                save { put("output_language", it) }
            }
        }

        SectionCard("處理") {
            val chunk = settings.optInt("chunk_minutes", 10)
            Text("每段長度：$chunk 分鐘", style = MaterialTheme.typography.bodyLarge)
            Text("段落短，單次處理時間短，比較不會撞到服務的逾時。", color = Pal.current.textMuted, style = MaterialTheme.typography.bodySmall)
            var chunkDraft by remember(chunk) { mutableStateOf(chunk.toFloat()) }
            Slider(
                value = chunkDraft, onValueChange = { chunkDraft = it }, valueRange = 2f..30f, steps = 27,
                onValueChangeFinished = { save { put("chunk_minutes", chunkDraft.roundToInt()) } },
            )
            val conc = settings.optInt("stt_concurrency", 3)
            Text("同時轉錄：$conc 段", style = MaterialTheme.typography.bodyLarge)
            var concDraft by remember(conc) { mutableStateOf(conc.toFloat()) }
            Slider(
                value = concDraft, onValueChange = { concDraft = it }, valueRange = 1f..6f, steps = 4,
                onValueChangeFinished = { save { put("stt_concurrency", concDraft.roundToInt()) } },
            )
            SwitchRow(
                "只用不保留、不拿資料訓練的供應商", settings.optBoolean("deny_data_collection"),
                "OpenRouter 的 data_collection=deny；可用的模型會變少",
            ) { v -> save { put("deny_data_collection", v) } }
        }

        ExportDefaults(settings) { ex -> save { put("export", ex) } }

        SectionCard("關於") {
            val pal = Pal.current
            Text("MeetScribe ${Repo.version}（Android）", style = MaterialTheme.typography.bodyMedium)
            Text("匯出資料夾：${Repo.exportsDir.absolutePath}", color = pal.textMuted, style = MaterialTheme.typography.bodySmall)
            Text(
                "提示詞可在電腦版自訂；手機版使用內建提示詞。",
                color = pal.textMuted, style = MaterialTheme.typography.bodySmall,
            )
        }
        Spacer(Modifier.height(8.dp))
    }
}

@Composable
private fun ModelPicker(title: String, role: String, settings: JSONObject, onChange: (String, String) -> Unit) {
    val scope = rememberCoroutineScope()
    val models by Repo.models.collectAsState()
    var provider = settings.optString("${role}_provider", "Google")
    // 手機沒有 Claude Code CLI：設成它的話顯示成 Google（核心在 Android 上也會退回）
    if (provider !in PROVIDERS.map { it.first }) provider = "Google"
    val model = settings.optString("${role}_model")
    var text by remember(model) { mutableStateOf(model) }
    var picking by remember { mutableStateOf(false) }
    val key = if (provider == "Google") "google" else "openrouter"

    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(title, fontWeight = FontWeight.Medium)
        SelectRow("供應商", PROVIDERS, provider) { p ->
            if (p != provider) onChange(p, DEFAULT_MODELS[role]!![p]!!)
        }
        OutlinedTextField(
            text, { text = it.trim() }, label = { Text("模型") }, singleLine = true, modifier = Modifier.fillMaxWidth(),
        )
        Row(verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = { onChange(provider, text) }, enabled = text.isNotEmpty() && text != model) { Text("套用") }
            TextButton(onClick = {
                picking = true
                Repo.models.value = Repo.models.value - key
                scope.launch { Repo.tryCall(if (key == "google") "load_google_models" else "load_openrouter_models") }
            }) { Text("從清單選…") }
            TextButton(onClick = { onChange(provider, DEFAULT_MODELS[role]!![provider]!!) }) { Text("恢復預設") }
        }
    }
    if (picking) {
        val r = models[key]
        when {
            r == null -> ChoiceDialog("載入中…", emptyList(), null, onDismiss = { picking = false }) {}
            r.isFailure -> ChoiceDialog(r.exceptionOrNull()?.message ?: "載入失敗", emptyList(), null, onDismiss = { picking = false }) {}
            else -> {
                val list = r.getOrDefault(emptyList())
                ChoiceDialog("選擇模型", list.map { it to it }, model, onDismiss = { picking = false }) {
                    picking = false
                    onChange(provider, it)
                }
            }
        }
    }
}

@Composable
private fun ExportDefaults(settings: JSONObject, onChange: (JSONObject) -> Unit) {
    val ex = settings.optJSONObject("export") ?: JSONObject()
    val formats = ex.optJSONArray("formats").strings().toSet()
    fun put(f: JSONObject.() -> Unit) = onChange(JSONObject(ex.toString()).apply(f))

    SectionCard("輸出", "新增轉錄時套用；每場會議也可以在「匯出」分頁另外選。") {
        SwitchRow("處理完成後自動匯出", ex.optBoolean("auto", true)) { v -> put { put("auto", v) } }
        Text("格式", fontWeight = FontWeight.Medium)
        for (e in Repo.exporters) {
            Row(
                Modifier.fillMaxWidth().clickable(role = Role.Checkbox) {
                    put { put("formats", JSONArray(if (e.id in formats) formats - e.id else formats + e.id)) }
                },
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Checkbox(checked = e.id in formats, onCheckedChange = null)
                Spacer(Modifier.width(8.dp))
                Text("${e.label}（.${e.extension}）")
            }
        }
        SelectRow("內容", Repo.contents.map { it.id to it.label }, ex.optString("content", "Both")) { v -> put { put("content", v) } }
        SelectRow("檔名", Repo.filenamePresets.map { it to it }, ex.optString("filename", "{標題}")) { v -> put { put("filename", v) } }
        SwitchRow("每場會議一個子資料夾", ex.optBoolean("subfolder")) { v -> put { put("subfolder", v) } }
        SwitchRow("時間戳記", ex.optBoolean("timestamps", true)) { v -> put { put("timestamps", v) } }
        SwitchRow("標出說話者", ex.optBoolean("speaker_names", true)) { v -> put { put("speaker_names", v) } }
        SwitchRow("同一人連續發言併成一段", ex.optBoolean("merge_turns", true)) { v -> put { put("merge_turns", v) } }
        SwitchRow("開頭附檔案資訊與發言統計", ex.optBoolean("header", true)) { v -> put { put("header", v) } }
    }
}
