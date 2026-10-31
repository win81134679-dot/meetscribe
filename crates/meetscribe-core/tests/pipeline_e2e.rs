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

    // 完成時依專案的輸出選項自動匯出到錄音檔旁邊、這場會議自己的子資料夾（預設）
    let names: Vec<String> = done
        .exported
        .iter()
        .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        ["預算會議 摘要.md", "預算會議 逐字稿.md", "預算會議 摘要.txt", "預算會議 逐字稿.txt"]
    );
    let folder = tmp.path().join("預算會議");
    assert!(done.exported.iter().all(|f| f.parent() == Some(folder.as_path()) && f.is_file()));

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

/// 請求本文比 `0` 位元組長才算符合（用來分辨送出的音訊有多長：64 kbps、base64 約每秒 10.7 KB）。
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

/// 單段（300 秒、chunk_minutes 4 不切）的專案，轉錄請求先回 `timeouts` 次 524 再成功。
async fn run_with_queue_timeouts(timeouts: Option<u64>) -> (Project, MockServer, Vec<String>) {
    let ffmpeg = Ffmpeg::locate("").unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::at(tmp.path().join("home"));
    paths.ensure().unwrap();
    let source = tmp.path().join("長會議.wav");
    make_audio(&ffmpeg, &source).await;

    let server = mock_openrouter().await;
    let mut timeout = Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(524).set_body_json(serde_json::json!({
            "error": { "code": 524, "message": "Provider returned 524" }
        })))
        .with_priority(1);
    if let Some(n) = timeouts {
        timeout = timeout.up_to_n_times(n);
    }
    timeout.mount(&server).await;
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
    let mut engine = Engine::from_settings(&paths, settings, keys).await.unwrap();
    engine.queue_retry_wait = std::time::Duration::ZERO;
    let logs: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink: EventSink = {
        let logs = logs.clone();
        Arc::new(move |e| {
            if let Event::Log { message, .. } = e {
                logs.lock().unwrap().push(message);
            }
        })
    };
    let p = engine
        .run(Project::new(source, JobOptions::default()), sink, CancellationToken::new())
        .await;
    let logs = logs.lock().unwrap().clone();
    (p, server, logs)
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_timeout_resends_same_chunk() {
    if Ffmpeg::locate("").is_err() {
        return;
    }
    let (done, server, logs) = run_with_queue_timeouts(Some(2)).await;
    assert_eq!(done.status, ProjectStatus::Done, "warnings: {:?}", done.warnings);
    // 排隊逾時跟長度無關：不切段，同一段原樣重送
    assert_eq!(done.chunks.len(), 1);
    // 503（HTTP 層重試）→ 524（第 1 輪）→ 524（第 2 輪）→ 成功
    assert_eq!(stt_requests(&server, 0).await, 4);
    assert_eq!(logs.iter().filter(|l| l.contains("排隊逾時（不收費）")).count(), 2, "{logs:?}");
    assert!(!logs.iter().any(|l| l.contains("太長")));
    assert!((done.usage.stt_usd - 0.004).abs() < 1e-9);
}

#[tokio::test(flavor = "multi_thread")]
async fn endless_queue_timeouts_stop_resumably() {
    if Ffmpeg::locate("").is_err() {
        return;
    }
    let (p, server, _) = run_with_queue_timeouts(None).await;
    assert!(matches!(&p.status, ProjectStatus::Failed(m) if m.contains("排隊")), "{:?}", p.status);
    assert!(p.status.is_resumable());
    // 第 1 次送出＋6 輪重送（另加一次 503 的 HTTP 層重試）
    assert_eq!(stt_requests(&server, 0).await, 8);
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
        .and(BodyLongerThan(1_600_000))
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

/// 跟 [`full_pipeline_with_two_chunks`] 同一套流程，但音訊全程走 Android 用的內建後端（不叫 ffmpeg）：
/// 16 kHz 測試音直接用 Rust 寫成 WAV → 轉 MP3、找停頓切段、剪聲音樣本都由 symphonia＋LAME 完成。
#[cfg(feature = "native-audio")]
#[tokio::test(flavor = "multi_thread")]
async fn full_pipeline_with_native_audio_backend() {
    use meetscribe_core::audio::AudioTool;
    use meetscribe_core::audio_native::NativeAudio;

    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::at(tmp.path().join("home"));
    paths.ensure().unwrap();
    // 300 秒 300 Hz，115～118、235～238 秒靜音（同 make_audio）
    let source = tmp.path().join("手機錄音.wav");
    {
        let rate = 16_000u32;
        let mut data = Vec::with_capacity(rate as usize * 300 * 2);
        for i in 0..rate * 300 {
            let t = i as f64 / rate as f64;
            let silent = (115.0..118.0).contains(&t) || (235.0..238.0).contains(&t);
            let v = if silent { 0.0 } else { (t * 300.0 * std::f64::consts::TAU).sin() * 0.5 };
            data.extend(((v * 32767.0) as i16).to_le_bytes());
        }
        let mut f = Vec::new();
        f.extend(b"RIFF");
        f.extend((36 + data.len() as u32).to_le_bytes());
        f.extend(b"WAVEfmt ");
        f.extend(16u32.to_le_bytes());
        f.extend(1u16.to_le_bytes());
        f.extend(1u16.to_le_bytes());
        f.extend(rate.to_le_bytes());
        f.extend((rate * 2).to_le_bytes());
        f.extend(2u16.to_le_bytes());
        f.extend(16u16.to_le_bytes());
        f.extend(b"data");
        f.extend((data.len() as u32).to_le_bytes());
        f.extend(data);
        std::fs::write(&source, f).unwrap();
    }

    let server = mock_openrouter().await;
    let settings = Settings {
        api_base: server.uri(),
        google_api_base: format!("{}/gemini", server.uri()),
        attribution_provider: LlmProvider::Google,
        attribution_model: "gemini-3.8-flash".into(),
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
    let Ok(mut engine) = Engine::from_settings(&paths, settings, keys).await else {
        eprintln!("略過：桌面建立引擎需要 ffmpeg");
        return;
    };
    engine.audio = AudioTool::Native(NativeAudio);

    let options = JobOptions {
        participants: vec!["王經理".into()],
        export: Some(ExportOptions {
            formats: vec!["docx".into(), "srt".into()],
            dir: tmp.path().join("exports").display().to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let done = engine
        .run(Project::new(source, options), Arc::new(|_| {}), CancellationToken::new())
        .await;

    assert_eq!(done.status, ProjectStatus::Done, "warnings: {:?}", done.warnings);
    assert!((done.duration - 300.0).abs() < 0.5, "長度 {}", done.duration);
    assert_eq!(done.chunks.len(), 2, "應在 115～118 秒的停頓切成兩段");
    assert!((done.chunks[0].end - 116.5).abs() < 0.5, "切點 {}", done.chunks[0].end);
    // 每段都是讀得回來、長度對的 MP3
    for c in &done.chunks {
        let f = engine.store.path_in(done.id, &c.file);
        let d = NativeAudio.probe_duration(&f).await.unwrap();
        assert!((d - c.duration()).abs() < 0.5, "{}：{d} vs {}", c.file, c.duration());
    }
    assert_eq!(done.segments.len(), 6);
    assert_eq!(done.speakers.len(), 1);
    let clip = done.speakers[0].sample_clip.as_ref().expect("聲音樣本");
    assert!(engine.store.path_in(done.id, clip).metadata().unwrap().len() > 1000);
    assert!(done.summary.is_some());
    assert_eq!(done.exported.len(), 2);
    assert!(done.exported.iter().all(|f| f.starts_with(tmp.path().join("exports")) && f.is_file()));
}

/// 2026-10-31「跟隨播放根本不準」：真實會議的兩種壞時間——模型回的假等差秒數（0,1,2…）、供應商 300 秒一整段——
/// 由 retime_project 重新對齊；句子開頭不落在靜音裡。順便測 1.1 之前的專案補做高品質播放音訊。
#[tokio::test(flavor = "multi_thread")]
async fn retime_fixes_fake_model_times_and_provider_blocks() {
    use meetscribe_core::audio::AudioTool;
    use meetscribe_core::model::{ChunkRecord, Segment};
    let Ok(ffmpeg) = Ffmpeg::locate("") else { return };
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("會議.wav");
    make_audio(&ffmpeg, &source).await;
    let dir = tmp.path().join("proj");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&source, dir.join("audio.wav")).unwrap();

    let make = |segments: Vec<Segment>| {
        let mut p = Project::new(source.clone(), JobOptions::default());
        p.timing_rev = 0;
        p.duration = 300.0;
        p.audio_file = Some("audio.wav".into());
        p.chunks = vec![ChunkRecord { index: 0, start: 0.0, end: 300.0, file: "c.mp3".into(), stt: None, attributed: true }];
        p.segments = segments;
        p
    };
    let seg = |id: u64, start: f64, end: f64, timing: Timing, text: String| Segment {
        id, start, end, timing, speaker: Some("S1".into()), text, chunk: 0, edited: id == 3,
    };
    let sentence = |i: usize| format!("第{i}句話大家討論預算與人力安排。");
    let audio = AudioTool::Ffmpeg(ffmpeg.clone());
    let cancel = CancellationToken::new();
    let in_silence = |t: f64| (115.3..117.7).contains(&t) || (235.3..237.7).contains(&t);

    // 模型回 0,1,2…29：整段字被擠在前 30 秒
    let fake: Vec<Segment> = (0..30).map(|i| seg(i as u64 + 1, i as f64, i as f64 + 1.0, Timing::Model, sentence(i))).collect();
    let mut p = make(fake);
    assert!(meetscribe_core::pipeline::retime_project(&audio, &dir, &mut p, &cancel).await.unwrap());
    assert_eq!(p.segments.len(), 30, "句子不增不減");
    assert!(p.segments.iter().all(|s| s.timing == Timing::Estimated));
    assert!(p.segments[28].start > 250.0, "最後幾句應在段落尾端，實際 {:.1}", p.segments[28].start);
    assert!(p.segments.iter().all(|s| !in_silence(s.start)), "句子開頭不落在靜音裡");
    assert!(p.segments.iter().any(|s| s.edited), "使用者改過的標記留著");
    assert!(!meetscribe_core::pipeline::retime_project(&audio, &dir, &mut p, &cancel).await.unwrap(), "只做一次");

    // 供應商 300 秒一整段
    let block: String = (0..40).map(sentence).collect();
    let mut p = make(vec![seg(1, 0.0, 300.0, Timing::Provider, block)]);
    assert!(meetscribe_core::pipeline::retime_project(&audio, &dir, &mut p, &cancel).await.unwrap());
    assert!(p.segments.len() >= 10, "切成句子，實際 {} 句", p.segments.len());
    assert!(p.segments.iter().all(|s| s.end - s.start <= 45.0));
    assert!(p.segments.iter().all(|s| !in_silence(s.start)));
    let ids: std::collections::HashSet<u64> = p.segments.iter().map(|s| s.id).collect();
    assert_eq!(ids.len(), p.segments.len(), "句子編號不重複");

    // 高品質播放音訊（AAC）
    assert!(meetscribe_core::pipeline::ensure_playback(&audio, &dir, &mut p, &cancel).await.unwrap());
    let playback = dir.join(p.playback_audio().unwrap());
    assert_eq!(playback.extension().unwrap(), "m4a");
    let d = ffmpeg.probe_duration(&playback).await.unwrap();
    assert!((d - 300.0).abs() < 1.0, "{d}");
}

/// 2026-10-31「辨識說話者幫我等待時間縮短 若是辨識不出來就趕快接下去辨識不要卡這麼久」：
/// 說話者模型一直不回應時，一段只等 attribution_timeout、不重試；連續失敗到上限就不再呼叫，剩下的段落直接做完。
#[tokio::test(flavor = "multi_thread")]
async fn stuck_speaker_model_does_not_hold_up_the_meeting() {
    let Ok(ffmpeg) = Ffmpeg::locate("") else { return };
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::at(tmp.path().join("home"));
    paths.ensure().unwrap();
    let source = tmp.path().join("卡住的會議.wav");
    make_audio(&ffmpeg, &source).await;

    let server = mock_openrouter().await;
    // 說話者模型：60 秒都不回（模擬排隊、配額用完時的 Retry-After）
    Mock::given(method("POST"))
        .and(path("/gemini/models/gemini-3.8-flash:generateContent"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(60)))
        .with_priority(1)
        .mount(&server)
        .await;
    let settings = Settings {
        api_base: server.uri(),
        google_api_base: format!("{}/gemini", server.uri()),
        attribution_provider: LlmProvider::Google,
        attribution_model: "gemini-3.8-flash".into(),
        summary_provider: LlmProvider::OpenRouter,
        summary_model: "nvidia/nemotron-3-ultra-550b-a55b:free".into(),
        chunk_minutes: 2,
        max_retries: 4,
        ..Default::default()
    };
    let keys = ApiKeys { openrouter: "sk-test".into(), google: Some("AIza-test".into()) };
    let mut engine = Engine::from_settings(&paths, settings, keys).await.unwrap();
    engine.attribution_timeout = std::time::Duration::from_secs(2);
    engine.attribution_give_up_after = 1;
    let sink: EventSink = Arc::new(|_| {});
    let t = std::time::Instant::now();
    let done = engine.run(Project::new(source, JobOptions::default()), sink, CancellationToken::new()).await;
    let secs = t.elapsed().as_secs_f64();

    assert_eq!(done.status, ProjectStatus::Done, "{:?}", done.warnings);
    assert_eq!(done.chunks.len(), 2);
    assert!(!done.segments.is_empty(), "沒有說話者也照樣有逐字稿");
    let asked = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path().ends_with(":generateContent"))
        .count();
    assert_eq!(asked, 1, "第一段等 2 秒就放棄、不重試；第二段已達上限不再呼叫");
    assert!(done.warnings.iter().any(|w| w.contains("沒有回應")), "{:?}", done.warnings);
    assert!(done.warnings.iter().any(|w| w.contains("連續")), "{:?}", done.warnings);
    assert!(secs < 20.0, "整場只花 {secs:.1} 秒（以前一段就可能卡好幾分鐘）");
}
