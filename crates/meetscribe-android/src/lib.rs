//! MeetScribe Android 版的原生函式庫（`libmeetscribe_android.so`）。
//!
//! Kotlin 端是 `io.github.win81134679.meetscribe.core.Native`：
//! `init(dataDir, exportsDir)`、`call(json)`、`poll(timeoutMs)`，全部回 JSON 字串。
//! 協定見 [`bridge`]。這裡只做 JNI 轉接與攔 panic（panic 穿過 JNI 會讓整個 App 閃退）。

pub mod bridge;

use bridge::Bridge;
use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{jlong, jstring};
use serde_json::json;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

fn error_json(e: impl std::fmt::Display) -> String {
    json!({ "ok": false, "error": e.to_string() }).to_string()
}

fn guarded(f: impl FnOnce() -> String) -> String {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|p| {
        let msg = p
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "未知錯誤".into());
        error_json(format!("內部錯誤：{msg}"))
    })
}

fn read(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s).map(Into::into).unwrap_or_default()
}

fn out(env: &mut JNIEnv, s: String) -> jstring {
    env.new_string(s).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

/// 建立背景執行環境（只做一次；Activity 重建時再叫也沒關係）。
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_win81134679_meetscribe_core_Native_init<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    data_dir: JString<'l>,
    exports_dir: JString<'l>,
) -> jstring {
    let data = PathBuf::from(read(&mut env, &data_dir));
    let exports = PathBuf::from(read(&mut env, &exports_dir));
    let r = guarded(|| {
        if BRIDGE.get().is_some() {
            return json!({ "ok": true, "data": null }).to_string();
        }
        match Bridge::new(data, exports) {
            Ok(b) => {
                let _ = BRIDGE.set(b);
                json!({ "ok": true, "data": null }).to_string()
            }
            Err(e) => error_json(e),
        }
    });
    out(&mut env, r)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_win81134679_meetscribe_core_Native_call<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    request: JString<'l>,
) -> jstring {
    let req = read(&mut env, &request);
    let r = guarded(|| match BRIDGE.get() {
        Some(b) => b.call(&req),
        None => error_json("尚未初始化"),
    });
    out(&mut env, r)
}

/// 等事件，最多 `timeout_ms` 毫秒；回 JSON 陣列（可能是空的）。
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_win81134679_meetscribe_core_Native_poll<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    timeout_ms: jlong,
) -> jstring {
    let r = guarded(|| match BRIDGE.get() {
        Some(b) => b.poll(Duration::from_millis(timeout_ms.clamp(0, 60_000) as u64)),
        None => "[]".into(),
    });
    out(&mut env, r)
}
