package io.github.win81134679.meetscribe.core

/**
 * Rust 核心（crates/meetscribe-android）的 JNI 入口。全部是 JSON 字串進出，協定見該 crate 的 bridge.rs。
 * 名稱與簽名是跟 Rust 的 `Java_io_github_win81134679_meetscribe_core_Native_*` 對應的契約，改了兩邊要一起改。
 */
object Native {
    init {
        System.loadLibrary("meetscribe_android")
    }

    /** 建立背景執行環境；重複呼叫沒關係。 */
    external fun init(dataDir: String, exportsDir: String): String

    /** 同步請求：`{"op": …}` → `{"ok":true,"data":…}` 或 `{"ok":false,"error":…}`。 */
    external fun call(request: String): String

    /** 等事件最多 [timeoutMs] 毫秒，回 JSON 陣列。 */
    external fun poll(timeoutMs: Long): String
}
