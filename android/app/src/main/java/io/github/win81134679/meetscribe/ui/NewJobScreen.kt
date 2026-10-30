package io.github.win81134679.meetscribe.ui

import android.net.Uri
import android.util.Log
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import io.github.win81134679.meetscribe.core.AudioImport
import io.github.win81134679.meetscribe.core.ImportedAudio
import io.github.win81134679.meetscribe.core.ProjectDetail
import io.github.win81134679.meetscribe.core.Repo
import io.github.win81134679.meetscribe.core.formatClock
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

/** 同桌面 new_job.rs 的語言清單（ISO-639-1；空字串＝自動判斷）。 */
private val LANGUAGES = listOf(
    "" to "自動判斷", "zh" to "中文", "en" to "English", "ja" to "日本語", "ko" to "한국어", "fr" to "Français",
    "de" to "Deutsch", "es" to "Español", "pt" to "Português", "it" to "Italiano", "ru" to "Русский",
    "th" to "ไทย", "vi" to "Tiếng Việt",
)

/** 表單草稿：切到別的分頁再回來還在。 */
private object Draft {
    var audio by mutableStateOf<ImportedAudio?>(null)
    var status by mutableStateOf<String?>(null)
    var cost by mutableStateOf<Double?>(null)
    var title by mutableStateOf("")
    var language by mutableStateOf("")
    var identify by mutableStateOf(true)
    var speakers by mutableStateOf("")
    var participants by mutableStateOf("")
    var keyterms by mutableStateOf("")
    var summary by mutableStateOf(true)
    var instructions by mutableStateOf("")
    var starting by mutableStateOf(false)

    fun reset() {
        audio = null; status = null; cost = null; title = ""; speakers = ""; participants = ""; keyterms = ""
        instructions = ""; starting = false
    }
}

@Composable
fun NewJobScreen(
    incoming: Uri?,
    onIncomingHandled: () -> Unit,
    onStarted: (String) -> Unit,
    onOpenSettings: () -> Unit,
) {
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    val keys by Repo.keys.collectAsState()
    val hasKey = keys["openrouter"] != null
    val d = Draft

    fun importUri(uri: Uri) {
        val old = d.audio
        d.audio = null
        d.cost = null
        d.status = "準備中…"
        scope.launch {
            try {
                old?.let(AudioImport::discard)
                val a = AudioImport.import(ctx, uri) { d.status = it }
                d.audio = a
                if (d.title.isBlank()) d.title = a.displayName.substringBeforeLast('.')
                d.status = null
                d.cost = (Repo.tryCall("estimate_cost", "duration" to a.duration) as? Number)?.toDouble()
            } catch (e: Exception) {
                d.status = null
                Log.w("MeetScribe", "匯入失敗：$uri", e)
                Repo.toasts.emit(true to (e.message ?: "無法匯入這個檔案"))
            }
        }
    }

    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri -> uri?.let(::importUri) }
    LaunchedEffect(incoming) {
        if (incoming != null) {
            importUri(incoming)
            onIncomingHandled()
        }
    }

    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Text("新增轉錄", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.SemiBold)

        if (!hasKey) SetupCard(onOpenSettings) else AccountRow()

        SectionCard("錄音檔", "支援手機上常見的錄音與影片格式（m4a、mp3、wav、aac、ogg、opus、amr、mp4…）。") {
            val a = d.audio
            when {
                d.status != null -> Busy(d.status!!)
                a != null -> {
                    Text(a.displayName, fontWeight = FontWeight.Medium)
                    Text(
                        buildString {
                            append("長度 ${formatClock(a.duration)}")
                            d.cost?.let { append("　預估轉錄費用 ${formatUsd(it)}") }
                        },
                        color = Pal.current.textMuted, style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
            IconLabelButton(Ic.Add, if (a == null) "選擇錄音檔" else "換一個檔案", primary = a == null, enabled = d.status == null) {
                picker.launch(arrayOf("audio/*", "video/*"))
            }
        }

        SectionCard("會議資訊", "寫得越清楚，說話者的名字與專有名詞越準。都可以留白。") {
            OutlinedTextField(d.title, { d.title = it }, label = { Text("標題") }, singleLine = true, modifier = Modifier.fillMaxWidth())
            SelectRow("會議語言", LANGUAGES, d.language) { d.language = it }
            SwitchRow("辨識說話者", d.identify, "分出誰說了哪一句（免費模型處理）") { d.identify = it }
            if (d.identify) {
                OutlinedTextField(
                    d.speakers, { d.speakers = it.filter(Char::isDigit).take(2) },
                    label = { Text("大約幾個人發言（選填）") }, singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    d.participants, { d.participants = it },
                    label = { Text("與會者，一行一位（可加職稱）") }, minLines = 2, modifier = Modifier.fillMaxWidth(),
                )
            }
            OutlinedTextField(
                d.keyterms, { d.keyterms = it },
                label = { Text("專有名詞、人名、產品代號（一行一個）") }, minLines = 2, modifier = Modifier.fillMaxWidth(),
            )
        }

        SectionCard("摘要") {
            SwitchRow("處理完自動產生摘要", d.summary, "依會議內容決定格式；預設用免費模型") { d.summary = it }
            if (d.summary) {
                OutlinedTextField(
                    d.instructions, { d.instructions = it },
                    label = { Text("摘要的額外指示（選填）") }, placeholder = { Text("例如：著重技術決策與待辦事項") },
                    minLines = 2, modifier = Modifier.fillMaxWidth(),
                )
            }
        }

        Button(
            onClick = {
                val a = d.audio ?: return@Button
                d.starting = true
                scope.launch {
                    val options = JSONObject()
                        .put("language", d.language.ifEmpty { JSONObject.NULL })
                        .put("identify_speakers", d.identify)
                        .put("expected_speakers", d.speakers.toIntOrNull()?.takeIf { it > 0 } ?: JSONObject.NULL)
                        .put("participants", JSONArray(lines(d.participants)))
                        .put("keyterms", JSONArray(lines(d.keyterms)))
                        .put("generate_summary", d.summary)
                        .put("summary_instructions", d.instructions.trim())
                    val r = Repo.tryCall(
                        "create_project",
                        "source" to a.file.absolutePath, "title" to d.title.trim(), "duration" to a.duration,
                        "options" to options, "start" to true,
                    ) as? JSONObject
                    d.starting = false
                    if (r != null) {
                        val p = ProjectDetail.from(r)
                        Repo.refreshProjects()
                        Draft.reset()
                        onStarted(p.id)
                    }
                }
            },
            enabled = hasKey && d.audio != null && !d.starting,
            modifier = Modifier.fillMaxWidth().height(52.dp),
        ) {
            Text(if (d.starting) "建立中…" else "開始轉錄", style = MaterialTheme.typography.titleMedium)
        }
        if (!hasKey) {
            Text("先完成上面的 OpenRouter 設定才能開始。", color = Pal.current.textMuted, style = MaterialTheme.typography.bodySmall)
        }
        Spacer(Modifier.height(8.dp))
    }
}

private fun lines(s: String) = s.lines().map { it.trim() }.filter { it.isNotEmpty() }

/** 短錄音的費用常常不到一分錢，用兩位小數會顯示成 US$0.00。 */
fun formatUsd(v: Double): String = when {
    v <= 0.0 -> "US$0"
    v < 0.01 -> "不到 US$0.01（約 US$%.4f）".format(v)
    else -> "US$%.2f".format(v)
}

/** 沒有 OpenRouter Key：三步驟設定（儲值 → 建立 Key → 貼上），同桌面版。 */
@Composable
private fun SetupCard(onOpenSettings: () -> Unit) {
    val ctx = LocalContext.current
    SectionCard(
        "開始之前：設定 OpenRouter",
        "逐字稿用 NVIDIA Nemotron 轉錄，是唯一付費的部分（依音訊長度計費）；說話者與摘要預設用免費模型。",
    ) {
        Step(1, "到 OpenRouter 儲值") { TextButton(onClick = { Files.openUrl(ctx, Links.OPENROUTER_CREDITS) }) { Text("開啟儲值頁 ↗") } }
        Step(2, "建立一把 API Key") { TextButton(onClick = { Files.openUrl(ctx, Links.OPENROUTER_KEYS) }) { Text("開啟 API Keys ↗") } }
        Step(3, "把金鑰貼在這裡") {}
        KeyBox("openrouter", "OpenRouter API Key", "sk-or-v1-…", emptyList())
        TextButton(onClick = onOpenSettings) { Text("另外設定 Google AI Studio 免費金鑰（建議）→") }
    }
}

@Composable
private fun Step(n: Int, text: String, action: @Composable () -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Chip("$n", Pal.current.accent)
        Spacer(Modifier.width(10.dp))
        Text(text, Modifier.weight(1f))
        action()
    }
}

/** 有 Key：一列帳戶資訊（餘額、儲值、API Keys）。開啟時自動查一次餘額。 */
@Composable
private fun AccountRow() {
    val ctx = LocalContext.current
    val tests by Repo.keyTests.collectAsState()
    val google = Repo.keys.collectAsState().value["google"]
    LaunchedEffect(Unit) {
        if (tests["openrouter"] == null) Repo.tryCall("test_key", "kind" to "openrouter")
    }
    SectionCard {
        val t = tests["openrouter"]
        Text(
            when {
                t == null -> "OpenRouter：查詢餘額中…"
                else -> "OpenRouter：${t.text}"
            },
            color = if (t?.ok == false) Pal.current.danger else Pal.current.text,
            style = MaterialTheme.typography.bodyMedium,
        )
        if (google == null) {
            Text(
                "尚未設定 Google AI Studio Key：說話者與摘要會改用 OpenRouter 的免費模型。",
                color = Pal.current.textMuted, style = MaterialTheme.typography.bodySmall,
            )
        }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            TextButton(onClick = { Files.openUrl(ctx, Links.OPENROUTER_CREDITS) }) { Text("儲值 ↗") }
            TextButton(onClick = { Files.openUrl(ctx, Links.OPENROUTER_KEYS) }) { Text("API Keys ↗") }
        }
    }
}
