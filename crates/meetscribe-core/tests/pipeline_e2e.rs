//! 端對端：真的 ffmpeg ＋ 模擬的 OpenRouter。沒有 ffmpeg 的機器會略過。

use meetscribe_core::audio::Ffmpeg;
use meetscribe_core::config::{AppPaths, LlmProvider, Settings};
use meetscribe_core::export::{self, Content, ExportOptions};
use meetscribe_core::model::{JobOptions, Project, ProjectStatus, Timing};
use meetscribe_core::pipeline::{Engine, Event, EventSink};
use meetscribe_core::secrets::ApiKeys;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 300 秒的測試音，115～118 秒、235～238 秒是靜音。
async fn make_audio(ffmpeg: &Ffmpeg, out: &Path) {
    let status = tokio::process::Command::new(&ffmpeg.ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=300:duration=300",
        ])
        .args(["-af", "volume=enable='between(t,115,118)+between(t,235,238)':volume=0"])
        .arg(out)
        .status()
        .await
        .unwrap();
    assert!(status.success());
}

fn chat(content: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "choices": [{ "message": { "content": content }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 100, "completion_tokens": 20, "cost": 0.01 }
    }))
}

async fn mock_openrouter() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models/nvidia/nemotron-3.5-transcribe/endpoints"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": { "endpoints": [{ "provider_name": "DeepInfra", "supports_diarization": false,
                                      "pricing": { "prompt": "0.0000333" } }] }
        })))
        .mount(&server)
        .await;
    // 第一次轉錄回 503（要能自動重試）
    Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("Retry-After", "0")
                .set_body_json(serde_json::json!({ "error": { "code": 503, "message": "overloaded" } })),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "大家好。我是王經理。今天討論預算。",
            "usage": { "cost": 0.004, "seconds": 120.0 }
        })))
        .with_priority(2)
        .mount(&server)
        .await;
    // 說話者分配走模擬的 Google AI Studio（Gemini 原生 API）
    Mock::given(method("GET"))
        .and(path("/gemini/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models": [
                { "name": "models/gemini-3.8-flash", "supportedGenerationMethods": ["generateContent"] },
                { "name": "models/gemini-3.5-flash-lite", "supportedGenerationMethods": ["generateContent"] }
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/gemini/models/gemini-3.8-flash:generateContent"))
        .and(body_string_contains("說話者標註員"))
        .and(body_string_contains("inline_data"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "candidates": [{ "content": { "parts": [{ "text":
                "```json
{\"speakers\":[{\"id\":\"N1\",\"name\":\"王經理\",\"note\":\"男聲\"}],\"a\":[[0,\"N1\",0],[1,\"N1\",30.5],[2,\"N1\",60]]}
```"
            }] }, "finishReason": "STOP" }],
            "usageMetadata": { "promptTokenCount": 5000, "candidatesTokenCount": 50 }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_string_contains("說話者清單"))
        .respond_with(chat(
            r#"{"merges":[{"from":"S2","into":"S1","reason":"同一人"}],"names":[]}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_string_contains("會議記錄秘書"))
        .respond_with(chat("```markdown\n# 預算會議\n- 王經理說明預算\n```"))
        .mount(&server)
        .await;
    server
}

#[tokio::test(flavor = "multi_thread")]
async fn full_pipeline_with_two_chunks() {
    let Ok(ffmpeg) = Ffmpeg::locate("") else {
        eprintln!("略過：找不到 ffmpeg");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::at(tmp.path().join("home"));
    paths.ensure().unwrap();
    let source = tmp.path().join("預算會議.wav");
    make_audio(&ffmpeg, &source).await;

    let server = mock_openrouter().await;
    // 不經 sanitized()：測試需要比使用者下限更短的分段
    let settings = Settings {
        api_base: server.uri(),
        google_api_base: format!("{}/gemini", server.uri()),
        attribution_provider: LlmProvider::Google,
        attribution_model: "gemini-9-不存在".into(),
        summary_provider: LlmProvider::OpenRouter,
        summary_model: "nvidia/nemotron-3-ultra-550b-a55b:free".into(),
        chunk_minutes: 2,
        max_retries: 2,
        ..Default::default()
    };
    let keys = ApiKeys {
        openrouter: "sk-test".into(),
        google: Some("AIza-test".into()),
    };
    let engine = Engine::from_settings(&paths, settings, keys).await.unwrap();
    // 設定的 Gemini 模型不存在 → 自動挑最好的 Flash
    assert_eq!(engine.settings.attribution_model, "gemini-3.8-flash");
    assert!((engine.estimate_cost(3600.0).unwrap() - 0.11988).abs() < 1e-6);

    let events: Arc<Mutex<Vec<Event>>> = Arc::default();
    let sink: EventSink = {
        let events = events.clone();
        Arc::new(move |e| events.lock().unwrap().push(e))
    };
    let options = JobOptions {
        participants: vec!["王經理".into()],
        export: Some(ExportOptions {
            formats: vec!["txt".into(), "md".into()],
            content: Content::Separate,
            ..Default::default()
        }),
        ..Default::default()
    };
    let project = Project::new(source.clone(), options);
    let done = engine.run(project, sink, CancellationToken::new()).await;

    assert_eq!(done.status, ProjectStatus::Done, "warnings: {:?}", done.warnings);
    assert_eq!(done.chunks.len(), 2, "應在 115～118 秒的停頓切成兩段");
    assert!((done.chunks[0].end - 116.5).abs() < 0.5, "切點 {}", done.chunks[0].end);
    assert_eq!(done.segments.len(), 6);
    assert!(done.segments.iter().all(|s| s.timing == Timing::Model));
    // 第二段的時間是全域時間
    assert!((done.segments[4].start - (done.chunks[1].start + 30.5)).abs() < 1e-6);
    // 跨段同一人被合併，名字來自模型
    assert_eq!(done.speakers.len(), 1);
    assert_eq!(done.speakers[0].name, "王經理");
    assert!(done.speakers[0].sample_clip.is_some());
    assert!(done.segments.iter().all(|s| s.speaker.as_deref() == Some("S1")));
    let summary = done.summary.as_ref().unwrap();
    assert!(summary.markdown.starts_with("# 預算會議"), "外層 ``` 應被拿掉");
    assert!((done.usage.stt_usd - 0.008).abs() < 1e-9);
    assert!(done.usage.llm_usd > 0.0);
    assert_eq!(done.models.attribution, "gemini-3.8-flash");
    assert!(done.usage.llm_input_tokens >= 10_000, "Gemini 的 token 有記到");

    // 完成時依專案的輸出選項自動匯出到錄音檔旁邊
    let names: Vec<String> = done
        .exported
        .iter()
        .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        ["預算會議 摘要.md", "預算會議 逐字稿.md", "預算會議 摘要.txt", "預算會議 逐字稿.txt"]
    );
    assert!(done.exported.iter().all(|f| f.parent() == Some(tmp.path()) && f.is_file()));

    // 存檔可重新讀回，並可匯出所有格式
    let reloaded = engine.store.load(done.id).unwrap();
    assert_eq!(reloaded, done);
    let all = ExportOptions {
        formats: export::registry().iter().map(|e| e.id().to_string()).collect(),
        dir: tmp.path().join("out").display().to_string(),
        ..Default::default()
    };
    let written = export::export_project(&done, &all).unwrap();
    assert_eq!(written.len(), all.formats.len());

    // 再跑一次（續跑）不應重送任何請求，也不重複匯出
    let before = server.received_requests().await.unwrap().len();
    let again = engine.run(reloaded, Arc::new(|_| {}), CancellationToken::new()).await;
    assert_eq!(again.status, ProjectStatus::Done);
    assert_eq!(server.received_requests().await.unwrap().len(), before);
    assert_eq!(again.exported, done.exported);
    assert!(!tmp.path().join("預算會議 逐字稿 (2).txt").exists());

    let ev = events.lock().unwrap();
    assert!(ev.iter().any(|e| matches!(e, Event::Progress { .. })));
}

/// 請求本文比 `0` 位元組長才算符合（用來分辨送出的音訊有多長：32 kbps、base64 約每秒 5.3 KB）。
struct BodyLongerThan(usize);

impl wiremock::Match for BodyLongerThan {
    fn matches(&self, request: &wiremock::Request) -> bool {
        request.body.len() > self.0
    }
}

async fn stt_requests(server: &MockServer, longer_than: usize) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/audio/transcriptions" && r.body.len() > longer_than)
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn gateway_timeout_splits_chunk_in_half() {
    let Ok(ffmpeg) = Ffmpeg::locate("") else { return };
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::at(tmp.path().join("home"));
    paths.ensure().unwrap();
    let source = tmp.path().join("長會議.wav");
    make_audio(&ffmpeg, &source).await;

    let server = mock_openrouter().await;
    // 整段 300 秒（約 1.6 MB）送出去，供應商一律處理不完回 524；切半之後就正常
    const WHOLE: usize = 1_300_000;
    Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .and(BodyLongerThan(WHOLE))
        .respond_with(ResponseTemplate::new(524).set_body_json(serde_json::json!({
            "error": { "code": 524, "message": "Provider returned 524" }
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    let settings = Settings {
        api_base: server.uri(),
        google_api_base: format!("{}/gemini", server.uri()),
        summary_provider: LlmProvider::OpenRouter,
        chunk_minutes: 4,
        max_retries: 2,
        ..Default::default()
    };
    let keys = ApiKeys {
        openrouter: "sk-test".into(),
        google: Some("AIza-test".into()),
    };
    let engine = Engine::from_settings(&paths, settings, keys).await.unwrap();
    let done = engine
        .run(Project::new(source, JobOptions::default()), Arc::new(|_| {}), CancellationToken::new())
        .await;

    assert_eq!(done.status, ProjectStatus::Done, "warnings: {:?}", done.warnings);
    assert_eq!(done.chunks.len(), 2, "524 的那段要切成兩段");
    assert!((done.chunks[0].end - 116.5).abs() < 1.5, "在停頓處切：{}", done.chunks[0].end);
    assert_eq!(done.chunks[1].start, done.chunks[0].end);
    assert!((done.chunks[1].end - done.duration).abs() < 1e-6);
    assert_eq!(done.chunks.iter().map(|c| c.index).collect::<Vec<_>>(), vec![0, 1]);
    // 524 不在同樣長度上重試：整段只送過兩次（第一次 503 重試、第二次 524 就切）
    assert_eq!(stt_requests(&server, WHOLE).await, 2);
    assert!((done.usage.stt_usd - 0.008).abs() < 1e-9);
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_chunk_keeps_finished_ones() {
    let Ok(ffmpeg) = Ffmpeg::locate("") else { return };
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::at(tmp.path().join("home"));
    paths.ensure().unwrap();
    let source = tmp.path().join("a.wav");
    make_audio(&ffmpeg, &source).await;

    // 切成 116.5 秒與 183.5 秒兩段；長的那段馬上失敗（不可重試、也不該切），短的晚一點才成功
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .and(BodyLongerThan(800_000))
        .respond_with(ResponseTemplate::new(402).set_body_json(serde_json::json!({
            "error": { "code": 402, "message": "Insufficient credits" }
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "text": "大家好。", "usage": { "cost": 0.004 } }))
                .set_delay(std::time::Duration::from_millis(800)),
        )
        .with_priority(2)
        .mount(&server)
        .await;
    let settings = Settings {
        api_base: server.uri(),
        chunk_minutes: 2,
        max_retries: 2,
        ..Default::default()
    };
    let keys = ApiKeys {
        openrouter: "sk-test".into(),
        google: None,
    };
    let engine = Engine::from_settings(&paths, settings, keys).await.unwrap();
    let p = engine
        .run(Project::new(source, JobOptions::default()), Arc::new(|_| {}), CancellationToken::new())
        .await;

    assert!(matches!(&p.status, ProjectStatus::Failed(m) if m.contains("402")), "{:?}", p.status);
    assert!(p.status.is_resumable());
    assert_eq!(p.chunks.len(), 2, "402 不是太長，不該切");
    assert!(p.chunks[0].stt.is_some(), "另一段要做完並存檔");
    assert!(p.chunks[1].stt.is_none());
    assert_eq!(engine.store.load(p.id).unwrap(), p);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_marks_project_resumable() {
    let Ok(ffmpeg) = Ffmpeg::locate("") else { return };
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::at(tmp.path().to_path_buf());
    paths.ensure().unwrap();
    let source = tmp.path().join("a.wav");
    make_audio(&ffmpeg, &source).await;
    let server = mock_openrouter().await;
    let settings = Settings {
        api_base: server.uri(),
        ..Default::default()
    };
    let keys = ApiKeys {
        openrouter: "sk-test".into(),
        google: None,
    };
    let engine = Engine::from_settings(&paths, settings, keys).await.unwrap();
    // 沒有 Google Key → 說話者分配改用 OpenRouter 免費模型
    assert!(engine.settings.attribution_model.ends_with(":free"));
    // 預設摘要也是免費：沒有 Google Key 時退到 OpenRouter 的免費模型
    assert!(
        engine.settings.summary_model.ends_with(":free"),
        "{}",
        engine.settings.summary_model
    );
    assert!(!engine.notices.is_empty());
    let cancel = CancellationToken::new();
    cancel.cancel();
    let p = engine
        .run(Project::new(source, JobOptions::default()), Arc::new(|_| {}), cancel)
        .await;
    assert_eq!(p.status, ProjectStatus::Cancelled);
    assert!(p.status.is_resumable());
}
