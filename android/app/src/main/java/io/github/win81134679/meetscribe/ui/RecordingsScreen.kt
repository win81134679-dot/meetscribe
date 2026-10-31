package io.github.win81134679.meetscribe.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.win81134679.meetscribe.core.ProjectDetail
import io.github.win81134679.meetscribe.core.RecordingItem
import io.github.win81134679.meetscribe.core.Repo
import io.github.win81134679.meetscribe.core.Storage
import io.github.win81134679.meetscribe.core.formatClock
import io.github.win81134679.meetscribe.core.formatSize
import io.github.win81134679.meetscribe.service.Recording
import kotlinx.coroutines.launch
import java.io.File

/*
 * 錄音管理（2026-10-31 使用者：「我想多做一個頁面 錄音管理 不管有沒有轉錄 我希望該頁面做完善 有轉錄可以支援跳轉」）。
 * 清單：App 錄的音檔（未轉錄）＋每場會議的錄音；可以直接錄一段先存著，之後再轉錄。
 * 每一列左邊的播放鍵直接試聽（底部迷你播放器），點整列進詳細頁：完整播放器、資訊、轉錄或開啟逐字稿、分享、存到手機、改名、刪除。
 */

private enum class Filter(val label: String) { All("全部"), Untranscribed("未轉錄"), Transcribed("已轉錄") }

@Composable
fun RecordingsScreen(onOpen: (RecordingItem) -> Unit) {
    val items by Repo.recordings.collectAsState()
    val rec by Recording.state.collectAsState()
    val progress by Repo.progress.collectAsState()
    val pal = Pal.current
    LaunchedEffect(Unit) { runCatching { Repo.refreshRecordings() } }
    var query by rememberSaveable { mutableStateOf("") }
    var filter by rememberSaveable { mutableStateOf(Filter.All) }
    var playingKey by remember { mutableStateOf<String?>(null) }
    val playing = items.firstOrNull { it.key == playingKey }
    val player = rememberPlayer(playing?.path, autoPlay = true)
    val startRecording = rememberStartRecording(transcribe = false)

    Column(Modifier.fillMaxSize()) {
        LazyColumn(
            Modifier.weight(1f),
            contentPadding = PaddingValues(16.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            item {
                Text("錄音管理", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.SemiBold)
                if (items.isNotEmpty()) {
                    val usable = items.filter { !it.broken }
                    Text(
                        "${items.size} 段錄音　共 ${formatClock(usable.sumOf { it.duration })}　佔用 ${formatSize(items.sumOf { it.size })}",
                        color = pal.textMuted, style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
            item {
                SectionCard {
                    if (rec.phase != Recording.Phase.Idle) {
                        RecordingPanel()
                    } else {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            IconLabelButton(Ic.Record, "開始錄音", primary = true) { startRecording() }
                            Spacer(Modifier.width(12.dp))
                            Text(
                                "先錄起來，想轉錄時再轉；錄音不會自動刪除。",
                                color = pal.textMuted, style = MaterialTheme.typography.bodySmall, modifier = Modifier.weight(1f),
                            )
                        }
                    }
                }
            }
            if (items.isEmpty()) {
                item {
                    Text(
                        "還沒有錄音。按上面「開始錄音」，或到「新增轉錄」選手機裡的錄音檔。",
                        color = pal.textMuted, modifier = Modifier.padding(vertical = 24.dp),
                    )
                }
            } else {
                item {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        for (f in Filter.entries) {
                            val n = items.count { matches(it, f) }
                            FilterChip(selected = filter == f, onClick = { filter = f }, label = { Text("${f.label} $n") })
                        }
                    }
                }
                if (items.size >= 5 || query.isNotEmpty()) {
                    item { SearchField(query, "搜尋錄音名稱或日期") { query = it } }
                }
                val q = query.trim()
                val shown = items.filter { matches(it, filter) && (q.isEmpty() || it.title.contains(q, true) || it.createdAt.contains(q)) }
                if (shown.isEmpty()) item { Text("沒有符合的錄音。", color = pal.textMuted) }
                items(shown, key = { it.key }) { item ->
                    RecordingRow(
                        item,
                        playing = item.key == playingKey && player.playing,
                        progress = item.projectId?.let { progress[it] }?.fraction,
                        onPlay = {
                            if (item.key == playingKey) player.toggle() else playingKey = item.key
                        },
                        onClick = {
                            player.pause()
                            onOpen(item)
                        },
                    )
                }
            }
        }
        // 迷你播放器：正在試聽的那段
        if (playing != null) {
            Column(
                Modifier.fillMaxWidth().background(pal.surface).border(1.dp, pal.border).padding(start = 12.dp, end = 4.dp, top = 6.dp),
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        playing.title, Modifier.weight(1f), fontWeight = FontWeight.Medium,
                        maxLines = 1, overflow = TextOverflow.Ellipsis,
                    )
                    IconButton(onClick = { player.pause(); playingKey = null }) { Icon(Ic.Close, contentDescription = "關閉播放器") }
                }
                PlayerBar(player)
            }
        }
    }
}

private fun matches(item: RecordingItem, f: Filter) = when (f) {
    Filter.All -> true
    Filter.Untranscribed -> !item.isMeeting
    Filter.Transcribed -> item.isMeeting
}

@Composable
private fun recordingColor(item: RecordingItem): Color = when {
    item.broken -> Pal.current.danger
    !item.isMeeting -> Pal.current.textMuted
    item.busy -> Pal.current.accentStrong
    else -> statusColor(item.status)
}

private fun statusText(item: RecordingItem) = when {
    item.broken -> "不完整"
    item.busy && item.status != "running" -> "排隊中"
    item.busy -> "轉錄中"
    else -> item.statusLabel
}

@Composable
private fun RecordingRow(item: RecordingItem, playing: Boolean, progress: Float?, onPlay: () -> Unit, onClick: () -> Unit) {
    val pal = Pal.current
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .background(pal.surface)
            .border(1.dp, pal.border, RoundedCornerShape(16.dp))
            .clickable(onClick = onClick)
            .padding(start = 4.dp, end = 14.dp, top = 10.dp, bottom = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconButton(onClick = onPlay, enabled = item.playable) {
            Icon(if (playing) Ic.Pause else Ic.Play, contentDescription = if (playing) "暫停" else "試聽")
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Text(
                item.title, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Medium,
                maxLines = 1, overflow = TextOverflow.Ellipsis,
            )
            Text(
                buildString {
                    append(item.createdAt)
                    if (item.duration > 0) append("　${formatClock(item.duration)}")
                    if (item.size > 0) append("　${formatSize(item.size)}")
                },
                color = pal.textMuted, style = MaterialTheme.typography.bodySmall,
            )
            if (item.busy) {
                if (progress != null) LinearProgressIndicator(progress = { progress }, modifier = Modifier.fillMaxWidth())
                else LinearProgressIndicator(Modifier.fillMaxWidth())
            }
        }
        Spacer(Modifier.width(8.dp))
        Chip(statusText(item), recordingColor(item))
    }
}

// ---------- 詳細頁 ----------

/**
 * 一段錄音的頁面。`key` 是 [RecordingItem.key]；清單更新（例如轉錄完成）時跟著變，找不到（被刪掉）就返回。
 * `onTranscribe`：未轉錄的錄音帶到「新增轉錄」；`onOpenMeeting`：已建會議的開逐字稿頁。
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RecordingScreen(
    key: String,
    onBack: () -> Unit,
    onOpenMeeting: (String) -> Unit,
    onTranscribe: (RecordingItem) -> Unit,
) {
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    val items by Repo.recordings.collectAsState()
    val rec by Recording.state.collectAsState()
    val progress by Repo.progress.collectAsState()
    val pal = Pal.current
    var item by remember(key) { mutableStateOf(items.firstOrNull { it.key == key }) }
    LaunchedEffect(items) {
        val now = items.firstOrNull { it.key == key }
        // 轉錄完成後錄音庫那筆併進會議：改看那場會議
        val merged = item?.let { old -> items.firstOrNull { it.isMeeting && it.id == old.projectId } }
        when {
            now != null -> item = now
            merged != null -> item = merged
            // 被刪掉了（包括從疊在上面的會議頁刪掉、回來時這頁重新建立）：回清單
            else -> onBack()
        }
    }
    val it = item
    var menu by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    var meeting by remember { mutableStateOf<ProjectDetail?>(null) }
    var savedTo by remember(key) { mutableStateOf<File?>(null) }
    var saving by remember { mutableStateOf(false) }
    var cost by remember(key) { mutableStateOf<Double?>(null) }
    var sending by remember { mutableStateOf(false) }
    val player = rememberPlayer(it?.takeIf { r -> r.playable }?.path)
    LaunchedEffect(it?.key) {
        if (it != null && !it.isMeeting && it.duration > 0) {
            cost = (Repo.tryCall("estimate_cost", "duration" to it.duration) as? Number)?.toDouble()
        }
    }

    Column(Modifier.fillMaxSize()) {
        TopAppBar(
            title = {
                Text(
                    it?.title ?: "", maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.clickable(enabled = it != null && !it.busy) { renaming = true },
                )
            },
            navigationIcon = { IconButton(onClick = { player.pause(); onBack() }) { Icon(Ic.Back, contentDescription = "返回") } },
            windowInsets = WindowInsets(0),
            actions = {
                Box {
                    TextButton(onClick = { menu = true }, enabled = it != null) { Text("更多") }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        DropdownMenuItem(text = { Text("重新命名") }, enabled = it?.busy == false, onClick = { menu = false; renaming = true })
                        DropdownMenuItem(
                            text = { Text(if (it?.isMeeting == true) "刪除這場會議" else "刪除錄音", color = pal.danger) },
                            enabled = it?.busy == false,
                            onClick = {
                                menu = false
                                if (it?.isMeeting == true) scope.launch { meeting = Repo.loadProject(it.id) } else confirmDelete = true
                            },
                        )
                    }
                }
            },
        )
        if (it == null) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Busy("載入中…") }
            return@Column
        }
        Column(
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            SectionCard {
                when {
                    it.broken -> Text(
                        "這段錄音不完整（錄音時 App 被系統關掉），無法播放或轉錄，建議刪除。",
                        color = pal.danger,
                    )
                    player.mp == null -> Text("找不到錄音檔。", color = pal.textMuted)
                    else -> PlayerPanel(player)
                }
            }

            SectionCard("資訊") {
                InfoRow("錄製時間", it.createdAt)
                if (it.duration > 0) InfoRow("長度", formatClock(it.duration))
                if (it.size > 0) InfoRow("檔案大小", formatSize(it.size))
                it.ext?.let { e -> InfoRow("格式", e.uppercase()) }
                InfoRow("狀態", statusText(it), recordingColor(it))
            }

            if (it.isMeeting) {
                SectionCard("逐字稿與摘要") {
                    when {
                        it.busy -> {
                            val pr = progress[it.id]
                            Text(pr?.stageLabel ?: "排隊中", fontWeight = FontWeight.Medium)
                            if (pr != null) LinearProgressIndicator(progress = { pr.fraction }, modifier = Modifier.fillMaxWidth())
                            else LinearProgressIndicator(Modifier.fillMaxWidth())
                        }
                        it.status == "done" -> Text(
                            buildString {
                                append("${it.segments} 段逐字稿")
                                if (it.speakers > 0) append("　${it.speakers} 位說話者")
                                append(if (it.hasSummary) "　有摘要" else "　沒有摘要")
                            },
                            color = pal.textMuted,
                        )
                        else -> Text("轉錄${it.statusLabel}，可以到會議頁從中斷處續跑。", color = pal.textMuted)
                    }
                    Button(onClick = { player.pause(); onOpenMeeting(it.id) }, modifier = Modifier.fillMaxWidth()) {
                        Text("開啟逐字稿與摘要")
                    }
                }
            } else {
                SectionCard("轉錄", "逐字稿、說話者、摘要；完成後這段錄音會移到那場會議底下，手機上不會存兩份。") {
                    cost?.let { c -> Text("預估轉錄費用 ${formatUsd(c)}", color = pal.textMuted, style = MaterialTheme.typography.bodySmall) }
                    Button(
                        onClick = { player.pause(); onTranscribe(it) },
                        enabled = !it.broken && it.file != null && rec.phase == Recording.Phase.Idle,
                        modifier = Modifier.fillMaxWidth(),
                    ) { Text("轉錄這段錄音") }
                }
            }

            SectionCard("分享與保存") {
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    IconLabelButton(Ic.Share, "分享錄音檔", primary = true, enabled = it.playable) {
                        val f = it.file ?: return@IconLabelButton
                        scope.launch { runCatching { Files.shareAudio(ctx, f, it.title) }.onFailure { e -> Repo.toasts.emit(true to "無法分享：${e.message}") } }
                    }
                    IconLabelButton(Ic.Send, "傳到其他裝置", enabled = !it.busy && (it.isMeeting || it.playable)) { sending = true }
                    IconLabelButton(Ic.Download, if (saving) "存檔中…" else "存到手機", enabled = it.playable && !saving) {
                        saving = true
                        scope.launch {
                            val r = Repo.tryCall("save_recording", "id" to it.id, "kind" to it.kind) as? String
                            saving = false
                            if (r != null) {
                                val f = File(r)
                                savedTo = f
                                Storage.scan(ctx, listOf(f))
                                Repo.toasts.emit(false to "已存到「${Storage.displayPath(f.parentFile ?: f)}」")
                            }
                        }
                    }
                }
                Text(
                    if (it.isMeeting) "分享可以直接傳到 LINE、存到雲端硬碟或寄信；「傳到其他裝置」會把整場會議（逐字稿、摘要、錄音）傳過去。"
                    else "分享可以直接傳到 LINE、存到雲端硬碟或寄信；「傳到其他裝置」傳到你自己的電腦或另一支手機。",
                    color = pal.textMuted, style = MaterialTheme.typography.bodySmall,
                )
                savedTo?.let { f ->
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            Storage.displayPath(f), Modifier.weight(1f), color = pal.textMuted, style = MaterialTheme.typography.bodySmall,
                            maxLines = 2, overflow = TextOverflow.Ellipsis,
                        )
                        TextButton(onClick = {
                            val dir = f.parentFile
                            if (dir == null || !Storage.openFolder(ctx, dir)) {
                                scope.launch { Repo.toasts.emit(false to "這支手機沒有能直接開資料夾的 App，請到「檔案」App 的「文件 › 語音轉錄」") }
                            }
                        }) { Text("開啟資料夾") }
                    }
                }
            }
        }
    }

    if (renaming && it != null) {
        TextInputDialog("重新命名", it.title, onDismiss = { renaming = false }) { t ->
            renaming = false
            scope.launch {
                if (it.isMeeting) Repo.edit("rename_project", "id" to it.id, "title" to t)
                else Repo.tryCall("rename_recording", "id" to it.id, "title" to t)
                Repo.refreshRecordings()
            }
        }
    }
    if (confirmDelete && it != null) {
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            title = { Text("刪除這段錄音？") },
            text = { Text("錄音會從手機上刪除，無法復原。之前「存到手機」或分享出去的副本不受影響。") },
            confirmButton = {
                TextButton(onClick = {
                    confirmDelete = false
                    player.pause()
                    scope.launch {
                        // 成功回 JSONObject.NULL（不是 Kotlin 的 null）、失敗回 null 並由 tryCall 提示；兩種都重新整理清單
                        if (Repo.tryCall("delete_recording", "id" to it.id) != null) it.path?.let(::forgetRecordingInNewJob)
                        Repo.refreshRecordings()
                    }
                }) { Text("刪除", color = pal.danger) }
            },
            dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text("取消") } },
        )
    }
    if (sending && it != null) {
        SendToDeviceDialog(if (it.isMeeting) "meeting" else "recording", it.title, id = it.id) { sending = false }
    }
    meeting?.let { p ->
        DeleteMeetingDialog(p, onDismiss = { meeting = null }) {
            meeting = null
            player.pause()
            Repo.refreshRecordings()
        }
    }
}

@Composable
private fun InfoRow(label: String, value: String, color: Color = Pal.current.text) {
    Row {
        Text(label, Modifier.width(88.dp), color = Pal.current.textMuted)
        Text(value, color = color)
    }
}
