//! 電腦版自動更新的「安裝」那一半（檢查與下載在 `meetscribe_core::update`，由 worker 跑）。
//!
//! 新版下載好後：使用者按「立即更新」→ 靜默安裝並重新開啟；或下次關閉程式時靜默安裝（不重開）。
//! 安裝檔是 Inno Setup：`/VERYSILENT … /SELFUPDATE=1`，安裝程式先等程式的 [`RUNNING_MUTEX`] 消失再換檔，
//! `/RELAUNCH=1` 時裝完把程式開回來（installer/MeetScribe.iss）。
//! 只在「用安裝程式裝的」那一份做（exe 旁有 `unins000.exe`）；開發中的 `target\` 版本只檢查、不安裝。

use std::path::Path;

/// 程式執行期間一直持有；安裝程式（`[Code]` 的 `CheckForMutexes`）用它判斷程式關了沒。
pub const RUNNING_MUTEX: &str = "MeetScribe-Running";

/// 建立具名 mutex 並一直留著（程式結束時系統自動釋放）。
pub fn hold_running_mutex() {
    #[cfg(windows)]
    {
        let name: Vec<u16> = RUNNING_MUTEX.encode_utf16().chain(Some(0)).collect();
        // SAFETY: name 以 0 結尾；handle 刻意不關，讓 mutex 跟著行程活到結束
        let handle = unsafe { windows_sys::Win32::System::Threading::CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            tracing::warn!("無法建立 {RUNNING_MUTEX}");
        }
    }
}

/// 這一份可以自己更新嗎？不行時回傳原因（顯示在設定頁）。
pub fn self_install_blocker() -> Option<&'static str> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    if dir.join("unins000.exe").is_file() {
        None
    } else {
        Some("這一份不是用安裝程式裝的（開發版），只檢查新版、不自動安裝")
    }
}

/// 安裝程式的參數（測試看得到）。
pub fn installer_args(log: &Path, relaunch: bool) -> Vec<String> {
    let mut args: Vec<String> = ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-", "/SELFUPDATE=1"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    args.push(format!("/LOG={}", log.display()));
    if relaunch {
        args.push("/RELAUNCH=1".into());
    }
    args
}

/// 啟動安裝程式（不等它）。呼叫端接著關掉程式。
pub fn launch_installer(installer: &Path, logs_dir: &Path, relaunch: bool) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(installer);
    cmd.args(installer_args(&logs_dir.join("update-setup.log"), relaunch));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(DETACHED_PROCESS);
    }
    cmd.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installer_runs_silently_and_relaunches_only_when_asked() {
        let a = installer_args(Path::new(r"C:\logs\u.log"), false);
        assert!(a.contains(&"/VERYSILENT".into()) && a.contains(&"/SELFUPDATE=1".into()));
        assert!(a.contains(&r"/LOG=C:\logs\u.log".into()));
        assert!(!a.iter().any(|x| x.starts_with("/RELAUNCH")));
        assert!(installer_args(Path::new("x"), true).contains(&"/RELAUNCH=1".into()));
    }

    #[test]
    fn test_binary_is_not_an_installed_copy() {
        assert!(self_install_blocker().is_some());
    }
}
