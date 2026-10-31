//! 專案用的小元件：說話者顏色、狀態標籤、摘要方式切換、輸出選項。共用外觀在 [`crate::theme`]。

use crate::theme::{self, icon, pal};
use egui::{Color32, RichText, Ui};
use meetscribe_core::config::{LlmProvider, Settings};
use meetscribe_core::export::{self, Content, ExportOptions};
use meetscribe_core::model::{Project, ProjectStatus};
use meetscribe_core::openrouter::{self, ModelInfo};
use std::path::PathBuf;

/// 說話者顏色：12 色輪替，跟 Soft 主題的柔和色調搭配，深淺色都看得清楚。
const PALETTE: [Color32; 12] = [
    Color32::from_rgb(0x6A, 0x62, 0xD0),
    Color32::from_rgb(0xD0, 0x6B, 0x5E),
    Color32::from_rgb(0x3E, 0x92, 0x70),
    Color32::from_rgb(0x4C, 0x7C, 0xD6),
    Color32::from_rgb(0xC0, 0x7A, 0x1E),
    Color32::from_rgb(0xC9, 0x52, 0x7A),
    Color32::from_rgb(0x4E, 0x85, 0x44),
    Color32::from_rgb(0x2E, 0x99, 0xA8),
    Color32::from_rgb(0x8C, 0x5A, 0xB8),
    Color32::from_rgb(0x8A, 0x6B, 0x4F),
    Color32::from_rgb(0x5E, 0x6E, 0x88),
    Color32::from_rgb(0xA8, 0x8A, 0x10),
];

pub fn speaker_color(index: usize) -> Color32 {
    PALETTE[index % PALETTE.len()]
}

/// 說話者膠囊：色點＋名稱。
pub fn speaker_chip(ui: &mut Ui, color: Color32, name: &str) -> egui::Response {
    egui::Frame::NONE
        .fill(color.gamma_multiply(0.12))
        .corner_radius(99)
        .inner_margin(egui::Margin::symmetric(9, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, color);
                ui.label(theme::bold(name, 13.0).color(color));
            });
        })
        .response
}

pub fn status_color(ui: &Ui, status: &ProjectStatus) -> Color32 {
    let p = pal(ui);
    match status {
        ProjectStatus::Queued => p.text_muted,
        ProjectStatus::Running => p.accent_strong,
        ProjectStatus::Done => p.success,
        ProjectStatus::Failed(_) => p.danger,
        ProjectStatus::Cancelled => p.warning,
    }
}

pub fn status_badge(ui: &mut Ui, status: &ProjectStatus) -> egui::Response {
    let c = status_color(ui, status);
    theme::badge(ui, status.label(), c)
}

/// 摘要模型的下拉選單（2026-10-31 使用者：「OPENROTUER 是下拉選單的選取模型 一定要標注免費/付費」「越推薦的越上面 但免費的優先再付費的」）。
/// 清單是 OpenRouter 上能寫摘要的模型（`openrouter::summary_models` 排好序：免費組在前、各組推薦的在前），另加本機 Claude Code CLI。
/// 選單旁邊是「在 OpenRouter 看費率」直接開該模型的官網頁。`models` 是啟動時載入的清單（還沒載好是 None）。回傳是否有改。
pub fn summary_model_picker(
    ui: &mut Ui,
    settings: &mut Settings,
    models: Option<&Result<Vec<ModelInfo>, String>>,
    id: &str,
) -> bool {
    let p = pal(ui);
    let list = match models {
        Some(Ok(l)) => openrouter::summary_models(l),
        _ => Vec::new(),
    };
    let claude = settings.summary_provider == LlmProvider::ClaudeCode;
    let current = list.iter().find(|m| m.id == settings.summary_model);
    let selected = if claude {
        "本機 Claude Code CLI".to_string()
    } else {
        match current {
            Some(m) => format!("{}　{}", m.name, if m.is_free() { "免費" } else { "付費" }),
            None => settings.summary_model.clone(),
        }
    };
    let filter_id = egui::Id::unique((id, "summary-model-filter"));
    let mut filter: String = ui.data_mut(|d| d.get_temp(filter_id).unwrap_or_default());
    let mut chosen: Option<(LlmProvider, String)> = None;
    let combo = egui::ComboBox::from_id_salt(id)
        .selected_text(selected)
        .width(320.0)
        .height(440.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            ui.set_min_width(460.0);
            ui.add(egui::TextEdit::singleline(&mut filter).hint_text("搜尋模型").desired_width(f32::INFINITY));
            if ui
                .selectable_label(claude, "本機 Claude Code CLI（用你的 Claude 訂閱）")
                .clicked()
            {
                chosen = Some((LlmProvider::ClaudeCode, meetscribe_core::config::DEFAULT_SUMMARY_MODEL_CLAUDE_CODE.into()));
            }
            match models {
                None => {
                    ui.label(RichText::new("載入 OpenRouter 模型清單中…").color(p.text_muted));
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(format!("載入 OpenRouter 模型清單失敗：{e}")).color(p.danger));
                }
                Some(Ok(_)) => {}
            }
            let f = filter.trim().to_lowercase();
            let mut group: Option<bool> = None;
            for m in list
                .iter()
                .filter(|m| f.is_empty() || m.id.to_lowercase().contains(&f) || m.name.to_lowercase().contains(&f))
            {
                let free = m.is_free();
                if group != Some(free) {
                    ui.add_space(4.0);
                    ui.separator();
                    ui.label(
                        theme::bold(if free { "免費模型" } else { "付費模型（從你的 OpenRouter 餘額扣款）" }, 13.0)
                            .color(if free { p.success } else { p.warning }),
                    );
                    group = Some(free);
                }
                let rec = openrouter::recommend_rank(&m.id).is_some();
                ui.horizontal(|ui| {
                    let on = !claude && m.id == settings.summary_model;
                    if ui.selectable_label(on, &m.name).on_hover_text(&m.id).clicked() {
                        chosen = Some((LlmProvider::OpenRouter, m.id.clone()));
                    }
                    if rec {
                        theme::badge(ui, "推薦", p.accent_strong);
                    }
                    if free {
                        theme::badge(ui, "免費", p.success);
                    } else {
                        theme::badge(ui, "付費", p.warning);
                        ui.label(RichText::new(m.price_label().trim_start_matches("付費 · ")).size(11.5).color(p.text_muted));
                    }
                });
            }
            if chosen.is_some() {
                ui.close();
            }
        });
    ui.data_mut(|d| d.insert_temp(filter_id, filter));
    combo
        .response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "摘要模型"));
    if !claude {
        let url = openrouter::model_page_url(&settings.summary_model);
        theme::link_button(ui, icon::OPEN, "在 OpenRouter 看費率", &url);
        if let Some(m) = current {
            ui.label(
                RichText::new(m.price_label())
                    .size(12.0)
                    .color(if m.is_free() { p.success } else { p.warning }),
            );
        }
    }
    match chosen {
        Some((provider, model)) if provider != settings.summary_provider || model != settings.summary_model => {
            settings.summary_provider = provider;
            settings.summary_model = model;
            true
        }
        _ => false,
    }
}

/// 輸出選項（設定頁、新增轉錄、匯出頁共用）：格式、位置，以及檔名、內容、逐字稿樣式、同名檔案。
/// `more_collapsed` 時後面四項收在「檔名與內容」裡。`project` 給了就預覽實際路徑，否則用範例。回傳是否有改。
pub fn export_options(
    ui: &mut Ui,
    o: &mut ExportOptions,
    id: &str,
    more_collapsed: bool,
    project: Option<&Project>,
) -> bool {
    let before = o.clone();
    theme::label(ui, "格式");
    ui.columns(2, |cols| {
        for (i, ex) in export::registry().into_iter().enumerate() {
            let ui = &mut cols[i % 2];
            let mut on = o.formats.iter().any(|f| f == ex.id());
            if ui.checkbox(&mut on, format!("{}　.{}", ex.label(), ex.extension())).changed() {
                if on {
                    o.formats.push(ex.id().to_string());
                } else {
                    o.formats.retain(|f| f != ex.id());
                }
            }
        }
    });
    ui.add_space(6.0);

    theme::label(ui, "存到");
    let custom = !o.dir.trim().is_empty();
    ui.horizontal_wrapped(|ui| {
        if ui.radio(!custom, "原始錄音所在的資料夾").clicked() {
            o.dir.clear();
        }
        if ui.radio(custom, "指定資料夾…").clicked()
            && !custom
            && let Some(d) = rfd::FileDialog::new().pick_folder()
        {
            o.dir = d.display().to_string();
        }
    });
    if custom {
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut o.dir).desired_width(ui.available_width() - 200.0));
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "匯出資料夾"));
            if theme::button(ui, Some(icon::FOLDER), "選擇…").clicked()
                && let Some(d) = rfd::FileDialog::new().set_directory(o.dir.trim()).pick_folder()
            {
                o.dir = d.display().to_string();
            }
            if theme::button(ui, Some(icon::OPEN), "開啟").clicked() {
                let _ = std::fs::create_dir_all(o.dir.trim());
                let _ = open::that(o.dir.trim());
            }
        });
    }
    ui.checkbox(&mut o.subfolder, "每場會議另建一個資料夾（名稱同檔名）");

    let more = |ui: &mut Ui, o: &mut ExportOptions| {
        let l = theme::label(ui, "檔名");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut o.filename).desired_width(240.0))
                .labelled_by(l.id);
            egui::ComboBox::from_id_salt(format!("{id}_name"))
                .selected_text("常用樣式")
                .width(150.0)
                .show_ui(ui, |ui| {
                    for preset in export::FILENAME_PRESETS {
                        ui.selectable_value(&mut o.filename, preset.to_string(), preset);
                    }
                })
                .response
                .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "常用檔名樣式"));
        });
        theme::hint(ui, &format!("可用 {}；日期、時間是開始轉錄的時間。", export::FILENAME_TOKENS));
        ui.add_space(6.0);

        let l = theme::label(ui, "Markdown、純文字的內容（Word、PDF 只放摘要）");
        egui::ComboBox::from_id_salt(format!("{id}_content"))
            .selected_text(o.content.label())
            .width(260.0)
            .show_ui(ui, |ui| {
                for c in Content::ALL {
                    ui.selectable_value(&mut o.content, c, c.label());
                }
            })
            .response
            .labelled_by(l.id);
        theme::hint(ui, "字幕、CSV、JSON 一律是逐字稿／完整資料。");
        ui.add_space(6.0);

        theme::label(ui, "逐字稿樣式");
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut o.timestamps, "時間戳");
            ui.checkbox(&mut o.speaker_names, "說話者名稱");
            ui.checkbox(&mut o.merge_turns, "同一人連續發言併成一段");
            ui.checkbox(&mut o.header, "開頭附檔案資訊與發言者統計");
        });
        ui.add_space(6.0);

        theme::label(ui, "同名檔案已存在時");
        ui.horizontal_wrapped(|ui| {
            if ui.radio(!o.overwrite, "另存成「名稱 (2)」").clicked() {
                o.overwrite = false;
            }
            if ui.radio(o.overwrite, "直接覆蓋").clicked() {
                o.overwrite = true;
            }
        });
    };
    if more_collapsed {
        egui::CollapsingHeader::new("檔名與內容")
            .id_salt(format!("{id}_more"))
            .default_open(false)
            .show(ui, |ui| more(ui, o));
    } else {
        ui.add_space(6.0);
        more(ui, o);
    }

    ui.add_space(6.0);
    preview(ui, o, project);
    *o != before
}

/// 「會存成」：第一個選定格式的完整路徑。
fn preview(ui: &mut Ui, o: &ExportOptions, project: Option<&Project>) {
    let p = pal(ui);
    let sample;
    let (proj, prefix) = match project {
        Some(pr) => (pr, "會存成"),
        None => {
            sample = Project::new(PathBuf::from(r"C:\錄音\產品週會.m4a"), Default::default());
            (&sample, "例：")
        }
    };
    let text = match export::registry().into_iter().find(|e| o.formats.iter().any(|f| f == e.id())) {
        Some(ex) => format!(
            "{prefix} {}",
            o.target_dir(proj)
                .join(format!("{}.{}", o.file_stem(proj), ex.extension()))
                .display()
        ),
        None => "還沒選任何格式".into(),
    };
    ui.horizontal(|ui| {
        ui.label(theme::ic(icon::DOCUMENT, 13.0).color(p.text_muted));
        ui.add(egui::Label::new(RichText::new(text).size(12.5).color(p.text_muted)).wrap());
    });
}

/// 圓點＋文字的狀態列（側欄底部的金鑰狀態）。
pub fn dot_line(ui: &mut Ui, ok: bool, text: &str) {
    let p = pal(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.spacing_mut().interact_size.y = 18.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), 3.5, if ok { p.success } else { p.text_muted });
        ui.label(RichText::new(text).size(12.0).color(p.text_muted));
    });
}
