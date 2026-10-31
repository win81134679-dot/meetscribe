package io.github.win81134679.meetscribe.ui

import android.media.MediaPlayer
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.PrimaryTabRow
import androidx.compose.material3.Slider
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.win81134679.meetscribe.core.ProjectDetail
import io.github.win81134679.meetscribe.core.Repo
import io.github.win81134679.meetscribe.core.Segment
import io.github.win81134679.meetscribe.core.Storage
import io.github.win81134679.meetscribe.core.Transfers
import io.github.win81134679.meetscribe.core.formatClock
import io.github.win81134679.meetscribe.core.strings
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ProjectScreen(id: String, onBack: () -> Unit) {
    val scope = rememberCoroutineScope()
    var p by remember(id) { mutableStateOf<ProjectDetail?>(null) }
    val busySet by Repo.busy.collectAsState()
    val busy = id in busySet
    var tab by rememberSaveable(id) { mutableIntStateOf(0) }
    // 四個分頁可以左右滑動切換（2026-10-31 使用者：「按進去會議逐字稿頁面沒辦法左右滑」）
    val pager = rememberPagerState(initialPage = tab) { 4 }
    LaunchedEffect(pager.currentPage) { tab = pager.currentPage }
    var menu by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    var sending by remember { mutableStateOf(false) }
    val ctx = LocalContext.current

    LaunchedEffect(id) {
        p = Repo.loadProject(id)
        Repo.projectUpdates.collect { if (it.id == id) p = it }
    }

    val player = rememberPlayer(p?.audioPath)

    Column(Modifier.fillMaxSize()) {
        TopAppBar(
            title = {
                Text(
                    p?.summary?.title ?: "", maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.clickable(enabled = p != null && !busy) { renaming = true },
                )
            },
            navigationIcon = { IconButton(onClick = onBack) { Icon(Ic.Back, contentDescription = "返回") } },
            // 外層 Scaffold 已經留了狀態列的空間，這裡不要再加一次
            windowInsets = WindowInsets(0),
            actions = {
                Box {
                    TextButton(onClick = { menu = true }) { Text("更多") }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        DropdownMenuItem(text = { Text("重新命名") }, enabled = !busy, onClick = { menu = false; renaming = true })
                        DropdownMenuItem(
                            text = { Text("重新辨識說話者") },
                            enabled = !busy && (p?.chunksTotal ?: 0) > 0,
                            onClick = { menu = false; scope.launch { Repo.tryCall("reidentify", "id" to id) } },
                        )
                        DropdownMenuItem(
                            text = { Text("傳到其他裝置") },
                            enabled = p != null && !busy,
                            onClick = { menu = false; sending = true },
                        )
                        DropdownMenuItem(
                            text = { Text("分享錄音檔") },
                            enabled = p?.audioPath != null,
                            onClick = {
                                menu = false
                                p?.let { d -> scope.launch { shareRecording(ctx, d) } }
                            },
                        )
                        DropdownMenuItem(
                            text = { Text("刪除這場會議", color = Pal.current.danger) },
                            enabled = !busy,
                            onClick = { menu = false; confirmDelete = true },
                        )
                    }
                }
            },
        )
        val proj = p
        if (proj == null) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Busy("載入中…") }
            return@Column
        }
        StatusPanel(proj, busy)
        PrimaryTabRow(selectedTabIndex = pager.currentPage) {
            listOf("逐字稿", "摘要", "說話者", "匯出").forEachIndexed { i, t ->
                Tab(
                    selected = pager.currentPage == i,
                    onClick = { scope.launch { pager.animateScrollToPage(i) } },
                    text = { Text(t) },
                )
            }
        }
        HorizontalPager(state = pager, modifier = Modifier.weight(1f), verticalAlignment = Alignment.Top) { page ->
            Box(Modifier.fillMaxSize()) {
                when (page) {
                    0 -> TranscriptTab(proj, busy, player)
                    1 -> SummaryTab(proj, busy)
                    2 -> SpeakersTab(proj, busy)
                    else -> ExportTab(proj)
                }
            }
        }
    }

    if (renaming && p != null) {
        TextInputDialog("重新命名", p!!.summary.title, onDismiss = { renaming = false }) { t ->
            renaming = false
            scope.launch { Repo.edit("rename_project", "id" to id, "title" to t) }
        }
    }
    if (sending && p != null) {
        SendToDeviceDialog("meeting", p!!.summary.title, id = id) { sending = false }
    }
    if (confirmDelete && p != null) {
        DeleteMeetingDialog(p!!, onDismiss = { confirmDelete = false }) {
            confirmDelete = false
            Repo.refreshProjects()
            onBack()
        }
    }
}

/**
 * 刪除會議的確認（會議頁、錄音管理共用）。「同時刪除已匯出到手機的檔案」預設勾（2026-10-31 使用者：大部分人不會自己去資料夾刪），
 * 匯出的錄音也算在內。`onDeleted` 在刪除成功後呼叫。
 */
@Composable
fun DeleteMeetingDialog(p: ProjectDetail, onDismiss: () -> Unit, onDeleted: suspend () -> Unit) {
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    val exported = p.exportFiles
    var alsoFiles by remember { mutableStateOf(true) }
    val where = exported.firstOrNull()?.let { File(it).parentFile }?.let(Storage::displayPath)
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("刪除這場會議？") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(
                    if (p.keepsRecording) "逐字稿、摘要會刪除，無法復原；錄音仍保留在「錄音管理」。"
                    else "逐字稿、摘要與錄音都會刪除，無法復原。",
                )
                if (exported.isNotEmpty()) {
                    Row(
                        Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp))
                            .clickable(role = Role.Checkbox) { alsoFiles = !alsoFiles },
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Checkbox(checked = alsoFiles, onCheckedChange = null)
                        Spacer(Modifier.width(8.dp))
                        Column {
                            Text("同時刪除已匯出到手機的 ${exported.size} 個檔案")
                            if (where != null) {
                                Text(where, color = Pal.current.textMuted, style = MaterialTheme.typography.bodySmall)
                            }
                        }
                    }
                    if (!alsoFiles) {
                        Text("匯出的 Word、PDF、字幕與錄音檔會留在手機上。", color = Pal.current.textMuted, style = MaterialTheme.typography.bodySmall)
                    }
                } else {
                    Text("這場會議沒有匯出到手機的檔案。", color = Pal.current.textMuted, style = MaterialTheme.typography.bodySmall)
                }
            }
        },
        confirmButton = {
            TextButton(onClick = {
                val withFiles = alsoFiles && exported.isNotEmpty()
                scope.launch {
                    // 成功的回傳值是 {deleted, failed}；失敗丟 BridgeError，所以用 runCatching 判斷成敗
                    val r = runCatching { Repo.call("delete_project", "id" to p.id, "delete_exports" to withFiles) as? JSONObject }
                        .onFailure { Repo.toasts.emit(true to (it.message ?: "刪除失敗")) }
                    if (r.isSuccess) {
                        val res = r.getOrNull()
                        val deleted = res?.optJSONArray("deleted").strings()
                        val failed = res?.optJSONArray("failed").strings()
                        Storage.forget(ctx, deleted)
                        Repo.toasts.emit(
                            when {
                                failed.isNotEmpty() -> true to "會議已刪除；有 ${failed.size} 個匯出檔刪不掉，請到「檔案」App 手動刪除"
                                deleted.isNotEmpty() -> false to "已刪除會議與 ${deleted.size} 個匯出檔"
                                else -> false to "已刪除會議"
                            },
                        )
                        Repo.refreshRecordingsLater()
                        onDeleted()
                    } else {
                        onDismiss()
                    }
                }
            }) { Text("刪除", color = Pal.current.danger) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("取消") } },
    )
}

@Composable
private fun StatusPanel(p: ProjectDetail, busy: Boolean) {
    val scope = rememberCoroutineScope()
    val progress by Repo.progress.collectAsState()
    val logs by Repo.logs.collectAsState()
    val pal = Pal.current
    val s = p.summary
    if (!busy && s.status == "done") return
    Column(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 8.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(pal.surface)
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        if (busy) {
            val pr = progress[p.id]
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(pr?.stageLabel ?: "排隊中", fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f))
                TextButton(onClick = { scope.launch { Repo.tryCall("cancel", "id" to p.id) } }) { Text("取消") }
            }
            if (pr != null) LinearProgressIndicator(progress = { pr.fraction }, modifier = Modifier.fillMaxWidth())
            else LinearProgressIndicator(Modifier.fillMaxWidth())
            if (pr != null) Text(pr.message, color = pal.textMuted, style = MaterialTheme.typography.bodySmall)
            if (p.chunksTotal > 1) {
                Text("已轉錄 ${p.chunksDone}／${p.chunksTotal} 段", color = pal.textMuted, style = MaterialTheme.typography.bodySmall)
            }
            Text("可以切到別的 App 或關螢幕，處理會在背景繼續。", color = pal.textMuted, style = MaterialTheme.typography.bodySmall)
        } else {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Chip(s.statusLabel, statusColor(s.status))
                Spacer(Modifier.width(8.dp))
                Text(s.stageLabel, color = pal.textMuted, modifier = Modifier.weight(1f))
                if (s.resumable || s.status == "queued") {
                    TextButton(onClick = { scope.launch { Repo.tryCall("run", "id" to p.id) } }) { Text("從中斷處續跑") }
                }
            }
            s.error?.let { Text(it, color = pal.danger, style = MaterialTheme.typography.bodySmall) }
        }
        val lines = logs[p.id].orEmpty().takeLast(3)
        for (l in lines) {
            Text(
                l.message, style = MaterialTheme.typography.bodySmall, maxLines = 2, overflow = TextOverflow.Ellipsis,
                color = when (l.level) {
                    "error" -> pal.danger
                    "warn" -> pal.warning
                    else -> pal.textMuted
                },
            )
        }
    }
}

// ---------- 逐字稿 ----------

@Composable
private fun TranscriptTab(p: ProjectDetail, busy: Boolean, player: Player) {
    val pal = Pal.current
    val ctx = LocalContext.current
    var editing by remember { mutableStateOf<Segment?>(null) }
    var query by rememberSaveable(p.id) { mutableStateOf("") }
    val colors = p.speakers.associate { it.id to speakerColor(it.color) }
    Column(Modifier.fillMaxSize()) {
        if (p.segments.isNotEmpty()) {
            Row(Modifier.padding(start = 16.dp, end = 4.dp, top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                val q = query.trim()
                val hits = if (q.isEmpty()) null else p.segments.count { it.text.contains(q, true) || it.speakerName.contains(q, true) }
                SearchField(query, "搜尋逐字稿", Modifier.weight(1f), trailing = hits?.let { "$it 段" }) { query = it }
                IconButton(onClick = { copyText(ctx, "逐字稿", transcriptText(p)) }) {
                    Icon(Ic.Copy, contentDescription = "複製全文")
                }
            }
        }
        if (player.mp != null) {
            PlayerBar(player, Modifier.padding(horizontal = 12.dp, vertical = 4.dp))
        }
        if (p.segments.isEmpty()) {
            Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
                Text(if (busy) "逐字稿處理中，完成一段就會出現在這裡。" else "沒有逐字稿。", color = pal.textMuted)
            }
            return@Column
        }
        val currentId = if (player.playing) {
            p.segments.lastOrNull { it.start <= player.position + 0.05 }?.id
        } else null
        val q = query.trim()
        val shown = if (q.isEmpty()) p.segments else p.segments.filter { it.text.contains(q, true) || it.speakerName.contains(q, true) }
        // 跟隨播放：播到下一句就捲過去（使用者手動捲動時不搶）
        val list = rememberLazyListState()
        LaunchedEffect(currentId) {
            val idx = shown.indexOfFirst { it.id == currentId }
            if (idx >= 0 && !list.isScrollInProgress) list.animateScrollToItem((idx - 1).coerceAtLeast(0))
        }
        LazyColumn(
            state = list,
            contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            items(shown, key = { it.id }) { seg ->
                val c = seg.speaker?.let { colors[it] } ?: pal.textMuted
                Column(
                    Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(10.dp))
                        .background(if (seg.id == currentId) pal.accentFaint else pal.page)
                        .clickable(enabled = !busy) { editing = seg }
                        .padding(8.dp),
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Dot(c, 8)
                        Spacer(Modifier.width(6.dp))
                        Text(seg.speakerName, color = c, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.labelLarge)
                        Spacer(Modifier.width(8.dp))
                        Text(
                            (if (seg.estimated) "≈" else "") + formatClock(seg.start),
                            color = pal.accentStrong, style = MaterialTheme.typography.labelMedium, fontFamily = FontFamily.Monospace,
                            modifier = Modifier
                                .clip(RoundedCornerShape(6.dp))
                                .clickable(enabled = player.mp != null, role = Role.Button) { player.playFrom(seg.start) }
                                .padding(horizontal = 4.dp, vertical = 2.dp),
                        )
                        if (seg.edited) Text("（已修改）", color = pal.textMuted, style = MaterialTheme.typography.labelSmall)
                    }
                    Text(highlight(seg.text, q, pal.accentSoft), style = MaterialTheme.typography.bodyLarge)
                }
            }
        }
    }
    editing?.let { seg -> SegmentDialog(p, seg) { editing = null } }
}

@Composable
private fun SegmentDialog(p: ProjectDetail, seg: Segment, onDismiss: () -> Unit) {
    val scope = rememberCoroutineScope()
    var text by remember { mutableStateOf(seg.text) }
    var speaker by remember { mutableStateOf(seg.speaker) }
    var pick by remember { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("修改這一段　${formatClock(seg.start)}") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                if (p.speakers.isNotEmpty()) {
                    val name = p.speakers.firstOrNull { it.id == speaker }?.name ?: "（未指定）"
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text("說話者：", color = Pal.current.textMuted)
                        TextButton(onClick = { pick = true }) { Text(name) }
                    }
                }
                OutlinedTextField(text, { text = it }, minLines = 3, modifier = Modifier.fillMaxWidth())
            }
        },
        confirmButton = {
            TextButton(onClick = {
                onDismiss()
                scope.launch {
                    if (text.trim() != seg.text) Repo.edit("edit_segment", "id" to p.id, "segment" to seg.id, "text" to text)
                    if (speaker != null && speaker != seg.speaker) {
                        Repo.edit("set_segment_speaker", "id" to p.id, "segment" to seg.id, "speaker" to speaker)
                    }
                }
            }) { Text("儲存") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("取消") } },
    )
    if (pick) {
        ChoiceDialog("說話者", p.speakers.map { it.id to it.name }, speaker, onDismiss = { pick = false }) {
            speaker = it
            pick = false
        }
    }
}

// ---------- 摘要 ----------

@Composable
private fun SummaryTab(p: ProjectDetail, busy: Boolean) {
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    var regen by remember { mutableStateOf(false) }
    var working by remember(p.summaryDoc?.createdAt) { mutableStateOf(false) }
    val peers = Transfers.state.collectAsState().value.peers
    LaunchedEffect(Unit) {
        Repo.toasts.collect { (err, _) -> if (err) working = false }
    }
    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        val s = p.summaryDoc
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (s != null) IconLabelButton(Ic.Copy, "複製") { copyText(ctx, "摘要", s.markdown) }
            if (s != null) IconLabelButton(Ic.Share, "分享文字") { Files.shareText(ctx, s.markdown) }
            // 同步到其他裝置（標題、摘要、逐字稿、說話者），有同一場會議的會自動更新
            if (peers.isNotEmpty()) IconLabelButton(Ic.Send, "同步到其他裝置", enabled = !busy) { scope.launch { Transfers.syncMeeting(p.id) } }
            if (p.hasPreviousContent) IconLabelButton(Ic.Refresh, "還原上一版", enabled = !busy) {
                scope.launch {
                    if (Repo.tryCall("restore_content", "id" to p.id) != null) Repo.toasts.tryEmit(false to "已還原成同步之前的內容")
                }
            }
            IconLabelButton(Ic.Refresh, if (s == null) "產生摘要" else "重新產生", enabled = !busy && !working && p.segments.isNotEmpty()) {
                regen = true
            }
        }
        when {
            working -> Busy("產生摘要中…")
            s == null -> Text(if (busy) "處理完成後會自動產生摘要。" else "還沒有摘要。", color = Pal.current.textMuted)
            else -> {
                Text("${s.model}　${s.createdAt}", color = Pal.current.textMuted, style = MaterialTheme.typography.bodySmall)
                MarkdownText(s.markdown)
            }
        }
    }
    if (regen) {
        TextInputDialog(
            "重新產生摘要", p.summaryDoc?.instructions ?: p.options.optString("summary_instructions"),
            label = "額外指示（選填）", allowEmpty = true, onDismiss = { regen = false },
        ) { instr ->
            regen = false
            working = true
            scope.launch { if (Repo.tryCall("regenerate_summary", "id" to p.id, "instructions" to instr) == null) working = false }
        }
    }
}

// ---------- 說話者 ----------

@Composable
private fun SpeakersTab(p: ProjectDetail, busy: Boolean) {
    val scope = rememberCoroutineScope()
    var renaming by remember { mutableStateOf<String?>(null) }
    var merging by remember { mutableStateOf<String?>(null) }
    val pal = Pal.current
    LazyColumn(contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (p.speakers.isEmpty()) {
            item { Text(if (busy) "辨識完成後會列出說話者。" else "沒有說話者資料。", color = pal.textMuted) }
        }
        items(p.speakers, key = { it.id }) { sp ->
            Row(
                Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(12.dp))
                    .background(pal.surface)
                    .padding(12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Dot(speakerColor(sp.color), 14)
                Spacer(Modifier.width(10.dp))
                Column(Modifier.weight(1f)) {
                    Text(sp.name, fontWeight = FontWeight.Medium)
                    Text("${sp.segments} 段發言", color = pal.textMuted, style = MaterialTheme.typography.bodySmall)
                }
                IconButton(onClick = { renaming = sp.id }, enabled = !busy) { Icon(Ic.Edit, contentDescription = "改名") }
                if (p.speakers.size > 1) {
                    TextButton(onClick = { merging = sp.id }, enabled = !busy) { Text("合併到…") }
                }
            }
        }
        item {
            Text(
                "名字認錯了就改名；同一個人被分成兩位就用「合併到…」。整個重新辨識在右上「更多」。",
                color = pal.textMuted, style = MaterialTheme.typography.bodySmall,
            )
        }
    }
    renaming?.let { sid ->
        TextInputDialog("說話者名稱", p.speakers.first { it.id == sid }.name, onDismiss = { renaming = null }) { name ->
            renaming = null
            scope.launch { Repo.edit("rename_speaker", "id" to p.id, "speaker" to sid, "name" to name) }
        }
    }
    merging?.let { from ->
        ChoiceDialog(
            "把「${p.speakers.first { it.id == from }.name}」的發言併到", p.speakers.filter { it.id != from }.map { it.id to it.name },
            null, onDismiss = { merging = null },
        ) { into ->
            merging = null
            scope.launch { Repo.edit("merge_speakers", "id" to p.id, "from" to from, "into" to into) }
        }
    }
}

// ---------- 匯出 ----------

@Composable
private fun ExportTab(p: ProjectDetail) {
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    val settings by Repo.settings.collectAsState()
    // 這場會議建立時複製的輸出選項；舊專案沒有就用目前設定
    var opts by remember(p.id) {
        mutableStateOf(JSONObject((p.options.optJSONObject("export") ?: settings.optJSONObject("export") ?: JSONObject()).toString()))
    }
    var files by remember(p.id) { mutableStateOf(p.exported.map(::File).filter { it.isFile }) }
    var working by remember { mutableStateOf(false) }
    var fresh by remember(p.id) { mutableStateOf(false) }
    var sendFile by remember { mutableStateOf<File?>(null) }
    val formats = opts.optJSONArray("formats").strings().toSet()

    fun update(f: JSONObject.() -> Unit) {
        opts = JSONObject(opts.toString()).apply(f)
    }

    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        SectionCard("格式") {
            for (e in Repo.exporters) {
                Row(
                    Modifier.fillMaxWidth().clickable(role = Role.Checkbox) {
                        update { put("formats", JSONArray(if (e.id in formats) formats - e.id else formats + e.id)) }
                    },
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Checkbox(checked = e.id in formats, onCheckedChange = null)
                    Spacer(Modifier.width(8.dp))
                    Text("${e.label}（.${e.extension}）")
                }
            }
            SelectRow("Markdown／純文字內容", Repo.contents.map { it.id to it.label }, opts.optString("content", "Both")) { update { put("content", it) } }
            SwitchRow("時間戳記", opts.optBoolean("timestamps", true)) { v -> update { put("timestamps", v) } }
            SwitchRow("標出說話者", opts.optBoolean("speaker_names", true)) { v -> update { put("speaker_names", v) } }
            SwitchRow("同一人連續發言併成一段", opts.optBoolean("merge_turns", true)) { v -> update { put("merge_turns", v) } }
            SwitchRow("開頭附檔案資訊與發言統計", opts.optBoolean("header", true)) { v -> update { put("header", v) } }
            SwitchRow("附上錄音檔", opts.optBoolean("audio", true), "會議資料夾多放一份錄音，跟 Word、字幕放一起") { v -> update { put("audio", v) } }
        }
        IconLabelButton(null, if (working) "匯出中…" else "匯出", primary = true, enabled = !working && formats.isNotEmpty()) {
            working = true
            scope.launch {
                val r = Repo.tryCall("export", "id" to p.id, "options" to opts) as? JSONArray
                working = false
                if (r != null) {
                    files = r.strings().map(::File)
                    fresh = true
                    Storage.scan(ctx, files)
                    Repo.loadProject(p.id)?.let { Repo.projectUpdates.emit(it) }
                }
            }
        }
        // 剛匯出的檔所在處優先，其次核心算的會議資料夾（1.0 的會議以前匯出在舊的私有區，那份最後才用）
        val folder = ((if (fresh) files.firstOrNull()?.parent else null) ?: p.folder ?: files.firstOrNull()?.parent)
            ?.let(::File)?.takeIf { it.isDirectory }
        // 這次沒匯出過：列資料夾裡現有的檔（例如完成時自動匯出的）
        val listed = files.ifEmpty {
            folder?.listFiles()?.filter { it.isFile && !it.name.startsWith(".") }?.sortedBy { it.name }.orEmpty()
        }
        if (folder != null) {
            SectionCard("會議資料夾", Storage.displayPath(folder)) {
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    IconLabelButton(Ic.Folder, "開啟資料夾", primary = true) {
                        if (!Storage.openFolder(ctx, folder)) {
                            scope.launch { Repo.toasts.emit(false to "這支手機沒有能直接開資料夾的 App，檔案列在下面") }
                        }
                    }
                    IconLabelButton(Ic.Share, "全部分享", enabled = listed.isNotEmpty()) { Files.share(ctx, listed) }
                    if (p.audioPath != null) {
                        IconLabelButton(Ic.Mic, "分享錄音檔") { scope.launch { shareRecording(ctx, p) } }
                    }
                }
                Text(
                    "也可以在「檔案」App 的「文件 › 語音轉錄」找到；手機接電腦時在「Documents\\語音轉錄」。",
                    color = Pal.current.textMuted, style = MaterialTheme.typography.bodySmall,
                )
            }
        }
        if (listed.isNotEmpty()) {
            SectionCard("匯出的檔案", "用「分享」存到雲端硬碟、傳 LINE 或寄信；➤ 傳到自己的其他裝置。") {
                for (f in listed) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(f.name, Modifier.weight(1f), maxLines = 2, overflow = TextOverflow.Ellipsis)
                        IconButton(onClick = {
                            if (!Files.open(ctx, f)) scope.launch { Repo.toasts.emit(true to "手機上沒有能開 .${f.extension} 的 App") }
                        }) { Icon(Ic.OpenIn, contentDescription = "開啟", Modifier.size(20.dp)) }
                        IconButton(onClick = { Files.share(ctx, listOf(f)) }) { Icon(Ic.Share, contentDescription = "分享", Modifier.size(20.dp)) }
                        IconButton(onClick = { sendFile = f }) { Icon(Ic.Send, contentDescription = "傳到其他裝置", Modifier.size(20.dp)) }
                    }
                }
            }
        }
        if (p.sttUsd + p.llmUsd > 0 || p.modelStt.isNotEmpty()) {
            SectionCard("這場會議") {
                val pal = Pal.current
                Text("轉錄 US$%.3f　其他 US$%.3f".format(p.sttUsd, p.llmUsd), color = pal.textMuted, style = MaterialTheme.typography.bodySmall)
                if (p.modelStt.isNotEmpty()) {
                    Text(
                        "模型：${p.modelStt}／${p.modelAttribution}／${p.modelSummary}",
                        color = pal.textMuted, style = MaterialTheme.typography.bodySmall,
                    )
                }
                for (w in p.warnings) Text(w, color = pal.warning, style = MaterialTheme.typography.bodySmall)
            }
        }
    }
    sendFile?.let { f -> SendToDeviceDialog("file", f.name, path = f.absolutePath) { sendFile = null } }
}

@Composable
fun TextInputDialog(
    title: String,
    initial: String,
    label: String? = null,
    allowEmpty: Boolean = false,
    onDismiss: () -> Unit,
    onConfirm: (String) -> Unit,
) {
    var text by remember { mutableStateOf(initial) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            OutlinedTextField(text, { text = it }, label = label?.let { { Text(it) } }, modifier = Modifier.fillMaxWidth())
        },
        confirmButton = {
            TextButton(onClick = { onConfirm(text.trim()) }, enabled = allowEmpty || text.isNotBlank()) { Text("確定") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("取消") } },
    )
}

/** 分享這場會議的錄音（LINE、雲端硬碟…）：檔名用會議名稱。 */
private suspend fun shareRecording(ctx: android.content.Context, p: ProjectDetail) {
    val f = p.audioPath?.let(::File)?.takeIf { it.isFile }
    if (f == null) {
        Repo.toasts.emit(true to "找不到這場會議的錄音檔")
        return
    }
    runCatching { Files.shareAudio(ctx, f, p.summary.title) }.onFailure { Repo.toasts.emit(true to "無法分享：${it.message}") }
}

/** 複製用的逐字稿全文：[時間] 說話者：內容，一段一行。 */
private fun transcriptText(p: ProjectDetail): String = buildString {
    appendLine(p.summary.title)
    appendLine()
    for (seg in p.segments) {
        append("[").append(formatClock(seg.start)).append("] ")
        if (seg.speakerName.isNotBlank()) append(seg.speakerName).append("：")
        appendLine(seg.text)
    }
}

/** 搜尋字詞反白。 */
private fun highlight(text: String, q: String, bg: androidx.compose.ui.graphics.Color): androidx.compose.ui.text.AnnotatedString {
    if (q.isEmpty()) return androidx.compose.ui.text.AnnotatedString(text)
    return androidx.compose.ui.text.buildAnnotatedString {
        var i = 0
        while (true) {
            val j = text.indexOf(q, i, ignoreCase = true)
            if (j < 0) break
            append(text.substring(i, j))
            pushStyle(androidx.compose.ui.text.SpanStyle(background = bg, fontWeight = FontWeight.SemiBold))
            append(text.substring(j, j + q.length))
            pop()
            i = j + q.length
        }
        append(text.substring(i))
    }
}
