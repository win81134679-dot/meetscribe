package io.github.win81134679.meetscribe.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.repeatOnLifecycle
import io.github.win81134679.meetscribe.core.IncomingItem
import io.github.win81134679.meetscribe.core.PairReq
import io.github.win81134679.meetscribe.core.PeerDevice
import io.github.win81134679.meetscribe.core.ReceivedInfo
import io.github.win81134679.meetscribe.core.Repo
import io.github.win81134679.meetscribe.core.Transfers
import io.github.win81134679.meetscribe.core.formatSize
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import org.json.JSONObject
import java.io.File

/*
 * 「傳到其他裝置」的畫面元件（2026-10-31 使用者：「用識別碼傳給自己別台裝置 要有資安防護 傳完線上馬上刪除」）：
 * 設定頁的裝置卡、配對對話框、要傳給哪台、收件詢問與進度。加密與上傳都在 Rust，這裡只叫 Repo。
 */

/** 設定頁：這台的名稱與識別碼、配對的裝置、配對新裝置。 */
@Composable
fun DevicesCard() {
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    val st by Transfers.state.collectAsState()
    val err by Transfers.lastError.collectAsState()
    val pal = Pal.current
    var renaming by remember { mutableStateOf(false) }
    var pairing by remember { mutableStateOf(false) }
    var unpairing by remember { mutableStateOf<PeerDevice?>(null) }
    if (!st.enabled) {
        SectionCard("傳到其他裝置", "中轉站尚未啟用，等發佈新版後就能用。") {}
        return
    }
    SectionCard(
        "傳到其他裝置",
        "會議、錄音、匯出檔加密後經中轉站傳到你自己的其他裝置，對方收完線上馬上刪除；沒收的 3 小時後自動刪除。",
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("這台名稱　", color = pal.textMuted)
            Text(st.name, fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f))
            TextButton(onClick = { renaming = true }) { Text("改名") }
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("識別碼　", color = pal.textMuted)
            Text(st.deviceId, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            TextButton(onClick = { copyText(ctx, "識別碼", st.deviceId) }) { Text("複製") }
        }
        Text("配對的裝置（最多 5 台）", fontWeight = FontWeight.Medium)
        if (st.peers.isEmpty()) {
            Text("還沒有。在另一台輸入這台的識別碼，或按下面「配對新裝置」。", color = pal.textMuted, style = MaterialTheme.typography.bodySmall)
        }
        for (p in st.peers) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(p.name)
                    Text("${p.id}　${p.pairedAt} 配對", color = pal.textMuted, style = MaterialTheme.typography.bodySmall)
                }
                TextButton(onClick = { unpairing = p }) { Text("取消配對", color = pal.danger) }
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            IconLabelButton(Ic.Add, "配對新裝置", primary = true, enabled = st.peers.size < 5) { pairing = true }
            if (st.peers.isNotEmpty()) IconLabelButton(Ic.Key, "立即同步金鑰") { scope.launch { Transfers.syncKeys() } }
        }
        Text(
            "配對的裝置之間會自動同步 API 金鑰：剛配對時互相補上對方沒有的，之後在任一台改了金鑰就推到其他台。一樣加密傳送、只給配對過的裝置。",
            color = pal.textMuted, style = MaterialTheme.typography.bodySmall,
        )
        err?.let { Text("中轉站：$it", color = pal.danger, style = MaterialTheme.typography.bodySmall) }
    }
    if (renaming) {
        TextInputDialog("這台的名稱", st.name, label = "配對清單上顯示的名稱", onDismiss = { renaming = false }) { n ->
            renaming = false
            scope.launch { (Repo.tryCall("transfer_set_name", "name" to n) as? JSONObject)?.let(Transfers::applyState) }
        }
    }
    if (pairing) PairDialog { pairing = false }
    unpairing?.let { p ->
        AlertDialog(
            onDismissRequest = { unpairing = null },
            title = { Text("取消跟「${p.name}」配對？") },
            text = { Text("兩邊都收不到對方的東西，還沒收的也會從中轉站刪除。之後要再傳得重新配對。") },
            confirmButton = {
                TextButton(onClick = {
                    unpairing = null
                    scope.launch { (Repo.tryCall("transfer_unpair", "peer" to p.id) as? JSONObject)?.let(Transfers::applyState) }
                }) { Text("取消配對", color = Pal.current.danger) }
            },
            dismissButton = { TextButton(onClick = { unpairing = null }) { Text("保留") } },
        )
    }
}

/** 配對新裝置：輸入對方識別碼 → 等對方接受 → 輸入對方畫面上的 6 位數配對碼（錯 3 次作廢）。 */
@Composable
private fun PairDialog(onDismiss: () -> Unit) {
    val scope = rememberCoroutineScope()
    val mine by Transfers.myPairRequests.collectAsState()
    val pal = Pal.current
    var target by remember { mutableStateOf("") }
    var reqId by remember { mutableStateOf<String?>(null) }
    var code by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }
    val status = reqId?.let { id -> mine.firstOrNull { it.id == id }?.status }
    // 等對方接受時每 3 秒查一次
    LaunchedEffect(reqId) {
        while (reqId != null) {
            Transfers.poll()
            delay(3000)
        }
    }
    AlertDialog(
        onDismissRequest = {},
        title = { Text("配對新裝置") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                if (reqId == null) {
                    Text("在另一台的「設定 › 傳到其他裝置」找到它的識別碼，輸入在這裡。")
                    OutlinedTextField(
                        target, { v -> target = v.uppercase().filter { it.isLetterOrDigit() || it == '-' }.take(9) },
                        label = { Text("對方的識別碼（例如 K7Q2-9XMA）") }, singleLine = true,
                        keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Characters),
                        modifier = Modifier.fillMaxWidth(),
                    )
                } else {
                    Text(
                        when (status) {
                            "accepted" -> "對方已接受。輸入「${target}」畫面上的 6 位數配對碼："
                            "declined" -> "對方拒絕了這次配對。"
                            else -> "已送出。請到「${target}」那台打開 App，按「接受」，它會顯示 6 位數配對碼。"
                        },
                    )
                    if (status != "declined") {
                        OutlinedTextField(
                            code, { v -> code = v.filter(Char::isDigit).take(6) },
                            label = { Text("配對碼") }, singleLine = true, enabled = status == "accepted",
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
                            modifier = Modifier.fillMaxWidth(),
                        )
                    }
                    Text("配對碼可以確認中間沒有人冒充，5 分鐘內有效。", color = pal.textMuted, style = MaterialTheme.typography.bodySmall)
                }
                message?.let { Text(it, color = pal.danger, style = MaterialTheme.typography.bodySmall) }
            }
        },
        confirmButton = {
            if (reqId == null) {
                TextButton(enabled = !busy && target.length == 9, onClick = {
                    busy = true
                    message = null
                    scope.launch {
                        val r = runCatching { Repo.call("transfer_pair_start", "to" to target) as JSONObject }
                        busy = false
                        r.onSuccess { reqId = it.optString("id") }.onFailure { message = it.message }
                    }
                }) { Text("送出") }
            } else if (status != "declined") {
                TextButton(enabled = !busy && status == "accepted" && code.length == 6, onClick = {
                    busy = true
                    message = null
                    scope.launch {
                        val r = runCatching { Repo.call("transfer_pair_confirm", "id" to reqId, "code" to code) as JSONObject }
                        busy = false
                        r.onFailure { message = it.message }
                        r.onSuccess { o ->
                            when (o.optString("result")) {
                                "paired" -> {
                                    Transfers.load()
                                    Repo.toasts.emit(false to "已跟「$target」配對")
                                    onDismiss()
                                }
                                "wrong" -> {
                                    code = ""
                                    message = "配對碼不對，還可以再試 ${o.optInt("left")} 次"
                                }
                                else -> {
                                    reqId = null
                                    code = ""
                                    message = "錯太多次，這次配對已作廢，請重新送出"
                                }
                            }
                        }
                    }
                }) { Text("確認") }
            }
        },
        dismissButton = {
            TextButton(onClick = {
                reqId?.let { id -> if (status != "declined") scope.launch { runCatching { Repo.call("transfer_pair_cancel", "id" to id) } } }
                onDismiss()
            }) { Text(if (reqId == null) "取消" else "關閉") }
        },
    )
}

/**
 * 「傳到其他裝置」：選一台配對的裝置送出。`kind`：meeting（id＝會議）／recording（id＝錄音庫的錄音）／file（path）。
 */
@Composable
fun SendToDeviceDialog(kind: String, title: String, id: String? = null, path: String? = null, onDismiss: () -> Unit) {
    val scope = rememberCoroutineScope()
    val st by Transfers.state.collectAsState()
    val pal = Pal.current
    LaunchedEffect(Unit) { Transfers.load() }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("傳到其他裝置") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("「$title」要傳給哪一台？", maxLines = 3, overflow = TextOverflow.Ellipsis)
                when {
                    !st.enabled -> Text("中轉站尚未啟用。", color = pal.textMuted)
                    st.peers.isEmpty() -> Text("還沒有配對的裝置：到「設定 › 傳到其他裝置」按「配對新裝置」。", color = pal.textMuted)
                    else -> for (p in st.peers) {
                        Row(
                            Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(pal.accentFaint)
                                .clickable {
                                    onDismiss()
                                    scope.launch {
                                        Repo.tryCall("transfer_send", "peer" to p.id, "kind" to kind, "id" to id, "path" to path)
                                    }
                                }
                                .padding(12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(p.name, fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f))
                            Text(p.id, color = pal.textMuted, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
                Text("加密後經中轉站傳送，對方收完線上馬上刪除；對方 3 小時內沒收也會刪除。", color = pal.textMuted, style = MaterialTheme.typography.bodySmall)
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("取消") } },
    )
}

/**
 * 全域：查收件匣（畫面開著時每 15 秒一次）、別台的配對請求、收到的東西要不要接收、傳送／接收進度、收好了開啟。
 * 放在 AppRoot 的內容上方。
 */
@Composable
fun TransferOverlays(onOpenProject: (String) -> Unit, onOpenRecording: (String) -> Unit) {
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val st by Transfers.state.collectAsState()
    val requests by Transfers.pairRequests.collectAsState()
    val accepted by Transfers.accepted.collectAsState()
    val incoming by Transfers.incoming.collectAsState()
    val jobs by Transfers.jobs.collectAsState()
    var dismissed by remember { mutableStateOf(setOf<String>()) }
    var done by remember { mutableStateOf<ReceivedInfo?>(null) }
    val pal = Pal.current

    // 一打開先查一次，之後每 15 秒（金鑰改了要馬上同步到其他台）；等對方輸入配對碼時每 5 秒；Transfers.wake 可以叫它馬上查
    LaunchedEffect(st.enabled) {
        if (!st.enabled) return@LaunchedEffect
        lifecycle.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            while (true) {
                Transfers.poll()
                val wait = if (Transfers.accepted.value.isNotEmpty()) 5_000L else 15_000L
                withTimeoutOrNull(wait) { Transfers.wake.first() }
            }
        }
    }
    LaunchedEffect(Unit) { Transfers.received.collect { done = it } }

    // 進度
    if (jobs.isNotEmpty()) {
        Column(
            Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp).clip(RoundedCornerShape(12.dp))
                .background(pal.surface).padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            for (j in jobs.values) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        (if (j.direction == "send") "傳給「${j.peerName}」：" else "接收「${j.peerName}」的：") + j.title,
                        Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall,
                    )
                    Text(
                        if (j.total > 0) "${j.done * 100 / j.total}%" else "",
                        color = pal.textMuted, style = MaterialTheme.typography.bodySmall,
                    )
                    TextButton(onClick = { scope.launch { Repo.tryCall("transfer_cancel", "key" to j.key) } }) { Text("取消") }
                }
                LinearProgressIndicator(progress = { if (j.total > 0) j.done.toFloat() / j.total else 0f }, modifier = Modifier.fillMaxWidth())
            }
        }
    }

    // 別台想配對
    val req = requests.firstOrNull { it.id !in dismissed }
    if (req != null) {
        PairRequestDialog(req, onDone = { dismissed = dismissed + req.id })
        return
    }
    // 我接受了，顯示配對碼等對方輸入
    accepted.values.firstOrNull { it.first.id !in dismissed }?.let { (r, _) ->
        AlertDialog(
            onDismissRequest = {},
            title = { Text("在「${r.fromName}」輸入配對碼") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(spaced(r.code), style = MaterialTheme.typography.displaySmall, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.SemiBold)
                    Text("對方輸入正確就完成配對，這個視窗會自動關閉。", color = pal.textMuted, style = MaterialTheme.typography.bodySmall)
                }
            },
            confirmButton = { TextButton(onClick = { dismissed = dismissed + r.id }) { Text("隱藏") } },
        )
        return
    }
    // 收到東西
    val item = incoming.firstOrNull { it.id !in dismissed && jobs[it.id] == null }
    if (item != null) {
        IncomingDialog(item, onDone = { dismissed = dismissed + item.id })
        return
    }
    done?.let { r ->
        AlertDialog(
            onDismissRequest = { done = null },
            title = { Text("已收到「${r.title}」") },
            text = {
                Text(
                    when (r.kind) {
                        "meeting" -> "來自「${r.peerName}」的會議已放進「會議記錄」。"
                        "recording" -> "來自「${r.peerName}」的錄音已放進「錄音管理」。"
                        else -> "來自「${r.peerName}」的檔案存在「文件／語音轉錄／收到的檔案」。"
                    },
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    done = null
                    when {
                        r.projectId != null -> onOpenProject(r.projectId)
                        r.recordingId != null -> onOpenRecording("recording:${r.recordingId}")
                        r.paths.isNotEmpty() -> {
                            val f = File(r.paths[0])
                            if (!Files.open(ctx, f)) scope.launch { Repo.toasts.emit(true to "手機上沒有能開 .${f.extension} 的 App") }
                        }
                    }
                }) { Text("開啟") }
            },
            dismissButton = { TextButton(onClick = { done = null }) { Text("關閉") } },
        )
    }
}

private fun spaced(code: String) = if (code.length == 6) code.substring(0, 3) + " " + code.substring(3) else code

@Composable
private fun PairRequestDialog(req: PairReq, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    val pal = Pal.current
    AlertDialog(
        onDismissRequest = {},
        title = { Text("「${req.fromName}」想跟這台配對") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("識別碼 ${req.from}。是你自己的裝置才接受；接受後要在那台輸入這組配對碼：")
                Text(spaced(req.code), style = MaterialTheme.typography.displaySmall, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.SemiBold)
                Text("配對後它可以把東西傳到這台，這台也能傳給它。", color = pal.textMuted, style = MaterialTheme.typography.bodySmall)
            }
        },
        confirmButton = {
            TextButton(onClick = {
                scope.launch { if (!Transfers.accept(req)) onDone() }
            }) { Text("接受") }
        },
        dismissButton = {
            TextButton(onClick = {
                onDone()
                scope.launch { Transfers.decline(req) }
            }) { Text("拒絕", color = pal.danger) }
        },
    )
}

@Composable
private fun IncomingDialog(item: IncomingItem, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    val pal = Pal.current
    fun receive(mode: String) {
        onDone()
        scope.launch { Repo.tryCall("transfer_receive", "id" to item.id, "mode" to mode) }
    }
    AlertDialog(
        onDismissRequest = {},
        title = { Text("收到來自「${item.fromName}」的${item.kindLabel}") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("「${item.title}」", fontWeight = FontWeight.Medium)
                Text(
                    buildString {
                        append(formatSize(item.total))
                        if (item.files > 1) append("　${item.files} 個檔案")
                        val mins = item.expiresIn / 60
                        append(if (mins >= 60) "　${mins / 60} 小時 ${mins % 60} 分內有效" else "　$mins 分內有效")
                    },
                    color = pal.textMuted, style = MaterialTheme.typography.bodySmall,
                )
                if (item.exists) Text("這台已經有這場會議：要取代它，還是另存一份？")
                Text("收完或拒收，中轉站上的那份都會馬上刪除。", color = pal.textMuted, style = MaterialTheme.typography.bodySmall)
            }
        },
        confirmButton = {
            if (item.exists) {
                FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    TextButton(onClick = { receive("copy") }) { Text("另存一份") }
                    TextButton(onClick = { receive("replace") }) { Text("取代") }
                }
            } else {
                TextButton(onClick = { receive("copy") }) { Text("接收") }
            }
        },
        dismissButton = {
            TextButton(onClick = {
                onDone()
                scope.launch { Repo.tryCall("transfer_decline", "id" to item.id) }
            }) { Text("拒收", color = pal.danger) }
        },
    )
}
