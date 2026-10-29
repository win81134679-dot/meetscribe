//! 擴充點：語音轉文字與大型語言模型各是一個 trait。
//!
//! 現在的實作是 [`crate::openrouter::OpenRouter`]。要接其他雲端供應商（Azure、其他 STT 服務），
//! 實作這兩個 trait 再交給 [`crate::pipeline::Engine`] 即可，UI 與流程都不用改。

use crate::Result;
use crate::model::SttResult;
use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct TranscribeRequest<'a> {
    pub audio: &'a [u8],
    /// 副檔名形式：`mp3`、`wav`…
    pub format: &'a str,
    pub language: Option<&'a str>,
    pub keyterms: &'a [String],
    /// 要求供應商回傳說話者（只在供應商支援時有效）。
    pub diarize: bool,
}

#[async_trait]
pub trait Transcriber: Send + Sync {
    fn model(&self) -> &str;
    /// 供應商能不能直接給說話者標籤。
    fn supports_diarization(&self) -> bool;
    async fn transcribe(&self, req: TranscribeRequest<'_>, cancel: &CancellationToken) -> Result<SttResult>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

#[derive(Clone, Debug)]
pub enum ContentPart {
    Text(String),
    Audio { data: Arc<Vec<u8>>, format: String },
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: Role,
    pub content: Vec<ContentPart>,
}

impl ChatMessage {
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            content: vec![ContentPart::Text(text.into())],
        }
    }
}

#[derive(Clone, Debug)]
pub struct LlmRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct LlmResponse {
    pub text: String,
    pub cost: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub finish_reason: Option<String>,
}

impl LlmResponse {
    /// 輸出被長度上限截斷。
    pub fn truncated(&self) -> bool {
        matches!(self.finish_reason.as_deref(), Some("length" | "max_tokens"))
    }
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(&self, req: LlmRequest, cancel: &CancellationToken) -> Result<LlmResponse>;
}
