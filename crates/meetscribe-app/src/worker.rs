//! 背景工作：自己的 tokio runtime，依序處理排隊的專案，其他請求（測金鑰、載模型清單、重新摘要）並行。
//! UI 透過 [`Cmd`] 下指令，從 [`Msg`] 收結果；每則訊息都會叫 UI 重畫。

use meetscribe_core::audio::Ffmpeg;
use meetscribe_core::config::{AppPaths, Settings};
use meetscribe_core::gemini::Gemini;
use meetscribe_core::model::{Project, ProjectStatus};
use meetscribe_core::openrouter::{KeyInfo, ModelInfo, OpenRouter, SttCapabilities};
use meetscribe_core::pipeline::{Engine, Event, EventSink};
use meetscribe_core::secrets::{ApiKeys, KeyKind};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use tokio::sync::mpsc as tmpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub enum Cmd {
    Enqueue(Project),
    Cancel(Uuid),
    RegenerateSummary { project: Project, instructions: String },
    Reidentify(Project),
    UpdateSettings(Settings),
    TestKey(KeyKind),
    LoadOpenRouterModels,
    LoadGoogleModels,
    Probe(Vec<PathBuf>),
    CheckFfmpeg(String),
}

pub enum Msg {
    Pipeline(Event),
    Finished(Box<Project>),
    Toast {
        error: bool,
        text: String,
    },
    KeyTest {
        kind: KeyKind,
        result: Result<String, String>,
    },
    OpenRouterModels(Result<Vec<ModelInfo>, String>),
    GoogleModels(Result<Vec<String>, String>),
    SttCaps(SttCapabilities),
    Durations(Vec<(PathBuf, Result<f64, String>)>),
    Ffmpeg(Result<String, String>),
    /// 排隊中、正在跑的專案 ID（UI 用來顯示狀態與停用編輯）。
    Busy(Vec<Uuid>),
}

pub struct Worker {
    tx: tmpsc::UnboundedSender<Cmd>,
    pub rx: mpsc::Receiver<Msg>,
}

struct Shared {
    paths: AppPaths,
    settings: Mutex<Settings>,
    out: mpsc::Sender<Msg>,
    repaint: egui::Context,
    tokens: Mutex<HashMap<Uuid, CancellationToken>>,
    queue: Mutex<Vec<Uuid>>,
}

impl Shared {
    fn send(&self, m: Msg) {
        let _ = self.out.send(m);
        self.repaint.request_repaint();
    }
    fn settings(&self) -> Settings {
        self.settings.lock().expect("settings lock").clone()
    }
    fn publish_busy(&self) {
        let ids = self.queue.lock().expect("queue lock").clone();
        self.send(Msg::Busy(ids));
    }
    fn toast_err(&self, text: impl Into<String>) {
        self.send(Msg::Toast {
            error: true,
            text: text.into(),
        });
    }
    fn sink(self: &Arc<Self>) -> EventSink {
        let me = self.clone();
        Arc::new(move |e| me.send(Msg::Pipeline(e)))
    }
    async fn engine(&self) -> Result<Engine, String> {
        let keys = ApiKeys::load();
        Engine::from_settings(&self.paths, self.settings(), keys)
            .await
            .map_err(|e| e.to_string())
    }
}

impl Worker {
    pub fn start(paths: AppPaths, settings: Settings, repaint: egui::Context) -> Self {
        let (tx, mut cmd_rx) = tmpsc::unbounded_channel::<Cmd>();
        let (out, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            paths,
            settings: Mutex::new(settings),
            out,
            repaint,
            tokens: Mutex::default(),
            queue: Mutex::default(),
        });
        std::thread::Builder::new()
            .name("meetscribe-worker".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                rt.block_on(async move {
                    let (job_tx, job_rx) = tmpsc::unbounded_channel::<Job>();
                    tokio::spawn(job_runner(shared.clone(), job_rx));
                    tokio::spawn(fetch_stt_caps(shared.clone()));
                    while let Some(cmd) = cmd_rx.recv().await {
                        handle(&shared, &job_tx, cmd);
                    }
                });
            })
            .expect("spawn worker thread");
        Self { tx, rx }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

enum Job {
    Run(Project),
    Reidentify(Project),
}

impl Job {
    fn id(&self) -> Uuid {
        match self {
            Job::Run(p) | Job::Reidentify(p) => p.id,
        }
    }
}

fn handle(shared: &Arc<Shared>, job_tx: &tmpsc::UnboundedSender<Job>, cmd: Cmd) {
    match cmd {
        Cmd::Enqueue(p) => enqueue(shared, job_tx, Job::Run(p)),
        Cmd::Reidentify(p) => enqueue(shared, job_tx, Job::Reidentify(p)),
        Cmd::Cancel(id) => {
            if let Some(t) = shared.tokens.lock().expect("tokens").get(&id) {
                t.cancel();
            }
        }
        Cmd::UpdateSettings(s) => *shared.settings.lock().expect("settings") = s,
        Cmd::RegenerateSummary { project, instructions } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let engine = match shared.engine().await {
                    Ok(e) => e,
                    Err(e) => return shared.toast_err(e),
                };
                let token = CancellationToken::new();
                match engine
                    .regenerate_summary(project, instructions, shared.sink(), token)
                    .await
                {
                    Ok(p) => {
                        shared.send(Msg::Toast {
                            error: false,
                            text: "摘要已重新產生".into(),
                        });
                        shared.send(Msg::Finished(Box::new(p)));
                    }
                    Err(e) => shared.toast_err(format!("重新產生摘要失敗：{e}")),
                }
            });
        }
        Cmd::TestKey(kind) => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let result = test_key(&shared.settings(), kind).await;
                shared.send(Msg::KeyTest { kind, result });
            });
        }
        Cmd::LoadOpenRouterModels => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let s = shared.settings();
                // 模型清單不需要金鑰，給個占位值通過檢查
                let r = match OpenRouter::new(&s, "public".into()) {
                    Ok(c) => c.list_models().await.map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                };
                shared.send(Msg::OpenRouterModels(r));
            });
        }
        Cmd::LoadGoogleModels => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let s = shared.settings();
                let r = match ApiKeys::load().google {
                    Some(k) => match Gemini::new(&s, k) {
                        Ok(g) => g.list_models().await.map_err(|e| e.to_string()),
                        Err(e) => Err(e.to_string()),
                    },
                    None => Err("尚未設定 Google AI Studio API Key".into()),
                };
                shared.send(Msg::GoogleModels(r));
            });
        }
        Cmd::Probe(files) => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let ff = match Ffmpeg::locate(&shared.settings().ffmpeg_path) {
                    Ok(f) => f,
                    Err(e) => {
                        let err = e.to_string();
                        return shared.send(Msg::Durations(
                            files.into_iter().map(|f| (f, Err(err.clone()))).collect(),
                        ));
                    }
                };
                let mut out = Vec::new();
                for f in files {
                    let d = ff.probe_duration(&f).await.map_err(|e| e.to_string());
                    out.push((f, d));
                }
                shared.send(Msg::Durations(out));
            });
        }
        Cmd::CheckFfmpeg(path) => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let r = match Ffmpeg::locate(&path) {
                    Ok(f) => f
                        .version()
                        .await
                        .map(|v| format!("{v}\n{}", f.ffmpeg.display()))
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                };
                shared.send(Msg::Ffmpeg(r));
            });
        }
    }
}

fn enqueue(shared: &Arc<Shared>, job_tx: &tmpsc::UnboundedSender<Job>, job: Job) {
    let id = job.id();
    {
        let mut q = shared.queue.lock().expect("queue");
        if q.contains(&id) {
            return;
        }
        q.push(id);
    }
    shared
        .tokens
        .lock()
        .expect("tokens")
        .insert(id, CancellationToken::new());
    shared.publish_busy();
    let _ = job_tx.send(job);
}

/// 一次處理一個專案：避免同時打爆 API 額度，也讓進度清楚。
async fn job_runner(shared: Arc<Shared>, mut rx: tmpsc::UnboundedReceiver<Job>) {
    while let Some(job) = rx.recv().await {
        let id = job.id();
        let token = shared
            .tokens
            .lock()
            .expect("tokens")
            .get(&id)
            .cloned()
            .unwrap_or_default();
        let finished = if token.is_cancelled() {
            let mut p = match job {
                Job::Run(p) | Job::Reidentify(p) => p,
            };
            p.status = ProjectStatus::Cancelled;
            let _ = meetscribe_core::store::ProjectStore::new(shared.paths.projects_dir()).save(&p);
            p
        } else {
            match shared.engine().await {
                Ok(engine) => match job {
                    Job::Run(p) => engine.run(p, shared.sink(), token).await,
                    Job::Reidentify(p) => engine.reidentify_speakers(p, shared.sink(), token).await,
                },
                Err(e) => {
                    let mut p = match job {
                        Job::Run(p) | Job::Reidentify(p) => p,
                    };
                    p.status = ProjectStatus::Failed(e.clone());
                    let _ = meetscribe_core::store::ProjectStore::new(shared.paths.projects_dir()).save(&p);
                    shared.toast_err(e);
                    p
                }
            }
        };
        shared.tokens.lock().expect("tokens").remove(&id);
        shared.queue.lock().expect("queue").retain(|x| *x != id);
        if finished.status == ProjectStatus::Done {
            shared.send(Msg::Toast {
                error: false,
                text: format!("「{}」處理完成", finished.title),
            });
        }
        shared.send(Msg::Finished(Box::new(finished)));
        shared.publish_busy();
    }
}

async fn fetch_stt_caps(shared: Arc<Shared>) {
    let s = shared.settings();
    if let Ok(c) = OpenRouter::new(&s, "public".into())
        && let Ok(caps) = c.stt_capabilities(&s.stt_model).await
    {
        shared.send(Msg::SttCaps(caps));
    }
}

async fn test_key(settings: &Settings, kind: KeyKind) -> Result<String, String> {
    let Some((key, _)) = meetscribe_core::secrets::load(kind) else {
        return Err(format!("尚未設定 {}", kind.label()));
    };
    match kind {
        KeyKind::OpenRouter => {
            let c = OpenRouter::new(settings, key).map_err(|e| e.to_string())?;
            let info: KeyInfo = c.key_info().await.map_err(|e| e.to_string())?;
            let mut s = String::from("金鑰有效");
            if let Some(c) = info.credits_remaining {
                s.push_str(&format!("，帳戶餘額 US${c:.2}"));
            }
            if let Some(r) = info.limit_remaining {
                s.push_str(&format!("，此金鑰剩餘額度 US${r:.2}"));
            }
            s.push_str(&format!("，累計用量 US${:.2}", info.usage));
            Ok(s)
        }
        KeyKind::Google => {
            let g = Gemini::new(settings, key).map_err(|e| e.to_string())?;
            let models = g.list_models().await.map_err(|e| e.to_string())?;
            let pick = meetscribe_core::gemini::pick_audio_model(&settings.attribution_model, &models);
            Ok(format!(
                "金鑰有效，可用 {} 個模型；說話者分配將使用 {}",
                models.len(),
                pick.unwrap_or_else(|| "（找不到 Flash 模型）".into())
            ))
        }
    }
}
