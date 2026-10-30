//! 使用者設定與資料夾位置。API Key 不在這裡（見 [`crate::secrets`]）。

use crate::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_API_BASE: &str = "https://openrouter.ai/api/v1";
pub const DEFAULT_STT_MODEL: &str = "nvidia/nemotron-3.5-transcribe";
/// 說話者分配預設：Google AI Studio 免費額度的 Gemini Flash（需使用者自己的 AI Studio Key）。
pub const DEFAULT_ATTRIBUTION_MODEL_GOOGLE: &str = "gemini-3.8-flash";
/// 沒有 Google Key 時改用 OpenRouter 上免費、能聽音訊的模型。
pub const DEFAULT_ATTRIBUTION_MODEL_OPENROUTER: &str = "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free";
/// 摘要預設也走免費：Gemini 免費額度；沒有 Google Key 時用 OpenRouter 上 100 萬上下文的免費模型。
pub const DEFAULT_SUMMARY_MODEL_GOOGLE: &str = "gemini-3.8-flash";
pub const DEFAULT_SUMMARY_MODEL_OPENROUTER: &str = "nvidia/nemotron-3-ultra-550b-a55b:free";
/// 本機 Claude Code CLI 的 `--model`（sonnet／opus／haiku 或完整模型 ID；空白用 CLI 預設）。
pub const DEFAULT_SUMMARY_MODEL_CLAUDE_CODE: &str = "sonnet";

/// 聊天模型（說話者分配、整合、摘要）由哪個供應商提供。語音轉文字固定走 OpenRouter。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LlmProvider {
    /// Google AI Studio（Gemini API），有免費額度。
    Google,
    OpenRouter,
    /// 使用者電腦上的 Claude Code CLI（只能做文字工作，不能聽音訊）。
    ClaudeCode,
}

impl LlmProvider {
    pub fn label(self) -> &'static str {
        match self {
            LlmProvider::Google => "Google AI Studio（Gemini 免費額度）",
            LlmProvider::OpenRouter => "OpenRouter",
            LlmProvider::ClaudeCode => "本機 Claude Code CLI",
        }
    }
    /// 能不能聽音訊（說話者分配需要）。
    pub fn hears_audio(self) -> bool {
        !matches!(self, LlmProvider::ClaudeCode)
    }
    pub fn default_attribution_model(self) -> &'static str {
        match self {
            LlmProvider::Google => DEFAULT_ATTRIBUTION_MODEL_GOOGLE,
            LlmProvider::OpenRouter | LlmProvider::ClaudeCode => DEFAULT_ATTRIBUTION_MODEL_OPENROUTER,
        }
    }
    pub fn default_summary_model(self) -> &'static str {
        match self {
            LlmProvider::Google => DEFAULT_SUMMARY_MODEL_GOOGLE,
            LlmProvider::OpenRouter => DEFAULT_SUMMARY_MODEL_OPENROUTER,
            LlmProvider::ClaudeCode => DEFAULT_SUMMARY_MODEL_CLAUDE_CODE,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub api_base: String,
    pub google_api_base: String,
    /// 語音轉文字模型（OpenRouter `/audio/transcriptions`）。
    pub stt_model: String,
    /// 說話者分配：必須能聽音訊的聊天模型。
    pub attribution_provider: LlmProvider,
    pub attribution_model: String,
    /// 摘要與說話者整合用的文字模型。
    pub summary_provider: LlmProvider,
    pub summary_model: String,
    /// 摘要輸出語言；`auto` 表示跟會議主要語言相同。
    pub output_language: String,
    /// 每段音訊的目標長度（分鐘）。Nemotron 單次上限 60 分，但 20 分鐘的段落常讓供應商處理超時（524），
    /// 所以預設 10 分；還是超時的段落轉錄時會再自動對半切。
    pub chunk_minutes: u32,
    /// 同時送出的轉錄請求數。
    pub stt_concurrency: usize,
    pub request_timeout_secs: u64,
    pub max_retries: u32,
    /// 下一段比對聲音時，最多附幾位已知說話者的聲音樣本。
    pub reference_clip_limit: usize,
    /// 逐字稿超過這個字數就改用分段摘要再合併。
    pub summary_single_pass_chars: usize,
    /// 請 OpenRouter 只路由到不保留、不拿資料訓練的供應商。
    pub deny_data_collection: bool,
    /// claude.exe 的完整路徑；空白則自動尋找（PATH、使用者資料夾的 .local\bin）。
    pub claude_cli_path: String,
    /// ffmpeg.exe 所在資料夾或完整路徑；空白則自動尋找。
    pub ffmpeg_path: String,
    /// 匯出預設資料夾；空白則放在原始錄音旁邊。
    pub export_dir: String,
    pub export_formats: Vec<String>,
    /// 介面字級倍率。
    pub ui_scale: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            api_base: DEFAULT_API_BASE.into(),
            google_api_base: crate::gemini::DEFAULT_BASE.into(),
            stt_model: DEFAULT_STT_MODEL.into(),
            attribution_provider: LlmProvider::Google,
            attribution_model: DEFAULT_ATTRIBUTION_MODEL_GOOGLE.into(),
            summary_provider: LlmProvider::Google,
            summary_model: DEFAULT_SUMMARY_MODEL_GOOGLE.into(),
            claude_cli_path: String::new(),
            output_language: "zh-TW".into(),
            chunk_minutes: 10,
            stt_concurrency: 3,
            request_timeout_secs: 900,
            max_retries: 4,
            reference_clip_limit: 8,
            summary_single_pass_chars: 300_000,
            deny_data_collection: false,
            ffmpeg_path: String::new(),
            export_dir: String::new(),
            export_formats: vec!["md".into(), "docx".into(), "srt".into()],
            ui_scale: 1.0,
        }
    }
}

impl Settings {
    /// 把使用者可能填錯的值拉回合理範圍。
    pub fn sanitized(mut self) -> Self {
        self.chunk_minutes = self.chunk_minutes.clamp(2, 55);
        self.stt_concurrency = self.stt_concurrency.clamp(1, 16);
        self.request_timeout_secs = self.request_timeout_secs.clamp(30, 3600);
        self.max_retries = self.max_retries.min(10);
        self.reference_clip_limit = self.reference_clip_limit.min(20);
        self.summary_single_pass_chars = self.summary_single_pass_chars.max(20_000);
        self.ui_scale = self.ui_scale.clamp(0.7, 2.0);
        if self.api_base.trim().is_empty() {
            self.api_base = DEFAULT_API_BASE.into();
        }
        if self.google_api_base.trim().is_empty() {
            self.google_api_base = crate::gemini::DEFAULT_BASE.into();
        }
        if self.attribution_model.trim().is_empty() {
            self.attribution_model = self.attribution_provider.default_attribution_model().into();
        }
        if self.summary_model.trim().is_empty() {
            self.summary_model = self.summary_provider.default_summary_model().into();
        }
        if !self.attribution_provider.hears_audio() {
            self.attribution_provider = LlmProvider::Google;
            self.attribution_model = DEFAULT_ATTRIBUTION_MODEL_GOOGLE.into();
        }
        self
    }

    /// 摘要提示詞裡用的語言描述。
    pub fn output_language_label(&self) -> String {
        match self.output_language.as_str() {
            "auto" | "" => "與會議主要使用的語言相同".into(),
            "zh-TW" => "繁體中文（台灣用語）".into(),
            "zh-CN" => "简体中文".into(),
            "en" => "English".into(),
            "ja" => "日本語".into(),
            other => other.into(),
        }
    }

    pub fn load(paths: &AppPaths) -> Self {
        std::fs::read_to_string(paths.settings_file())
            .ok()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .unwrap_or_default()
            .sanitized()
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        crate::store::write_atomic(&paths.settings_file(), json.as_bytes())
    }
}

/// 程式資料夾。預設 `%APPDATA%\MeetScribe`；設環境變數 `MEETSCRIBE_HOME` 可改（可攜版、測試）。
#[derive(Clone, Debug)]
pub struct AppPaths {
    pub root: PathBuf,
}

impl AppPaths {
    pub fn detect() -> Self {
        if let Ok(home) = std::env::var("MEETSCRIBE_HOME")
            && !home.trim().is_empty()
        {
            return Self::at(PathBuf::from(home));
        }
        let root = directories::BaseDirs::new()
            .map(|b| b.config_dir().join("MeetScribe"))
            .unwrap_or_else(|| PathBuf::from("MeetScribeData"));
        Self::at(root)
    }

    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn ensure(&self) -> Result<()> {
        for d in [
            self.root.clone(),
            self.projects_dir(),
            self.prompts_dir(),
            self.logs_dir(),
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }

    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }
    pub fn projects_dir(&self) -> PathBuf {
        self.root.join("projects")
    }
    pub fn prompts_dir(&self) -> PathBuf {
        self.root.join("prompts")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
    /// Claude Code CLI 的工作目錄（空資料夾，避免讀到任何專案的 CLAUDE.md）。
    pub fn claude_workdir(&self) -> PathBuf {
        self.root.join("claude-work")
    }
}

/// 匯出資料夾：設定值優先，否則原始錄音所在資料夾。
pub fn default_export_dir(settings: &Settings, source_file: &Path) -> PathBuf {
    if !settings.export_dir.trim().is_empty() {
        return PathBuf::from(settings.export_dir.trim());
    }
    source_file
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clamps() {
        let s = Settings {
            chunk_minutes: 120,
            stt_concurrency: 0,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(s.chunk_minutes, 55);
        assert_eq!(s.stt_concurrency, 1);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let s: Settings = serde_json::from_str(r#"{"chunk_minutes": 15}"#).unwrap();
        assert_eq!(s.chunk_minutes, 15);
        assert_eq!(s.stt_model, DEFAULT_STT_MODEL);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::at(dir.path().to_path_buf());
        paths.ensure().unwrap();
        let s = Settings {
            summary_model: "x/y".into(),
            ..Default::default()
        };
        s.save(&paths).unwrap();
        assert_eq!(Settings::load(&paths).summary_model, "x/y");
    }
}
