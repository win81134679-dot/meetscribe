//! Google AI Studio（Gemini API）client。用原生 `generateContent`，音訊以 inline_data 送出。
//!
//! 用途：說話者分配預設走這裡的免費額度（使用者自己申請的 AI Studio API Key）。
//! 免費額度依 Google 專案計算、會變動；用完時回 429 RESOURCE_EXHAUSTED，由重試或降級處理。

use crate::config::Settings;
use crate::http::{self, num};
use crate::providers::{ContentPart, LlmClient, LlmRequest, LlmResponse, Role};
use crate::{Error, Result};
use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

pub const DEFAULT_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

pub struct Gemini {
    http: reqwest::Client,
    base: String,
    key: String,
    max_retries: u32,
}

impl Gemini {
    pub fn new(settings: &Settings, key: String) -> Result<Self> {
        if key.trim().is_empty() {
            return Err(Error::Secret("尚未設定 Google AI Studio API Key".into()));
        }
        Ok(Self {
            http: http::client(settings.request_timeout_secs)?,
            base: settings.google_api_base.trim_end_matches('/').to_string(),
            key: key.trim().to_string(),
            max_retries: settings.max_retries,
        })
    }

    /// 可用來 generateContent 的模型（去掉 `models/` 前綴），新的在前。
    pub async fn list_models(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut rb = self
                .http
                .get(format!("{}/models", self.base))
                .header("x-goog-api-key", &self.key)
                .query(&[("pageSize", "200")]);
            if let Some(t) = &page {
                rb = rb.query(&[("pageToken", t.as_str())]);
            }
            let v = http::get_json(rb).await?;
            for m in v["models"].as_array().into_iter().flatten() {
                let supports = m["supportedGenerationMethods"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|x| x.as_str() == Some("generateContent")));
                if let (true, Some(name)) = (supports, m["name"].as_str()) {
                    out.push(name.trim_start_matches("models/").to_string());
                }
            }
            match v["nextPageToken"].as_str() {
                Some(t) if !t.is_empty() => page = Some(t.to_string()),
                _ => break,
            }
        }
        out.sort_by(|a, b| b.cmp(a));
        Ok(out)
    }
}

/// 從可用清單挑一個適合聽音訊的：指定的有就用；否則挑版本最新、非 lite、非 preview 的 flash。
pub fn pick_audio_model(preferred: &str, available: &[String]) -> Option<String> {
    if available.iter().any(|m| m == preferred) {
        return Some(preferred.to_string());
    }
    let score = |m: &str| -> Option<(u32, u32)> {
        if !m.starts_with("gemini-")
            || !m.contains("flash")
            || m.contains("image")
            || m.contains("tts")
            || m.contains("live")
        {
            return None;
        }
        let ver: String = m["gemini-".len()..]
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let mut parts = ver.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
        let v = parts.next().unwrap_or(0) * 100 + parts.next().unwrap_or(0);
        let stable = !m.contains("preview") && !m.contains("exp");
        let full = !m.contains("lite");
        Some((u32::from(stable) * 2 + u32::from(full), v))
    };
    available
        .iter()
        .filter_map(|m| score(m).map(|s| (s, m)))
        .max_by_key(|(s, _)| *s)
        .map(|(_, m)| m.clone())
}

fn mime(format: &str) -> String {
    match format {
        "mp3" => "audio/mp3".into(),
        "wav" => "audio/wav".into(),
        "flac" => "audio/flac".into(),
        "ogg" => "audio/ogg".into(),
        "m4a" | "aac" => "audio/aac".into(),
        other => format!("audio/{other}"),
    }
}

pub fn build_body(req: &LlmRequest) -> Value {
    let mut system = Vec::new();
    let mut contents = Vec::new();
    for m in &req.messages {
        let parts: Vec<Value> = m
            .content
            .iter()
            .map(|p| match p {
                ContentPart::Text(t) => json!({ "text": t }),
                ContentPart::Audio { data, format } => json!({
                    "inline_data": {
                        "mime_type": mime(format),
                        "data": base64::engine::general_purpose::STANDARD.encode(data.as_slice()),
                    }
                }),
            })
            .collect();
        match m.role {
            Role::System => system.extend(parts),
            Role::User => contents.push(json!({ "role": "user", "parts": parts })),
            Role::Assistant => contents.push(json!({ "role": "model", "parts": parts })),
        }
    }
    let mut config = json!({});
    if let Some(t) = req.temperature {
        config["temperature"] = json!(t);
    }
    if let Some(m) = req.max_tokens {
        config["maxOutputTokens"] = json!(m);
    }
    let mut body = json!({ "contents": contents, "generationConfig": config });
    if !system.is_empty() {
        body["systemInstruction"] = json!({ "parts": system });
    }
    body
}

pub fn parse_response(v: &Value) -> Result<LlmResponse> {
    if let Some(reason) = v["promptFeedback"]["blockReason"].as_str() {
        return Err(Error::Api {
            status: 400,
            message: format!("Google 拒絕處理這段內容（{reason}）"),
        });
    }
    let cand = &v["candidates"][0];
    let text: String = cand["content"]["parts"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter(|p| !p["thought"].as_bool().unwrap_or(false))
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    let finish = cand["finishReason"].as_str().map(|r| {
        if r == "MAX_TOKENS" {
            "length".to_string()
        } else {
            r.to_lowercase()
        }
    });
    if text.trim().is_empty() {
        return Err(Error::BadModelOutput(format!(
            "Gemini 回傳空白內容（{}）",
            finish.as_deref().unwrap_or("無原因")
        )));
    }
    let u = &v["usageMetadata"];
    Ok(LlmResponse {
        text,
        cost: 0.0,
        input_tokens: num(&u["promptTokenCount"]).unwrap_or(0.0) as u64,
        output_tokens: (num(&u["candidatesTokenCount"]).unwrap_or(0.0) + num(&u["thoughtsTokenCount"]).unwrap_or(0.0))
            as u64,
        finish_reason: finish,
    })
}

#[async_trait]
impl LlmClient for Gemini {
    async fn complete(&self, req: LlmRequest, cancel: &CancellationToken) -> Result<LlmResponse> {
        let url = format!(
            "{}/models/{}:generateContent",
            self.base,
            req.model.trim_start_matches("models/")
        );
        let body = build_body(&req);
        let v = http::send_with_retry(
            || self.http.post(&url).header("x-goog-api-key", &self.key).json(&body),
            self.max_retries,
            "Gemini",
            cancel,
        )
        .await?;
        parse_response(&v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ChatMessage;
    use std::sync::Arc;

    #[test]
    fn body_maps_roles_and_audio() {
        let req = LlmRequest {
            model: "gemini-x".into(),
            messages: vec![
                ChatMessage::text(Role::System, "系統"),
                ChatMessage {
                    role: Role::User,
                    content: vec![
                        ContentPart::Text("聽".into()),
                        ContentPart::Audio {
                            data: Arc::new(vec![1, 2, 3]),
                            format: "mp3".into(),
                        },
                    ],
                },
            ],
            temperature: Some(0.0),
            max_tokens: Some(100),
        };
        let b = build_body(&req);
        assert_eq!(b["systemInstruction"]["parts"][0]["text"], "系統");
        assert_eq!(b["contents"][0]["parts"][1]["inline_data"]["mime_type"], "audio/mp3");
        assert_eq!(b["contents"][0]["parts"][1]["inline_data"]["data"], "AQID");
        assert_eq!(b["generationConfig"]["maxOutputTokens"], 100);
    }

    #[test]
    fn response_skips_thoughts() {
        let v = json!({"candidates":[{"content":{"parts":[{"text":"想","thought":true},{"text":"{\"a\":[]}"}]},"finishReason":"MAX_TOKENS"}],
                       "usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}});
        let r = parse_response(&v).unwrap();
        assert_eq!(r.text, "{\"a\":[]}");
        assert!(r.truncated());
        assert_eq!(r.input_tokens, 10);
        assert!(parse_response(&json!({"promptFeedback":{"blockReason":"SAFETY"}})).is_err());
    }

    #[test]
    fn picks_best_flash() {
        let avail: Vec<String> = [
            "gemini-3.5-flash-lite",
            "gemini-3.8-flash",
            "gemini-4-argon",
            "gemini-3.9-flash-preview",
            "gemini-3.6-flash",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            pick_audio_model("gemini-3.6-flash", &avail).as_deref(),
            Some("gemini-3.6-flash")
        );
        assert_eq!(pick_audio_model("不存在", &avail).as_deref(), Some("gemini-3.8-flash"));
        assert_eq!(pick_audio_model("x", &[]), None);
    }
}
