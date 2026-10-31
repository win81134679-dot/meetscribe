package io.github.win81134679.meetscribe.ui

import android.Manifest
import android.content.ClipData
import android.content.Context
import android.content.ClipboardManager
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import io.github.win81134679.meetscribe.core.Repo
import io.github.win81134679.meetscribe.core.UpdateUi
import io.github.win81134679.meetscribe.core.Updater
import io.github.win81134679.meetscribe.core.formatClock
import io.github.win81134679.meetscribe.service.Recording
import kotlinx.coroutines.delay

/** 複製文字到剪貼簿並提示。 */
fun copyText(ctx: Context, label: String, text: String) {
    ctx.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText(label, text))
    Repo.toasts.tryEmit(false to "$label 已複製")
}

/** 新版下載好時出現在「新增轉錄」頂端：一鍵更新。 */
@Composable
fun UpdateBanner() {
    val ctx = LocalContext.current
    val u by Repo.update.collectAsState()
    val r = u as? UpdateUi.Ready ?: return
    val pal = Pal.current
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(pal.accentFaint).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text("新版 ${r.version} 已備妥", fontWeight = FontWeight.SemiBold, color = pal.accentStrong)
        if (r.notes.isNotBlank()) Text(r.notes.trim(), style = MaterialTheme.typography.bodySmall, color = pal.textMuted)
        IconLabelButton(Ic.Download, "更新", primary = true) { Repo.installUpdate(ctx) }
    }
}

/** 設定頁：版本、識別碼、自動更新、檢查更新。 */
@Composable
fun AboutUpdateCard(autoUpdate: Boolean, onAutoUpdate: (Boolean) -> Unit) {
    val ctx = LocalContext.current
    val u by Repo.update.collectAsState()
    val pal = Pal.current
    SectionCard("關於與更新") {
        Text("語音轉錄 ${Repo.appVersion}（Android）", fontWeight = FontWeight.Medium)
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("本機識別碼　", color = pal.textMuted)
            Text(Repo.deviceId, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            TextButton(onClick = {
                copyText(ctx, "識別碼", "語音轉錄 裝置識別碼 ${Repo.deviceId}（Android ${Repo.appVersion}）")
            }) { Text("複製") }
        }
        Text("要單獨推送更新給這支手機時，把識別碼傳給發佈者。", color = pal.textMuted, style = MaterialTheme.typography.bodySmall)
        SwitchRow("自動下載新版", autoUpdate, "下載好會通知你，按一下「更新」就安裝") { onAutoUpdate(it) }
        Row(verticalAlignment = Alignment.CenterVertically) {
            val busy = u is UpdateUi.Checking || u is UpdateUi.Downloading
            IconLabelButton(Ic.Refresh, "檢查更新", enabled = !busy) { Repo.checkUpdate(manual = true) }
            Spacer(Modifier.width(10.dp))
            when (val s = u) {
                UpdateUi.Checking -> Text("檢查中…", color = pal.textMuted)
                is UpdateUi.Downloading -> Text(
                    "下載 ${s.version}… ${if (s.total > 0) s.done * 100 / s.total else 0}%", color = pal.textMuted,
                )
                UpdateUi.Latest -> Text("已是最新版", color = pal.success)
                is UpdateUi.Failed -> Text("檢查失敗", color = pal.danger)
                else -> {}
            }
        }
        (u as? UpdateUi.Failed)?.let { Text(it.message, color = pal.danger, style = MaterialTheme.typography.bodySmall) }
        (u as? UpdateUi.Ready)?.let { r ->
            Text("新版 ${r.version} 已下載好${if (r.targeted) "（指定推送）" else ""}", color = pal.accentStrong)
            if (!Updater.canInstall(ctx)) {
                Text(
                    "第一次更新要先允許本 App「安裝不明應用程式」，按「更新」會帶你去設定。",
                    color = pal.textMuted, style = MaterialTheme.typography.bodySmall,
                )
            }
            IconLabelButton(Ic.Download, "更新", primary = true) { Repo.installUpdate(ctx) }
        }
    }
}

/** 開始錄音（第一次先要麥克風權限）。`transcribe`：錄完直接帶進「新增轉錄」；false＝只存進錄音管理。 */
@Composable
fun rememberStartRecording(transcribe: Boolean): () -> Unit {
    val ctx = LocalContext.current
    val askMic = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) Recording.start(ctx, transcribe) else Repo.toasts.tryEmit(true to "沒有麥克風權限，無法錄音")
    }
    return {
        if (ContextCompat.checkSelfPermission(ctx, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
            Recording.start(ctx, transcribe)
        } else {
            askMic.launch(Manifest.permission.RECORD_AUDIO)
        }
    }
}

/** 錄音中：計時、音量、暫停／繼續、停止、捨棄。錄好的都會存進「錄音管理」。 */
@Composable
fun RecordingPanel() {
    val ctx = LocalContext.current
    val st by Recording.state.collectAsState()
    val pal = Pal.current
    var now by remember { mutableLongStateOf(android.os.SystemClock.elapsedRealtime()) }
    var level by remember { mutableFloatStateOf(0f) }
    LaunchedEffect(st.phase) {
        while (st.phase != Recording.Phase.Idle) {
            now = android.os.SystemClock.elapsedRealtime()
            // 音量條：取最大振幅，往下慢慢掉，比較好看
            level = maxOf(Recording.level(), level * 0.8f)
            delay(150)
        }
    }
    val recording = st.phase == Recording.Phase.Recording
    Row(verticalAlignment = Alignment.CenterVertically) {
        Dot(if (recording) pal.danger else pal.textMuted, 12)
        Spacer(Modifier.width(10.dp))
        Text(
            formatClock(st.elapsedMs(now) / 1000.0),
            style = MaterialTheme.typography.headlineMedium, fontFamily = FontFamily.Monospace,
        )
        Spacer(Modifier.width(10.dp))
        Text(if (recording) "錄音中" else "已暫停", color = pal.textMuted)
    }
    LinearProgressIndicator(
        progress = { if (recording) level else 0f }, modifier = Modifier.fillMaxWidth().height(6.dp),
    )
    Text("關掉螢幕、切到別的 App 都會繼續錄；錄好的會存在「錄音管理」。", color = pal.textMuted, style = MaterialTheme.typography.bodySmall)
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        IconLabelButton(Ic.Stop, if (Recording.transcribeAfter) "停止並使用" else "停止並儲存", primary = true) { Recording.stop(ctx) }
        if (recording) {
            IconLabelButton(Ic.Pause, "暫停") { Recording.pause(ctx) }
        } else {
            IconLabelButton(Ic.Record, "繼續錄") { Recording.resume(ctx) }
        }
        IconLabelButton(Ic.Delete, "捨棄") { Recording.discard(ctx) }
    }
}

/** 搜尋框（會議清單、逐字稿共用）。 */
@Composable
fun SearchField(value: String, placeholder: String, modifier: Modifier = Modifier, trailing: String? = null, onChange: (String) -> Unit) {
    OutlinedTextField(
        value, onChange,
        placeholder = { Text(placeholder) },
        leadingIcon = { Icon(Ic.Search, contentDescription = null) },
        trailingIcon = {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (trailing != null) Text(trailing, color = Pal.current.textMuted, style = MaterialTheme.typography.bodySmall)
                if (value.isNotEmpty()) IconButton(onClick = { onChange("") }) { Icon(Ic.Close, contentDescription = "清除") }
            }
        },
        singleLine = true,
        modifier = modifier.fillMaxWidth(),
    )
}
