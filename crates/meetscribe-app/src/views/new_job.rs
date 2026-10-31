//! 新增轉錄：拖放區、檔案清單、選項、輸出、費用預估、開始（版型參考 Filee 首頁）。

use crate::app::{MeetApp, View};
use crate::theme::{self, icon, pal};
use crate::widgets;
use crate::worker::Cmd;
use egui::{Margin, RichText, Stroke};
use meetscribe_core::model::{JobOptions, Project};
use meetscribe_core::secrets::KeyKind;
use meetscribe_core::textutil::fmt_duration_zh;
use std::collections::HashMap;
use std::path::PathBuf;

pub const AUDIO_EXTS: &[&str] = &[
    "mp3", "wav", "m4a", "aac", "flac", "ogg", "opus", "wma", "amr", "webm", "mp4", "mkv", "mov", "avi", "m4v", "3gp",
];

/// (代碼, 顯示名稱)；空字串＝自動判斷。
const LANGUAGES: &[(&str, &str)] = &[
    ("", "自動判斷"),
    ("zh", "中文"),
    ("en", "English"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("es", "Español"),
    ("pt", "Português"),
    ("it", "Italiano"),
    ("ru", "Русский"),
    ("th", "ไทย"),
    ("vi", "Tiếng Việt"),
    ("id", "Bahasa Indonesia"),
    ("hi", "हिन्दी"),
    ("ar", "العربية"),
];

pub struct NewJobForm {
    pub files: Vec<PathBuf>,
    pub durations: HashMap<PathBuf, Result<f64, String>>,
    pub language: usize,
    pub identify_speakers: bool,
    pub expected_speakers: String,
    pub participants: String,
    pub keyterms: String,
    pub generate_summary: bool,
    pub summary_instructions: String,
}

impl Default for NewJobForm {
    fn default() -> Self {
        Self {
            files: Vec::new(),
            durations: HashMap::new(),
            language: 0,
            identify_speakers: true,
            expected_speakers: String::new(),
            participants: String::new(),
            keyterms: String::new(),
            generate_summary: true,
            summary_instructions: String::new(),
        }
    }
}

impl NewJobForm {
    /// 加入檔案（略過重複與不支援的格式），回傳真的加進來的。
    pub fn add_files(&mut self, files: Vec<PathBuf>) -> Vec<PathBuf> {
        let mut added = Vec::new();
        for f in files {
            let ok = f
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()));
            if ok && f.is_file() && !self.files.contains(&f) {
                self.files.push(f.clone());
                added.push(f);
            }
        }
        added
    }

    pub fn durations(&mut self, d: Vec<(PathBuf, Result<f64, String>)>) {
        self.durations.extend(d);
    }

    fn total_seconds(&self) -> f64 {
        self.files
            .iter()
            .filter_map(|f| self.durations.get(f)?.as_ref().ok())
            .sum()
    }

    fn options(&self) -> JobOptions {
        let lines = |s: &str| -> Vec<String> {
            s.split(['\n', ',', '，', '、', ';', '；'])
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        };
        JobOptions {
            language: Some(LANGUAGES[self.language].0.to_string()).filter(|l| !l.is_empty()),
            identify_speakers: self.identify_speakers,
            expected_speakers: self.expected_speakers.trim().parse().ok().filter(|n: &u32| *n > 0),
            participants: self
                .participants
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect(),
            keyterms: lines(&self.keyterms),
            generate_summary: self.generate_summary,
            summary_instructions: self.summary_instructions.trim().to_string(),
            export: None,
        }
    }
}

fn pick_files(app: &mut MeetApp) {
    if let Some(files) = rfd::FileDialog::new().add_filter("音訊／影片", AUDIO_EXTS).pick_files() {
        let added = app.new_job.add_files(files);
        if !added.is_empty() {
            app.worker.send(Cmd::Probe(added));
        }
    }
}

/// 還沒有 OpenRouter Key：三步驟引導（儲值 → 建立 Key → 貼上），外加選用的 Gemini 免費 Key。
fn setup_card(app: &mut MeetApp, ui: &mut egui::Ui) {
    let p = pal(ui);
    let hours_per_5usd = app
        .stt_caps
        .as_ref()
        .and_then(|c| c.price_per_second)
        .filter(|x| *x > 0.0)
        .map(|x| 5.0 / (x * 3600.0));
    let mut changed = false;
    theme::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(theme::ic(icon::KEY, 18.0).color(p.accent_strong));
            ui.label(theme::bold("開始前：設定 OpenRouter（逐字稿用，約 3 分鐘）", 16.0));
        });
        theme::hint(
            ui,
            "逐字稿用 NVIDIA Nemotron 3.5 Transcribe，在 OpenRouter 依音訊長度計費；說話者辨識與摘要用免費模型。",
        );
        ui.add_space(8.0);

        step(ui, 1, "儲值", |ui| {
            let price = match hours_per_5usd {
                Some(h) => format!("約 US$0.12／小時音訊，儲值 US$5 可轉約 {h:.0} 小時。"),
                None => "約 US$0.12／小時音訊。".into(),
            };
            ui.label(RichText::new(price).color(p.text_muted));
            theme::link_button(
                ui,
                icon::MONEY,
                "開啟 OpenRouter 儲值頁",
                theme::links::OPENROUTER_CREDITS,
            );
        });
        step(ui, 2, "建立 API Key", |ui| {
            ui.label(
                RichText::new("按「Create Key」，名稱隨意（例如「語音轉錄」），建立後複製那串 sk-or-… 金鑰。")
                    .color(p.text_muted),
            );
            theme::link_button(
                ui,
                icon::KEY,
                "開啟 OpenRouter API Keys 頁",
                theme::links::OPENROUTER_KEYS,
            );
        });
        step(ui, 3, "貼上金鑰", |ui| {
            let f = &mut app.settings_form;
            changed |=
                crate::views::settings::key_box(ui, &app.worker, KeyKind::OpenRouter, &mut f.or_key, &mut f.or_status);
        });

        ui.add_space(4.0);
        egui::CollapsingHeader::new(RichText::new("選用：Gemini 免費金鑰（說話者辨識與摘要更準）").color(p.text_muted))
            .default_open(false)
            .show(ui, |ui| {
                theme::hint(
                    ui,
                    "沒設定也能用（會改用 OpenRouter 上的免費模型）。Google 免費方案的輸入可能被用來改善其產品。",
                );
                theme::link_button(
                    ui,
                    icon::KEY,
                    "到 Google AI Studio 免費取得金鑰",
                    theme::links::GOOGLE_AI_STUDIO_KEYS,
                );
                let f = &mut app.settings_form;
                changed |=
                    crate::views::settings::key_box(ui, &app.worker, KeyKind::Google, &mut f.g_key, &mut f.g_status);
            });
    });
    if changed && !app.transfer.peers.is_empty() {
        // 配對後自動同步金鑰
        app.worker.send(Cmd::Transfer(crate::worker::TransferCmd::SyncKeys { manual: false }));
    }
    if changed {
        app.refresh_key_flags();
    }
}

/// 已有 OpenRouter Key：一列帳戶狀態（餘額）＋儲值、API Keys 的快速入口。
fn account_bar(app: &mut MeetApp, ui: &mut egui::Ui) {
    let p = pal(ui);
    let mut test = false;
    egui::Frame::NONE
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(theme::RADIUS)
        .inner_margin(Margin::symmetric(18, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(theme::ic(icon::CHECK, 14.0).color(p.success));
                ui.label(theme::bold("OpenRouter", 14.0));
                let status = match &app.settings_form.or_status {
                    Some(Ok(t)) => RichText::new(t).color(p.text_muted),
                    Some(Err(e)) => RichText::new(e).color(p.danger),
                    None => RichText::new("已設定").color(p.text_muted),
                };
                ui.add(egui::Label::new(status.size(12.5)).truncate());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::link_button(ui, icon::KEY, "API Keys", theme::links::OPENROUTER_KEYS);
                    theme::link_button(ui, icon::MONEY, "儲值", theme::links::OPENROUTER_CREDITS);
                    if theme::ghost(ui, Some(icon::REFRESH), "", Some(p.text_muted))
                        .on_hover_text("重新查詢餘額")
                        .clicked()
                    {
                        test = true;
                    }
                    if !app.has_google_key
                        && theme::ghost(ui, Some(icon::INFO), "Gemini 免費金鑰", Some(p.accent_strong))
                            .on_hover_text("設定後說話者辨識與摘要改用 Gemini 免費額度")
                            .clicked()
                    {
                        app.settings_form = crate::views::settings::SettingsForm::new(&app.settings);
                        app.view = View::Settings;
                    }
                });
            });
        });
    ui.add_space(14.0);
    if test {
        app.settings_form.or_status = Some(Ok("查詢中…".into()));
        app.worker.send(Cmd::TestKey(KeyKind::OpenRouter));
    }
}

/// 編號步驟：圓形數字＋標題，內容縮排在右邊。
fn step(ui: &mut egui::Ui, n: u32, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    let p = pal(ui);
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 14.0, p.accent_soft);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            n.to_string(),
            egui::FontId::new(14.0, theme::bold_family()),
            p.accent_strong,
        );
        ui.vertical(|ui| {
            ui.label(theme::bold(title, 15.0));
            add(ui);
        });
    });
    ui.add_space(10.0);
}

pub fn show(app: &mut MeetApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.set_max_width(900.0);
        let p = pal(ui);
        theme::page_header(ui, "新增轉錄", "會議錄音 → 完整逐字稿（標出誰說的）＋ 會議摘要");

        if app.has_openrouter_key {
            account_bar(app, ui);
        } else {
            setup_card(app, ui);
        }

        // 拖放區（Filee 首頁的大卡片）
        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        let drop = egui::Frame::NONE
            .fill(if hovering { p.accent_soft } else { p.accent_faint })
            .stroke(Stroke::new(
                if hovering { 2.0 } else { 1.5 },
                p.accent.gamma_multiply(if hovering { 0.9 } else { 0.35 }),
            ))
            .corner_radius(22)
            .inner_margin(Margin::symmetric(24, 30))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    theme::bubble_icon(ui, icon::UPLOAD);
                    ui.add_space(8.0);
                    ui.label(theme::bold("把錄音或影片檔拖到這裡", 17.0));
                    ui.label(
                        RichText::new(
                            "MP3、WAV、M4A、FLAC、OGG、MP4、MKV、MOV… 長度不限，超過 10 分鐘會在停頓處自動分段",
                        )
                        .color(p.text_muted),
                    );
                    ui.add_space(10.0);
                    if theme::button(ui, Some(icon::FOLDER), "選擇檔案…").clicked() {
                        pick_files(app);
                    }
                });
            })
            .response;
        if drop.interact(egui::Sense::click()).double_clicked() {
            pick_files(app);
        }
        ui.add_space(16.0);

        if !app.new_job.files.is_empty() {
            theme::section(
                ui,
                &format!("要處理的檔案（{}）", app.new_job.files.len()),
                |ui| {
                    let mut remove = None;
                    for (i, f) in app.new_job.files.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(theme::ic(icon::MIC, 15.0).color(p.accent_strong));
                            ui.label(
                                f.file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                            );
                            match app.new_job.durations.get(f) {
                                Some(Ok(d)) => {
                                    theme::badge(ui, &fmt_duration_zh(*d), p.text_muted);
                                }
                                Some(Err(e)) => {
                                    ui.label(RichText::new(format!("無法讀取：{e}")).color(p.danger));
                                }
                                None => {
                                    ui.spinner();
                                }
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if theme::ghost(ui, Some(icon::CANCEL), "", Some(p.text_muted))
                                    .on_hover_text("移除")
                                    .clicked()
                                {
                                    remove = Some(i);
                                }
                            });
                        });
                    }
                    if let Some(i) = remove {
                        let f = app.new_job.files.remove(i);
                        app.new_job.durations.remove(&f);
                    }
                },
            );
        }

        theme::section(ui, "選項", |ui| {
            ui.columns(2, |cols| {
                let lang_label = theme::label(&mut cols[0], "語言");
                egui::ComboBox::from_id_salt("lang")
                    .selected_text(LANGUAGES[app.new_job.language].1)
                    .width(220.0)
                    .show_ui(&mut cols[0], |ui| {
                        for (i, (_, name)) in LANGUAGES.iter().enumerate() {
                            ui.selectable_value(&mut app.new_job.language, i, *name);
                        }
                    })
                    .response
                    .labelled_by(lang_label.id);
                if app.new_job.identify_speakers {
                    let l = theme::label(&mut cols[1], "預估發言人數（選填）");
                    cols[1]
                        .add(
                            egui::TextEdit::singleline(&mut app.new_job.expected_speakers)
                                .hint_text("不確定就留空")
                                .desired_width(220.0),
                        )
                        .labelled_by(l.id);
                }
            });
            ui.add_space(6.0);
            ui.checkbox(
                &mut app.new_job.identify_speakers,
                "辨識說話者：標出每一句是誰說的（2～1000 人皆可）",
            );
            theme::hint(
                ui,
                "由 Gemini 聽音訊判斷（免費額度）。對話裡聽得出姓名時會自動命名，之後可在「說話者」頁修改、合併。",
            );
            if app.new_job.identify_speakers {
                ui.add_space(6.0);
                let l = theme::label(ui, "與會者名單（選填，一行一位，可加職稱）");
                ui.add(
                    egui::TextEdit::multiline(&mut app.new_job.participants)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .hint_text("王小明 主持人\n陳經理 業務部"),
                )
                .labelled_by(l.id);
                theme::hint(ui, "有名單時，模型比較容易把「說話者 1」對上真實姓名。");
            }
            ui.add_space(6.0);
            let l = theme::label(ui, "專有名詞（選填）");
            ui.add(
                egui::TextEdit::multiline(&mut app.new_job.keyterms)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .hint_text("人名、產品代號、術語，以逗號或換行分隔"),
            )
            .labelled_by(l.id);
        });

        theme::section(ui, "會議摘要", |ui| {
            ui.checkbox(
                &mut app.new_job.generate_summary,
                "完成後自動產生摘要（格式依會議內容決定）",
            );
            if app.new_job.generate_summary {
                ui.add_space(4.0);
                theme::label(ui, "摘要方式");
                let mut s = app.settings.clone();
                if widgets::summary_mode_picker(ui, &mut s, "job_summary_mode") {
                    app.apply_settings(s);
                }
                ui.add_space(4.0);
                let l = theme::label(ui, "額外要求（選填）");
                ui.add(
                    egui::TextEdit::multiline(&mut app.new_job.summary_instructions)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text("例如「著重技術決策」「列出每位主管的意見」"),
                )
                .labelled_by(l.id);
            }
        });

        theme::section(ui, "輸出", |ui| {
            // 跟摘要方式一樣，改了就記成下次的預設
            let mut s = app.settings.clone();
            let mut changed = ui.checkbox(&mut s.export.auto, "完成後自動匯出").changed();
            if s.export.auto {
                ui.add_space(4.0);
                changed |= widgets::export_options(ui, &mut s.export, "job_export", true, None);
            } else {
                theme::hint(ui, "做完後到該場會議的「匯出」頁手動匯出。");
            }
            if changed {
                app.apply_settings(s);
            }
        });

        ui.horizontal(|ui| {
            let total = app.new_job.total_seconds();
            if total > 0.0 {
                let price = app.stt_caps.as_ref().and_then(|c| c.price_per_second);
                let cost = price
                    .map(|x| format!("　逐字稿約 US${:.2}", x * total))
                    .unwrap_or_default();
                ui.label(
                    RichText::new(format!("總長 {}{cost}　說話者與摘要免費", fmt_duration_zh(total)))
                        .color(p.text_muted),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let n = app.new_job.files.len();
                let ready = n > 0 && app.has_openrouter_key;
                let label = if n > 1 {
                    format!("開始轉錄 {n} 個檔案")
                } else {
                    "開始轉錄".to_string()
                };
                if theme::primary(ui, Some(icon::PLAY), &label, ready).clicked() {
                    start(app);
                }
            });
        });
        ui.add_space(24.0);
    });
}

fn start(app: &mut MeetApp) {
    let options = JobOptions {
        export: Some(app.settings.export.clone()),
        ..app.new_job.options()
    };
    let mut first = None;
    for f in std::mem::take(&mut app.new_job.files) {
        let mut p = Project::new(f, options.clone());
        if let Some(Ok(d)) = app.new_job.durations.get(&p.source_file) {
            p.duration = *d;
        }
        if let Err(e) = app.store.save(&p) {
            app.toast(format!("建立專案失敗：{e}"), true);
            continue;
        }
        first.get_or_insert(p.id);
        app.upsert(p.clone());
        app.worker.send(Cmd::Enqueue(p));
    }
    app.new_job.durations.clear();
    if let Some(id) = first {
        app.selected = Some(id);
        app.view = View::Project;
        app.project_ui.reset();
    }
}
