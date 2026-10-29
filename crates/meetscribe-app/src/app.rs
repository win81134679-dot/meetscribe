//! 主視窗（Filee 版型）：左側白色側欄＝標誌、導覽、會議清單、金鑰狀態；右側暖灰頁面依畫面切換。

use crate::player::AudioPlayer;
use crate::theme::{self, icon, pal};
use crate::views::{new_job::NewJobForm, project::ProjectUi, settings::SettingsForm};
use crate::widgets;
use crate::worker::{Cmd, Msg, Worker};
use chrono::{DateTime, Local};
use egui::{Margin, RichText, Stroke};
use meetscribe_core::config::{AppPaths, Settings};
use meetscribe_core::model::{Project, ProjectStatus, Stage};
use meetscribe_core::openrouter::SttCapabilities;
use meetscribe_core::pipeline::{Event, LogLevel};
use meetscribe_core::secrets::{self, KeyKind};
use meetscribe_core::store::ProjectStore;
use meetscribe_core::textutil::fmt_duration_zh;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    NewJob,
    Project,
    Settings,
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub until: Instant,
}

pub struct LogLine {
    pub level: LogLevel,
    pub text: String,
    pub at: DateTime<Local>,
}

pub struct MeetApp {
    pub paths: AppPaths,
    pub settings: Settings,
    pub store: ProjectStore,
    pub worker: Worker,
    pub projects: Vec<Project>,
    pub selected: Option<Uuid>,
    pub view: View,
    pub busy: HashSet<Uuid>,
    pub progress: HashMap<Uuid, (Stage, f32, String)>,
    pub logs: HashMap<Uuid, Vec<LogLine>>,
    pub toasts: Vec<Toast>,
    pub stt_caps: Option<SttCapabilities>,
    pub new_job: NewJobForm,
    pub settings_form: SettingsForm,
    pub project_ui: ProjectUi,
    pub player: Option<AudioPlayer>,
    pub player_error: Option<String>,
    /// 有未存檔的編輯：(專案, 最後一次修改時間)。
    pub dirty: Option<(Uuid, Instant)>,
    pub list_filter: String,
    pub confirm_delete: Option<Uuid>,
    pub has_openrouter_key: bool,
    pub has_google_key: bool,
}

impl MeetApp {
    pub fn new(cc: &eframe::CreationContext<'_>, paths: AppPaths) -> Self {
        theme::install(&cc.egui_ctx);
        let settings = Settings::load(&paths);
        cc.egui_ctx.set_zoom_factor(settings.ui_scale);
        let store = ProjectStore::new(paths.projects_dir());
        let mut projects = store.list();
        // 上次關程式時還在跑的，標成可續跑
        for p in &mut projects {
            if matches!(p.status, ProjectStatus::Running | ProjectStatus::Queued) {
                p.status = ProjectStatus::Cancelled;
                let _ = store.save(p);
            }
        }
        let worker = Worker::start(paths.clone(), settings.clone(), cc.egui_ctx.clone());
        let has_openrouter_key = secrets::load(KeyKind::OpenRouter).is_some();
        let has_google_key = secrets::load(KeyKind::Google).is_some();
        if has_openrouter_key {
            // 新增轉錄頁的帳戶列要顯示餘額
            worker.send(Cmd::TestKey(KeyKind::OpenRouter));
        }
        let view = View::NewJob;
        let selected = projects.first().map(|p| p.id);
        Self {
            settings_form: SettingsForm::new(&settings),
            new_job: NewJobForm::default(),
            project_ui: ProjectUi::default(),
            paths,
            settings,
            store,
            worker,
            projects,
            selected,
            view,
            busy: HashSet::new(),
            progress: HashMap::new(),
            logs: HashMap::new(),
            toasts: Vec::new(),
            stt_caps: None,
            player: None,
            player_error: None,
            dirty: None,
            list_filter: String::new(),
            confirm_delete: None,
            has_openrouter_key,
            has_google_key,
        }
    }

    pub fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push(Toast {
            text: text.into(),
            error,
            until: Instant::now() + Duration::from_secs(if error { 8 } else { 4 }),
        });
    }

    pub fn project(&self, id: Uuid) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    pub fn upsert(&mut self, p: Project) {
        if let Some(slot) = self.projects.iter_mut().find(|x| x.id == p.id) {
            *slot = p;
        } else {
            self.projects.insert(0, p);
        }
    }

    pub fn mark_dirty(&mut self, id: Uuid) {
        self.dirty = Some((id, Instant::now()));
    }

    pub fn save_now(&mut self) {
        if let Some((id, _)) = self.dirty.take()
            && let Some(p) = self.projects.iter_mut().find(|p| p.id == id)
        {
            p.touch();
            if let Err(e) = self.store.save(p) {
                let msg = format!("存檔失敗：{e}");
                self.toast(msg, true);
            }
        }
    }

    /// 存設定並同步給背景工作。
    pub fn apply_settings(&mut self, s: Settings) {
        let s = s.sanitized();
        if let Err(e) = s.save(&self.paths) {
            self.toast(format!("設定存檔失敗：{e}"), true);
            return;
        }
        self.settings = s.clone();
        self.worker.send(Cmd::UpdateSettings(s));
    }

    pub fn refresh_key_flags(&mut self) {
        self.has_openrouter_key = secrets::load(KeyKind::OpenRouter).is_some();
        self.has_google_key = secrets::load(KeyKind::Google).is_some();
    }

    pub fn player(&mut self) -> Option<&mut AudioPlayer> {
        if self.player.is_none() && self.player_error.is_none() {
            match AudioPlayer::new() {
                Ok(p) => self.player = Some(p),
                Err(e) => self.player_error = Some(e),
            }
        }
        self.player.as_mut()
    }

    /// 播放專案的正規化音訊（或聲音樣本）從某秒開始。
    pub fn play(&mut self, id: Uuid, relative_file: &str, secs: f64) {
        let path = self.store.path_in(id, relative_file);
        let result = match self.player() {
            Some(p) => p.play_from(&path, secs),
            None => Err(self.player_error.clone().unwrap_or_default()),
        };
        if let Err(e) = result {
            self.toast(format!("無法播放：{e}"), true);
        }
    }

    fn drain_worker(&mut self) {
        while let Ok(msg) = self.worker.rx.try_recv() {
            match msg {
                Msg::Pipeline(Event::Progress {
                    id,
                    stage,
                    fraction,
                    message,
                }) => {
                    self.progress.insert(id, (stage, fraction, message));
                }
                Msg::Pipeline(Event::Log { id, level, message }) => {
                    let v = self.logs.entry(id).or_default();
                    v.push(LogLine {
                        level,
                        text: message,
                        at: Local::now(),
                    });
                    if v.len() > 1000 {
                        v.drain(..200);
                    }
                }
                Msg::Pipeline(Event::Updated(p)) | Msg::Finished(p) => {
                    // 使用者正在編輯同一個專案時不要被背景結果蓋掉（只在背景沒在跑時才可能編輯）
                    if self.dirty.map(|d| d.0) != Some(p.id) {
                        self.upsert(*p);
                    }
                }
                Msg::Toast { error, text } => self.toast(text, error),
                Msg::Busy(ids) => self.busy = ids.into_iter().collect(),
                Msg::SttCaps(c) => self.stt_caps = Some(c),
                Msg::KeyTest { kind, result } => self.settings_form.key_test_result(kind, result),
                Msg::OpenRouterModels(r) => self.settings_form.openrouter_models(r),
                Msg::GoogleModels(r) => self.settings_form.google_models(r),
                Msg::Durations(d) => self.new_job.durations(d),
                Msg::Ffmpeg(r) => self.settings_form.ffmpeg_result(r),
            }
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let p = pal(ui);
        ui.add_space(18.0);
        // 標誌
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 18.0, p.accent);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                icon::MIC,
                egui::FontId::new(17.0, theme::icons_family()),
                p.on_accent,
            );
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(theme::bold("MeetScribe", 18.0));
                ui.label(
                    RichText::new("錄音進來，逐字稿與摘要出去。")
                        .size(12.0)
                        .color(p.text_muted),
                );
            });
        });
        ui.add_space(18.0);

        if theme::nav_item(ui, icon::ADD, "新增轉錄", self.view == View::NewJob).clicked() {
            self.save_now();
            self.view = View::NewJob;
        }
        if theme::nav_item(ui, icon::SETTINGS, "設定", self.view == View::Settings).clicked() {
            self.save_now();
            self.settings_form = SettingsForm::new(&self.settings);
            self.view = View::Settings;
        }

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.label(theme::bold("會議紀錄", 13.0).color(p.text_muted));
            if !self.busy.is_empty() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("處理中 {}", self.busy.len()))
                            .size(12.0)
                            .color(p.accent_strong),
                    );
                    ui.spinner();
                });
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.list_filter)
                .hint_text("搜尋會議")
                .desired_width(f32::INFINITY)
                .margin(Margin::symmetric(10, 6)),
        );
        ui.add_space(4.0);

        let bottom_h = 74.0;
        let list_h = (ui.available_height() - bottom_h).max(80.0);
        let filter = self.list_filter.trim().to_lowercase();
        let mut clicked: Option<Uuid> = None;
        let mut delete: Option<Uuid> = None;
        let mut open_dir: Option<Uuid> = None;
        egui::ScrollArea::vertical()
            .max_height(list_h)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.projects.is_empty() {
                    theme::hint(ui, "還沒有會議。把錄音檔拖進視窗就能開始。");
                }
                for proj in &self.projects {
                    if !filter.is_empty() && !proj.title.to_lowercase().contains(&filter) {
                        continue;
                    }
                    let selected = self.selected == Some(proj.id) && self.view == View::Project;
                    let busy = self.busy.contains(&proj.id);
                    let resp = egui::Frame::NONE
                        .inner_margin(Margin::symmetric(12, 9))
                        .corner_radius(theme::RADIUS_SMALL)
                        .fill(if selected {
                            p.accent_soft
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 4.0;
                            let title_color = if selected { p.accent_strong } else { p.text };
                            ui.add(egui::Label::new(theme::bold(&proj.title, 14.0).color(title_color)).truncate());
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                let status = if busy && proj.status != ProjectStatus::Running {
                                    ProjectStatus::Queued
                                } else {
                                    proj.status.clone()
                                };
                                widgets::status_badge(ui, &status);
                                let mut meta = proj.created_at.format("%m/%d %H:%M").to_string();
                                if proj.duration > 0.0 {
                                    meta.push_str(&format!(" · {}", fmt_duration_zh(proj.duration)));
                                }
                                ui.label(RichText::new(meta).size(12.0).color(p.text_muted));
                            });
                            if busy && let Some((stage, f, _)) = self.progress.get(&proj.id) {
                                ui.add(
                                    egui::ProgressBar::new(*f)
                                        .desired_height(5.0)
                                        .fill(p.accent)
                                        .corner_radius(99),
                                );
                                ui.label(RichText::new(stage.label()).size(12.0).color(p.text_muted));
                            }
                        })
                        .response
                        .interact(egui::Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.hovered() && !selected {
                        ui.painter().rect_filled(resp.rect, theme::RADIUS_SMALL, p.accent_faint);
                    }
                    if resp.clicked() {
                        clicked = Some(proj.id);
                    }
                    resp.context_menu(|ui| {
                        if theme::ghost(ui, Some(icon::FOLDER), "開啟專案資料夾", None).clicked() {
                            open_dir = Some(proj.id);
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Button::new(theme::icon_text(ui, Some(icon::DELETE), "刪除…", Some(p.danger)))
                                    .frame(false),
                            )
                            .clicked()
                        {
                            delete = Some(proj.id);
                            ui.close();
                        }
                    });
                }
            });

        // 底部：金鑰狀態與版本
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.spacing_mut().interact_size.y = 18.0;
            ui.add_space(14.0);
            ui.label(
                RichText::new(format!("版本 {}", env!("CARGO_PKG_VERSION")))
                    .size(12.0)
                    .color(p.text_muted),
            );
            widgets::dot_line(
                ui,
                self.has_google_key,
                if self.has_google_key {
                    "Gemini 免費額度已設定"
                } else {
                    "Gemini 未設定（改用免費模型）"
                },
            );
            widgets::dot_line(
                ui,
                self.has_openrouter_key,
                if self.has_openrouter_key {
                    "OpenRouter 已設定"
                } else {
                    "OpenRouter 未設定"
                },
            );
        });

        if let Some(id) = clicked {
            self.save_now();
            self.selected = Some(id);
            self.view = View::Project;
            self.project_ui.reset();
        }
        if let Some(id) = open_dir {
            let _ = open::that(self.store.dir(id));
        }
        if delete.is_some() {
            self.confirm_delete = delete;
        }
    }

    fn delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(id) = self.confirm_delete else { return };
        let title = self.project(id).map(|p| p.title.clone()).unwrap_or_default();
        let mut close = false;
        egui::Window::new("刪除會議")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let p = pal(ui);
                ui.set_width(380.0);
                ui.label(theme::bold("刪除這場會議？", 18.0));
                ui.add_space(4.0);
                ui.label(format!("「{title}」"));
                theme::hint(
                    ui,
                    "會刪除逐字稿、摘要與轉檔後的音訊；原始錄音檔與已匯出的檔案不受影響。",
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let del = ui.add(
                        egui::Button::new(theme::bold("刪除", 14.0).color(egui::Color32::WHITE))
                            .fill(p.danger)
                            .stroke(Stroke::NONE)
                            .min_size(egui::vec2(0.0, 34.0)),
                    );
                    if del.clicked() {
                        if let Some(pl) = self.player.as_mut() {
                            pl.stop();
                        }
                        match self.store.delete(id) {
                            Ok(()) => {
                                self.projects.retain(|p| p.id != id);
                                if self.selected == Some(id) {
                                    self.selected = self.projects.first().map(|p| p.id);
                                    self.view = View::NewJob;
                                }
                            }
                            Err(e) => self.toast(format!("刪除失敗：{e}"), true),
                        }
                        close = true;
                    }
                    if theme::button(ui, None, "取消").clicked() {
                        close = true;
                    }
                });
            });
        if close {
            self.confirm_delete = None;
        }
    }

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        self.toasts.retain(|t| t.until > now);
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::unique("meetscribe-toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, [-20.0, -20.0])
            .show(ctx, |ui| {
                let p = pal(ui);
                for t in &self.toasts {
                    let (glyph, color) = if t.error {
                        (icon::WARNING, p.danger)
                    } else {
                        (icon::CHECK, p.success)
                    };
                    egui::Frame::popup(ui.style())
                        .corner_radius(12)
                        .inner_margin(Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.set_max_width(420.0);
                            ui.horizontal(|ui| {
                                ui.label(theme::ic(glyph, 15.0).color(color));
                                ui.add(egui::Label::new(RichText::new(&t.text).color(p.text)).wrap());
                            });
                        });
                    ui.add_space(8.0);
                }
            });
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

impl eframe::App for MeetApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_worker();
        // 編輯停 1 秒後自動存檔
        if let Some((_, at)) = self.dirty {
            if at.elapsed() > Duration::from_secs(1) {
                self.save_now();
            } else {
                ctx.request_repaint_after(Duration::from_millis(300));
            }
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
            self.save_now();
            self.toast("已存檔", false);
        }
        // 拖放檔案：一律進新增轉錄
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if !dropped.is_empty() {
            self.view = View::NewJob;
            let added = self.new_job.add_files(dropped);
            if !added.is_empty() {
                self.worker.send(Cmd::Probe(added));
            }
        }
        if self.player.as_ref().is_some_and(|p| p.is_playing()) {
            ctx.request_repaint_after(Duration::from_millis(250));
        }

        let p = pal(ui);
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(272.0)
            .min_size(220.0)
            .frame(egui::Frame::NONE.fill(p.surface).inner_margin(Margin {
                left: 14,
                right: 14,
                top: 0,
                bottom: 0,
            }))
            .show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.page).inner_margin(Margin {
                left: 32,
                right: 32,
                top: 26,
                bottom: 16,
            }))
            .show(ui, |ui| match self.view {
                View::NewJob => crate::views::new_job::show(self, ui),
                View::Settings => crate::views::settings::show(self, ui),
                View::Project => match self.selected {
                    Some(id) if self.project(id).is_some() => crate::views::project::show(self, ui, id),
                    _ => crate::views::new_job::show(self, ui),
                },
            });
        self.delete_dialog(&ctx);
        self.toasts_ui(&ctx);
    }

    fn on_exit(&mut self) {
        self.save_now();
    }
}
