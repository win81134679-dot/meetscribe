//! API Key：桌面存在 Windows 認證管理員（keyring），不落地成明碼檔。
//! Android 沒有認證管理員，存在 App 私有資料夾的檔案（[`use_file_store`]；其他 App 讀不到）。
//! 也接受環境變數（CLI、CI 用）。
//!
//! - OpenRouter：Nemotron 語音轉文字（付費）、OpenRouter 上的聊天模型。
//! - Google AI Studio：Gemini 免費額度（說話者分配預設用它）。

use crate::{Error, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[cfg(not(target_os = "android"))]
const SERVICE: &str = "MeetScribe";

static FILE_STORE: OnceLock<PathBuf> = OnceLock::new();

/// 改把金鑰存在這個 JSON 檔（Android 用；程式啟動時呼叫一次）。
pub fn use_file_store(path: PathBuf) {
    let _ = FILE_STORE.set(path);
}

fn file_map(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn file_write(path: &Path, map: &BTreeMap<String, String>) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::store::write_atomic(path, serde_json::to_string_pretty(map)?.as_bytes())
}

fn store_get(kind: KeyKind) -> Option<String> {
    if let Some(p) = FILE_STORE.get() {
        return file_map(p).remove(kind.account());
    }
    #[cfg(not(target_os = "android"))]
    {
        entry(kind).ok()?.get_password().ok()
    }
    #[cfg(target_os = "android")]
    {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyKind {
    OpenRouter,
    Google,
}

impl KeyKind {
    fn account(self) -> &'static str {
        match self {
            KeyKind::OpenRouter => "openrouter-api-key",
            KeyKind::Google => "google-ai-studio-api-key",
        }
    }
    pub fn env_vars(self) -> &'static [&'static str] {
        match self {
            KeyKind::OpenRouter => &["OPENROUTER_API_KEY"],
            KeyKind::Google => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            KeyKind::OpenRouter => "OpenRouter API Key",
            KeyKind::Google => "Google AI Studio API Key",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySource {
    CredentialStore,
    Environment,
}

/// 兩把金鑰。Google 那把可以沒有。
#[derive(Clone, Debug, Default)]
pub struct ApiKeys {
    pub openrouter: String,
    pub google: Option<String>,
}

impl ApiKeys {
    pub fn load() -> Self {
        Self {
            openrouter: load(KeyKind::OpenRouter).map(|k| k.0).unwrap_or_default(),
            google: load(KeyKind::Google).map(|k| k.0),
        }
    }
}

#[cfg(not(target_os = "android"))]
fn entry(kind: KeyKind) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, kind.account()).map_err(|e| Error::Secret(e.to_string()))
}

/// 讀金鑰：認證管理員優先，其次環境變數。
pub fn load(kind: KeyKind) -> Option<(String, KeySource)> {
    if let Some(k) = store_get(kind)
        && !k.trim().is_empty()
    {
        return Some((k.trim().to_string(), KeySource::CredentialStore));
    }
    kind.env_vars()
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|k| !k.trim().is_empty())
        .map(|k| (k.trim().to_string(), KeySource::Environment))
}

pub fn save(kind: KeyKind, key: &str) -> Result<()> {
    let key = key.trim();
    if key.is_empty() {
        return Err(Error::Secret(format!("{} 是空的", kind.label())));
    }
    if let Some(p) = FILE_STORE.get() {
        let mut m = file_map(p);
        m.insert(kind.account().into(), key.into());
        return file_write(p, &m);
    }
    #[cfg(not(target_os = "android"))]
    {
        entry(kind)?.set_password(key).map_err(|e| Error::Secret(e.to_string()))
    }
    #[cfg(target_os = "android")]
    {
        Err(Error::Secret("尚未設定金鑰存放位置".into()))
    }
}

pub fn delete(kind: KeyKind) -> Result<()> {
    if let Some(p) = FILE_STORE.get() {
        let mut m = file_map(p);
        m.remove(kind.account());
        return file_write(p, &m);
    }
    #[cfg(not(target_os = "android"))]
    {
        match entry(kind)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Error::Secret(e.to_string())),
        }
    }
    #[cfg(target_os = "android")]
    {
        Ok(())
    }
}

/// 顯示用：`sk-or-v1…a1b2`。
pub fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 12 {
        return "•".repeat(chars.len());
    }
    let head: String = chars[..8].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    /// 寫進真的 Windows 認證管理員再刪掉（用獨立的服務名稱，不碰正式金鑰）。
    #[test]
    #[cfg(not(target_os = "android"))]
    #[ignore = "會寫入系統認證管理員"]
    fn credential_store_roundtrip() {
        let e = keyring::Entry::new("MeetScribe-selftest", "roundtrip").unwrap();
        e.set_password("sk-test-123").unwrap();
        assert_eq!(e.get_password().unwrap(), "sk-test-123");
        e.delete_credential().unwrap();
        assert!(e.get_password().is_err());
    }

    #[test]
    fn mask_hides_middle() {
        assert_eq!(super::mask("sk-or-v1-abcdefghijklmnop"), "sk-or-v1…mnop");
        assert_eq!(super::mask("short"), "•••••");
    }
}
