//! 專案用的小元件：說話者顏色、狀態標籤、摘要方式切換。共用外觀在 [`crate::theme`]。

use crate::theme::{self, pal};
use egui::{Color32, RichText, Ui};
use meetscribe_core::config::{LlmProvider, Settings};
use meetscribe_core::model::ProjectStatus;

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

/// 摘要方式快速切換（免費為主，可手動改用本機 Claude Code CLI）。回傳是否有改。
pub fn summary_mode_picker(ui: &mut Ui, settings: &mut Settings, id: &str) -> bool {
    let current = settings.summary_provider;
    let label = |p: LlmProvider| match p {
        LlmProvider::Google => "免費模型（Gemini 免費額度）",
        LlmProvider::ClaudeCode => "本機 Claude Code CLI",
        LlmProvider::OpenRouter => "OpenRouter（自選模型）",
    };
    let mut chosen = current;
    let combo = egui::ComboBox::from_id_salt(id)
        .selected_text(label(current))
        .width(250.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut chosen, LlmProvider::Google, label(LlmProvider::Google))
                .on_hover_text("預設。需要 Google AI Studio Key；沒有的話自動改用 OpenRouter 上的免費模型");
            ui.selectable_value(&mut chosen, LlmProvider::ClaudeCode, label(LlmProvider::ClaudeCode))
                .on_hover_text("用你電腦上已登入的 Claude Code（claude -p）寫摘要，走你的 Claude 訂閱");
            if current == LlmProvider::OpenRouter {
                ui.selectable_value(&mut chosen, LlmProvider::OpenRouter, label(LlmProvider::OpenRouter));
            }
        });
    combo
        .response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "摘要方式"));
    if chosen != current {
        settings.summary_provider = chosen;
        settings.summary_model = chosen.default_summary_model().into();
        return true;
    }
    false
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
