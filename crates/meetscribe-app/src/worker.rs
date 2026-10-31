//! 背景工作：自己的 tokio runtime，依序處理排隊的專案，其他請求（測金鑰、載模型清單、重新摘要）並行。
//! UI 透過 [`Cmd`] 下指令，從 [`Msg`] 收結果；每則訊息都會叫 UI 重畫。

use meetscribe_core::audio::Ffmpeg;
use meetscribe_core::config::{AppPaths, Settings};
use meetscribe_core::gemini::Gemini;
use meetscribe_core::model::{Project, ProjectStatus};
use meetscribe_core::openrouter::{ModelInfo, OpenRouter, SttCapabilities};
use meetscribe_core::pipeline::{Engine, Event, EventSink};
use meetscribe_core::secrets::{ApiKeys, KeyKind};
use meetscribe_core::store::ProjectStore;
use meetscribe_core::transfer::{self, Hub, Inbox, KeySync, Kind, Outgoing, PairResult, Peer};
use meetscribe_core::update::{self, Outcome, Platform, Ready, UpdateState};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
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
    /// 檢查新版（`manual`：使用者按的，結果要明講，包括「已是最新版」）。
    CheckUpdate { manual: bool },
    /// 傳到其他裝置（2026-10-31）。
    Transfer(TransferCmd),
}

/// 「傳到其他裝置」（2026-10-31 使用者：「用識別碼傳給自己別台裝置 要有資安防護 傳完線上馬上刪除」）。
/// 加密、簽章、配對碼都在 core 的 `transfer.rs`；這裡只轉接，跟手機 bridge 的 `transfer_*` 指令一一對應。
pub enum TransferCmd {
    /// 馬上查一次收件匣（平常每分鐘自動查）。
    Poll,
    /// 這台改了名稱（設定已存好）：下次跟中轉站講話時重新登記。
    Renamed,
    PairStart(String),
    PairAccept(String),
    PairDecline(String),
    PairConfirm { id: String, code: String },
    PairCancel(String),
    Unpair(String),
    Send { peer: String, what: SendWhat },
    /// `replace`：對方傳來的會議這台已經有時取代，否則另存一份。
    Receive { id: String, replace: bool },
    Decline(String),
    /// 取消進行中的傳送或接收。
    Cancel(String),
    /// 把這台的 API 金鑰推到所有配對的裝置（改了金鑰時自動、或設定頁按「立即同步金鑰」）。`manual`：結果要明講。
    SyncKeys { manual: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub enum SendWhat {
    Meeting(Uuid),
    File(PathBuf),
}

/// 進行中的傳送或接收（畫面上的進度條）。
#[derive(Clone, Debug)]
pub struct TransferJob {
    pub send: bool,
    pub title: String,
    pub peer_name: String,
    pub done: u64,
    pub total: u64,
}

/// 收好了：畫面給「開啟」。
#[derive(Clone, Debug)]
pub struct ReceivedView {
    pub title: String,
    pub peer_name: String,
    pub project: Option<Project>,
    pub paths: Vec<PathBuf>,
}

pub enum TransferMsg {
    /// `enabled`：有中轉站網址。
    State { enabled: bool, device_id: String, name: String, peers: Vec<Peer> },
    Inbox(Box<Inbox>),
    PairStarted { id: String, to: String },
    PairResult(PairResult),
    Progress { key: String, job: TransferJob },
    /// 傳送或接收結束（成功失敗都發，進度條拿掉）。
    Ended { key: String },
    Received(Box<ReceivedView>),
    /// 跟中轉站講話失敗（設定頁顯示）。
    Error(String),
}

/// 更新的狀態（設定頁「關於與更新」與側欄提示用）。
#[derive(Clone, Debug, Default, PartialEq)]
pub enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    Downloading { version: String, done: u64, total: u64 },
    Latest,
    Ready(Ready),
    Failed(String),
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
    Update { status: UpdateStatus, manual: bool },
    Transfer(TransferMsg),
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
    hub: Mutex<Option<(String, Arc<Hub>)>>,
    transfers: Mutex<HashMap<String, CancellationToken>>,
    /// 叫輪詢別等了、馬上查（剛接受配對、剛收完）。
    poll_wake: tokio::sync::Notify,
    /// 這個時間之前每 5 秒查一次（等配對的另一邊）。
    fast_poll_until: Mutex<Option<Instant>>,
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

    /// 這台在配對清單上的名稱：設定的，或電腦名稱。
    fn device_name(&self) -> String {
        let n = self.settings().device_name.trim().to_string();
        if !n.is_empty() {
            return n;
        }
        std::env::var("COMPUTERNAME")
            .ok()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "Windows 電腦".into())
    }

    /// 中轉站網址沒設定（還沒部署）就回錯誤；網址改了就重開。
    fn hub(&self) -> Result<Arc<Hub>, String> {
        let url = self.settings().relay().trim().to_string();
        if url.is_empty() {
            return Err("中轉站尚未啟用".into());
        }
        let mut g = self.hub.lock().expect("hub");
        if let Some((u, h)) = g.as_ref()
            && *u == url
        {
            return Ok(h.clone());
        }
        let h = Arc::new(Hub::open(&self.paths.root, &url, &self.device_name()).map_err(|e| e.to_string())?);
        *g = Some((url, h.clone()));
        Ok(h)
    }

    fn transfer_state(&self) -> Msg {
        let hub = self.hub().ok();
        Msg::Transfer(TransferMsg::State {
            enabled: hub.is_some(),
            device_id: update::device_id(&self.paths.root),
            name: self.device_name(),
            peers: hub.map(|h| h.peers()).unwrap_or_default(),
        })
    }

    /// 接下來 5 分鐘每 5 秒查一次，並馬上查一次。
    fn fast_poll(&self) {
        *self.fast_poll_until.lock().expect("fast") = Some(Instant::now() + Duration::from_secs(300));
        self.poll_wake.notify_one();
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
            hub: Mutex::default(),
            transfers: Mutex::default(),
            poll_wake: tokio::sync::Notify::new(),
            fast_poll_until: Mutex::default(),
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
                    tokio::spawn(auto_update(shared.clone()));
                    tokio::spawn(upgrade_projects(shared.clone()));
                    tokio::spawn(transfer_poll_loop(shared.clone()));
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
                let result = meetscribe_core::account::test_key(&shared.settings(), kind).await;
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
        Cmd::Transfer(c) => {
            let shared = shared.clone();
            tokio::spawn(async move { transfer_cmd(&shared, c).await });
        }
        Cmd::CheckUpdate { manual } => {
            let shared = shared.clone();
            tokio::spawn(async move { check_update(&shared, manual).await });
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

/// 程式版本（安裝檔、update.json 都用這個）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 啟動 15 秒後、之後每小時看一次：開著自動更新且距上次檢查滿 20 小時就檢查並下載。
/// 已經下載好的版本一開程式就告訴 UI（側欄提示「新版已備妥」）。
async fn auto_update(shared: Arc<Shared>) {
    if let Some(r) = UpdateState::load(&shared.paths.root).ready_for(VERSION) {
        shared.send(Msg::Update { status: UpdateStatus::Ready(r.clone()), manual: false });
    }
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    loop {
        if shared.settings().auto_update && UpdateState::load(&shared.paths.root).due(20) {
            check_update(&shared, false).await;
        }
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}

static CHECKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

async fn check_update(shared: &Arc<Shared>, manual: bool) {
    use std::sync::atomic::Ordering;
    if CHECKING.swap(true, Ordering::SeqCst) {
        return;
    }
    shared.send(Msg::Update { status: UpdateStatus::Checking, manual });
    let mut last_sent = std::time::Instant::now() - std::time::Duration::from_secs(1);
    let progress_shared = shared.clone();
    let mut progress = move |version: &str, done: u64, total: u64| {
        if last_sent.elapsed().as_millis() >= 250 || done == total {
            last_sent = std::time::Instant::now();
            progress_shared.send(Msg::Update {
                status: UpdateStatus::Downloading { version: version.into(), done, total },
                manual,
            });
        }
    };
    let settings = shared.settings();
    let result = update::check_and_download(
        &shared.paths.root,
        &settings,
        VERSION,
        Platform::Windows,
        &CancellationToken::new(),
        &mut progress,
    )
    .await;
    let status = match result {
        Ok(Outcome::Latest) => UpdateStatus::Latest,
        Ok(Outcome::Ready(r)) => UpdateStatus::Ready(r),
        Err(e) => {
            tracing::warn!("檢查更新失敗：{e}");
            UpdateStatus::Failed(e.to_string())
        }
    };
    shared.send(Msg::Update { status, manual });
    CHECKING.store(false, Ordering::SeqCst);
}

/// 1.1 之前處理的會議升級一次（不呼叫 API）：重新對齊每句的時間（跟隨播放）、補做高品質播放音訊。
/// 啟動 3 秒後在背景一場一場做，正在排隊的跳過；做完的發 Updated 讓畫面換新。
async fn upgrade_projects(shared: Arc<Shared>) {
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let Ok(audio) = meetscribe_core::audio::AudioTool::locate(&shared.settings().ffmpeg_path) else { return };
    let store = meetscribe_core::store::ProjectStore::new(shared.paths.projects_dir());
    let cancel = CancellationToken::new();
    for mut p in store.list() {
        if p.segments.is_empty() || shared.queue.lock().expect("queue").contains(&p.id) {
            continue;
        }
        let dir = store.dir(p.id);
        let retimed = meetscribe_core::pipeline::retime_project(&audio, &dir, &mut p, &cancel).await.unwrap_or(false);
        let playback = meetscribe_core::pipeline::ensure_playback(&audio, &dir, &mut p, &cancel).await.unwrap_or(false);
        if (retimed || playback) && store.save(&p).is_ok() {
            tracing::info!("升級「{}」：重新對齊時間 {retimed}、高品質播放音訊 {playback}", p.title);
            shared.send(Msg::Pipeline(Event::Updated(Box::new(p))));
        }
    }
}

// ---------- 傳到其他裝置 ----------

/// 一打開先查一次，之後每 15 秒（2026-10-31 使用者：金鑰「更改後馬上同步到其他設備」；只用到 Workers／D1，免費方案超量只會暫停、不收費）；
/// 等配對的另一邊時每 5 秒；`poll_wake` 可以叫它馬上查。
async fn transfer_poll_loop(shared: Arc<Shared>) {
    shared.send(shared.transfer_state());
    loop {
        transfer_poll(&shared).await;
        let fast = shared.fast_poll_until.lock().expect("fast").is_some_and(|t| Instant::now() < t);
        let wait = Duration::from_secs(if fast { 5 } else { 15 });
        let _ = tokio::time::timeout(wait, shared.poll_wake.notified()).await;
    }
}

async fn transfer_poll(shared: &Arc<Shared>) {
    let Ok(hub) = shared.hub() else { return };
    match hub.poll().await {
        Ok(inbox) => {
            if inbox.peers_changed {
                *shared.fast_poll_until.lock().expect("fast") = None;
            }
            shared.send(shared.transfer_state());
            shared.send(Msg::Transfer(TransferMsg::Inbox(Box::new(inbox))));
        }
        Err(e) => shared.send(Msg::Transfer(TransferMsg::Error(e.to_string()))),
    }
}

async fn transfer_cmd(shared: &Arc<Shared>, cmd: TransferCmd) {
    if let TransferCmd::Cancel(key) = &cmd {
        if let Some(t) = shared.transfers.lock().expect("transfers").get(key) {
            t.cancel();
        }
        return;
    }
    let hub = match shared.hub() {
        Ok(h) => h,
        Err(e) => return shared.toast_err(e),
    };
    let r: Result<(), String> = match cmd {
        TransferCmd::Poll => {
            shared.poll_wake.notify_one();
            Ok(())
        }
        TransferCmd::Renamed => {
            hub.set_name(&shared.device_name());
            shared.send(shared.transfer_state());
            shared.poll_wake.notify_one();
            Ok(())
        }
        TransferCmd::PairStart(to) => match hub.pair_start(to.trim()).await {
            Ok(id) => {
                shared.fast_poll();
                shared.send(Msg::Transfer(TransferMsg::PairStarted { id, to }));
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        },
        TransferCmd::PairAccept(id) => hub.pair_accept(&id).await.map(|_| shared.fast_poll()).map_err(|e| e.to_string()),
        TransferCmd::PairDecline(id) => {
            hub.pair_decline(&id).await.map(|_| shared.poll_wake.notify_one()).map_err(|e| e.to_string())
        }
        TransferCmd::PairConfirm { id, code } => match hub.pair_confirm(&id, &code).await {
            Ok(result) => {
                let paired = result == PairResult::Paired;
                shared.send(Msg::Transfer(TransferMsg::PairResult(result)));
                if paired {
                    shared.send(shared.transfer_state());
                }
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        },
        TransferCmd::PairCancel(id) => hub.pair_cancel(&id).await.map_err(|e| e.to_string()),
        TransferCmd::Unpair(peer) => {
            hub.unpair(&peer).await.map(|_| shared.send(shared.transfer_state())).map_err(|e| e.to_string())
        }
        TransferCmd::Send { peer, what } => transfer_send(shared, &hub, &peer, &what).await,
        TransferCmd::Receive { id, replace } => transfer_receive(shared, &hub, &id, replace).await,
        TransferCmd::Decline(id) => hub.decline(&id).await.map(|_| shared.poll_wake.notify_one()).map_err(|e| e.to_string()),
        TransferCmd::Cancel(_) => Ok(()),
        TransferCmd::SyncKeys { manual } => match hub.sync_keys(None, KeySync::Replace).await {
            Ok(n) if manual || n > 0 => {
                let text = match n {
                    0 if hub.peers().is_empty() => "還沒有配對的裝置".to_string(),
                    0 => "這台沒有存 API 金鑰".to_string(),
                    n => format!("已把 API 金鑰同步到 {n} 台裝置"),
                };
                shared.send(Msg::Toast { error: false, text });
                if manual {
                    // 手動同步是雙向的：推完自己的，馬上收對方送來的
                    shared.poll_wake.notify_one();
                }
                Ok(())
            }
            Ok(_) => Ok(()),
            Err(e) => Err(format!("同步 API 金鑰失敗：{e}")),
        },
    };
    if let Err(e) = r {
        shared.toast_err(e);
    }
}

/// 進度（每 250 ms 最多一次）。
fn progress_fn(shared: &Arc<Shared>, key: &str, send: bool, title: &str, peer: &str) -> impl FnMut(u64, u64) + Send + use<> {
    let (sh, key, title, peer) = (shared.clone(), key.to_string(), title.to_string(), peer.to_string());
    let mut last = Instant::now() - Duration::from_secs(1);
    move |done, total| {
        if last.elapsed() >= Duration::from_millis(250) || done == total {
            last = Instant::now();
            let job = TransferJob { send, title: title.clone(), peer_name: peer.clone(), done, total };
            sh.send(Msg::Transfer(TransferMsg::Progress { key: key.clone(), job }));
        }
    }
}

async fn transfer_send(shared: &Arc<Shared>, hub: &Hub, peer: &str, what: &SendWhat) -> Result<(), String> {
    let store = ProjectStore::new(shared.paths.projects_dir());
    let out = match what {
        SendWhat::Meeting(id) => {
            if shared.queue.lock().expect("queue").contains(id) {
                return Err("這場會議正在處理，完成後再傳".into());
            }
            let p = store.load(*id).map_err(|e| e.to_string())?;
            Outgoing::meeting(&store, &p).map_err(|e| e.to_string())?
        }
        SendWhat::File(path) => {
            if !path.is_file() {
                return Err(format!("找不到檔案：{}", path.display()));
            }
            let title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            Outgoing::file(Kind::File, &title, path)
        }
    };
    let peer_name = hub.peers().into_iter().find(|p| p.id == peer).map(|p| p.name).unwrap_or_default();
    let key = Uuid::new_v4().to_string();
    let token = CancellationToken::new();
    shared.transfers.lock().expect("transfers").insert(key.clone(), token.clone());
    let mut progress = progress_fn(shared, &key, true, &out.title, &peer_name);
    let r = hub.send(peer, &out, &mut progress, &token).await;
    shared.transfers.lock().expect("transfers").remove(&key);
    shared.send(Msg::Transfer(TransferMsg::Ended { key }));
    match r {
        Ok(_) => {
            shared.send(Msg::Toast {
                error: false,
                text: format!("「{}」已傳給「{peer_name}」，對方打開程式就會收到", out.title),
            });
            Ok(())
        }
        Err(_) if token.is_cancelled() => Ok(()),
        Err(e) => Err(format!("傳送「{}」失敗：{e}", out.title)),
    }
}

async fn transfer_receive(shared: &Arc<Shared>, hub: &Hub, id: &str, replace: bool) -> Result<(), String> {
    let item = hub.incoming(id).ok_or("這筆已經不在收件匣（可能過期或被取消）")?;
    let title = item.meta.title.clone();
    let token = CancellationToken::new();
    shared.transfers.lock().expect("transfers").insert(id.to_string(), token.clone());
    let mut progress = progress_fn(shared, id, false, &title, &item.from_name);
    let staging = shared.paths.root.join("transfer").join("staging").join(id);
    let r = hub.receive(id, &staging, &mut progress, &token).await;
    shared.transfers.lock().expect("transfers").remove(id);
    let placed = match r {
        Ok(got) => place_received(shared, &got, replace),
        Err(_) if token.is_cancelled() => Err(String::new()),
        Err(e) => Err(e.to_string()),
    };
    let _ = std::fs::remove_dir_all(&staging);
    shared.send(Msg::Transfer(TransferMsg::Ended { key: id.to_string() }));
    shared.poll_wake.notify_one();
    let mut view = match placed {
        Ok(v) => v,
        Err(e) if e.is_empty() => return Ok(()),
        Err(e) => return Err(format!("接收「{title}」失敗：{e}")),
    };
    view.title = title;
    view.peer_name = item.from_name;
    if let Some(p) = &view.project {
        shared.send(Msg::Pipeline(Event::Updated(Box::new(p.clone()))));
    }
    shared.send(Msg::Transfer(TransferMsg::Received(Box::new(view))));
    Ok(())
}

/// 收到的東西放到該放的地方：會議進會議紀錄；錄音、匯出檔進「文件\語音轉錄\收到的檔案」（電腦版沒有錄音庫）。
fn place_received(shared: &Arc<Shared>, got: &transfer::Received, replace: bool) -> Result<ReceivedView, String> {
    let empty = ReceivedView { title: String::new(), peer_name: String::new(), project: None, paths: Vec::new() };
    match got.meta.kind {
        Kind::Meeting => {
            let mut export = shared.settings().export;
            // 沒設匯出資料夾＝放在原始錄音旁，而收到的會議的錄音在程式資料夾裡；改放「文件\語音轉錄」
            if export.dir.trim().is_empty() {
                export.dir = shared.paths.documents_dir().display().to_string();
            }
            let store = ProjectStore::new(shared.paths.projects_dir());
            let p = transfer::import_meeting(got, &store, replace, export).map_err(|e| e.to_string())?;
            Ok(ReceivedView { project: Some(p), ..empty })
        }
        // 金鑰由 Hub::poll 自動收存，不會走到這裡
        Kind::Keys => Err("金鑰不用手動接收".into()),
        Kind::Recording | Kind::File => {
            let dir = shared.paths.documents_dir().join("收到的檔案");
            let paths = transfer::place_file(got, &dir).map_err(|e| e.to_string())?;
            Ok(ReceivedView { paths, ..empty })
        }
    }
}

#[cfg(test)]
mod tests {
    //! 兩個桌面 worker 經本機中轉站配對、傳檔、傳整場會議。要先開 `relay/` 的 `wrangler dev`，
    //! 再設 `MEETSCRIBE_RELAY_TEST=http://127.0.0.1:8787` 跑（見 meetscribe-core/tests/transfer_relay.rs）。
    use super::*;
    use meetscribe_core::model::JobOptions;

    fn start(url: &str) -> (tempfile::TempDir, Worker) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::at(tmp.path().to_path_buf());
        paths.ensure().unwrap();
        let settings = Settings { relay_url: url.into(), ..Default::default() };
        settings.save(&paths).unwrap();
        (tmp, Worker::start(paths, settings, egui::Context::default()))
    }

    /// 等到第一個符合的傳送訊息（其他的丟掉）；錯誤提示直接讓測試失敗。
    fn wait<T>(w: &Worker, what: &str, mut f: impl FnMut(TransferMsg) -> Option<T>) -> T {
        let until = Instant::now() + Duration::from_secs(60);
        while Instant::now() < until {
            match w.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(Msg::Transfer(m)) => {
                    if let Some(v) = f(m) {
                        return v;
                    }
                }
                Ok(Msg::Toast { error: true, text }) => panic!("{what}：{text}"),
                _ => {}
            }
        }
        panic!("等不到{what}");
    }

    fn poll_until<T>(w: &Worker, what: &str, mut f: impl FnMut(&Inbox) -> Option<T>) -> T {
        for _ in 0..20 {
            w.send(Cmd::Transfer(TransferCmd::Poll));
            let got = wait(w, what, |m| match m {
                TransferMsg::Inbox(i) => Some(f(&i)),
                _ => None,
            });
            if let Some(v) = got {
                return v;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        panic!("收件匣一直沒有{what}");
    }

    #[test]
    fn desktop_workers_pair_and_send_via_local_relay() {
        let Ok(url) = std::env::var("MEETSCRIBE_RELAY_TEST") else {
            eprintln!("略過：沒有設定 MEETSCRIBE_RELAY_TEST");
            return;
        };
        // 收到的檔案放在測試資料夾裡，不碰使用者的文件資料夾
        // SAFETY: 測試開頭設一次，這個測試的 worker 都還沒讀
        unsafe { std::env::set_var("MEETSCRIBE_HOME", "test") };
        let (ta, a) = start(&url);
        let (tb, b) = start(&url);
        let id_of = |w: &Worker| {
            wait(w, "狀態", |m| match m {
                TransferMsg::State { enabled: true, device_id, .. } => Some(device_id),
                _ => None,
            })
        };
        let (ida, idb) = (id_of(&a), id_of(&b));

        // 配對：A 送出 → B 接受 → A 輸入 B 畫面上的碼（先錯一次）
        a.send(Cmd::Transfer(TransferCmd::PairStart(idb.clone())));
        let req = wait(&a, "配對請求編號", |m| match m {
            TransferMsg::PairStarted { id, to } => Some((id, to)),
            _ => None,
        });
        assert_eq!(req.1, idb);
        let pr = poll_until(&b, "配對請求", |i| i.pair_requests.first().cloned());
        assert_eq!(pr.from, ida);
        b.send(Cmd::Transfer(TransferCmd::PairAccept(pr.id.clone())));
        poll_until(&a, "對方接受", |i| i.my_pair_requests.iter().find(|r| r.status == "accepted").map(|_| ()));
        let wrong = if pr.code == "000000" { "111111" } else { "000000" };
        a.send(Cmd::Transfer(TransferCmd::PairConfirm { id: req.0.clone(), code: wrong.into() }));
        assert_eq!(wait(&a, "配對結果", |m| match m { TransferMsg::PairResult(r) => Some(r), _ => None }), PairResult::Wrong { left: 2 });
        a.send(Cmd::Transfer(TransferCmd::PairConfirm { id: req.0, code: pr.code.clone() }));
        assert_eq!(wait(&a, "配對結果", |m| match m { TransferMsg::PairResult(r) => Some(r), _ => None }), PairResult::Paired);
        poll_until(&b, "配對完成", |i| i.notices.iter().find(|n| matches!(n, transfer::Notice::Paired { .. })).map(|_| ()));

        // 傳一個匯出檔
        let f = ta.path().join("週會 摘要.pdf");
        std::fs::write(&f, b"%PDF-1.7 test").unwrap();
        a.send(Cmd::Transfer(TransferCmd::Send { peer: idb.clone(), what: SendWhat::File(f) }));
        wait(&a, "傳送結束", |m| matches!(m, TransferMsg::Ended { .. }).then_some(()));
        let item = poll_until(&b, "收件", |i| i.incoming.first().cloned());
        assert_eq!(item.meta.title, "週會 摘要.pdf");
        b.send(Cmd::Transfer(TransferCmd::Receive { id: item.id, replace: false }));
        let got = wait(&b, "收到", |m| match m { TransferMsg::Received(v) => Some(v), _ => None });
        assert_eq!(got.paths.len(), 1);
        assert!(got.paths[0].starts_with(tb.path().join("文件").join("收到的檔案")), "{:?}", got.paths);
        assert_eq!(std::fs::read(&got.paths[0]).unwrap(), b"%PDF-1.7 test");

        // 傳整場會議，B 收兩次：第一次新增，第二次另存一份（標題加「來自」）
        let store_a = ProjectStore::new(ta.path().join("projects"));
        let mut p = Project::new(ta.path().join("錄音.m4a"), JobOptions::default());
        p.title = "預算會議".into();
        p.status = ProjectStatus::Done;
        p.playback_file = Some("playback.m4a".into());
        store_a.save(&p).unwrap();
        std::fs::write(store_a.dir(p.id).join("playback.m4a"), vec![7u8; 4096]).unwrap();
        for round in 0..2 {
            a.send(Cmd::Transfer(TransferCmd::Send { peer: idb.clone(), what: SendWhat::Meeting(p.id) }));
            wait(&a, "傳送結束", |m| matches!(m, TransferMsg::Ended { .. }).then_some(()));
            let item = poll_until(&b, "收件", |i| i.incoming.first().cloned());
            assert_eq!(item.meta.project_id, Some(p.id));
            b.send(Cmd::Transfer(TransferCmd::Receive { id: item.id, replace: false }));
            let got = wait(&b, "收到", |m| match m { TransferMsg::Received(v) => Some(v), _ => None });
            let q = got.project.clone().expect("會議");
            if round == 0 {
                assert_eq!(q.id, p.id);
                assert_eq!(q.title, "預算會議");
            } else {
                assert_ne!(q.id, p.id);
                assert!(q.title.contains("來自"), "{}", q.title);
            }
            let dir = tb.path().join("projects").join(q.id.to_string());
            assert_eq!(std::fs::read(dir.join("playback.m4a")).unwrap().len(), 4096);
            assert_eq!(q.source_file, dir.join("playback.m4a"), "原始錄音改指這台的播放檔");
            let export = q.options.export.expect("輸出選項");
            assert_eq!(PathBuf::from(&export.dir), tb.path().join("文件"), "沒設匯出資料夾時放「文件 語音轉錄」");
        }

        // 取消配對：兩邊都拿掉
        a.send(Cmd::Transfer(TransferCmd::Unpair(idb)));
        wait(&a, "取消配對後的狀態", |m| match m { TransferMsg::State { peers, .. } => peers.is_empty().then_some(()), _ => None });
        poll_until(&b, "對方取消配對", |i| i.notices.iter().find(|n| matches!(n, transfer::Notice::Unpaired { .. })).map(|_| ()));
    }
}
