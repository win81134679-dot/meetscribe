//! 各供應商共用的 HTTP：JSON 送出、錯誤轉換、限流退避重試、取消。

use crate::{Error, Result};
use serde_json::Value;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub fn client(timeout_secs: u64) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(timeout_secs))
        .user_agent(concat!("MeetScribe/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// 送出請求（`build` 每次重試都會重新建立），可重試的錯誤依 Retry-After 或指數退避。
pub async fn send_with_retry(
    build: impl Fn() -> reqwest::RequestBuilder,
    max_retries: u32,
    label: &str,
    cancel: &CancellationToken,
) -> Result<Value> {
    let mut attempt = 0u32;
    loop {
        let result = tokio::select! {
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            r = build().send() => match r {
                Ok(resp) => decode(resp).await,
                Err(e) => Err((Error::Http(e), None)),
            },
        };
        match result {
            Ok(v) => return Ok(v),
            Err((e, retry_after)) if e.is_retryable() && attempt < max_retries => {
                let backoff = retry_after.unwrap_or_else(|| Duration::from_secs(2u64.pow(attempt + 1).min(60)));
                tracing::warn!("{label} 第 {} 次失敗，{:?} 後重試：{e}", attempt + 1, backoff);
                attempt += 1;
                tokio::select! {
                    _ = cancel.cancelled() => return Err(Error::Cancelled),
                    _ = tokio::time::sleep(backoff) => {}
                }
            }
            Err((e, _)) => return Err(e),
        }
    }
}

pub async fn get_json(rb: reqwest::RequestBuilder) -> Result<Value> {
    decode(rb.send().await?).await.map_err(|(e, _)| e)
}

/// 回應轉 JSON；非 2xx 或 body 帶 `error` 都轉成 [`Error::Api`]，附上建議的重試等待。
pub async fn decode(resp: reqwest::Response) -> std::result::Result<Value, (Error, Option<Duration>)> {
    let status = resp.status();
    let header_wait = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|s| Duration::from_secs(s.min(120)));
    let text = resp.text().await.map_err(|e| (Error::Http(e), None))?;
    let parsed: Option<Value> = serde_json::from_str(&text).ok();
    let wait = header_wait.or_else(|| parsed.as_ref().and_then(google_retry_delay));
    if !status.is_success() {
        let message = parsed
            .as_ref()
            .and_then(error_message)
            .unwrap_or_else(|| text.chars().take(300).collect());
        return Err((
            Error::Api {
                status: status.as_u16(),
                message,
            },
            wait,
        ));
    }
    let v = parsed.ok_or_else(|| {
        (
            Error::BadModelOutput(format!("不是 JSON：{}", text.chars().take(200).collect::<String>())),
            None,
        )
    })?;
    // OpenRouter 在供應商出錯時可能回 200 但 body 是 error
    if v.get("error").is_some()
        && v.get("choices").is_none()
        && v.get("text").is_none()
        && v.get("candidates").is_none()
    {
        let code = v["error"]["code"].as_u64().unwrap_or(502) as u16;
        let message = error_message(&v).unwrap_or_default();
        return Err((Error::Api { status: code, message }, wait));
    }
    Ok(v)
}

fn error_message(v: &Value) -> Option<String> {
    let e = v.get("error")?;
    let mut msg = e["message"].as_str().unwrap_or("未知錯誤").to_string();
    if let Some(raw) = e["metadata"]["raw"].as_str() {
        msg.push_str(&format!("（{}）", raw.chars().take(200).collect::<String>()));
    }
    if let Some(p) = e["metadata"]["provider_name"].as_str() {
        msg.push_str(&format!(" [供應商 {p}]"));
    }
    if e["status"].as_str() == Some("RESOURCE_EXHAUSTED") {
        msg = format!("Google 免費額度暫時用完（每分鐘或每日上限）：{msg}");
    }
    Some(msg)
}

/// Google 的 429 會在 `error.details[].retryDelay` 給 `"38s"`。
fn google_retry_delay(v: &Value) -> Option<Duration> {
    v["error"]["details"].as_array()?.iter().find_map(|d| {
        let s = d["retryDelay"].as_str()?.trim_end_matches('s');
        s.parse::<f64>()
            .ok()
            .map(|x| Duration::from_secs_f64(x.clamp(0.0, 120.0)))
    })
}

/// 數字可能是 number 也可能是字串（OpenRouter 價格欄位是字串）。
pub fn num(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn google_quota_error() {
        let v = json!({"error":{"code":429,"message":"Quota exceeded","status":"RESOURCE_EXHAUSTED",
            "details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"38s"}]}});
        assert_eq!(google_retry_delay(&v), Some(Duration::from_secs(38)));
        assert!(error_message(&v).unwrap().starts_with("Google 免費額度"));
    }
}
