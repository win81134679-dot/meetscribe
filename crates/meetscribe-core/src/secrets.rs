//! API Key：存在 Windows 認證管理員（keyring），不落地成明碼檔。
//! 也接受環境變數（CLI、CI 用）。
//!
//! - OpenRouter：Nemotron 語音轉文字（付費）、OpenRouter 上的聊天模型。
//! - Google AI Studio：Gemini 免費額度（說話者分配預設用它）。

use crate::{Error, Result};

const SERVICE: &str = "MeetScribe";

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

fn entry(kind: KeyKind) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, kind.account()).map_err(|e| Error::Secret(e.to_string()))
}

/// 讀金鑰：認證管理員優先，其次環境變數。
pub fn load(kind: KeyKind) -> Option<(String, KeySource)> {
    if let Ok(e) = entry(kind)
        && let Ok(k) = e.get_password()
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
    entry(kind)?.set_password(key).map_err(|e| Error::Secret(e.to_string()))
}

pub fn delete(kind: KeyKind) -> Result<()> {
    match entry(kind)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(Error::Secret(e.to_string())),
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
