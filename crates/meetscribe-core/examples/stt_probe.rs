//! 診斷用：把一個音訊檔直接送 OpenRouter 轉錄，量時間、看回應。金鑰從認證管理員讀，不印出。
//!
//! cargo run -p meetscribe-core --example stt_probe -- <檔案.mp3> <verbose|minimal> [標籤] [合併進 body 的 JSON]

use base64::Engine;
use meetscribe_core::secrets::{self, KeyKind};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let key = secrets::load(KeyKind::OpenRouter).expect("沒有 OpenRouter Key").0;
    if args[1] == "usage" {
        let v: Value = reqwest::Client::new()
            .get("https://openrouter.ai/api/v1/key")
            .bearer_auth(key.trim())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let d = &v["data"];
        return println!("usage {} / usage_daily {} / limit_remaining {}", d["usage"], d["usage_daily"], d["limit_remaining"]);
    }
    let (file, mode) = (&args[1], args[2].as_str());
    let tag = args.get(3).cloned().unwrap_or_else(|| mode.to_string());
    let audio = std::fs::read(file).unwrap();
    let mut body = json!({
        "model": "nvidia/nemotron-3.5-transcribe",
        "input_audio": { "data": base64::engine::general_purpose::STANDARD.encode(&audio), "format": "mp3" },
        "temperature": 0,
    });
    if mode == "verbose" {
        body["response_format"] = json!("verbose_json");
        body["timestamp_granularities"] = json!(["segment", "word"]);
    }
    // 第 4 個參數：要合併進 body 的 JSON（例如 provider.options）
    if let Some(extra) = args.get(4) {
        let extra: Value = serde_json::from_str(extra).expect("第 4 個參數要是 JSON");
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
    }
    let client = reqwest::Client::builder().timeout(Duration::from_secs(600)).build().unwrap();
    let t = Instant::now();
    let resp = client
        .post("https://openrouter.ai/api/v1/audio/transcriptions")
        .bearer_auth(key.trim())
        .header("X-Title", "MeetScribe-probe")
        .json(&body)
        .send()
        .await;
    let secs = t.elapsed().as_secs_f64();
    let resp = match resp {
        Ok(r) => r,
        Err(e) => return println!("[{tag}] {secs:.0}s 連線錯誤：{e}"),
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        return println!("[{tag}] {secs:.0}s HTTP {status}：{}", text.chars().take(300).collect::<String>());
    }
    let t = v["text"].as_str().unwrap_or_default();
    let n = t.chars().count();
    let head: String = t.chars().take(80).collect();
    let tail: String = t.chars().rev().take(80).collect::<Vec<_>>().into_iter().rev().collect();
    println!(
        "[{tag}] {secs:.0}s OK 字數 {n}　segments {}　words {}　usage {}\n  開頭：{head}\n  結尾：{tail}",
        v["segments"].as_array().map_or(0, Vec::len),
        v["words"].as_array().map_or(0, Vec::len),
        v["usage"]
    );
}
