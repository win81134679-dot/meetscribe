#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("找不到 ffmpeg／ffprobe：{0}")]
    FfmpegMissing(String),
    #[error("ffmpeg 執行失敗：{0}")]
    Ffmpeg(String),
    #[error("尚未設定 OpenRouter API Key（到「設定」輸入）")]
    NoApiKey,
    #[error("OpenRouter 回應 {status}：{message}")]
    Api { status: u16, message: String },
    #[error("網路錯誤：{0}")]
    Http(#[from] reqwest::Error),
    #[error("模型回傳的內容無法解析：{0}")]
    BadModelOutput(String),
    #[error("已取消")]
    Cancelled,
    #[error("檔案錯誤：{0}")]
    Io(#[from] std::io::Error),
    #[error("JSON 錯誤：{0}")]
    Json(#[from] serde_json::Error),
    #[error("金鑰儲存錯誤：{0}")]
    Secret(String),
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// 值得自動重試的錯誤：限流、供應商暫時故障、逾時、連線中斷。
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Api { status, .. } => {
                matches!(status, 408 | 409 | 425 | 429 | 500 | 502 | 503 | 504 | 524 | 529)
            }
            Error::Http(e) => e.is_timeout() || e.is_connect() || e.is_request() || e.is_body(),
            Error::BadModelOutput(_) => true,
            _ => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
