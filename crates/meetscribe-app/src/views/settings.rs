//! 設定：兩把 API Key（存在 Windows 認證管理員）、各工作的模型與供應商、輸出、處理參數、進階。

use crate::app::MeetApp;
use crate::theme::{self, icon, pal};
use crate::worker::{Cmd, TransferCmd, UpdateStatus, Worker};
use egui::{RichText, Ui};
use meetscribe_core::claude_code;
use meetscribe_core::config::{LlmProvider, Settings};
use meetscribe_core::openrouter::ModelInfo;
use meetscribe_core::prompts::Prompts;
use meetscribe_core::secrets::{self, KeyKind, KeySource};

pub struct SettingsForm {
    pub draft: Settings,
    pub(crate) or_key: String,
    pub(crate) g_key: String,
    pub(crate) or_status: Option<Result<String, String>>,
    pub(crate) g_status: Option<Result<String, String>>,
    or_models: Option<Result<Vec<ModelInfo>, String>>,
    g_models: Option<Result<Vec<String>, String>>,
    ffmpeg: Option<Result<String, String>>,
    model_filter: String,
    /// 在設定頁按了 Ctrl+S（app.rs 收到鍵盤事件，這頁畫的時候存）。
    pub(crate) save_requested: bool,
}

impl SettingsForm {
    pub fn new(s: &Settings) -> Self {
        Self {
            draft: s.clone(),
            or_key: String::new(),
            g_key: String::new(),
            or_status: None,
            g_status: None,
            or_models: None,
            g_models: None,
            ffmpeg: None,
            model_filter: String::new(),
            save_requested: false,
        }
    }
    pub fn key_test_result(&mut self, kind: KeyKind, r: Result<String, String>) {
        match kind {
            KeyKind::OpenRouter => self.or_status = Some(r),
            KeyKind::Google => self.g_status = Some(r),
        }
    }
    pub fn openrouter_models(&mut self, r: Result<Vec<ModelInfo>, String>) {
        self.or_models = Some(r);
    }
    pub fn google_models(&mut self, r: Result<Vec<String>, String>) {
        self.g_models = Some(r);
    }
    pub fn ffmpeg_result(&mut self, r: Result<String, String>) {
        self.ffmpeg = Some(r);
    }
}

pub(crate) fn status_line(ui: &mut Ui, s: &Option<Result<String, String>>) {
    let p = pal(ui);
    match s {
        Some(Ok(t)) => {
            ui.horizontal(|ui| {
                ui.label(theme::ic(icon::CHECK, 13.0).color(p.success));
                ui.add(egui::Label::new(RichText::new(t).color(p.success)).wrap());
            });
        }
        Some(Err(e)) => {
            ui.horizontal(|ui| {
                ui.label(theme::ic(icon::WARNING, 13.0).color(p.danger));
                ui.add(egui::Label::new(RichText::new(e).color(p.danger)).wrap());
            });
        }
        None => {}
    }
}

/// 一把金鑰的輸入區。回傳是否存了或刪了（要更新側欄狀態）。
pub(crate) fn key_box(
    ui: &mut Ui,
    worker: &Worker,
    kind: KeyKind,
    input: &mut String,
    status: &mut Option<Result<String, String>>,
) -> bool {
    let p = pal(ui);
    let mut changed = false;
    ui.horizontal(|ui| {
        theme::label(ui, "目前");
        match secrets::load(kind) {
            Some((k, src)) => {
                let from = match src {
                    KeySource::CredentialStore => "Windows 認證管理員",
                    KeySource::Environment => "環境變數",
                };
                ui.label(RichText::new(secrets::mask(&k)).monospace());
                theme::badge(ui, from, p.success);
            }
            None => {
                theme::badge(ui, "尚未設定", p.danger);
            }
        }
    });
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::TextEdit::singleline(input)
                .password(true)
                .hint_text("貼上新的金鑰")
                .desired_width(360.0),
        );
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::PasswordInput, true, kind.label()));
        if theme::primary(ui, Some(icon::CHECK), "儲存", !input.trim().is_empty()).clicked() {
            match secrets::save(kind, input) {
                Ok(()) => {
                    input.clear();
                    *status = Some(Ok("已儲存，測試中…".into()));
                    worker.send(Cmd::TestKey(kind));
                    changed = true;
                }
                Err(e) => *status = Some(Err(e.to_string())),
            }
        }
        if theme::button(ui, Some(icon::REFRESH), "測試").clicked() {
            *status = Some(Ok("測試中…".into()));
            worker.send(Cmd::TestKey(kind));
        }
        if theme::ghost(ui, Some(icon::DELETE), "刪除", Some(p.danger)).clicked() {
            match secrets::delete(kind) {
                Ok(()) => {
                    *status = Some(Ok("已從認證管理員刪除".into()));
                    changed = true;
                }
                Err(e) => *status = Some(Err(e.to_string())),
            }
        }
    });
    status_line(ui, status);
    changed
}

/// 供應商單選列。
fn provider_row(ui: &mut Ui, current: &mut LlmProvider, options: &[LlmProvider]) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        for p in options {
            if ui.radio(*current == *p, p.label()).clicked() && *current != *p {
                *current = *p;
                changed = true;
            }
        }
    });
    changed
}

pub fn show(app: &mut MeetApp, ui: &mut Ui) {
    let mut keys_changed = false;
    let mut save = false;
    let mut reset = false;
    let mut update_action = None;
    let mut rename_device: Option<String> = None;
    // 「儲存設定」固定在頁面底部，不用捲到最下面（2026-10-31 使用者：「設定無法快速到最下面儲存設定很困擾」）
    const FOOTER: f32 = 64.0;
    let scroll_h = (ui.available_height() - FOOTER).max(120.0);
    egui::ScrollArea::vertical().max_height(scroll_h).auto_shrink([false, false]).show(ui, |ui| {
        ui.set_max_width(900.0);
        let p = pal(ui);
        theme::page_header(ui, "設定", "金鑰存在 Windows 認證管理員，不會寫進任何設定檔。");
        let f = &mut app.settings_form;

        theme::section(ui, "OpenRouter API Key　逐字稿（付費）", |ui| {
            theme::hint(ui, "用來呼叫 NVIDIA Nemotron 3.5 Transcribe 產生逐字稿，約 US$0.12／小時音訊。");
            keys_changed |= key_box(ui, &app.worker, KeyKind::OpenRouter, &mut f.or_key, &mut f.or_status);
            ui.horizontal(|ui| {
                theme::link_button(ui, icon::MONEY, "OpenRouter 儲值", theme::links::OPENROUTER_CREDITS);
                theme::link_button(ui, icon::KEY, "OpenRouter API Keys", theme::links::OPENROUTER_KEYS);
            });
        });

        theme::section(ui, "Google AI Studio API Key　說話者與摘要（免費）", |ui| {
            theme::hint(ui, "說話者辨識與會議摘要預設用 Gemini 免費額度；額度用完會自動重試或改天繼續。沒設定時改用 OpenRouter 上的免費模型。");
            keys_changed |= key_box(ui, &app.worker, KeyKind::Google, &mut f.g_key, &mut f.g_status);
            theme::link_button(ui, icon::KEY, "到 Google AI Studio 免費取得金鑰", theme::links::GOOGLE_AI_STUDIO_KEYS);
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(theme::ic(icon::INFO, 13.0).color(p.warning));
                ui.label(
                    RichText::new("Google 免費方案的輸入可能被用來改善其產品；機密會議請把摘要切成 Claude Code CLI。")
                        .size(12.5)
                        .color(p.warning),
                );
            });
        });

        theme::section(ui, "模型", |ui| {
            let d = &mut f.draft;
            let l = theme::label(ui, "逐字稿（OpenRouter 語音轉文字模型）");
            ui.add(egui::TextEdit::singleline(&mut d.stt_model).desired_width(380.0)).labelled_by(l.id);
            ui.add_space(8.0);

            theme::label(ui, "說話者辨識（要能聽音訊）");
            if provider_row(ui, &mut d.attribution_provider, &[LlmProvider::Google, LlmProvider::OpenRouter]) {
                d.attribution_model = d.attribution_provider.default_attribution_model().into();
            }
            let r = ui.add(egui::TextEdit::singleline(&mut d.attribution_model).desired_width(380.0));
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "說話者辨識模型"));
            theme::hint(ui, "Gemini 模型名稱不存在時，程式會自動改用目前最新的 Flash。");
            ui.add_space(8.0);

            theme::label(ui, "摘要");
            if provider_row(ui, &mut d.summary_provider, &[LlmProvider::Google, LlmProvider::OpenRouter, LlmProvider::ClaudeCode]) {
                d.summary_model = d.summary_provider.default_summary_model().into();
            }
            let r = ui.add(egui::TextEdit::singleline(&mut d.summary_model).desired_width(380.0));
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "摘要模型"));
            match d.summary_provider {
                LlmProvider::ClaudeCode => {
                    theme::hint(ui, "填 sonnet、opus、haiku 或完整模型 ID；用你電腦上已登入的 Claude Code。");
                    match claude_code::locate(&d.claude_cli_path) {
                        Some(path) => ui.label(RichText::new(format!("找到：{}", path.display())).size(12.5).color(p.success)),
                        None => ui.label(RichText::new("找不到 claude；請安裝 Claude Code 或在「進階」指定路徑").size(12.5).color(p.danger)),
                    };
                }
                LlmProvider::Google => theme::hint(ui, "預設：Gemini 免費額度（沒 Google Key 時自動用 OpenRouter 免費模型）。"),
                LlmProvider::OpenRouter => theme::hint(ui, "OpenRouter 模型 ID；名稱結尾是 :free 的不收費。"),
            }
            ui.add_space(8.0);

            let l = theme::label(ui, "摘要語言");
            egui::ComboBox::from_id_salt("out_lang")
                .selected_text(d.output_language_label())
                .width(220.0)
                .show_ui(ui, |ui| {
                    for (code, name) in [
                        ("zh-TW", "繁體中文（台灣用語）"),
                        ("auto", "跟會議語言相同"),
                        ("zh-CN", "简体中文"),
                        ("en", "English"),
                        ("ja", "日本語"),
                    ] {
                        ui.selectable_value(&mut d.output_language, code.to_string(), name);
                    }
                })
                .response
                .labelled_by(l.id);

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if theme::button(ui, Some(icon::LIST), "載入 OpenRouter 模型清單").clicked() {
                    f.or_models = None;
                    app.worker.send(Cmd::LoadOpenRouterModels);
                }
                if theme::button(ui, Some(icon::LIST), "載入 Gemini 模型清單").clicked() {
                    f.g_models = None;
                    app.worker.send(Cmd::LoadGoogleModels);
                }
                ui.add(egui::TextEdit::singleline(&mut f.model_filter).hint_text("篩選").desired_width(160.0));
            });
            let filter = f.model_filter.trim().to_lowercase();
            if let Some(r) = &f.or_models {
                match r {
                    Ok(list) => {
                        egui::CollapsingHeader::new(format!("OpenRouter 模型（{}）", list.len())).default_open(true).show(ui, |ui| {
                            egui::ScrollArea::vertical().id_salt("or_list").max_height(240.0).show(ui, |ui| {
                                for m in list.iter().filter(|m| filter.is_empty() || m.id.to_lowercase().contains(&filter)) {
                                    ui.horizontal(|ui| {
                                        let free = m.id.ends_with(":free") || (m.prompt_price == 0.0 && m.completion_price == 0.0);
                                        ui.label(RichText::new(&m.id).monospace().size(12.5));
                                        if free {
                                            theme::badge(ui, "免費", p.success);
                                        }
                                        if m.accepts_audio() {
                                            theme::badge(ui, "可聽音訊", p.accent_strong);
                                        }
                                        if m.is_transcription() {
                                            theme::badge(ui, "語音轉文字", p.warning);
                                        }
                                        if m.is_transcription() && ui.small_button("用於逐字稿").clicked() {
                                            f.draft.stt_model = m.id.clone();
                                        }
                                        if m.accepts_audio() && !m.is_transcription() && ui.small_button("用於說話者").clicked() {
                                            f.draft.attribution_provider = LlmProvider::OpenRouter;
                                            f.draft.attribution_model = m.id.clone();
                                        }
                                        if m.outputs_text() && !m.is_transcription() && ui.small_button("用於摘要").clicked() {
                                            f.draft.summary_provider = LlmProvider::OpenRouter;
                                            f.draft.summary_model = m.id.clone();
                                        }
                                    });
                                }
                            });
                        });
                    }
                    Err(e) => {
                        ui.label(RichText::new(e).color(p.danger));
                    }
                }
            }
            if let Some(r) = &f.g_models {
                match r {
                    Ok(list) => {
                        egui::CollapsingHeader::new(format!("Gemini 模型（{}）", list.len())).default_open(true).show(ui, |ui| {
                            egui::ScrollArea::vertical().id_salt("g_list").max_height(220.0).show(ui, |ui| {
                                for m in list.iter().filter(|m| filter.is_empty() || m.to_lowercase().contains(&filter)) {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(m).monospace().size(12.5));
                                        if ui.small_button("用於說話者").clicked() {
                                            f.draft.attribution_provider = LlmProvider::Google;
                                            f.draft.attribution_model = m.clone();
                                        }
                                        if ui.small_button("用於摘要").clicked() {
                                            f.draft.summary_provider = LlmProvider::Google;
                                            f.draft.summary_model = m.clone();
                                        }
                                    });
                                }
                            });
                        });
                    }
                    Err(e) => {
                        ui.label(RichText::new(e).color(p.danger));
                    }
                }
            }
        });

        theme::section(ui, "輸出（預設值）", |ui| {
            let d = &mut f.draft;
            ui.checkbox(&mut d.export.auto, "處理完成後自動匯出");
            theme::hint(ui, "新增轉錄時可以只改那一次；之後也能在每場會議的「匯出」頁用其他格式再匯出。");
            ui.add_space(6.0);
            crate::widgets::export_options(ui, &mut d.export, "settings_export", false, None);
        });

        theme::section(ui, "處理", |ui| {
            let d = &mut f.draft;
            ui.add(egui::Slider::new(&mut d.chunk_minutes, 5..=55).text("每段分鐘數（長錄音依停頓切段）"));
            ui.add(egui::Slider::new(&mut d.stt_concurrency, 1..=8).text("同時轉錄段數"));
            ui.add(egui::Slider::new(&mut d.max_retries, 0..=8).text("失敗自動重試次數"));
            ui.add(egui::Slider::new(&mut d.reference_clip_limit, 0..=15).text("跨段比對時附幾位說話者的聲音樣本"));
            ui.checkbox(&mut d.deny_data_collection, "OpenRouter 只路由到不保留、不拿資料訓練的供應商（可能使部分模型無法使用）");
        });

        theme::section(ui, "傳到其他裝置", |ui| {
            rename_device = crate::views::transfer::devices_section(ui, &mut app.transfer, &app.worker);
        });

        theme::section(ui, "關於與更新", |ui| {
            about_and_update(ui, app_update_view(&app.update, &app.device_id), &mut f.draft.auto_update, &mut update_action);
        });

        theme::section(ui, "進階", |ui| {
            let d = &mut f.draft;
            let l = theme::label(ui, "ffmpeg 位置（空白＝自動尋找）");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut d.ffmpeg_path).desired_width(380.0)).labelled_by(l.id);
                if theme::button(ui, Some(icon::REFRESH), "檢查").clicked() {
                    f.ffmpeg = None;
                    app.worker.send(Cmd::CheckFfmpeg(d.ffmpeg_path.clone()));
                }
            });
            status_line(ui, &f.ffmpeg);
            ui.add_space(6.0);
            let l = theme::label(ui, "claude.exe 位置（空白＝自動尋找）");
            ui.add(egui::TextEdit::singleline(&mut d.claude_cli_path).desired_width(380.0)).labelled_by(l.id);
            ui.add_space(6.0);
            ui.add(egui::Slider::new(&mut d.ui_scale, 0.8..=1.6).step_by(0.05).text("介面縮放"));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if theme::button(ui, Some(icon::FOLDER), "開啟程式資料夾").clicked() {
                    let _ = open::that(&app.paths.root);
                }
                if theme::button(ui, Some(icon::EDIT), "自訂提示詞…").on_hover_text("開啟提示詞資料夾，裡面有範本與說明").clicked() {
                    let dir = app.paths.prompts_dir();
                    let _ = Prompts::write_examples(&dir);
                    let _ = open::that(dir);
                }
            });
        });

        ui.add_space(24.0);
    });
    let dirty = app.settings_form.draft != app.settings;
    let p = pal(ui);
    egui::Frame::NONE
        .stroke(egui::Stroke::new(1.0, p.border))
        .fill(p.surface)
        .corner_radius(theme::RADIUS_SMALL)
        .inner_margin(egui::Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.set_max_width(900.0 - 32.0);
            ui.set_width(ui.available_width().min(900.0 - 32.0));
            ui.horizontal(|ui| {
                if dirty {
                    ui.label(theme::ic(icon::EDIT, 14.0).color(p.accent_strong));
                    ui.label(RichText::new("有未儲存的變更").color(p.accent_strong));
                } else {
                    ui.label(RichText::new("設定都已儲存　·　Ctrl+S 也能儲存").color(p.text_muted));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::primary(ui, Some(icon::CHECK), "儲存設定", dirty).clicked() {
                        save = true;
                    }
                    if theme::button(ui, None, "還原預設值").on_hover_text("不含金鑰、這台的名稱與中轉站；按「儲存設定」才生效").clicked() {
                        reset = true;
                    }
                });
            });
        });
    save |= std::mem::take(&mut app.settings_form.save_requested);
    if keys_changed && !app.transfer.peers.is_empty() {
        // 配對後自動同步金鑰（2026-10-31 使用者：「配對後就同步喔！」）
        app.worker.send(Cmd::Transfer(TransferCmd::SyncKeys { manual: false }));
    }
    if keys_changed {
        app.refresh_key_flags();
    }
    match update_action {
        Some(UpdateAction::Check) => app.worker.send(Cmd::CheckUpdate { manual: true }),
        Some(UpdateAction::InstallNow) => app.install_update_now(ui.ctx()),
        Some(UpdateAction::CopyId) => {
            ui.ctx().copy_text(format!("語音轉錄 裝置識別碼 {}（電腦版 {}）", app.device_id, crate::worker::VERSION));
            app.toast("識別碼已複製", false);
        }
        None => {}
    }
    if reset {
        // 這台的名稱與中轉站設定不算「預設值」的一部分（名稱在「傳到其他裝置」另外存）
        let keep = &app.settings_form.draft;
        app.settings_form.draft = Settings { device_name: keep.device_name.clone(), relay_url: keep.relay_url.clone(), ..Settings::default() };
    }
    if let Some(name) = rename_device {
        let mut s = app.settings.clone();
        s.device_name = name;
        app.apply_settings(s);
        app.settings_form.draft.device_name = app.settings.device_name.clone();
        app.worker.send(Cmd::Transfer(TransferCmd::Renamed));
        app.toast("名稱已儲存", false);
    }
    if save {
        let s = app.settings_form.draft.clone();
        let scale_changed = (s.ui_scale - app.settings.ui_scale).abs() > f32::EPSILON;
        app.apply_settings(s);
        app.settings_form.draft = app.settings.clone();
        if scale_changed {
            ui.ctx().set_zoom_factor(app.settings.ui_scale);
        }
        app.toast("設定已儲存", false);
    }
}

pub(crate) enum UpdateAction {
    Check,
    InstallNow,
    CopyId,
}

pub(crate) struct UpdateView<'a> {
    status: &'a UpdateStatus,
    device_id: &'a str,
}

fn app_update_view<'a>(status: &'a UpdateStatus, device_id: &'a str) -> UpdateView<'a> {
    UpdateView { status, device_id }
}

/// 版本、本機識別碼（發佈者用它單獨推送）、自動更新開關、檢查更新與目前狀態。
fn about_and_update(ui: &mut Ui, v: UpdateView<'_>, auto: &mut bool, action: &mut Option<UpdateAction>) {
    let p = pal(ui);
    ui.horizontal(|ui| {
        ui.label(theme::bold(format!("語音轉錄 {}", crate::worker::VERSION), 15.0));
        ui.label(RichText::new("電腦版").color(p.text_muted));
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("本機識別碼");
        ui.label(RichText::new(v.device_id).monospace().strong());
        if theme::button(ui, Some(icon::COPY), "複製")
            .on_hover_text("要單獨推送更新給這台時，把識別碼傳給發佈者")
            .clicked()
        {
            *action = Some(UpdateAction::CopyId);
        }
    });
    ui.add_space(6.0);
    ui.checkbox(auto, "自動更新（背景下載新版，關閉程式時安裝）");
    if let Some(why) = crate::updater::self_install_blocker() {
        theme::hint(ui, why);
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let checking = matches!(v.status, UpdateStatus::Checking | UpdateStatus::Downloading { .. });
        if ui
            .add_enabled_ui(!checking, |ui| theme::button(ui, Some(icon::REFRESH), "檢查更新"))
            .inner
            .clicked()
        {
            *action = Some(UpdateAction::Check);
        }
        match v.status {
            UpdateStatus::Idle => {}
            UpdateStatus::Checking => {
                ui.spinner();
                ui.label("檢查中…");
            }
            UpdateStatus::Downloading { version, done, total } => {
                ui.spinner();
                let pct = if *total > 0 { *done as f64 * 100.0 / *total as f64 } else { 0.0 };
                ui.label(format!("下載 {version} 中… {pct:.0}%"));
            }
            UpdateStatus::Latest => {
                ui.label(RichText::new(format!("目前已是最新版：{}", crate::worker::VERSION)).color(p.success));
            }
            UpdateStatus::Ready(r) => {
                ui.label(RichText::new(format!("新版 {} 已下載好", r.version)).color(p.accent_strong));
                if theme::primary(ui, Some(icon::DOWNLOAD), "立即更新並重新開啟", true).clicked() {
                    *action = Some(UpdateAction::InstallNow);
                }
            }
            UpdateStatus::Failed(e) => {
                ui.label(RichText::new(format!("檢查失敗：{e}")).color(p.danger));
            }
        }
    });
    if let UpdateStatus::Ready(r) = v.status
        && !r.notes.trim().is_empty()
    {
        theme::hint(ui, &format!("這版的改變：{}", r.notes.trim()));
    }
}
