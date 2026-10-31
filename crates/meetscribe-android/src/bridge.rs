//! Kotlin 介面與 core 之間的橋：JSON 進、JSON 出，平台無關（桌面也能測）。
//!
//! - [`Bridge::call`]：同步請求（讀寫設定、專案、金鑰、匯出），回 `{"ok":true,"data":…}` 或 `{"ok":false,"error":…}`。
//! - [`Bridge::send`]：背景工作（跑專案、取消、重新摘要、測金鑰…），結果變成事件。
//! - [`Bridge::poll`]：取走累積的事件（最多等 `timeout`），回 JSON 陣列。
//!
//! 跟桌面的 `worker.rs` 一樣一次只跑一個專案；專案資料從磁碟讀，跑的途中每次存檔都會發 `updated` 事件。
//! 錄音管理頁的資料（App 內錄的音檔＋每場會議的錄音）見 [`crate::recordings`] 與 `list_recordings`。

use meetscribe_core::audio::AudioTool;
use meetscribe_core::config::{AppPaths, Settings};
use meetscribe_core::export::{self, Content, ExportOptions};
use meetscribe_core::gemini::Gemini;
use meetscribe_core::model::{JobOptions, Project, ProjectStatus, Stage};
use meetscribe_core::openrouter::OpenRouter;
use meetscribe_core::pipeline::{Engine, Event, EventSink, LogLevel};
use meetscribe_core::secrets::{self, ApiKeys, KeyKind};
use meetscribe_core::store::ProjectStore;
use meetscribe_core::update::{self, Outcome, Platform, UpdateState};
use crate::recordings::{Library, Recording};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tokio::sync::mpsc as tmpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

type Res<T> = std::result::Result<T, String>;

pub struct Bridge {
    rt: tokio::runtime::Runtime,
    shared: Arc<Shared>,
}

struct Shared {
    paths: AppPaths,
    /// 匯出檔的預設資料夾：公開的「文件/語音轉錄」（檔案 App 看得到）；寫不進去時 Kotlin 給 App 的外部檔案區。
    exports_dir: PathBuf,
    /// Kotlin 匯入的錄音副本放這裡；處理完成後刪掉。
    imports_dir: PathBuf,
    /// App 內錄的音檔（永久保存，見 [`crate::recordings`]）。
    library: Library,
    settings: Mutex<Settings>,
    events: Mutex<VecDeque<Value>>,
    ready: Condvar,
    tokens: Mutex<HashMap<Uuid, CancellationToken>>,
    queue: Mutex<Vec<Uuid>>,
    jobs: tmpsc::UnboundedSender<Job>,
}

#[derive(Clone, Copy)]
enum Job {
    Run(Uuid),
    Reidentify(Uuid),
}

impl Job {
    fn id(self) -> Uuid {
        match self {
            Job::Run(id) | Job::Reidentify(id) => id,
        }
    }
}

impl Bridge {
    /// `data_dir`：App 私有資料夾（設定、專案、金鑰）；`exports_dir`：匯出檔放這裡。
    pub fn new(data_dir: PathBuf, exports_dir: PathBuf) -> Res<Self> {
        let paths = AppPaths::at(data_dir.clone());
        paths.ensure().map_err(|e| e.to_string())?;
        secrets::use_file_store(data_dir.join("keys.json"));
        let imports_dir = data_dir.join("imports");
        let recordings_dir = data_dir.join("recordings");
        for d in [&exports_dir, &imports_dir, &recordings_dir] {
            std::fs::create_dir_all(d).map_err(|e| format!("無法建立 {}：{e}", d.display()))?;
        }
        let mut settings = Settings::load(&paths);
        // 桌面的「空白＝原始錄音旁邊」在手機上是 App 內部資料夾，看不到；改成匯出資料夾。
        // 1.0.0 的預設是 App 的外部檔案區（Android/data/…，Android 11 起檔案 App 看不到）：換到公開資料夾
        if migrate_export_dir(&mut settings, &exports_dir) {
            let _ = settings.save(&paths);
        }
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .thread_name("meetscribe")
            .build()
            .map_err(|e| e.to_string())?;
        let (jobs, rx) = tmpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            paths,
            exports_dir,
            imports_dir,
            library: Library::new(recordings_dir),
            settings: Mutex::new(settings),
            events: Mutex::default(),
            ready: Condvar::new(),
            tokens: Mutex::default(),
            queue: Mutex::default(),
            jobs,
        });
        rt.spawn(job_runner(shared.clone(), rx));
        // 上次被系統關掉時還在跑的專案，標成中斷（可續跑）
        let projects = shared.store().list();
        for p in &projects {
            if matches!(p.status, ProjectStatus::Running | ProjectStatus::Queued) {
                let mut p = p.clone();
                p.status = ProjectStatus::Cancelled;
                let _ = shared.store().save(&p);
            }
        }
        shared.clean_imports(&projects);
        // 錄到一半 App 被關掉的錄音補登記（錄音機跟核心在同一個行程，這時一定沒有在錄）
        if let Ok(audio) = AudioTool::locate("") {
            shared.library.recover(|f| rt.block_on(audio.probe_duration(f)).ok().filter(|d| *d > 0.0));
        }
        rt.spawn(upgrade_projects(shared.clone()));
        Ok(Self { rt, shared })
    }

    pub fn call(&self, request: &str) -> String {
        let out = serde_json::from_str::<Value>(request)
            .map_err(|e| format!("請求格式錯誤：{e}"))
            .and_then(|req| self.dispatch(&req));
        match out {
            Ok(data) => json!({ "ok": true, "data": data }),
            Err(error) => json!({ "ok": false, "error": error }),
        }
        .to_string()
    }

    pub fn poll(&self, timeout: Duration) -> String {
        let s = &self.shared;
        let mut q = s.events.lock().expect("events");
        if q.is_empty() {
            q = s.ready.wait_timeout(q, timeout).expect("events").0;
        }
        let items: Vec<Value> = q.drain(..).collect();
        Value::Array(items).to_string()
    }

    fn dispatch(&self, req: &Value) -> Res<Value> {
        let s = &self.shared;
        let op = req["op"].as_str().ok_or("缺少 op")?;
        match op {
            "state" => Ok(json!({
                "settings": s.settings(),
                "keys": key_status(),
                "projects": s.store().list().iter().map(|p| s.summary_view(p)).collect::<Vec<_>>(),
                "busy": s.busy(),
                "exporters": export::registry().iter().map(|e| json!({
                    "id": e.id(), "label": e.label(), "extension": e.extension(), "has_summary": e.has_summary(),
                })).collect::<Vec<_>>(),
                "contents": Content::ALL.iter().map(|c| json!({ "id": c, "label": c.label() })).collect::<Vec<_>>(),
                "filename_presets": export::FILENAME_PRESETS,
                "exports_dir": s.exports_dir,
                "imports_dir": s.imports_dir,
                "recordings_dir": s.library.root(),
                "version": env!("CARGO_PKG_VERSION"),
                "device_id": update::device_id(&s.paths.root),
            })),
            // App 啟動時叫一次：識別碼、剛從哪版更新上來（只回報一次）、已下載好等著安裝的新版
            "update_info" => {
                let version = str_field(req, "version")?;
                let mut state = UpdateState::load(&s.paths.root);
                let updated_from = state.note_version(version);
                let _ = state.save(&s.paths.root);
                Ok(json!({
                    "device_id": update::device_id(&s.paths.root),
                    "updated_from": updated_from,
                    "ready": state.ready_for(version),
                    // 距上次檢查滿 20 小時：開著自動更新的話 Kotlin 接著叫 check_update
                    "due": state.due(20),
                }))
            }
            // 檢查並下載新版；結果是 "update" 事件（status：checking／downloading／latest／ready／failed）
            "check_update" => {
                let version = str_field(req, "version")?.to_string();
                let manual = req["manual"].as_bool().unwrap_or(false);
                let sh = s.clone();
                self.rt.spawn(async move {
                    sh.emit(json!({ "type": "update", "status": "checking", "manual": manual }));
                    let progress_sh = sh.clone();
                    let mut last = std::time::Instant::now() - Duration::from_secs(1);
                    let mut progress = move |v: &str, done: u64, total: u64| {
                        if last.elapsed() >= Duration::from_millis(300) || done == total {
                            last = std::time::Instant::now();
                            progress_sh.emit(json!({
                                "type": "update", "status": "downloading", "manual": manual,
                                "version": v, "done": done, "total": total,
                            }));
                        }
                    };
                    let r = update::check_and_download(
                        &sh.paths.root,
                        &sh.settings(),
                        &version,
                        Platform::Android,
                        &CancellationToken::new(),
                        &mut progress,
                    )
                    .await;
                    sh.emit(match r {
                        Ok(Outcome::Latest) => json!({ "type": "update", "status": "latest", "manual": manual, "version": version }),
                        Ok(Outcome::Ready(r)) => json!({
                            "type": "update", "status": "ready", "manual": manual,
                            "version": r.version, "notes": r.notes, "path": r.path, "targeted": r.targeted,
                        }),
                        Err(e) => json!({ "type": "update", "status": "failed", "manual": manual, "error": e.to_string() }),
                    });
                });
                Ok(Value::Null)
            }
            "list_projects" => {
                let mut list = s.store().list();
                list.sort_by_key(|p| std::cmp::Reverse(p.created_at));
                Ok(Value::Array(list.iter().map(|p| s.summary_view(p)).collect()))
            }
            "load_project" => Ok(s.full_view(&s.load(req)?)),
            // ---------- 錄音管理 ----------
            // 錄音機停止後登記（讀長度、寫 info.json）；讀不出長度的標成不完整
            "add_recording" => {
                let path = PathBuf::from(str_field(req, "path")?);
                let audio = AudioTool::locate("").map_err(|e| e.to_string())?;
                let d = self.rt.block_on(audio.probe_duration(&path)).ok().filter(|d| *d > 0.0);
                let r = s.library.add(&path, req["title"].as_str().unwrap_or(""), d)?;
                Ok(s.recording_view(&r, &s.store().list()))
            }
            "list_recordings" => Ok(Value::Array(s.recordings())),
            "rename_recording" => {
                let r = s.library.rename(uuid(req)?, str_field(req, "title")?)?;
                Ok(s.recording_view(&r, &s.store().list()))
            }
            "delete_recording" => {
                let id = uuid(req)?;
                if let Some(p) = s.library.get(id).and_then(|r| s.store().list().into_iter().find(|p| p.source_file == r.path)) {
                    return Err(format!("這段錄音正用在會議「{}」，請到那場會議處理", p.title));
                }
                s.library.delete(id)?;
                Ok(Value::Null)
            }
            // 存一份到手機的公開資料夾：錄音庫的放「文件/語音轉錄/錄音」，會議的放進會議資料夾（跟 Word、字幕放一起）
            "save_recording" => {
                let id = uuid(req)?;
                let path = if req["kind"] == "meeting" {
                    let mut p = s.store().load(id).map_err(|e| e.to_string())?;
                    let mut opts = p.options.export.clone().unwrap_or_else(|| s.settings().export);
                    s.fix_export_dir(&mut opts);
                    let f = s.export_audio(&p, &opts)?.ok_or("這場會議沒有錄音檔")?;
                    if !s.is_busy(p.id) && !p.exported.contains(&f) {
                        p.exported.push(f.clone());
                        let _ = s.store().save(&p);
                    }
                    f
                } else {
                    let r = s.library.get(id).ok_or("找不到這段錄音")?;
                    let dir = s.exports_dir.join("錄音");
                    copy_audio(&r.path, &dir, &meetscribe_core::textutil::safe_filename(&r.info.title), false)?
                };
                Ok(json!(path))
            }
            "save_settings" => {
                let new: Settings = serde_json::from_value(req["settings"].clone())
                    .map_err(|e| format!("設定格式錯誤：{e}"))?;
                let new = new.sanitized();
                new.save(&s.paths).map_err(|e| e.to_string())?;
                *s.settings.lock().expect("settings") = new.clone();
                Ok(serde_json::to_value(new).unwrap_or_default())
            }
            "save_key" => {
                secrets::save(kind(req)?, str_field(req, "key")?).map_err(|e| e.to_string())?;
                Ok(key_status())
            }
            "delete_key" => {
                secrets::delete(kind(req)?).map_err(|e| e.to_string())?;
                Ok(key_status())
            }
            "probe" => {
                let path = PathBuf::from(str_field(req, "path")?);
                let audio = AudioTool::locate("").map_err(|e| e.to_string())?;
                self.rt
                    .block_on(audio.probe_duration(&path))
                    .map(|d| json!(d))
                    .map_err(|e| e.to_string())
            }
            "create_project" => {
                let source = PathBuf::from(str_field(req, "source")?);
                if !source.is_file() {
                    return Err(format!("找不到錄音檔：{}", source.display()));
                }
                let mut options: JobOptions = serde_json::from_value(req["options"].clone()).unwrap_or_default();
                let mut ex = options.export.take().unwrap_or_else(|| s.settings().export);
                if ex.dir.trim().is_empty() {
                    ex.dir = s.exports_dir.to_string_lossy().into_owned();
                }
                options.export = Some(ex);
                let mut p = Project::new(source, options);
                if let Some(t) = req["title"].as_str().map(str::trim).filter(|t| !t.is_empty()) {
                    p.title = t.to_string();
                }
                if let Some(d) = req["duration"].as_f64() {
                    p.duration = d;
                }
                s.store().save(&p).map_err(|e| e.to_string())?;
                if req["start"].as_bool().unwrap_or(true) {
                    s.enqueue(Job::Run(p.id));
                }
                Ok(s.full_view(&p))
            }
            // delete_exports：一併刪掉匯出到手機的檔（2026-10-31 使用者：「刪除這場會議 可以詢問是否要刪除已匯出到手機的檔案
            // 大部分User並不會開啟資料夾手動刪除」）。只刪 export_files 列出的、本程式產生的檔；資料夾空了也刪。
            "delete_project" => {
                let p = s.load(req)?;
                if s.is_busy(p.id) {
                    return Err("這場會議正在處理，先取消再刪除".into());
                }
                let (mut deleted, mut failed) = (Vec::new(), Vec::new());
                if req["delete_exports"].as_bool().unwrap_or(false) {
                    let files = s.export_files(&p);
                    for f in &files {
                        match std::fs::remove_file(f) {
                            Ok(()) => deleted.push(f.clone()),
                            // 刪不掉的（例如 App 重裝後，舊安裝建的檔不再屬於這個 App）回報給畫面
                            Err(_) => failed.push(f.clone()),
                        }
                    }
                    // 這場會議自己的資料夾空了就一起刪（共用的匯出資料夾本身不刪）
                    if let Some(dir) = s.own_folder(&p)
                        && std::fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_none())
                    {
                        let _ = std::fs::remove_dir(&dir);
                    }
                }
                s.store().delete(p.id).map_err(|e| e.to_string())?;
                s.remove_import(&p);
                Ok(json!({ "deleted": deleted, "failed": failed }))
            }
            "export" => {
                let p = s.load(req)?;
                let mut opts: ExportOptions = match req.get("options").filter(|v| !v.is_null()) {
                    Some(v) => serde_json::from_value(v.clone()).map_err(|e| format!("匯出選項格式錯誤：{e}"))?,
                    None => p.options.export.clone().unwrap_or_else(|| s.settings().export),
                };
                s.fix_export_dir(&mut opts);
                let mut files = export::export_project(&p, &opts).map_err(|e| e.to_string())?;
                // 會議資料夾附一份錄音（2026-10-31 使用者：「匯出時一併存一份錄音」）
                if opts.audio
                    && let Ok(Some(f)) = s.export_audio(&p, &opts)
                {
                    files.push(f);
                }
                // 記在專案上：刪除會議時才知道有哪些檔要一起刪（處理中的專案由背景工作存檔，不在這裡改）
                if !s.is_busy(p.id) {
                    let mut p = p;
                    for f in &files {
                        if !p.exported.contains(f) {
                            p.exported.push(f.clone());
                        }
                    }
                    let _ = s.store().save(&p);
                }
                Ok(json!(files))
            }
            // 以下是使用者在逐字稿頁的編輯；回傳更新後的專案
            "rename_project" => s.edit(req, |p| {
                let t = str_field(req, "title")?.trim();
                if t.is_empty() {
                    return Err("標題不能空白".into());
                }
                p.title = t.to_string();
                Ok(())
            }),
            "rename_speaker" => s.edit(req, |p| {
                p.rename_speaker(str_field(req, "speaker")?, str_field(req, "name")?);
                Ok(())
            }),
            "merge_speakers" => s.edit(req, |p| {
                if p.merge_speakers(str_field(req, "from")?, str_field(req, "into")?) {
                    Ok(())
                } else {
                    Err("無法合併這兩位說話者".into())
                }
            }),
            "edit_segment" => s.edit(req, |p| {
                let seg = segment(p, req)?;
                seg.text = str_field(req, "text")?.trim().to_string();
                seg.edited = true;
                Ok(())
            }),
            "set_segment_speaker" => s.edit(req, |p| {
                let sp = str_field(req, "speaker")?.to_string();
                if p.speaker(&sp).is_none() {
                    return Err(format!("沒有說話者 {sp}"));
                }
                segment(p, req)?.speaker = Some(sp);
                p.prune_unused_speakers();
                Ok(())
            }),
            // 背景工作
            "run" => {
                let p = s.load(req)?;
                s.enqueue(Job::Run(p.id));
                Ok(Value::Null)
            }
            "reidentify" => {
                let p = s.load(req)?;
                s.enqueue(Job::Reidentify(p.id));
                Ok(Value::Null)
            }
            "cancel" => {
                let id = uuid(req)?;
                if let Some(t) = s.tokens.lock().expect("tokens").get(&id) {
                    t.cancel();
                }
                Ok(Value::Null)
            }
            "regenerate_summary" => {
                let p = s.load(req)?;
                if s.is_busy(p.id) {
                    return Err("這場會議正在處理".into());
                }
                let instructions = req["instructions"].as_str().unwrap_or("").trim().to_string();
                let sh = s.clone();
                self.rt.spawn(async move {
                    let engine = match sh.engine().await {
                        Ok(e) => e,
                        Err(e) => return sh.toast(true, e),
                    };
                    let id = p.id;
                    match engine
                        .regenerate_summary(p, instructions, sh.sink(), CancellationToken::new())
                        .await
                    {
                        Ok(p) => {
                            sh.toast(false, "摘要已重新產生");
                            sh.emit(json!({ "type": "finished", "project": sh.full_view(&p) }));
                        }
                        Err(e) => {
                            sh.toast(true, format!("重新產生摘要失敗：{e}"));
                            sh.emit(json!({ "type": "summary_failed", "id": id }));
                        }
                    }
                });
                Ok(Value::Null)
            }
            "test_key" => {
                let k = kind(req)?;
                let sh = s.clone();
                self.rt.spawn(async move {
                    let r = meetscribe_core::account::test_key(&sh.settings(), k).await;
                    sh.emit(json!({
                        "type": "key_test", "kind": kind_name(k), "ok": r.is_ok(),
                        "text": r.unwrap_or_else(|e| e),
                    }));
                });
                Ok(Value::Null)
            }
            "load_google_models" => {
                let sh = s.clone();
                self.rt.spawn(async move {
                    let r = match ApiKeys::load().google {
                        Some(k) => match Gemini::new(&sh.settings(), k) {
                            Ok(g) => g.list_models().await.map_err(|e| e.to_string()),
                            Err(e) => Err(e.to_string()),
                        },
                        None => Err("尚未設定 Google AI Studio API Key".into()),
                    };
                    sh.emit(models_event("google", r));
                });
                Ok(Value::Null)
            }
            "load_openrouter_models" => {
                let sh = s.clone();
                self.rt.spawn(async move {
                    // 模型清單不需要金鑰，給個占位值通過檢查
                    let r = match OpenRouter::new(&sh.settings(), "public".into()) {
                        Ok(c) => c
                            .list_models()
                            .await
                            .map(|v| v.into_iter().filter(|m| !m.output_modalities.is_empty()).map(|m| m.id).collect())
                            .map_err(|e| e.to_string()),
                        Err(e) => Err(e.to_string()),
                    };
                    sh.emit(models_event("openrouter", r));
                });
                Ok(Value::Null)
            }
            "estimate_cost" => {
                // 只查 STT 價格，不需要金鑰
                let secs = req["duration"].as_f64().unwrap_or(0.0);
                let settings = s.settings();
                let price = self.rt.block_on(async {
                    let c = OpenRouter::new(&settings, "public".into()).ok()?;
                    c.stt_capabilities(&settings.stt_model).await.ok()?.price_per_second
                });
                Ok(json!(price.map(|p| p * secs)))
            }
            other => Err(format!("不認得的指令：{other}")),
        }
    }
}

impl Shared {
    fn store(&self) -> ProjectStore {
        ProjectStore::new(self.paths.projects_dir())
    }
    fn settings(&self) -> Settings {
        self.settings.lock().expect("settings").clone()
    }
    fn busy(&self) -> Vec<Uuid> {
        self.queue.lock().expect("queue").clone()
    }
    fn is_busy(&self, id: Uuid) -> bool {
        self.queue.lock().expect("queue").contains(&id)
    }

    fn emit(&self, v: Value) {
        self.events.lock().expect("events").push_back(v);
        self.ready.notify_all();
    }
    fn toast(&self, error: bool, text: impl Into<String>) {
        self.emit(json!({ "type": "toast", "error": error, "text": text.into() }));
    }
    fn publish_busy(&self) {
        self.emit(json!({ "type": "busy", "ids": self.busy() }));
    }

    fn sink(self: &Arc<Self>) -> EventSink {
        let me = self.clone();
        Arc::new(move |e| {
            let v = match e {
                Event::Progress {
                    id,
                    stage,
                    fraction,
                    message,
                } => json!({
                    "type": "progress", "id": id, "stage": stage, "stage_label": stage.label(),
                    "fraction": fraction, "message": message,
                }),
                Event::Log { id, level, message } => json!({
                    "type": "log", "id": id, "message": message,
                    "level": match level { LogLevel::Info => "info", LogLevel::Warn => "warn", LogLevel::Error => "error" },
                }),
                Event::Updated(p) => json!({ "type": "updated", "project": me.full_view(&p) }),
            };
            me.emit(v);
        })
    }

    async fn engine(&self) -> Res<Engine> {
        Engine::from_settings(&self.paths, self.settings(), ApiKeys::load())
            .await
            .map_err(|e| e.to_string())
    }

    fn load(&self, req: &Value) -> Res<Project> {
        self.store().load(uuid(req)?).map_err(|e| e.to_string())
    }

    fn edit(&self, req: &Value, f: impl FnOnce(&mut Project) -> Res<()>) -> Res<Value> {
        let mut p = self.load(req)?;
        if self.is_busy(p.id) {
            return Err("這場會議正在處理，完成後再編輯".into());
        }
        f(&mut p)?;
        p.touch();
        self.store().save(&p).map_err(|e| e.to_string())?;
        Ok(self.full_view(&p))
    }

    fn enqueue(&self, job: Job) {
        let id = job.id();
        {
            let mut q = self.queue.lock().expect("queue");
            if q.contains(&id) {
                return;
            }
            q.push(id);
        }
        self.tokens.lock().expect("tokens").insert(id, CancellationToken::new());
        self.publish_busy();
        let _ = self.jobs.send(job);
    }

    /// 匯入的錄音副本（`imports/<uuid>/<原檔名>`）：完成、刪除後就不需要了（轉好的 16 kHz 音檔留在專案資料夾）。
    fn remove_import(&self, p: &Project) {
        if let Ok(rel) = p.source_file.strip_prefix(&self.imports_dir)
            && let Some(first) = rel.components().next()
        {
            remove_entry(&self.imports_dir.join(first));
        }
    }

    /// 啟動時清掉沒有任何專案在用的匯入副本（選了檔案卻沒按開始、App 被關掉時留下的）。
    fn clean_imports(&self, projects: &[Project]) {
        let Ok(rd) = std::fs::read_dir(&self.imports_dir) else { return };
        for e in rd.flatten() {
            let path = e.path();
            if !projects.iter().any(|p| p.source_file.starts_with(&path)) {
                remove_entry(&path);
            }
        }
    }

    /// 匯出選項的資料夾：空白或還指在 1.0 的私有區（每場會議建立時會複製一份選項）時換成公開資料夾，
    /// 並照 1.1 的預設每場會議一個子資料夾（同設定檔的 schema 2 遷移）。
    fn fix_export_dir(&self, opts: &mut ExportOptions) {
        let old = opts.dir.clone();
        let mut probe = Settings { export: opts.clone(), ..Settings::default() };
        if migrate_export_dir(&mut probe, &self.exports_dir) {
            opts.dir = probe.export.dir;
            if !old.trim().is_empty() {
                opts.subfolder = true;
            }
        }
    }

    /// 這場會議專屬的子資料夾（輸出選項開了「每場會議一個子資料夾」才有；共用的匯出資料夾不算）。
    fn own_folder(&self, p: &Project) -> Option<PathBuf> {
        let mut opts = p.options.export.clone().unwrap_or_else(|| self.settings().export);
        self.fix_export_dir(&mut opts);
        opts.subfolder.then(|| opts.target_dir(p)).filter(|d| d.is_dir())
    }

    /// 這場會議匯出到手機、還在的檔：記在專案上的（自動匯出與手動匯出）＋專屬子資料夾裡、檔名以這場會議開頭的匯出格式檔。
    /// 只列本程式產生的，刪除會議時給使用者勾選一起刪。
    fn export_files(&self, p: &Project) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = p.exported.iter().filter(|f| f.is_file()).cloned().collect();
        if let Some(dir) = self.own_folder(p) {
            let mut opts = p.options.export.clone().unwrap_or_else(|| self.settings().export);
            self.fix_export_dir(&mut opts);
            let stem = opts.file_stem(p);
            let mut exts: Vec<String> = export::registry().iter().map(|e| e.extension().to_string()).collect();
            // 會議資料夾裡的那份錄音
            exts.extend(self.audio_of(p).and_then(|f| audio_ext(&f)));
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let f = e.path();
                    let name = f.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
                    let ext = f.extension().and_then(|x| x.to_str()).unwrap_or_default().to_ascii_lowercase();
                    if f.is_file() && name.starts_with(&stem) && exts.contains(&ext) && !out.contains(&f) {
                        out.push(f);
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// 這場會議的匯出資料夾（已存在才給）：依目前輸出選項算出來的位置；還沒有的話，以前匯出過的檔所在處
    /// （1.0 的檔還在舊的私有區，新匯出會放到公開資料夾，以新的為準）。
    fn meeting_folder(&self, p: &Project) -> Option<PathBuf> {
        let mut opts = p.options.export.clone().unwrap_or_else(|| self.settings().export);
        self.fix_export_dir(&mut opts);
        Some(opts.target_dir(p))
            .filter(|d| d.is_dir())
            .or_else(|| p.exported.iter().find_map(|f| f.parent().filter(|d| d.is_dir()).map(PathBuf::from)))
    }

    /// 這場會議的錄音：播放用的高品質副本（Android 就是原始錄音），還沒做出來（處理中）就用原始檔。
    fn audio_of(&self, p: &Project) -> Option<PathBuf> {
        p.playback_file
            .as_deref()
            .map(|f| self.store().path_in(p.id, f))
            .filter(|f| f.is_file())
            .or_else(|| Some(p.source_file.clone()).filter(|f| f.is_file()))
            .or_else(|| p.audio_file.as_deref().map(|f| self.store().path_in(p.id, f)).filter(|f| f.is_file()))
    }

    /// 會議資料夾放一份錄音：`<檔名>.<副檔名>`（Android 的播放檔就是原始錄音，不重壓）。已有一樣大小的同名檔就不再寫。
    fn export_audio(&self, p: &Project, opts: &ExportOptions) -> Res<Option<PathBuf>> {
        let Some(src) = self.audio_of(p) else { return Ok(None) };
        copy_audio(&src, &opts.target_dir(p), &opts.file_stem(p), opts.overwrite).map(Some)
    }

    /// 錄音管理頁的清單：錄音庫裡還沒建會議的錄音＋每場會議（新的在前）。
    /// 已建會議（轉錄中、失敗）的錄音不另外列，由那場會議代表，同一段錄音才不會出現兩次。
    fn recordings(&self) -> Vec<Value> {
        let projects = self.store().list();
        let mut items: Vec<(chrono::DateTime<chrono::Local>, Value)> = self
            .library
            .list()
            .iter()
            .filter(|r| !projects.iter().any(|p| p.source_file == r.path))
            .map(|r| (r.info.created_at, self.recording_view(r, &projects)))
            .collect();
        items.extend(projects.iter().map(|p| (p.created_at, self.meeting_recording_view(p))));
        items.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
        items.into_iter().map(|(_, v)| v).collect()
    }

    fn recording_view(&self, r: &Recording, projects: &[Project]) -> Value {
        let project = projects.iter().find(|p| p.source_file == r.path);
        json!({
            "kind": "recording",
            "id": r.id,
            "project_id": project.map(|p| p.id),
            "title": r.info.title,
            "created_at": r.info.created_at.format("%Y-%m-%d %H:%M").to_string(),
            "duration": r.info.duration,
            "size": r.size(),
            "path": r.path,
            "ext": audio_ext(&r.path),
            "status": "untranscribed",
            "status_label": "未轉錄",
            "busy": false,
            "broken": r.info.broken,
        })
    }

    fn meeting_recording_view(&self, p: &Project) -> Value {
        let summary = self.summary_view(p);
        let audio = self.audio_of(p);
        json!({
            "kind": "meeting",
            "id": p.id,
            "project_id": p.id,
            "title": p.title,
            "created_at": summary["created_at"],
            "duration": p.duration,
            "size": audio.as_ref().and_then(|f| std::fs::metadata(f).ok()).map(|m| m.len()).unwrap_or(0),
            "path": audio,
            "ext": audio.as_deref().and_then(audio_ext),
            "status": summary["status"],
            "status_label": if summary["status"] == "done" { json!("已轉錄") } else { summary["status_label"].clone() },
            "busy": summary["busy"],
            "broken": false,
            "segments": p.segments.len(),
            "has_summary": p.summary.is_some(),
            "speakers": p.speakers.len(),
        })
    }

    /// 處理完成：錄音庫的那段錄音併進會議（專案資料夾已有播放用副本才刪，免得只剩 16 kHz 的版本）；
    /// 開著自動匯出的話，會議資料夾也放一份錄音。
    fn after_done(&self, p: &mut Project) {
        if let Some(id) = self.library.id_of(&p.source_file)
            && p.playback_file.as_deref().is_some_and(|f| self.store().path_in(p.id, f).is_file())
        {
            let _ = self.library.delete(id);
        }
        let mut opts = p.options.export.clone().unwrap_or_else(|| self.settings().export);
        self.fix_export_dir(&mut opts);
        // 自動匯出有產出檔案才附錄音（沒開自動匯出、或全部格式都沒產出時不動）
        if opts.auto
            && opts.audio
            && !p.exported.is_empty()
            && let Ok(Some(f)) = self.export_audio(p, &opts)
            && !p.exported.contains(&f)
        {
            p.exported.push(f);
            let _ = self.store().save(p);
        }
    }

    /// 清單用：不含逐字稿。
    fn summary_view(&self, p: &Project) -> Value {
        let busy = self.is_busy(p.id);
        let (status, error) = status_key(&p.status);
        json!({
            "id": p.id,
            "title": p.title,
            "created_at": p.created_at.format("%Y-%m-%d %H:%M").to_string(),
            "duration": p.duration,
            "status": if busy && p.status != ProjectStatus::Running { "queued" } else { status },
            "status_label": p.status.label(),
            "error": error,
            "stage": p.stage,
            "stage_label": p.stage.label(),
            "resumable": p.status.is_resumable() && !busy,
            "busy": busy,
            "speakers": p.speakers.len(),
            "has_summary": p.summary.is_some(),
            "segments": p.segments.len(),
            "total_usd": p.usage.total_usd(),
        })
    }

    /// 專案頁用：完整內容。
    fn full_view(&self, p: &Project) -> Value {
        let mut v = self.summary_view(p);
        let store = self.store();
        let audio = p
            .playback_audio()
            .map(|rel| store.path_in(p.id, rel))
            .filter(|f| f.is_file());
        let done_chunks = p.chunks.iter().filter(|c| c.stt.is_some()).count();
        let extra = json!({
            "audio_path": audio,
            "chunks_total": p.chunks.len(),
            "chunks_done": done_chunks,
            "options": p.options,
            "speakers": p.speakers.iter().map(|s| json!({
                "id": s.id, "name": s.name, "color": s.color, "note": s.note,
                "segments": p.segments.iter().filter(|g| g.speaker.as_deref() == Some(&s.id)).count(),
            })).collect::<Vec<_>>(),
            "segments": p.segments.iter().map(|g| json!({
                "id": g.id, "start": g.start, "end": g.end, "speaker": g.speaker,
                "speaker_name": p.speaker_name(g.speaker.as_deref()),
                "text": g.text, "edited": g.edited,
                "estimated": g.timing == meetscribe_core::model::Timing::Estimated,
            })).collect::<Vec<_>>(),
            "summary": p.summary.as_ref().map(|s| json!({
                "markdown": s.markdown, "model": s.model, "instructions": s.instructions,
                "created_at": s.created_at.format("%Y-%m-%d %H:%M").to_string(),
            })),
            "usage": {
                "stt_usd": p.usage.stt_usd, "llm_usd": p.usage.llm_usd, "total_usd": p.usage.total_usd(),
                "stt_seconds": p.usage.stt_seconds,
            },
            "models": p.models,
            "warnings": p.warnings,
            "exported": p.exported,
            "folder": self.meeting_folder(p),
            "export_files": self.export_files(p),
            // 原始檔是錄音庫裡的錄音（還沒處理完）：刪掉會議後錄音留在錄音管理
            "keeps_recording": self.library.id_of(&p.source_file).is_some() && p.source_file.is_file(),
            "stages": Stage::ALL.iter().map(|s| json!({ "id": s, "label": s.label() })).collect::<Vec<_>>(),
        });
        if let (Value::Object(a), Value::Object(b)) = (&mut v, extra) {
            a.extend(b);
        }
        v
    }
}

/// 1.1 之前處理的會議升級一次（不呼叫 API）：重新對齊每句的時間、補做播放音訊（匯入檔還在時）。同桌面 worker。
async fn upgrade_projects(shared: Arc<Shared>) {
    let Ok(audio) = AudioTool::locate("") else { return };
    let store = shared.store();
    let cancel = CancellationToken::new();
    for mut p in store.list() {
        if p.segments.is_empty() || shared.is_busy(p.id) {
            continue;
        }
        let dir = store.dir(p.id);
        let retimed = meetscribe_core::pipeline::retime_project(&audio, &dir, &mut p, &cancel).await.unwrap_or(false);
        let playback = meetscribe_core::pipeline::ensure_playback(&audio, &dir, &mut p, &cancel).await.unwrap_or(false);
        // 每場會議自己那份輸出選項也從 1.0 的私有區搬到公開資料夾（之後續跑、自動匯出才會放對地方）
        let moved = match p.options.export.as_mut() {
            Some(ex) => {
                let before = ex.dir.clone();
                shared.fix_export_dir(ex);
                ex.dir != before
            }
            None => false,
        };
        if (retimed || playback || moved) && store.save(&p).is_ok() {
            shared.emit(json!({ "type": "updated", "project": shared.full_view(&p) }));
        }
    }
}

/// 匯出資料夾沒設、或還指在 App 的外部檔案區（`…/Android/data/…`，檔案 App 看不到）而現在有公開資料夾可用：換過去。
fn migrate_export_dir(settings: &mut Settings, exports_dir: &std::path::Path) -> bool {
    let current = settings.export.dir.trim();
    let private = |d: &str| d.replace('\\', "/").contains("/Android/data/");
    let target = exports_dir.to_string_lossy();
    if current.is_empty() || (private(current) && !private(&target) && current != target) {
        settings.export.dir = target.into_owned();
        return true;
    }
    false
}

/// 一次跑一個專案：避免同時打爆 API 額度，也讓進度清楚。
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
        let store = shared.store();
        let finished = match store.load(id) {
            Err(e) => {
                shared.toast(true, format!("讀不到專案：{e}"));
                None
            }
            Ok(mut p) if token.is_cancelled() => {
                p.status = ProjectStatus::Cancelled;
                let _ = store.save(&p);
                Some(p)
            }
            Ok(p) => match shared.engine().await {
                Ok(engine) => Some(match job {
                    Job::Run(_) => engine.run(p, shared.sink(), token).await,
                    Job::Reidentify(_) => engine.reidentify_speakers(p, shared.sink(), token).await,
                }),
                Err(e) => {
                    let mut p = p;
                    p.status = ProjectStatus::Failed(e.clone());
                    let _ = store.save(&p);
                    shared.toast(true, e);
                    Some(p)
                }
            },
        };
        shared.tokens.lock().expect("tokens").remove(&id);
        shared.queue.lock().expect("queue").retain(|x| *x != id);
        if let Some(mut p) = finished {
            if p.status == ProjectStatus::Done {
                shared.after_done(&mut p);
                shared.remove_import(&p);
                shared.toast(false, format!("「{}」處理完成", p.title));
            }
            shared.emit(json!({ "type": "finished", "project": shared.full_view(&p) }));
        }
        shared.publish_busy();
    }
}

/// 音檔的副檔名（小寫）。
fn audio_ext(f: &Path) -> Option<String> {
    f.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).filter(|e| !e.is_empty())
}

/// 複製錄音到 `dir/<stem>.<副檔名>`：已有一樣大小的就直接用那個檔；不一樣時 `overwrite` 覆蓋，否則另存「名稱 (2)」。
fn copy_audio(src: &Path, dir: &Path, stem: &str, overwrite: bool) -> Res<PathBuf> {
    let ext = audio_ext(src).unwrap_or_else(|| "m4a".into());
    std::fs::create_dir_all(dir).map_err(|e| format!("無法建立 {}：{e}", dir.display()))?;
    let len = std::fs::metadata(src).map_err(|e| format!("讀不到錄音：{e}"))?.len();
    let same = |f: &Path| std::fs::metadata(f).is_ok_and(|m| m.len() == len);
    let first = dir.join(format!("{stem}.{ext}"));
    let target = if !first.exists() || same(&first) || overwrite {
        first
    } else {
        (2..)
            .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
            .find(|f| !f.exists() || same(f))
            .expect("unbounded")
    };
    if !same(&target) {
        std::fs::copy(src, &target).map_err(|e| format!("無法存錄音：{e}"))?;
    }
    Ok(target)
}

/// 刪匯入區的一個項目（檔案或 `<uuid>` 資料夾）。只會被傳入 imports 資料夾的直接子項目。
fn remove_entry(path: &std::path::Path) {
    let _ = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
}

fn status_key(s: &ProjectStatus) -> (&'static str, Option<&str>) {
    match s {
        ProjectStatus::Queued => ("queued", None),
        ProjectStatus::Running => ("running", None),
        ProjectStatus::Done => ("done", None),
        ProjectStatus::Failed(e) => ("failed", Some(e.as_str())),
        ProjectStatus::Cancelled => ("cancelled", None),
    }
}

fn key_status() -> Value {
    let one = |k| secrets::load(k).map(|(key, _)| secrets::mask(&key));
    json!({ "openrouter": one(KeyKind::OpenRouter), "google": one(KeyKind::Google) })
}

fn models_event(provider: &str, r: Res<Vec<String>>) -> Value {
    match r {
        Ok(models) => json!({ "type": "models", "provider": provider, "ok": true, "models": models }),
        Err(e) => json!({ "type": "models", "provider": provider, "ok": false, "error": e }),
    }
}

fn str_field<'a>(req: &'a Value, name: &str) -> Res<&'a str> {
    req[name].as_str().ok_or_else(|| format!("缺少 {name}"))
}

fn uuid(req: &Value) -> Res<Uuid> {
    Uuid::parse_str(str_field(req, "id")?).map_err(|e| format!("專案 ID 錯誤：{e}"))
}

fn kind(req: &Value) -> Res<KeyKind> {
    match str_field(req, "kind")? {
        "openrouter" => Ok(KeyKind::OpenRouter),
        "google" => Ok(KeyKind::Google),
        k => Err(format!("不認得的金鑰種類：{k}")),
    }
}

fn kind_name(k: KeyKind) -> &'static str {
    match k {
        KeyKind::OpenRouter => "openrouter",
        KeyKind::Google => "google",
    }
}

fn segment<'a>(p: &'a mut Project, req: &Value) -> Res<&'a mut meetscribe_core::model::Segment> {
    let sid = req["segment"].as_u64().ok_or("缺少 segment")?;
    p.segments
        .iter_mut()
        .find(|g| g.id == sid)
        .ok_or_else(|| format!("找不到段落 {sid}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge() -> (tempfile::TempDir, Bridge) {
        let dir = tempfile::tempdir().unwrap();
        let b = Bridge::new(dir.path().join("data"), dir.path().join("exports")).unwrap();
        (dir, b)
    }

    fn ok(b: &Bridge, req: Value) -> Value {
        let r: Value = serde_json::from_str(&b.call(&req.to_string())).unwrap();
        assert_eq!(r["ok"], true, "{req} → {r}");
        r["data"].clone()
    }

    #[test]
    fn state_keys_and_settings_roundtrip() {
        let (dir, b) = bridge();
        let st = ok(&b, json!({ "op": "state" }));
        assert!(st["keys"]["openrouter"].is_null());
        // 匯出資料夾預設改成 App 的匯出區
        assert_eq!(
            st["settings"]["export"]["dir"].as_str().unwrap(),
            dir.path().join("exports").to_string_lossy()
        );
        assert!(st["exporters"].as_array().unwrap().iter().any(|e| e["id"] == "docx"));

        let keys = ok(&b, json!({ "op": "save_key", "kind": "openrouter", "key": "sk-or-v1-abcdefghijklmnop" }));
        // 金鑰檔位置是整個行程共用（App 只有一個 Bridge），測試平行跑時指向先初始化的那個
        assert_eq!(keys["openrouter"], "sk-or-v1…mnop");
        let keys = ok(&b, json!({ "op": "delete_key", "kind": "openrouter" }));
        assert!(keys["openrouter"].is_null());

        let mut s = st["settings"].clone();
        s["chunk_minutes"] = json!(999);
        let saved = ok(&b, json!({ "op": "save_settings", "settings": s }));
        assert_eq!(saved["chunk_minutes"], 55, "會被 sanitized 拉回範圍");
    }

    #[test]
    fn create_edit_export_delete_project() {
        let (dir, b) = bridge();
        let src = dir.path().join("data/imports/0001/週會錄音.m4a");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, b"not really audio").unwrap();
        let p = ok(
            &b,
            json!({ "op": "create_project", "source": src, "title": "週會", "start": false,
                    "options": { "participants": ["王小明"], "generate_summary": true } }),
        );
        let id = p["id"].as_str().unwrap().to_string();
        assert_eq!(p["title"], "週會");
        assert_eq!(p["options"]["participants"][0], "王小明");
        assert_eq!(p["status"], "queued");

        // 手動塞一段逐字稿，測編輯與匯出
        let store = ProjectStore::new(dir.path().join("data/projects"));
        let mut proj = store.load(Uuid::parse_str(&id).unwrap()).unwrap();
        let s1 = proj.add_speaker(Some("甲".into()), String::new());
        let s2 = proj.add_speaker(Some("乙".into()), String::new());
        for (i, sp) in [&s1, &s2].iter().enumerate() {
            proj.segments.push(meetscribe_core::model::Segment {
                id: i as u64 + 1,
                start: i as f64 * 5.0,
                end: i as f64 * 5.0 + 4.0,
                timing: meetscribe_core::model::Timing::Model,
                speaker: Some(sp.to_string()),
                text: format!("第{i}句"),
                chunk: 0,
                edited: false,
            });
        }
        proj.status = ProjectStatus::Done;
        store.save(&proj).unwrap();

        let v = ok(&b, json!({ "op": "rename_speaker", "id": id, "speaker": s1, "name": "主席" }));
        assert_eq!(v["segments"][0]["speaker_name"], "主席");
        let v = ok(&b, json!({ "op": "edit_segment", "id": id, "segment": 2, "text": "改過的句子" }));
        assert_eq!(v["segments"][1]["text"], "改過的句子");
        assert_eq!(v["segments"][1]["edited"], true);
        let v = ok(&b, json!({ "op": "set_segment_speaker", "id": id, "segment": 2, "speaker": s1 }));
        assert_eq!(v["speakers"].as_array().unwrap().len(), 1, "沒有發言的說話者會被移除");

        let files = ok(&b, json!({ "op": "export", "id": id }));
        let files: Vec<PathBuf> = serde_json::from_value(files).unwrap();
        assert!(!files.is_empty());
        assert!(files.iter().all(|f| f.starts_with(dir.path().join("exports")) && f.is_file()));
        // 預設每場會議一個子資料夾；專案頁的「開啟資料夾」用 folder
        let folder = dir.path().join("exports").join("週會");
        assert!(files.iter().all(|f| f.parent() == Some(folder.as_path())));
        let v = ok(&b, json!({ "op": "load_project", "id": id }));
        assert_eq!(PathBuf::from(v["folder"].as_str().unwrap()), folder);

        let list = ok(&b, json!({ "op": "list_projects" }));
        assert_eq!(list.as_array().unwrap().len(), 1);
        ok(&b, json!({ "op": "delete_project", "id": id }));
        assert!(!src.parent().unwrap().exists(), "匯入的副本（含資料夾）跟著刪掉");
        assert_eq!(ok(&b, json!({ "op": "list_projects" })).as_array().unwrap().len(), 0);
    }

    #[test]
    fn export_dir_moves_from_app_private_area_to_public_documents() {
        let public = PathBuf::from("/storage/emulated/0/Documents/語音轉錄");
        let old = "/storage/emulated/0/Android/data/io.github.win81134679.meetscribe/files/exports";
        let mut s = Settings::default();
        s.export.dir = old.into();
        assert!(migrate_export_dir(&mut s, &public));
        assert_eq!(s.export.dir, public.to_string_lossy());
        // 使用者自己選的資料夾不動；公開資料夾不能用（Kotlin 退回私有區）時也不動
        s.export.dir = "/storage/emulated/0/Download/會議".into();
        assert!(!migrate_export_dir(&mut s, &public));
        s.export.dir = old.into();
        assert!(!migrate_export_dir(&mut s, std::path::Path::new(old)));
        s.export.dir = String::new();
        assert!(migrate_export_dir(&mut s, &public));
    }

    #[test]
    fn old_meeting_options_export_to_the_public_folder() {
        let dir = tempfile::tempdir().unwrap();
        let public = dir.path().join("Documents/語音轉錄");
        let b = Bridge::new(dir.path().join("data"), public.clone()).unwrap();
        let mut opts = ExportOptions {
            dir: "/storage/emulated/0/Android/data/io.github.win81134679.meetscribe/files/exports".into(),
            subfolder: false,
            ..ExportOptions::default()
        };
        b.shared.fix_export_dir(&mut opts);
        assert_eq!(PathBuf::from(&opts.dir), public);
        assert!(opts.subfolder, "1.0 的會議也照新預設一場一個資料夾");
        let mut mine = ExportOptions { dir: "/storage/emulated/0/Download".into(), subfolder: false, ..ExportOptions::default() };
        b.shared.fix_export_dir(&mut mine);
        assert_eq!(mine.dir, "/storage/emulated/0/Download", "使用者自選的不動");
        assert!(!mine.subfolder);
    }

    #[test]
    fn update_info_reports_device_and_version_change_once() {
        let (_dir, b) = bridge();
        let first = ok(&b, json!({ "op": "update_info", "version": "1.0.0" }));
        let id = first["device_id"].as_str().unwrap().to_string();
        assert!(update::is_device_id(&id));
        assert!(first["updated_from"].is_null());
        assert!(first["ready"].is_null());
        let st = ok(&b, json!({ "op": "state" }));
        assert_eq!(st["device_id"], id.as_str());
        let after = ok(&b, json!({ "op": "update_info", "version": "1.1.0" }));
        assert_eq!(after["updated_from"], "1.0.0");
        assert_eq!(after["device_id"], id.as_str());
        assert!(ok(&b, json!({ "op": "update_info", "version": "1.1.0" }))["updated_from"].is_null());
    }

    #[test]
    fn deleting_a_meeting_can_remove_its_exported_files() {
        let (dir, b) = bridge();
        let make = |name: &str| {
            let src = dir.path().join(format!("data/imports/{name}/{name}.m4a"));
            std::fs::create_dir_all(src.parent().unwrap()).unwrap();
            std::fs::write(&src, b"x").unwrap();
            let p = ok(&b, json!({ "op": "create_project", "source": src, "title": name, "start": false }));
            let id = p["id"].as_str().unwrap().to_string();
            let store = ProjectStore::new(dir.path().join("data/projects"));
            let mut proj = store.load(Uuid::parse_str(&id).unwrap()).unwrap();
            proj.segments.push(meetscribe_core::model::Segment {
                id: 1, start: 0.0, end: 2.0, timing: meetscribe_core::model::Timing::Model,
                speaker: None, text: "大家好".into(), chunk: 0, edited: false,
            });
            proj.status = ProjectStatus::Done;
            store.save(&proj).unwrap();
            id
        };
        let keep = make("留著");
        let gone = make("週會");
        for id in [&keep, &gone] {
            ok(&b, json!({ "op": "export", "id": id }));
        }
        let folder = dir.path().join("exports").join("週會");
        // 使用者自己放進來的檔（不是匯出格式、或檔名不是這場會議）不能被刪
        std::fs::write(folder.join("我的筆記.jpg"), b"x").unwrap();
        let view = ok(&b, json!({ "op": "load_project", "id": gone }));
        let listed = view["export_files"].as_array().unwrap().len();
        assert!(listed >= 2, "md、srt 都列出來（沒有摘要不產 Word、PDF），實際 {listed}");

        let r = ok(&b, json!({ "op": "delete_project", "id": gone, "delete_exports": true }));
        assert_eq!(r["deleted"].as_array().unwrap().len(), listed);
        assert!(r["failed"].as_array().unwrap().is_empty());
        assert!(folder.join("我的筆記.jpg").exists(), "不是我們產生的檔留著");
        assert!(!folder.join("週會.md").exists());

        // 不勾：匯出檔留著
        let r = ok(&b, json!({ "op": "delete_project", "id": keep, "delete_exports": false }));
        assert!(r["deleted"].as_array().unwrap().is_empty());
        assert!(dir.path().join("exports/留著/留著.md").exists());

        // 資料夾清空的話一起刪
        std::fs::remove_file(folder.join("我的筆記.jpg")).unwrap();
        let again = make("週會");
        ok(&b, json!({ "op": "export", "id": again }));
        ok(&b, json!({ "op": "delete_project", "id": again, "delete_exports": true }));
        assert!(!folder.exists(), "空的會議資料夾一起刪掉");
    }

    /// 1 秒的靜音 WAV（兩個音訊後端都讀得出長度）。
    fn wav(path: &std::path::Path) {
        let rate = 16000u32;
        let data = vec![0u8; rate as usize * 2];
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(&data);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b).unwrap();
    }

    /// 錄音管理（2026-10-31）：錄完登記、改名、存到手機、轉錄時由會議代表、完成後併進會議、
    /// 匯出時會議資料夾附一份錄音、刪除會議可一併刪掉那份錄音。
    #[test]
    fn recording_library_lifecycle_and_audio_export() {
        let (dir, b) = bridge();
        let st = ok(&b, json!({ "op": "state" }));
        let rec_dir = PathBuf::from(st["recordings_dir"].as_str().unwrap());
        let file = rec_dir.join(Uuid::new_v4().to_string()).join("會議錄音 2026-10-31 0900.wav");
        wav(&file);
        let r = ok(&b, json!({ "op": "add_recording", "path": file, "title": "會議錄音 2026-10-31 0900" }));
        assert_eq!(r["kind"], "recording");
        assert_eq!(r["status"], "untranscribed");
        assert_eq!(r["broken"], false);
        assert!((r["duration"].as_f64().unwrap() - 1.0).abs() < 0.05, "{r}");
        let rid = r["id"].as_str().unwrap().to_string();

        // 壞掉的檔（讀不出長度）也收，標成不完整
        let broken = rec_dir.join(Uuid::new_v4().to_string()).join("壞的.m4a");
        std::fs::create_dir_all(broken.parent().unwrap()).unwrap();
        std::fs::write(&broken, b"not audio").unwrap();
        let bad = ok(&b, json!({ "op": "add_recording", "path": broken, "title": "壞的" }));
        assert_eq!(bad["broken"], true);
        ok(&b, json!({ "op": "delete_recording", "id": bad["id"] }));
        assert!(!broken.exists());

        let v = ok(&b, json!({ "op": "rename_recording", "id": rid, "title": "週會" }));
        assert_eq!(v["title"], "週會");
        let saved = ok(&b, json!({ "op": "save_recording", "id": rid, "kind": "recording" }));
        let saved = PathBuf::from(saved.as_str().unwrap());
        assert_eq!(saved, dir.path().join("exports/錄音/週會.wav"));
        assert!(saved.is_file());
        let again = ok(&b, json!({ "op": "save_recording", "id": rid, "kind": "recording" }));
        assert_eq!(PathBuf::from(again.as_str().unwrap()), saved, "同一段錄音不會存成「週會 (2)」");

        // 拿去轉錄：清單上由會議代表，錄音庫那筆不另外列、也不能從錄音管理刪
        let p = ok(&b, json!({ "op": "create_project", "source": file, "title": "週會", "duration": 1.0, "start": false }));
        let pid = p["id"].as_str().unwrap().to_string();
        let list = ok(&b, json!({ "op": "list_recordings" }));
        let list = list.as_array().unwrap();
        assert_eq!(list.len(), 1, "{list:?}");
        assert_eq!(list[0]["kind"], "meeting");
        assert_eq!(PathBuf::from(list[0]["path"].as_str().unwrap()), file, "處理前播原始錄音");
        let r: Value = serde_json::from_str(&b.call(&json!({ "op": "delete_recording", "id": rid }).to_string())).unwrap();
        assert_eq!(r["ok"], false);

        // 模擬處理完成：專案資料夾有播放用副本 → 錄音庫那筆併進會議
        let store = ProjectStore::new(dir.path().join("data/projects"));
        let mut proj = store.load(Uuid::parse_str(&pid).unwrap()).unwrap();
        std::fs::copy(&file, store.path_in(proj.id, "playback.wav")).unwrap();
        proj.playback_file = Some("playback.wav".into());
        proj.segments.push(meetscribe_core::model::Segment {
            id: 1, start: 0.0, end: 1.0, timing: meetscribe_core::model::Timing::Model,
            speaker: None, text: "大家好".into(), chunk: 0, edited: false,
        });
        proj.status = ProjectStatus::Done;
        store.save(&proj).unwrap();
        b.shared.after_done(&mut proj);
        assert!(!file.parent().unwrap().exists(), "錄音庫的原檔併進會議後刪掉");
        let list = ok(&b, json!({ "op": "list_recordings" }));
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["status"], "done");
        assert_eq!(list[0]["status_label"], "已轉錄");
        assert_eq!(PathBuf::from(list[0]["path"].as_str().unwrap()), store.path_in(proj.id, "playback.wav"));

        // 匯出：會議資料夾附一份錄音；刪除會議時列進可一起刪的檔
        let files: Vec<PathBuf> = serde_json::from_value(ok(&b, json!({ "op": "export", "id": pid }))).unwrap();
        let audio = dir.path().join("exports/週會/週會.wav");
        assert!(files.contains(&audio), "{files:?}");
        assert!(audio.is_file());
        let files2: Vec<PathBuf> = serde_json::from_value(ok(&b, json!({ "op": "export", "id": pid }))).unwrap();
        assert!(files2.contains(&audio), "再匯出一次沿用同一份錄音");
        assert!(!dir.path().join("exports/週會/週會 (2).wav").exists());
        let view = ok(&b, json!({ "op": "load_project", "id": pid }));
        let listed: Vec<PathBuf> = serde_json::from_value(view["export_files"].clone()).unwrap();
        assert!(listed.contains(&audio));
        let r = ok(&b, json!({ "op": "delete_project", "id": pid, "delete_exports": true }));
        assert!(r["deleted"].as_array().unwrap().iter().any(|f| std::path::Path::new(f.as_str().unwrap()) == audio));
        assert!(!audio.exists());
        assert!(saved.is_file(), "存到「錄音」資料夾的那份是使用者的，不跟著刪");
    }

    #[test]
    fn export_without_audio_option_skips_the_recording() {
        let (dir, b) = bridge();
        let src = dir.path().join("data/imports/a/會議.wav");
        wav(&src);
        let p = ok(&b, json!({ "op": "create_project", "source": src, "title": "會議", "start": false }));
        let store = ProjectStore::new(dir.path().join("data/projects"));
        let mut proj = store.load(Uuid::parse_str(p["id"].as_str().unwrap()).unwrap()).unwrap();
        proj.segments.push(meetscribe_core::model::Segment {
            id: 1, start: 0.0, end: 1.0, timing: meetscribe_core::model::Timing::Model,
            speaker: None, text: "大家好".into(), chunk: 0, edited: false,
        });
        store.save(&proj).unwrap();
        let mut opts = serde_json::to_value(ExportOptions::default()).unwrap();
        opts["dir"] = json!(dir.path().join("exports"));
        opts["audio"] = json!(false);
        let files: Vec<PathBuf> = serde_json::from_value(ok(&b, json!({ "op": "export", "id": p["id"], "options": opts }))).unwrap();
        assert!(files.iter().all(|f| f.extension().unwrap() != "wav"), "{files:?}");
        opts["audio"] = json!(true);
        let files: Vec<PathBuf> = serde_json::from_value(ok(&b, json!({ "op": "export", "id": p["id"], "options": opts }))).unwrap();
        assert!(files.iter().any(|f| f.extension().unwrap() == "wav"), "還沒處理完的會議用原始錄音：{files:?}");
    }

    #[test]
    fn startup_removes_unreferenced_imports() {
        let dir = tempfile::tempdir().unwrap();
        let (data, exports) = (dir.path().join("data"), dir.path().join("exports"));
        let used = data.join("imports/used/會議.m4a");
        let orphan_dir = data.join("imports/orphan/沒按開始.m4a");
        let orphan_file = data.join("imports/old.wav");
        {
            // 先開一次（建立資料夾），再放檔案、建專案，模擬上次用到一半
            let b = Bridge::new(data.clone(), exports.clone()).unwrap();
            for f in [&used, &orphan_dir, &orphan_file] {
                std::fs::create_dir_all(f.parent().unwrap()).unwrap();
                std::fs::write(f, b"x").unwrap();
            }
            ok(&b, json!({ "op": "create_project", "source": used, "start": false }));
        }
        let _b = Bridge::new(data.clone(), exports).unwrap();
        assert!(used.is_file(), "專案還在用的保留");
        assert!(!orphan_dir.parent().unwrap().exists());
        assert!(!orphan_file.exists());
    }

    #[test]
    fn bad_requests_are_errors_not_panics() {
        let (_dir, b) = bridge();
        for req in ["not json", r#"{"op":"nope"}"#, r#"{"op":"load_project","id":"x"}"#, r#"{"op":"save_key","kind":"aws"}"#] {
            let r: Value = serde_json::from_str(&b.call(req)).unwrap();
            assert_eq!(r["ok"], false, "{req}");
            assert!(r["error"].as_str().is_some_and(|e| !e.is_empty()));
        }
        assert_eq!(b.poll(Duration::from_millis(10)), "[]");
    }

    #[test]
    fn running_job_without_key_fails_and_reports() {
        let (dir, b) = bridge();
        let src = dir.path().join("data/imports/rec.wav");
        std::fs::write(&src, b"x").unwrap();
        let p = ok(&b, json!({ "op": "create_project", "source": src, "start": true }));
        // 等到 finished 事件
        let mut finished = None;
        for _ in 0..100 {
            let evs: Vec<Value> = serde_json::from_str(&b.poll(Duration::from_millis(200))).unwrap();
            if let Some(f) = evs.into_iter().find(|e| e["type"] == "finished") {
                finished = Some(f);
                break;
            }
        }
        let f = finished.expect("沒有收到 finished");
        assert_eq!(f["project"]["id"], p["id"]);
        assert_eq!(f["project"]["status"], "failed");
        assert!(f["project"]["resumable"].as_bool().unwrap());
    }
}
