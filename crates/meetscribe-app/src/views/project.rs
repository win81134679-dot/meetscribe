//! 單一會議：處理進度、摘要、逐字稿（可播放、搜尋、逐句編輯）、說話者（改名、合併）、匯出、紀錄。

use crate::app::MeetApp;
use crate::theme::{self, icon, pal};
use crate::widgets::{self, speaker_chip, speaker_color};
use crate::worker::{Cmd, SendWhat};
use egui::{Margin, RichText, Stroke, Ui};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use meetscribe_core::config::Settings;
use meetscribe_core::export::{self, ExportOptions};
use meetscribe_core::model::{NameSource, Project, ProjectStatus, Stage, Timing};
use meetscribe_core::pipeline::LogLevel;
use meetscribe_core::textutil::{fmt_clock, fmt_duration_zh};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Summary,
    Transcript,
    Speakers,
    Export,
    Log,
}

#[derive(Default)]
pub struct ProjectUi {
    pub tab: Tab,
    search: String,
    speaker_filter: Option<String>,
    editing: Option<u64>,
    edit_text: String,
    follow_playback: bool,
    last_playing: Option<u64>,
    rename: HashMap<String, String>,
    merge_target: HashMap<String, String>,
    speaker_search: String,
    summary_edit: bool,
    summary_buf: String,
    regen_instructions: String,
    md_cache: CommonMarkCache,
    /// 匯出頁正在編輯的選項（切換會議時清掉，改帶該會議自己的）。
    export: Option<ExportOptions>,
    last_export: Vec<PathBuf>,
    title_buf: Option<String>,
}

impl ProjectUi {
    pub fn reset(&mut self) {
        let follow = self.follow_playback;
        *self = Self {
            follow_playback: follow,
            ..Default::default()
        };
    }
}

/// 畫面上收集的動作，畫完再一起套用（避免同時借用 app 的多個部分）。
enum Action {
    Play { file: String, secs: f64 },
    TogglePlay,
    StopPlay,
    Cancel,
    Resume,
    Regenerate(String),
    Reidentify,
    Dirty,
    Toast(String, bool),
    Export,
    Settings(Box<Settings>),
    /// 傳到其他裝置：整場會議（None）或一個匯出檔。
    SendToDevice(Option<PathBuf>),
}

pub fn show(app: &mut MeetApp, ui: &mut Ui, id: Uuid) {
    let Some(idx) = app.projects.iter().position(|p| p.id == id) else {
        return;
    };
    let busy = app.busy.contains(&id);
    let mut actions: Vec<Action> = Vec::new();
    let audio_path = app.projects[idx].playback_audio().map(|a| app.store.path_in(id, a));
    let playing_main = app
        .player
        .as_ref()
        .filter(|p| p.has_track() && p.loaded().is_some() && p.loaded().map(|l| l.to_path_buf()) == audio_path)
        .map(|p| (p.position(), p.is_playing()));

    {
        let p = &mut app.projects[idx];
        header(ui, p, &mut app.project_ui, busy, &mut actions);
        if busy || p.status == ProjectStatus::Running {
            progress_card(ui, p, app.progress.get(&id), app.logs.get(&id));
        } else if let ProjectStatus::Failed(err) = &p.status {
            let pl = pal(ui);
            egui::Frame::NONE
                .fill(pl.danger.gamma_multiply(0.08))
                .stroke(Stroke::new(1.0, pl.danger.gamma_multiply(0.3)))
                .corner_radius(theme::RADIUS)
                .inner_margin(Margin::symmetric(20, 14))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(theme::ic(icon::WARNING, 18.0).color(pl.danger));
                        ui.label(theme::bold("處理中斷", 15.0).color(pl.danger));
                    });
                    ui.add(egui::Label::new(err.as_str()).wrap());
                    theme::hint(
                        ui,
                        "已完成的部分都保留了。修正問題（金鑰、額度、網路）後按「從中斷處繼續」，不會重複付費。",
                    );
                });
            ui.add_space(14.0);
        }
    }

    let has_content = !app.projects[idx].segments.is_empty() || app.projects[idx].summary.is_some();
    if !has_content {
        if !busy && !matches!(app.projects[idx].status, ProjectStatus::Failed(_)) {
            theme::hint(ui, "還沒有內容。");
        }
        apply(app, id, actions);
        return;
    }

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let t = &mut app.project_ui.tab;
        theme::tab(ui, t, Tab::Summary, icon::SPARKLE, "摘要");
        theme::tab(ui, t, Tab::Transcript, icon::DOCUMENT, "逐字稿");
        theme::tab(ui, t, Tab::Speakers, icon::PEOPLE, "說話者");
        theme::tab(ui, t, Tab::Export, icon::DOWNLOAD, "匯出");
        theme::tab(ui, t, Tab::Log, icon::LIST, "紀錄與費用");
    });
    ui.add_space(10.0);

    let editable = !busy;
    match app.project_ui.tab {
        Tab::Summary => {
            let settings = app.settings.clone();
            summary_tab(
                ui,
                &mut app.projects[idx],
                &mut app.project_ui,
                &settings,
                editable,
                &mut actions,
            )
        }
        Tab::Transcript => transcript_tab(
            ui,
            &mut app.projects[idx],
            &mut app.project_ui,
            editable,
            playing_main,
            &mut actions,
        ),
        Tab::Speakers => speakers_tab(ui, &mut app.projects[idx], &mut app.project_ui, editable, &mut actions),
        Tab::Export => {
            let settings = app.settings.clone();
            export_tab(ui, &app.projects[idx], &mut app.project_ui, &settings, &mut actions)
        }
        Tab::Log => log_tab(ui, &app.projects[idx], app.logs.get(&id)),
    }
    apply(app, id, actions);
}

fn apply(app: &mut MeetApp, id: Uuid, actions: Vec<Action>) {
    for a in actions {
        match a {
            Action::Play { file, secs } => app.play(id, &file, secs),
            Action::TogglePlay => {
                if let Some(p) = app.player.as_mut() {
                    p.toggle();
                }
            }
            Action::StopPlay => {
                if let Some(p) = app.player.as_mut() {
                    p.stop();
                }
            }
            Action::Cancel => app.worker.send(Cmd::Cancel(id)),
            Action::Resume => {
                app.save_now();
                if let Some(p) = app.project(id).cloned() {
                    app.worker.send(Cmd::Enqueue(p));
                }
            }
            Action::Regenerate(instructions) => {
                app.save_now();
                if let Some(p) = app.project(id).cloned() {
                    app.toast("正在重新產生摘要…", false);
                    app.worker.send(Cmd::RegenerateSummary {
                        project: p,
                        instructions,
                    });
                }
            }
            Action::Reidentify => {
                app.save_now();
                if let Some(p) = app.project(id).cloned() {
                    app.worker.send(Cmd::Reidentify(p));
                }
            }
            Action::Dirty => app.mark_dirty(id),
            Action::Toast(t, e) => app.toast(t, e),
            Action::Export => export_now(app, id),
            Action::Settings(s) => app.apply_settings(*s),
            Action::SendToDevice(file) => {
                app.save_now();
                match file {
                    Some(f) => {
                        let title = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        app.transfer.open_send(SendWhat::File(f), title);
                    }
                    None => {
                        let title = app.project(id).map(|p| p.title.clone()).unwrap_or_default();
                        app.transfer.open_send(SendWhat::Meeting(id), title);
                    }
                }
            }
        }
    }
}

fn header(ui: &mut Ui, p: &mut Project, st: &mut ProjectUi, busy: bool, actions: &mut Vec<Action>) {
    let pl = pal(ui);
    ui.horizontal(|ui| {
        if let Some(buf) = &mut st.title_buf {
            let r = ui.add(
                egui::TextEdit::singleline(buf)
                    .font(egui::FontId::new(22.0, theme::bold_family()))
                    .desired_width(460.0),
            );
            if r.lost_focus() {
                let t = buf.trim().to_string();
                if !t.is_empty() && t != p.title {
                    p.title = t;
                    actions.push(Action::Dirty);
                }
                st.title_buf = None;
            } else {
                r.request_focus();
            }
        } else {
            let r = ui
                .add(egui::Label::new(theme::bold(&p.title, 26.0)).sense(egui::Sense::click()))
                .on_hover_text("點一下改名稱");
            if r.clicked() && !busy {
                st.title_buf = Some(p.title.clone());
            }
        }
        let shown = if busy && p.status != ProjectStatus::Running {
            ProjectStatus::Queued
        } else {
            p.status.clone()
        };
        widgets::status_badge(ui, &shown);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if busy {
                if theme::button(ui, Some(icon::CANCEL), "取消").clicked() {
                    actions.push(Action::Cancel);
                }
            } else if p.status.is_resumable() && theme::primary(ui, Some(icon::PLAY), "從中斷處繼續", true).clicked()
            {
                actions.push(Action::Resume);
            }
            if !busy
                && theme::button(ui, Some(icon::SEND), "傳到其他裝置")
                    .on_hover_text("整場會議（逐字稿、摘要、錄音）加密後傳到你自己的電腦或手機")
                    .clicked()
            {
                actions.push(Action::SendToDevice(None));
            }
            if theme::button(ui, Some(icon::FOLDER), "原始檔位置").clicked()
                && let Some(dir) = p.source_file.parent()
            {
                let _ = open::that(dir);
            }
        });
    });
    let mut meta = vec![p.created_at.format("%Y-%m-%d %H:%M").to_string()];
    if p.duration > 0.0 {
        meta.push(fmt_duration_zh(p.duration));
    }
    if !p.speakers.is_empty() {
        meta.push(format!("{} 位說話者", p.speakers.len()));
    }
    if !p.segments.is_empty() {
        meta.push(format!("{} 句", p.segments.len()));
    }
    if p.usage.total_usd() > 0.0 {
        meta.push(format!("費用 US${:.3}", p.usage.total_usd()));
    }
    ui.label(RichText::new(meta.join("　·　")).color(pl.text_muted));
    ui.add_space(14.0);
}

fn progress_card(
    ui: &mut Ui,
    p: &Project,
    progress: Option<&(Stage, f32, String)>,
    logs: Option<&Vec<crate::app::LogLine>>,
) {
    theme::card(ui, |ui| {
        let pl = pal(ui);
        let current = progress.map(|x| x.0).unwrap_or(p.stage);
        let stages: Vec<Stage> = Stage::ALL
            .into_iter()
            .filter(|s| match s {
                Stage::Reconcile => p.options.identify_speakers,
                Stage::Summary => p.options.generate_summary,
                _ => true,
            })
            .collect();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (i, s) in stages.iter().enumerate() {
                let (glyph, color) = if *s < current {
                    (icon::CHECK, pl.success)
                } else if *s == current {
                    (icon::CLOCK, pl.accent_strong)
                } else {
                    (icon::CLOCK, pl.text_muted.gamma_multiply(0.6))
                };
                ui.label(theme::ic(glyph, 13.0).color(color));
                let text = if *s == current {
                    theme::bold(s.label(), 14.0).color(color)
                } else {
                    RichText::new(s.label()).color(color)
                };
                ui.label(text);
                if i + 1 < stages.len() {
                    ui.label(RichText::new("—").color(pl.text_muted.gamma_multiply(0.5)));
                }
            }
        });
        ui.add_space(8.0);
        let (fraction, msg) = progress
            .map(|x| (x.1, x.2.clone()))
            .unwrap_or((0.0, "排隊等待中…".into()));
        ui.add(
            egui::ProgressBar::new(fraction)
                .desired_height(8.0)
                .fill(pl.accent)
                .corner_radius(99)
                .animate(true),
        );
        ui.horizontal(|ui| {
            ui.label(msg);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("{:.0}%", fraction * 100.0)).color(pl.text_muted));
            });
        });
        if let Some(logs) = logs {
            for l in logs.iter().rev().take(4).collect::<Vec<_>>().into_iter().rev() {
                ui.label(log_text(ui, l.level, &format!("{}  {}", l.at.format("%H:%M:%S"), l.text)).size(12.5));
            }
        }
    });
}

fn log_text(ui: &Ui, level: LogLevel, text: &str) -> RichText {
    let p = pal(ui);
    match level {
        LogLevel::Info => RichText::new(text).color(p.text_muted),
        LogLevel::Warn => RichText::new(text).color(p.warning),
        LogLevel::Error => RichText::new(text).color(p.danger),
    }
}

// ---------- 摘要 ----------

fn summary_tab(
    ui: &mut Ui,
    p: &mut Project,
    st: &mut ProjectUi,
    settings: &Settings,
    editable: bool,
    actions: &mut Vec<Action>,
) {
    let pl = pal(ui);
    theme::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            theme::label(ui, "摘要方式");
            let mut s = settings.clone();
            if widgets::summary_mode_picker(ui, &mut s, "tab_summary_mode") {
                actions.push(Action::Settings(Box::new(s)));
            }
            ui.add(
                egui::TextEdit::singleline(&mut st.regen_instructions)
                    .hint_text("額外要求（選填）")
                    .desired_width(240.0),
            );
            let label = if p.summary.is_some() {
                "重新產生"
            } else {
                "產生摘要"
            };
            if theme::primary(ui, Some(icon::REFRESH), label, editable && !p.segments.is_empty()).clicked() {
                actions.push(Action::Regenerate(st.regen_instructions.trim().to_string()));
            }
        });
        if let Some(sum) = &p.summary {
            theme::hint(
                ui,
                &format!(
                    "由 {} 於 {} 產生{}",
                    sum.model,
                    sum.created_at.format("%Y-%m-%d %H:%M"),
                    if sum.instructions.is_empty() {
                        String::new()
                    } else {
                        format!("（要求：{}）", sum.instructions)
                    }
                ),
            );
        }
    });
    let Some(sum) = &mut p.summary else {
        theme::hint(ui, "這場會議還沒有摘要。");
        return;
    };
    theme::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(theme::bold("會議摘要", 16.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let edit_label = if st.summary_edit { "完成編輯" } else { "編輯" };
                if ui
                    .add_enabled(
                        editable,
                        egui::Button::new(theme::icon_text(ui, Some(icon::EDIT), edit_label, None)),
                    )
                    .clicked()
                {
                    st.summary_edit = !st.summary_edit;
                    st.summary_buf = sum.markdown.clone();
                }
                if theme::button(ui, Some(icon::COPY), "複製").clicked() {
                    ui.ctx().copy_text(sum.markdown.clone());
                    actions.push(Action::Toast("摘要已複製".into(), false));
                }
            });
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            if st.summary_edit {
                let r = ui.add(
                    egui::TextEdit::multiline(&mut st.summary_buf)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .desired_rows(28),
                );
                if r.changed() {
                    sum.markdown = st.summary_buf.clone();
                    actions.push(Action::Dirty);
                }
            } else {
                ui.visuals_mut().override_text_color = Some(pl.text);
                CommonMarkViewer::new().show(ui, &mut st.md_cache, &sum.markdown);
            }
        });
    });
}

// ---------- 逐字稿 ----------

fn transcript_tab(
    ui: &mut Ui,
    p: &mut Project,
    st: &mut ProjectUi,
    editable: bool,
    playing: Option<(f64, bool)>,
    actions: &mut Vec<Action>,
) {
    let pl = pal(ui);
    let audio = p.playback_audio().map(str::to_string);
    theme::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(theme::ic(icon::SEARCH, 14.0).color(pl.text_muted));
            ui.add(
                egui::TextEdit::singleline(&mut st.search)
                    .hint_text("搜尋逐字稿")
                    .desired_width(200.0),
            );
            let filter_name = st
                .speaker_filter
                .as_deref()
                .map(|s| p.speaker_name(Some(s)))
                .unwrap_or_else(|| "全部說話者".into());
            egui::ComboBox::from_id_salt("speaker_filter")
                .selected_text(filter_name)
                .width(160.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut st.speaker_filter, None, "全部說話者");
                    for s in &p.speakers {
                        ui.selectable_value(&mut st.speaker_filter, Some(s.id.clone()), &s.name);
                    }
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.checkbox(&mut st.follow_playback, "跟隨播放");
                match playing {
                    Some((pos, is_playing)) => {
                        ui.label(RichText::new(format!("{} / {}", fmt_clock(pos), fmt_clock(p.duration))).monospace());
                        if theme::button(ui, Some(icon::STOP), "").on_hover_text("停止").clicked() {
                            actions.push(Action::StopPlay);
                        }
                        let (g, t) = if is_playing {
                            (icon::PAUSE, "暫停")
                        } else {
                            (icon::PLAY, "繼續")
                        };
                        if theme::primary(ui, Some(g), t, true).clicked() {
                            actions.push(Action::TogglePlay);
                        }
                    }
                    None => {
                        if let Some(a) = &audio
                            && theme::primary(ui, Some(icon::PLAY), "從頭播放", true).clicked()
                        {
                            actions.push(Action::Play {
                                file: a.clone(),
                                secs: 0.0,
                            });
                        }
                    }
                }
            });
        });
        theme::hint(
            ui,
            "點時間從那裡播放；點說話者可改成別人；點文字可修改、併入上一句或刪除。修改會自動存檔。",
        );
    });

    let q = st.search.trim().to_lowercase();
    let now_playing: Option<u64> =
        playing.and_then(|(pos, _)| p.segments.iter().find(|s| pos >= s.start && pos < s.end).map(|s| s.id));
    let scroll_to = if st.follow_playback && now_playing.is_some() && now_playing != st.last_playing {
        now_playing
    } else {
        None
    };
    st.last_playing = now_playing;

    let speakers: Vec<(String, String, usize)> = p
        .speakers
        .iter()
        .map(|s| (s.id.clone(), s.name.clone(), s.color))
        .collect();
    let mut change_speaker: Option<(usize, Option<String>)> = None;
    let mut new_speaker_for: Option<usize> = None;
    let mut merge_prev: Option<usize> = None;
    let mut delete: Option<usize> = None;
    let mut save_edit: Option<(usize, String)> = None;
    let mut shown = 0usize;

    theme::card(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (i, seg) in p.segments.iter().enumerate() {
                if let Some(f) = &st.speaker_filter
                    && seg.speaker.as_ref() != Some(f)
                {
                    continue;
                }
                if !q.is_empty() && !seg.text.to_lowercase().contains(&q) {
                    continue;
                }
                shown += 1;
                let is_now = now_playing == Some(seg.id);
                let row = egui::Frame::NONE
                    .fill(if is_now {
                        pl.accent_soft
                    } else {
                        egui::Color32::TRANSPARENT
                    })
                    .corner_radius(theme::RADIUS_SMALL)
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            let approx = if seg.timing == Timing::Estimated { "≈" } else { "" };
                            let t = ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(format!("{approx}{}", fmt_clock(seg.start)))
                                            .monospace()
                                            .color(pl.text_muted),
                                    )
                                    .frame(false)
                                    .min_size(egui::vec2(64.0, 24.0)),
                                )
                                .on_hover_text(if seg.timing == Timing::Estimated {
                                    "時間為估計值；點一下從這裡播放"
                                } else {
                                    "從這裡播放"
                                });
                            if t.clicked()
                                && let Some(a) = &audio
                            {
                                actions.push(Action::Play {
                                    file: a.clone(),
                                    secs: (seg.start - 0.3).max(0.0),
                                });
                            }
                            if p.options.identify_speakers || seg.speaker.is_some() {
                                let (name, color) =
                                    match seg.speaker.as_deref().and_then(|s| speakers.iter().find(|x| x.0 == s)) {
                                        Some((_, n, c)) => (n.clone(), speaker_color(*c)),
                                        None => ("（未標註）".into(), pl.text_muted),
                                    };
                                ui.add_enabled_ui(editable, |ui| {
                                    ui.visuals_mut().widgets.inactive.weak_bg_fill = color.gamma_multiply(0.12);
                                    ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE;
                                    ui.visuals_mut().widgets.hovered.weak_bg_fill = color.gamma_multiply(0.22);
                                    ui.visuals_mut().widgets.inactive.corner_radius = egui::CornerRadius::same(99);
                                    ui.visuals_mut().widgets.hovered.corner_radius = egui::CornerRadius::same(99);
                                    ui.spacing_mut().button_padding = egui::vec2(10.0, 3.0);
                                    ui.menu_button(theme::bold(name, 13.0).color(color), |ui| {
                                        ui.label(RichText::new("改成").color(pal(ui).text_muted));
                                        egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                                            for (sid, sname, c) in &speakers {
                                                if ui.button(RichText::new(sname).color(speaker_color(*c))).clicked() {
                                                    change_speaker = Some((i, Some(sid.clone())));
                                                    ui.close();
                                                }
                                            }
                                        });
                                        ui.separator();
                                        if theme::ghost(ui, Some(icon::ADD), "新的說話者", None).clicked() {
                                            new_speaker_for = Some(i);
                                            ui.close();
                                        }
                                        if theme::ghost(ui, Some(icon::CANCEL), "不標註", None).clicked() {
                                            change_speaker = Some((i, None));
                                            ui.close();
                                        }
                                    });
                                });
                            }
                            if st.editing == Some(seg.id) {
                                ui.vertical(|ui| {
                                    ui.add(
                                        egui::TextEdit::multiline(&mut st.edit_text)
                                            .desired_width(f32::INFINITY)
                                            .desired_rows(2),
                                    );
                                    ui.horizontal(|ui| {
                                        if theme::primary(ui, Some(icon::CHECK), "儲存", true).clicked() {
                                            save_edit = Some((i, st.edit_text.clone()));
                                        }
                                        if theme::button(ui, None, "取消").clicked() {
                                            st.editing = None;
                                        }
                                        if i > 0 && theme::button(ui, None, "併入上一句").clicked() {
                                            merge_prev = Some(i);
                                        }
                                        if theme::ghost(ui, Some(icon::DELETE), "刪除此句", Some(pal(ui).danger))
                                            .clicked()
                                        {
                                            delete = Some(i);
                                        }
                                    });
                                });
                            } else {
                                let mut text = RichText::new(&seg.text).size(14.5);
                                if seg.edited {
                                    text = text.italics();
                                }
                                let r = ui.add(egui::Label::new(text).wrap().sense(egui::Sense::click()));
                                if r.clicked() && editable {
                                    st.editing = Some(seg.id);
                                    st.edit_text = seg.text.clone();
                                }
                            }
                        });
                    });
                if scroll_to == Some(seg.id) {
                    row.response.scroll_to_me(Some(egui::Align::Center));
                }
            }
            if shown == 0 {
                theme::hint(ui, "沒有符合條件的句子。");
            }
        });
    });

    if let Some(i) = new_speaker_for {
        let sid = p.add_speaker(None, String::new());
        change_speaker = Some((i, Some(sid)));
    }
    if let Some((i, sp)) = change_speaker {
        p.segments[i].speaker = sp;
        p.segments[i].edited = true;
        p.prune_unused_speakers();
        actions.push(Action::Dirty);
    }
    if let Some((i, text)) = save_edit {
        let t = text.trim().to_string();
        if t.is_empty() {
            p.segments.remove(i);
        } else if t != p.segments[i].text {
            p.segments[i].text = t;
            p.segments[i].edited = true;
        }
        st.editing = None;
        actions.push(Action::Dirty);
    }
    if let Some(i) = merge_prev {
        p.merge_segment_with_previous(i);
        st.editing = None;
        actions.push(Action::Dirty);
    }
    if let Some(i) = delete {
        p.segments.remove(i);
        p.prune_unused_speakers();
        st.editing = None;
        actions.push(Action::Dirty);
    }
}

// ---------- 說話者 ----------

fn speakers_tab(ui: &mut Ui, p: &mut Project, st: &mut ProjectUi, editable: bool, actions: &mut Vec<Action>) {
    let pl = pal(ui);
    if p.speakers.is_empty() {
        theme::card(ui, |ui| {
            theme::hint(ui, "這場會議沒有標註說話者。");
            if ui
                .add_enabled(
                    editable && !p.chunks.is_empty(),
                    egui::Button::new(theme::icon_text(ui, Some(icon::REFRESH), "辨識說話者", None)),
                )
                .clicked()
            {
                actions.push(Action::Reidentify);
            }
        });
        return;
    }
    let stats = p.speaker_stats();
    let q = st.speaker_search.trim().to_lowercase();
    let all: Vec<(String, String)> = p.speakers.iter().map(|s| (s.id.clone(), s.name.clone())).collect();
    let mut rename: Option<(String, String)> = None;
    let mut merge: Option<(String, String)> = None;
    let mut play: Option<String> = None;
    theme::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(theme::bold(format!("{} 位說話者", p.speakers.len()), 16.0));
            if p.speakers.len() > 12 {
                ui.add(
                    egui::TextEdit::singleline(&mut st.speaker_search)
                        .hint_text("找說話者")
                        .desired_width(180.0),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        editable && !p.chunks.is_empty(),
                        egui::Button::new(theme::icon_text(ui, Some(icon::REFRESH), "重新辨識", None)),
                    )
                    .on_hover_text("逐字稿不重轉，只重新分配說話者（用免費模型）。你改過的名字與合併會被清掉。")
                    .clicked()
                {
                    actions.push(Action::Reidentify);
                }
            });
        });
        theme::hint(
            ui,
            "標「推測」的名字是模型從對話判斷的，請核對。同一個人被拆成兩位時，用「合併到」併起來。",
        );
        ui.add_space(8.0);
        egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("speakers")
                .num_columns(6)
                .spacing([18.0, 12.0])
                .show(ui, |ui| {
                    for h in ["說話者", "名稱", "發言時間", "句數", "聲音樣本", "合併到"] {
                        ui.label(RichText::new(h).size(12.5).color(pl.text_muted));
                    }
                    ui.end_row();
                    for (sid, secs, count) in &stats {
                        let Some(sp) = p.speaker(sid) else { continue };
                        if !q.is_empty() && !sp.name.to_lowercase().contains(&q) && !sp.id.to_lowercase().contains(&q) {
                            continue;
                        }
                        speaker_chip(ui, speaker_color(sp.color), &sp.id);
                        ui.horizontal(|ui| {
                            let buf = st.rename.entry(sid.clone()).or_insert_with(|| sp.name.clone());
                            let r = ui.add_enabled(editable, egui::TextEdit::singleline(buf).desired_width(200.0));
                            if r.lost_focus() && buf.trim() != sp.name && !buf.trim().is_empty() {
                                rename = Some((sid.clone(), buf.trim().to_string()));
                            }
                            match sp.name_source {
                                NameSource::Inferred => {
                                    theme::badge(ui, "推測", pl.warning);
                                }
                                NameSource::User => {
                                    theme::badge(ui, "已確認", pl.success);
                                }
                                NameSource::Label => {}
                            }
                        })
                        .response
                        .on_hover_text(if sp.note.is_empty() {
                            "（無描述）".to_string()
                        } else {
                            sp.note.clone()
                        });
                        ui.label(fmt_duration_zh(*secs));
                        ui.label(count.to_string());
                        if let Some(clip) = &sp.sample_clip {
                            if theme::button(ui, Some(icon::PLAY), "聽").clicked() {
                                play = Some(clip.clone());
                            }
                        } else if let Some(seg) = p.segments.iter().find(|s| s.speaker.as_deref() == Some(sid.as_str()))
                        {
                            if theme::button(ui, Some(icon::PLAY), "第一句").clicked() {
                                play = Some(format!("@{}", seg.start));
                            }
                        } else {
                            ui.label("");
                        }
                        ui.add_enabled_ui(editable, |ui| {
                            ui.horizontal(|ui| {
                                let target = st.merge_target.entry(sid.clone()).or_default();
                                let shown = all
                                    .iter()
                                    .find(|(i, _)| i == target)
                                    .map(|x| x.1.clone())
                                    .unwrap_or_else(|| "選擇…".into());
                                egui::ComboBox::from_id_salt(format!("merge_{sid}"))
                                    .selected_text(shown)
                                    .width(150.0)
                                    .show_ui(ui, |ui| {
                                        for (oid, oname) in &all {
                                            if oid != sid {
                                                ui.selectable_value(target, oid.clone(), format!("{oname}（{oid}）"));
                                            }
                                        }
                                    });
                                if ui.add_enabled(!target.is_empty(), egui::Button::new("合併")).clicked() {
                                    merge = Some((sid.clone(), target.clone()));
                                }
                            });
                        });
                        ui.end_row();
                    }
                });
        });
    });
    if let Some((sid, name)) = rename {
        p.rename_speaker(&sid, &name);
        actions.push(Action::Dirty);
    }
    if let Some((from, into)) = merge
        && p.merge_speakers(&from, &into)
    {
        st.rename.clear();
        st.merge_target.clear();
        actions.push(Action::Dirty);
        actions.push(Action::Toast("已合併說話者".into(), false));
    }
    if let Some(target) = play {
        match target.strip_prefix('@') {
            Some(secs) => {
                if let Some(a) = p.playback_audio() {
                    actions.push(Action::Play {
                        file: a.to_string(),
                        secs: secs.parse().unwrap_or(0.0),
                    });
                }
            }
            None => actions.push(Action::Play {
                file: target,
                secs: 0.0,
            }),
        }
    }
}

// ---------- 匯出 ----------

fn export_tab(ui: &mut Ui, p: &Project, st: &mut ProjectUi, settings: &Settings, actions: &mut Vec<Action>) {
    // 預設帶這場會議建立時的輸出選項（舊專案用目前設定）
    let opts = st
        .export
        .get_or_insert_with(|| p.options.export.clone().unwrap_or_else(|| settings.export.clone()));
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        theme::section(ui, "匯出", |ui| {
            widgets::export_options(ui, opts, "project_export", false, Some(p));
        });
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ready = !opts.known_formats().is_empty();
                if theme::primary(ui, Some(icon::DOWNLOAD), "匯出", ready).clicked() {
                    actions.push(Action::Export);
                }
                if theme::button(ui, Some(icon::OPEN), "開啟資料夾").clicked() {
                    // 還沒匯出過時子資料夾不存在，改開上一層
                    let dir = opts.target_dir(p);
                    let _ = open::that(match dir.parent() {
                        Some(parent) if !dir.is_dir() => parent.to_path_buf(),
                        _ => dir,
                    });
                }
            });
        });
        let (title, files) = if st.last_export.is_empty() {
            ("處理完成時自動匯出的檔案", &p.exported)
        } else {
            ("已匯出", &st.last_export)
        };
        if !files.is_empty() {
            ui.add_space(10.0);
            exported_list(ui, title, files, actions);
        }
    });
}

fn exported_list(ui: &mut Ui, title: &str, files: &[PathBuf], actions: &mut Vec<Action>) {
    let pl = pal(ui);
    theme::section(ui, title, |ui| {
        for f in files {
            ui.horizontal(|ui| {
                ui.label(theme::ic(icon::CHECK, 13.0).color(pl.success));
                ui.label(f.display().to_string());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::ghost(ui, Some(icon::SEND), "", Some(pl.accent_strong))
                        .on_hover_text("傳到其他裝置")
                        .clicked()
                    {
                        actions.push(Action::SendToDevice(Some(f.clone())));
                    }
                    if theme::ghost(ui, Some(icon::OPEN), "開啟", Some(pl.accent_strong)).clicked() {
                        let _ = open::that(f);
                    }
                });
            });
        }
    });
}

fn export_now(app: &mut MeetApp, id: Uuid) {
    app.save_now();
    let Some(p) = app.project(id).cloned() else { return };
    let opts = app
        .project_ui
        .export
        .clone()
        .unwrap_or_else(|| p.options.export.clone().unwrap_or_else(|| app.settings.export.clone()));
    match export::export_project(&p, &opts) {
        Ok(files) => {
            app.toast(format!("已匯出 {} 個檔案", files.len()), false);
            app.project_ui.last_export = files;
        }
        Err(e) => app.toast(format!("匯出失敗：{e}"), true),
    }
}

// ---------- 紀錄 ----------

fn log_tab(ui: &mut Ui, p: &Project, logs: Option<&Vec<crate::app::LogLine>>) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        theme::section(ui, "費用與用量", |ui| {
            let pl = pal(ui);
            egui::Grid::new("usage")
                .num_columns(2)
                .spacing([24.0, 10.0])
                .show(ui, |ui| {
                    let row = |ui: &mut Ui, k: &str, v: String| {
                        ui.label(RichText::new(k).color(pl.text_muted));
                        ui.label(v);
                        ui.end_row();
                    };
                    row(
                        ui,
                        "逐字稿（OpenRouter）",
                        format!(
                            "US${:.4}　（{}）",
                            p.usage.stt_usd,
                            fmt_duration_zh(p.usage.stt_seconds)
                        ),
                    );
                    row(
                        ui,
                        "說話者與摘要",
                        format!(
                            "US${:.4}　（輸入 {} / 輸出 {} tokens；免費模型與 Claude Code 訂閱記為 0）",
                            p.usage.llm_usd, p.usage.llm_input_tokens, p.usage.llm_output_tokens
                        ),
                    );
                    row(ui, "轉錄模型", p.models.stt.clone());
                    row(ui, "說話者分配模型", p.models.attribution.clone());
                    row(
                        ui,
                        "摘要模型",
                        p.summary
                            .as_ref()
                            .map(|s| s.model.clone())
                            .unwrap_or_else(|| p.models.summary.clone()),
                    );
                    row(ui, "分段", format!("{} 段", p.chunks.len()));
                });
        });
        if !p.warnings.is_empty() {
            theme::section(ui, "注意事項", |ui| {
                for w in &p.warnings {
                    ui.label(log_text(ui, LogLevel::Warn, w));
                }
            });
        }
        theme::section(ui, "這次開啟程式後的處理紀錄", |ui| match logs {
            Some(l) if !l.is_empty() => {
                for line in l {
                    ui.label(log_text(
                        ui,
                        line.level,
                        &format!("{}  {}", line.at.format("%H:%M:%S"), line.text),
                    ));
                }
            }
            _ => theme::hint(ui, "（沒有）完整紀錄在程式資料夾的 logs 底下。"),
        });
    });
}
