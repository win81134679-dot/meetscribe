//! OpenRouter client：語音轉文字（`/audio/transcriptions`）、聊天（`/chat/completions`）、
//! 模型清單、金鑰額度。內建限流／暫時故障的退避重試與取消。

use crate::config::Settings;
use crate::http::{self, num};
use crate::model::{SttResult, TimedSegment, TimedWord};
use crate::providers::{ContentPart, LlmClient, LlmRequest, LlmResponse, TranscribeRequest, Transcriber};
use crate::{Error, Result};
use async_trait::async_trait;
use base64::Engine as _;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::sync::CancellationToken;

pub struct OpenRouter {
    http: reqwest::Client,
    base: String,
    key: String,
    max_retries: u32,
    deny_data_collection: bool,
    stt_model: String,
    stt_diarization: bool,
    /// 供應商不接受選用參數（verbose_json、keyterms）時改送精簡請求，記住以免每段都失敗一次。
    stt_minimal: AtomicBool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct KeyInfo {
    pub label: String,
    pub usage: f64,
    pub limit: Option<f64>,
    pub limit_remaining: Option<f64>,
    pub is_free_tier: bool,
    /// `/credits` 的剩餘額度（帳戶層級）。
    pub credits_remaining: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub context_length: u64,
    /// 每 token（或 STT 每秒）的美元價格。
    pub prompt_price: f64,
    pub completion_price: f64,
    pub input_modalities: Vec<String>,
    pub output_modalities: Vec<String>,
}

impl ModelInfo {
    pub fn accepts_audio(&self) -> bool {
        self.input_modalities.iter().any(|m| m == "audio")
    }
    pub fn is_transcription(&self) -> bool {
        self.output_modalities.iter().any(|m| m == "transcription")
    }
    pub fn outputs_text(&self) -> bool {
        self.output_modalities.is_empty() || self.output_modalities.iter().any(|m| m == "text")
    }
    fn accepts_text(&self) -> bool {
        self.input_modalities.is_empty() || self.input_modalities.iter().any(|m| m == "text")
    }
    /// 不收費（`:free`，或輸入、輸出都是 0 元）。
    pub fn is_free(&self) -> bool {
        self.id.ends_with(":free") || (self.prompt_price == 0.0 && self.completion_price == 0.0)
    }
    /// 「免費」或「付費 · 輸入 $0.30／輸出 $1.20（每百萬 tokens）」（2026-10-31 使用者：「一定要標注免費/付費」）。
    pub fn price_label(&self) -> String {
        if self.is_free() {
            return "免費".into();
        }
        let per_m = |p: f64| {
            let v = p * 1_000_000.0;
            if v >= 10.0 { format!("${v:.0}") } else if v >= 1.0 { format!("${v:.2}") } else { format!("${v:.3}") }
        };
        format!("付費 · 輸入 {}／輸出 {}（每百萬 tokens）", per_m(self.prompt_price), per_m(self.completion_price))
    }
    /// OpenRouter 官網上這個模型的頁面（看費率與說明）。
    pub fn page_url(&self) -> String {
        model_page_url(&self.id)
    }
}

/// OpenRouter 官網上某個模型的頁面。
pub fn model_page_url(id: &str) -> String {
    format!("https://openrouter.ai/{}", id.trim())
}

/// 推薦寫摘要的免費模型，越前面越推薦（2026-10-31 依 OpenRouter 上的模型規模與用途挑；下架的自動略過）。
pub const RECOMMENDED_FREE: &[&str] = &[
    "nvidia/nemotron-3-ultra-550b-a55b:free",
    "thinkingmachines/inkling:free",
    "nvidia/nemotron-3.5-super-vl:free",
    "google/gemma-4-31b-it:free",
    "nvidia/nemotron-3.5-lightning:free",
    "dots-studio/dots-3-note-preview:free",
];

/// 推薦寫摘要的付費模型，越前面越推薦（2026-10-31 依 OpenRouter 排行榜的 Artificial Analysis 智力指數挑，兼顧中文與價格；下架的自動略過）。
/// 會變：改之前到 https://openrouter.ai/rankings 重查。
pub const RECOMMENDED_PAID: &[&str] = &[
    "anthropic/claude-sonnet-5.5",
    "openai/gpt-6.1-terra",
    "qwen/qwen3.8-max-prime",
    "google/gemini-4-argon",
    "moonshotai/kimi-k3.1-pro",
    "anthropic/claude-haiku-5.5",
    "google/gemini-3.8-flash",
    "deepseek/deepseek-v4.1-flash",
];

/// 在推薦名單裡的名次（免費、付費各自排）。
pub fn recommend_rank(id: &str) -> Option<usize> {
    RECOMMENDED_FREE.iter().position(|r| *r == id).or_else(|| RECOMMENDED_PAID.iter().position(|r| *r == id))
}

/// 可以寫摘要的模型：吃文字、出文字、不是語音轉文字、價格有標（自動路由那種沒有固定價的不列）、不是批次版（`:batch` 不即時回）。
/// 排序（2026-10-31 使用者：「越推薦的越上面 但免費的優先再付費的」）：免費組在前、付費組在後；
/// 每組裡推薦名單照名次在前，其餘免費的依名稱、付費的照輸出價格由低到高。
pub fn summary_models(list: &[ModelInfo]) -> Vec<ModelInfo> {
    let mut out: Vec<ModelInfo> = list
        .iter()
        .filter(|m| m.accepts_text() && m.outputs_text() && !m.is_transcription())
        .filter(|m| m.prompt_price >= 0.0 && m.completion_price >= 0.0)
        .filter(|m| !m.output_modalities.iter().any(|o| o == "audio" || o == "image"))
        .filter(|m| !m.id.ends_with(":batch"))
        .cloned()
        .collect();
    out.sort_by(|a, b| {
        let rank = |m: &ModelInfo| recommend_rank(&m.id).unwrap_or(usize::MAX);
        b.is_free()
            .cmp(&a.is_free())
            .then(rank(a).cmp(&rank(b)))
            .then(a.completion_price.total_cmp(&b.completion_price))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

#[derive(Clone, Debug, Default)]
pub struct SttCapabilities {
    pub supports_diarization: bool,
    /// 美元／秒（取所有供應商最低價）。
    pub price_per_second: Option<f64>,
    pub providers: Vec<String>,
}

impl OpenRouter {
    pub fn new(settings: &Settings, key: String) -> Result<Self> {
        if key.trim().is_empty() {
            return Err(Error::NoApiKey);
        }
        let http = http::client(settings.request_timeout_secs)?;
        Ok(Self {
            http,
            base: settings.api_base.trim_end_matches('/').to_string(),
            key: key.trim().to_string(),
            max_retries: settings.max_retries,
            deny_data_collection: settings.deny_data_collection,
            stt_model: settings.stt_model.clone(),
            stt_diarization: false,
            stt_minimal: AtomicBool::new(false),
        })
    }

    /// 依 `/models/{id}/endpoints` 的查詢結果設定是否要求原生說話者標籤。
    pub fn with_stt_capabilities(mut self, caps: &SttCapabilities) -> Self {
        self.stt_diarization = caps.supports_diarization;
        self
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }

    fn provider_prefs(&self) -> Option<Value> {
        self.deny_data_collection.then(|| json!({ "data_collection": "deny" }))
    }

    async fn post_json(&self, path: &str, body: &Value, cancel: &CancellationToken) -> Result<Value> {
        self.post_json_if(path, body, cancel, Error::is_retryable).await
    }

    async fn post_json_if(
        &self,
        path: &str,
        body: &Value,
        cancel: &CancellationToken,
        retryable: impl Fn(&Error) -> bool,
    ) -> Result<Value> {
        let url = self.url(path);
        http::send_with_retry_if(
            || {
                self.http
                    .post(&url)
                    .bearer_auth(&self.key)
                    .header("X-Title", "MeetScribe")
                    .json(body)
            },
            self.max_retries,
            path,
            cancel,
            retryable,
        )
        .await
    }

    /// 轉錄請求：排隊逾時（504／524）不在這裡重試，交給流程降成一次一段、等一下再整輪重送。
    async fn post_stt(&self, req: &TranscribeRequest<'_>, minimal: bool, cancel: &CancellationToken) -> Result<Value> {
        self.post_json_if("audio/transcriptions", &self.stt_body(req, minimal), cancel, |e| {
            e.is_retryable() && !e.is_gateway_timeout()
        })
        .await
    }

    async fn get_json(&self, path: &str, auth: bool) -> Result<Value> {
        let mut rb = self.http.get(self.url(path)).header("X-Title", "MeetScribe");
        if auth {
            rb = rb.bearer_auth(&self.key);
        }
        http::get_json(rb).await
    }

    /// 金鑰資訊與剩餘額度（設定頁「測試連線」用）。
    pub async fn key_info(&self) -> Result<KeyInfo> {
        let v = self.get_json("key", true).await?;
        let d = &v["data"];
        let mut info = KeyInfo {
            label: d["label"].as_str().unwrap_or_default().to_string(),
            usage: d["usage"].as_f64().unwrap_or(0.0),
            limit: d["limit"].as_f64(),
            limit_remaining: d["limit_remaining"].as_f64(),
            is_free_tier: d["is_free_tier"].as_bool().unwrap_or(false),
            credits_remaining: None,
        };
        if let Ok(c) = self.get_json("credits", true).await {
            let total = c["data"]["total_credits"].as_f64();
            let used = c["data"]["total_usage"].as_f64();
            if let (Some(t), Some(u)) = (total, used) {
                info.credits_remaining = Some(t - u);
            }
        }
        Ok(info)
    }

    /// 全部模型（含語音轉文字模型）。不需要金鑰。
    pub async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let v = self.get_json("models", false).await?;
        let mut out: Vec<ModelInfo> = v["data"]
            .as_array()
            .map(|a| a.iter().map(parse_model).collect())
            .unwrap_or_default();
        // 有些版本的 /models 不含 STT 模型，另外補查
        if !out.iter().any(ModelInfo::is_transcription)
            && let Ok(v) = self.get_json("models?output_modalities=transcription", false).await
            && let Some(a) = v["data"].as_array()
        {
            out.extend(a.iter().map(parse_model));
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out.dedup_by(|a, b| a.id == b.id);
        Ok(out)
    }

    /// 查語音轉文字模型的供應商能力（是否支援原生說話者標籤、價格）。
    pub async fn stt_capabilities(&self, model: &str) -> Result<SttCapabilities> {
        let v = self.get_json(&format!("models/{model}/endpoints"), false).await?;
        let endpoints = v["data"]["endpoints"].as_array().cloned().unwrap_or_default();
        let mut caps = SttCapabilities::default();
        for e in &endpoints {
            caps.supports_diarization |= e["supports_diarization"].as_bool().unwrap_or(false);
            if let Some(p) = num(&e["pricing"]["prompt"]) {
                caps.price_per_second = Some(caps.price_per_second.map_or(p, |c: f64| c.min(p)));
            }
            if let Some(n) = e["provider_name"].as_str() {
                caps.providers.push(n.to_string());
            }
        }
        Ok(caps)
    }

    fn stt_body(&self, req: &TranscribeRequest<'_>, minimal: bool) -> Value {
        let mut body = json!({
            "model": self.stt_model,
            "input_audio": {
                "data": base64::engine::general_purpose::STANDARD.encode(req.audio),
                "format": req.format,
            },
            "temperature": 0,
        });
        if let Some(lang) = req.language.filter(|l| !l.is_empty()) {
            body["language"] = json!(lang);
        }
        if let Some(p) = self.provider_prefs() {
            body["provider"] = p;
        }
        if !minimal {
            body["response_format"] = json!("verbose_json");
            body["timestamp_granularities"] = json!(["segment", "word"]);
            if !req.keyterms.is_empty() {
                body["keyterms"] = json!(req.keyterms);
            }
            if req.diarize && self.stt_diarization {
                body["diarize"] = json!(true);
            }
        }
        body
    }
}

#[async_trait]
impl Transcriber for OpenRouter {
    fn model(&self) -> &str {
        &self.stt_model
    }

    fn supports_diarization(&self) -> bool {
        self.stt_diarization
    }

    async fn transcribe(&self, req: TranscribeRequest<'_>, cancel: &CancellationToken) -> Result<SttResult> {
        let minimal = self.stt_minimal.load(Ordering::Relaxed);
        let v = match self.post_stt(&req, minimal, cancel).await {
            Err(Error::Api {
                status: 400 | 422,
                message,
            }) if !minimal => {
                tracing::warn!("轉錄供應商不接受選用參數，改送精簡請求：{message}");
                self.stt_minimal.store(true, Ordering::Relaxed);
                self.post_stt(&req, true, cancel).await?
            }
            other => other?,
        };
        Ok(parse_stt(&v))
    }
}

#[async_trait]
impl LlmClient for OpenRouter {
    async fn complete(&self, req: LlmRequest, cancel: &CancellationToken) -> Result<LlmResponse> {
        let messages: Vec<Value> = req
            .messages
            .iter()
            .map(|m| {
                let parts: Vec<Value> = m
                    .content
                    .iter()
                    .map(|p| match p {
                        ContentPart::Text(t) => json!({ "type": "text", "text": t }),
                        ContentPart::Audio { data, format } => json!({
                            "type": "input_audio",
                            "input_audio": {
                                "data": base64::engine::general_purpose::STANDARD.encode(data.as_slice()),
                                "format": format,
                            }
                        }),
                    })
                    .collect();
                json!({ "role": m.role.as_str(), "content": parts })
            })
            .collect();
        let mut body = json!({ "model": req.model, "messages": messages, "usage": { "include": true } });
        if let Some(t) = req.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(m) = req.max_tokens {
            body["max_tokens"] = json!(m);
        }
        if let Some(p) = self.provider_prefs() {
            body["provider"] = p;
        }
        let v = self.post_json("chat/completions", &body, cancel).await?;
        let choice = &v["choices"][0];
        let text = match &choice["message"]["content"] {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        };
        if text.trim().is_empty() {
            return Err(Error::BadModelOutput("模型回傳空白內容".into()));
        }
        Ok(LlmResponse {
            text,
            cost: num(&v["usage"]["cost"]).unwrap_or(0.0),
            input_tokens: v["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
            output_tokens: v["usage"]["completion_tokens"].as_u64().unwrap_or(0),
            finish_reason: choice["finish_reason"].as_str().map(str::to_string),
        })
    }
}

fn speaker_of(v: &Value) -> Option<String> {
    match (&v["speaker"], &v["speaker_label"]) {
        (Value::String(s), _) if !s.is_empty() => Some(s.clone()),
        (Value::Number(n), _) => Some(format!("{n}")),
        (_, Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

pub fn parse_stt(v: &Value) -> SttResult {
    let segments = v["segments"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| TimedSegment {
                    text: s["text"].as_str().unwrap_or_default().trim().to_string(),
                    start: num(&s["start"]).unwrap_or(0.0),
                    end: num(&s["end"]).unwrap_or(0.0),
                    speaker: speaker_of(s),
                })
                .filter(|s| !s.text.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let words = v["words"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|w| w["type"].as_str().is_none_or(|t| t == "word"))
                .map(|w| TimedWord {
                    text: w["word"].as_str().unwrap_or_default().to_string(),
                    start: num(&w["start"]).unwrap_or(0.0),
                    end: num(&w["end"]).unwrap_or(0.0),
                    speaker: speaker_of(w),
                })
                .collect()
        })
        .unwrap_or_default();
    SttResult {
        text: v["text"].as_str().unwrap_or_default().trim().to_string(),
        language: v["language"].as_str().map(str::to_string),
        words,
        segments,
        cost: num(&v["usage"]["cost"]).unwrap_or(0.0),
        seconds: num(&v["usage"]["seconds"])
            .or_else(|| num(&v["duration"]))
            .unwrap_or(0.0),
    }
}

fn parse_model(m: &Value) -> ModelInfo {
    let strs = |v: &Value| -> Vec<String> {
        v.as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    ModelInfo {
        id: m["id"].as_str().unwrap_or_default().to_string(),
        name: m["name"].as_str().unwrap_or_default().to_string(),
        context_length: m["context_length"].as_u64().unwrap_or(0),
        prompt_price: num(&m["pricing"]["prompt"]).unwrap_or(0.0),
        completion_price: num(&m["pricing"]["completion"]).unwrap_or(0.0),
        input_modalities: strs(&m["architecture"]["input_modalities"]),
        output_modalities: strs(&m["architecture"]["output_modalities"]),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn summary_models_free_first_then_recommended_then_price() {
        let m = |id: &str, p: f64, c: f64| super::ModelInfo {
            id: id.into(),
            name: id.into(),
            context_length: 0,
            prompt_price: p,
            completion_price: c,
            input_modalities: vec!["text".into()],
            output_modalities: vec!["text".into()],
        };
        let mut stt = m("nvidia/nemotron-3.5-transcribe", 0.001, 0.0);
        stt.output_modalities = vec!["transcription".into()];
        let list = vec![
            m("zzz/cheap", 0.0000001, 0.0000002),
            m("anthropic/claude-haiku-5.5", 0.0000002, 0.0000005),
            m("aaa/other:free", 0.0, 0.0),
            m("anthropic/claude-sonnet-5.5", 0.000003, 0.00001),
            m("anthropic/claude-sonnet-5.5:batch", 0.0000015, 0.000005),
            m("openrouter/auto", -1.0, -1.0),
            m("thinkingmachines/inkling:free", 0.0, 0.0),
            m("nvidia/nemotron-3-ultra-550b-a55b:free", 0.0, 0.0),
            stt,
        ];
        let ids: Vec<String> = super::summary_models(&list).into_iter().map(|m| m.id).collect();
        assert_eq!(
            ids,
            [
                "nvidia/nemotron-3-ultra-550b-a55b:free",
                "thinkingmachines/inkling:free",
                "aaa/other:free",
                "anthropic/claude-sonnet-5.5",
                "anthropic/claude-haiku-5.5",
                "zzz/cheap",
            ]
        );
        assert_eq!(list[2].price_label(), "免費");
        assert_eq!(list[3].price_label(), "付費 · 輸入 $3.00／輸出 $10（每百萬 tokens）");
        assert_eq!(list[3].page_url(), "https://openrouter.ai/anthropic/claude-sonnet-5.5");
    }

    use super::*;

    #[test]
    fn parses_verbose_stt() {
        let v = json!({
            "text": " 大家好 ",
            "language": "zh",
            "segments": [{"id":0,"start":0.0,"end":1.5,"text":"大家好","speaker":1}],
            "words": [{"word":"大家","start":0.0,"end":0.6,"speaker_label":"A"},
                      {"word":"[laughter]","start":0.6,"end":0.7,"type":"audio_event"}],
            "usage": {"cost": 0.001, "seconds": 1.5}
        });
        let r = parse_stt(&v);
        assert_eq!(r.text, "大家好");
        assert_eq!(r.segments[0].speaker.as_deref(), Some("1"));
        assert_eq!(r.words.len(), 1);
        assert_eq!(r.words[0].speaker.as_deref(), Some("A"));
        assert!((r.cost - 0.001).abs() < 1e-12);
        assert!(r.has_speaker_labels());
    }

    #[test]
    fn parses_plain_stt() {
        let r = parse_stt(&json!({"text":"hello","usage":{"cost":"0.0002"}}));
        assert_eq!(r.text, "hello");
        assert!(r.segments.is_empty() && !r.has_speaker_labels());
        assert!((r.cost - 0.0002).abs() < 1e-12);
    }
}
