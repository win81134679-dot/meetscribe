package io.github.win81134679.meetscribe.core

import android.os.Build
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import org.json.JSONArray
import org.json.JSONObject

/*
 * 「傳到其他裝置」（2026-10-31 使用者：「用識別碼傳給自己別台裝置 要有資安防護 傳完線上馬上刪除」）的 Kotlin 端狀態。
 * 加密、簽章、配對碼、上傳下載都在 Rust（meetscribe-core 的 transfer.rs，bridge.rs 的 transfer_* 指令）；這裡只收事件、給畫面讀。
 * 欄位名稱是 bridge.rs inbox_view／transfer_state 的契約。
 */

data class PeerDevice(val id: String, val name: String, val pairedAt: String)

data class PairReq(val id: String, val from: String, val fromName: String, val code: String)

data class MyPairReq(val id: String, val to: String, val toName: String, val status: String)

data class IncomingItem(
    val id: String,
    val from: String,
    val fromName: String,
    /** meeting／recording／file */
    val kind: String,
    val kindLabel: String,
    val title: String,
    val total: Long,
    val files: Int,
    /** 會議：這台已經有同一場（問取代或另存）。 */
    val exists: Boolean,
    val expiresIn: Long,
)

/** 進行中的傳送或接收。 */
data class TransferJob(
    val key: String,
    /** send／receive */
    val direction: String,
    val title: String,
    val peerName: String,
    val done: Long,
    val total: Long,
)

/** 收好了：畫面給「開啟」。 */
data class ReceivedInfo(
    val kind: String,
    val title: String,
    val peerName: String,
    val projectId: String?,
    val recordingId: String?,
    val paths: List<String>,
)

data class TransferState(
    val enabled: Boolean = false,
    val deviceId: String = "",
    val name: String = "",
    val peers: List<PeerDevice> = emptyList(),
)

object Transfers {
    val state = MutableStateFlow(TransferState())
    val pairRequests = MutableStateFlow<List<PairReq>>(emptyList())
    val myPairRequests = MutableStateFlow<List<MyPairReq>>(emptyList())
    val incoming = MutableStateFlow<List<IncomingItem>>(emptyList())
    val jobs = MutableStateFlow<Map<String, TransferJob>>(emptyMap())
    /** 我按了「接受」、等對方輸入配對碼的請求，以對方識別碼為鍵（配對完成或 5 分鐘過期就拿掉）。 */
    val accepted = MutableStateFlow<Map<String, Pair<PairReq, Long>>>(emptyMap())
    /** 最近一次跟中轉站講話的錯誤（設定頁顯示）。 */
    val lastError = MutableStateFlow<String?>(null)
    val received = MutableSharedFlow<ReceivedInfo>(extraBufferCapacity = 8)
    /** 叫輪詢迴圈別等了、馬上查一次（例如剛按了「接受」，要盡快看到對方輸入完配對碼）。 */
    val wake = MutableSharedFlow<Unit>(extraBufferCapacity = 1)

    val defaultName: String
        get() = Build.MODEL?.takeIf { it.isNotBlank() } ?: "Android 手機"

    suspend fun load() {
        (Repo.tryCall("transfer_state", "default_name" to defaultName) as? JSONObject)?.let(::applyState)
    }

    fun applyState(o: JSONObject) {
        state.value = TransferState(
            enabled = o.optBoolean("enabled"),
            deviceId = o.optString("device_id"),
            name = o.optString("name"),
            peers = parsePeers(o.optJSONArray("peers")),
        )
    }

    /** 查收件匣（結果是 transfer_inbox 事件）。中轉站沒啟用就什麼都不做。 */
    suspend fun poll() {
        if (!state.value.enabled) return
        runCatching { Repo.call("transfer_poll") }.onFailure { lastError.value = it.message }
    }

    /** 接受別台的配對請求：之後畫面持續顯示配對碼，直到對方輸入完成。 */
    suspend fun accept(req: PairReq): Boolean {
        if (Repo.tryCall("transfer_pair_accept", "id" to req.id) == null) return false
        accepted.update { it + (req.from to (req to System.currentTimeMillis())) }
        pairRequests.update { list -> list.filter { it.id != req.id } }
        wake.tryEmit(Unit)
        return true
    }

    /** 設定頁「立即同步金鑰」：把這台的金鑰推到所有配對的裝置。 */
    suspend fun syncKeys() {
        val r = Repo.tryCall("transfer_sync_keys") as? JSONObject ?: return
        val n = r.optInt("sent")
        val text = when {
            n > 0 -> "已把 API 金鑰同步到 $n 台裝置"
            r.optInt("peers") == 0 -> "還沒有配對的裝置"
            else -> "這支手機沒有存 API 金鑰"
        }
        Repo.toasts.tryEmit(false to text)
        // 手動同步是雙向的：推完自己的，馬上收對方送來的
        wake.tryEmit(Unit)
    }

    suspend fun decline(req: PairReq) {
        Repo.tryCall("transfer_pair_decline", "id" to req.id)
        pairRequests.update { list -> list.filter { it.id != req.id } }
    }

    private fun parsePeers(a: JSONArray?) = a.objects().map { PeerDevice(it.optString("id"), it.optString("name"), it.optString("paired_at")) }

    /** Repo 把 transfer_* 事件轉過來。 */
    fun handle(e: JSONObject) {
        when (e.optString("type")) {
            "transfer_inbox" -> {
                lastError.value = null
                pairRequests.value = e.optJSONArray("pair_requests").objects().map {
                    PairReq(it.optString("id"), it.optString("from"), it.optString("from_name"), it.optString("code"))
                }
                myPairRequests.value = e.optJSONArray("my_pair_requests").objects().map {
                    MyPairReq(it.optString("id"), it.optString("to"), it.optString("to_name"), it.optString("status"))
                }
                incoming.value = e.optJSONArray("incoming").objects().map {
                    IncomingItem(
                        it.optString("id"), it.optString("from"), it.optString("from_name"), it.optString("kind"),
                        it.optString("kind_label"), it.optString("title"), it.optLong("total"), it.optInt("files"),
                        it.optBoolean("exists"), it.optLong("expires_in"),
                    )
                }
                val peers = parsePeers(e.optJSONArray("peers"))
                state.update { it.copy(peers = peers) }
                // 配對的裝置同步來的 API 金鑰（Rust 已經存好）
                val synced = e.optJSONArray("keys_synced").objects()
                for (k in synced) Repo.toasts.tryEmit(false to "已從「${k.optString("peer_name")}」同步 API 金鑰：${k.optString("title")}")
                if (synced.isNotEmpty()) e.optJSONObject("keys")?.let { Repo.keys.value = Repo.parseKeys(it) }
                // 配對完成或過期（5 分鐘）的就不用再顯示配對碼
                val now = System.currentTimeMillis()
                accepted.update { m -> m.filter { (from, v) -> peers.none { it.id == from } && now - v.second < 5 * 60_000 } }
                for (n in e.optJSONArray("notices").objects()) {
                    val who = n.optString("peer_name")
                    val text = when (n.optString("kind")) {
                        "paired" -> "已跟「$who」配對"
                        "unpaired" -> "「$who」取消了配對"
                        "pair_cancelled" -> "「$who」取消了配對請求"
                        "received" -> "「$who」已收到"
                        "declined" -> "「$who」拒收了"
                        else -> null
                    }
                    if (n.optString("kind") == "paired") accepted.update { it - n.optString("peer") }
                    text?.let { Repo.toasts.tryEmit(false to it) }
                }
            }
            "transfer_error" -> lastError.value = e.optString("text")
            "transfer_progress" -> jobs.update {
                val key = e.optString("key")
                it + (key to TransferJob(key, e.optString("direction"), e.optString("title"), e.optString("peer_name"), e.optLong("done"), e.optLong("total")))
            }
            "transfer_done" -> {
                jobs.update { it - e.optString("key") }
                Repo.toasts.tryEmit(false to "「${e.optString("title")}」已傳給「${e.optString("peer_name")}」，對方打開 App 就會收到")
            }
            "transfer_failed" -> {
                jobs.update { it - e.optString("key") }
                val what = if (e.optString("direction") == "send") "傳送" else "接收"
                Repo.toasts.tryEmit(true to "$what「${e.optString("title")}」失敗：${e.optString("error")}")
            }
            "transfer_received" -> {
                jobs.update { it - e.optString("key") }
                incoming.update { list -> list.filter { it.id != e.optString("key") } }
                received.tryEmit(
                    ReceivedInfo(
                        e.optString("kind"), e.optString("title"), e.optString("peer_name"),
                        e.optStringOrNull("project_id"), e.optStringOrNull("recording_id"), e.optJSONArray("paths").strings(),
                    ),
                )
            }
        }
    }
}
