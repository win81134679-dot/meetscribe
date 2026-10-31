//! MeetScribe 核心：會議錄音 → 完整逐字稿、說話者、依內容決定格式的摘要。
//!
//! 桌面程式（`meetscribe-app`）與 CLI（`meetscribe-cli`）共用這個函式庫。
//! 擴充點：[`providers::Transcriber`]、[`providers::LlmClient`]、[`export::Exporter`]、
//! 以及可外部覆寫的提示詞（[`prompts`]）。

pub mod account;
pub mod attribution;
pub mod audio;
#[cfg(feature = "native-audio")]
pub mod audio_native;
pub mod claude_code;
pub mod config;
pub mod error;
pub mod export;
pub mod gemini;
pub mod http;
pub mod model;
pub mod openrouter;
pub mod pipeline;
pub mod prompts;
pub mod providers;
pub mod secrets;
pub mod store;
pub mod summary;
pub mod textutil;
pub mod timing;
pub mod transfer;
pub mod units;
pub mod update;

pub use error::{Error, Result};
