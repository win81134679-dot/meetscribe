//! Kotlin 介面與 core 之間的橋：JSON 進、JSON 出，平台無關（桌面也能測）。
//!
//! - [`Bridge::call`]：同步請求（讀寫設定、專案、金鑰、匯出），回 `{"ok":true,"data":…}` 或 `{"ok":false,"error":…}`。
//! - [`Bridge::send`]：背景工作（跑專案、取消、重新摘要、測金鑰…），結果變成事件。
//! - [`Bridge::poll`]：取走累積的事件（最多等 `timeout`），回 JSON 陣列。
//!
//! 跟桌面的 `worker.rs` 一樣一次只跑一個專案；專案資料從磁碟讀，跑的途中每次存檔都會發 `updated` 事件。

use meetscribe_core::audio::AudioTool;
use meetscribe_core::config::{AppPaths, Settings};
use meetscribe_core::export::{self, Content, ExportOptions};
use meetscribe_core::gemini::Gemini;
use meetscribe_core::model::{JobOptions, Project, ProjectStatus, Stage};
use meetscribe_core::openrouter::OpenRouter;
use meetscribe_core::pipeline::{Engine, Event, EventSink, LogLevel};
use meetscribe_core::secrets::{self, ApiKeys, KeyKind};
use meetscribe_core::store::ProjectStore;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
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
    /// 匯出檔的預設資料夾（App 的外部檔案區，分享時由 FileProvider 開放）。
    exports_dir: PathBuf,
    /// Kotlin 匯入的錄音副本放這裡；處理完成後刪掉。
    imports_dir: PathBuf,
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
        for d in [&exports_dir, &imports_dir] {
            std::fs::create_dir_all(d).map_err(|e| format!("無法建立 {}：{e}", d.display()))?;
        }
        let mut settings = Settings::load(&paths);
        // 桌面的「空白＝原始錄音旁邊」在手機上是 App 內部資料夾，看不到；改成匯出資料夾
        if settings.export.dir.trim().is_empty() {
            settings.export.dir = exports_dir.to_string_lossy().into_owned();
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
                "version": env!("CARGO_PKG_VERSION"),
            })),
            "list_projects" => {
                let mut list = s.store().list();
                list.sort_by_key(|p| std::cmp::Reverse(p.created_at));
                Ok(Value::Array(list.iter().map(|p| s.summary_view(p)).collect()))
            }
            "load_project" => Ok(s.full_view(&s.load(req)?)),
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
            "delete_project" => {
                let p = s.load(req)?;
                if s.is_busy(p.id) {
                    return Err("這場會議正在處理，先取消再刪除".into());
                }
                s.store().delete(p.id).map_err(|e| e.to_string())?;
                s.remove_import(&p);
                Ok(Value::Null)
            }
            "export" => {
                let p = s.load(req)?;
                let mut opts: ExportOptions = match req.get("options").filter(|v| !v.is_null()) {
                    Some(v) => serde_json::from_value(v.clone()).map_err(|e| format!("匯出選項格式錯誤：{e}"))?,
                    None => p.options.export.clone().unwrap_or_else(|| s.settings().export),
                };
                if opts.dir.trim().is_empty() {
                    opts.dir = s.exports_dir.to_string_lossy().into_owned();
                }
                let files = export::export_project(&p, &opts).map_err(|e| e.to_string())?;
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
            .audio_file
            .as_ref()
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
            "stages": Stage::ALL.iter().map(|s| json!({ "id": s, "label": s.label() })).collect::<Vec<_>>(),
        });
        if let (Value::Object(a), Value::Object(b)) = (&mut v, extra) {
            a.extend(b);
        }
        v
    }
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
        if let Some(p) = finished {
            if p.status == ProjectStatus::Done {
                shared.remove_import(&p);
                shared.toast(false, format!("「{}」處理完成", p.title));
            }
            shared.emit(json!({ "type": "finished", "project": shared.full_view(&p) }));
        }
        shared.publish_busy();
    }
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

        let list = ok(&b, json!({ "op": "list_projects" }));
        assert_eq!(list.as_array().unwrap().len(), 1);
        ok(&b, json!({ "op": "delete_project", "id": id }));
        assert!(!src.parent().unwrap().exists(), "匯入的副本（含資料夾）跟著刪掉");
        assert_eq!(ok(&b, json!({ "op": "list_projects" })).as_array().unwrap().len(), 0);
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
