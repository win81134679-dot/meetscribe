//! 外觀：沿用 Filee（上游 `Styles/Soft.axaml`、`SoftPalette.cs`）的 Soft 主題——
//! 暖灰頁面、白色卡片 16px 圓角＋柔和陰影、薰衣草紫主色、跟隨 Windows 深淺色。
//! 字型：拉丁字 Segoe UI、中文 微軟正黑體 UI、粗體另一組、圖示用 Segoe Fluent Icons。

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Shadow, Stroke, Theme, Ui, Visuals};
use std::sync::Arc;

pub struct Pal {
    pub page: Color32,
    pub surface: Color32,
    pub surface_alt: Color32,
    pub input: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub accent_faint: Color32,
    pub accent_strong: Color32,
    pub on_accent: Color32,
    pub button: Color32,
    pub button_hover: Color32,
    pub button_press: Color32,
    pub button_border: Color32,
    pub button_border_hover: Color32,
    pub shadow: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}
const fn rgba(hex: u32, a: u8) -> Color32 {
    Color32::from_rgba_premultiplied(
        (((hex >> 16) & 0xFF) * a as u32 / 255) as u8,
        (((hex >> 8) & 0xFF) * a as u32 / 255) as u8,
        ((hex & 0xFF) * a as u32 / 255) as u8,
        a,
    )
}

pub const LIGHT: Pal = Pal {
    page: rgb(0xF6F5F1),
    surface: rgb(0xFFFFFF),
    surface_alt: rgb(0xEFEDE6),
    input: rgb(0xFBFAF8),
    border: rgba(0x000000, 0x16),
    text: rgb(0x1F1E1D),
    text_muted: rgb(0x6B6A66),
    danger: rgb(0xC73A39),
    success: rgb(0x0F7A5A),
    warning: rgb(0x9A5B0C),
    accent: rgb(0x7F77DD),
    accent_soft: rgba(0x7F77DD, 0x2E),
    accent_faint: rgba(0x7F77DD, 0x17),
    accent_strong: rgb(0x5F59A6),
    on_accent: rgb(0xFFFFFF),
    button: rgb(0xFFFFFF),
    button_hover: rgb(0xEFEDE6),
    button_press: rgb(0xE6E3DA),
    button_border: rgba(0x000000, 0x29),
    button_border_hover: rgba(0x000000, 0x3D),
    shadow: rgba(0x000000, 0x10),
};

pub const DARK: Pal = Pal {
    page: rgb(0x1B1B19),
    surface: rgb(0x262624),
    surface_alt: rgb(0x31312E),
    input: rgb(0x1F1F1D),
    border: rgba(0xFFFFFF, 0x1C),
    text: rgb(0xF1EFE8),
    text_muted: rgb(0xAEACA4),
    danger: rgb(0xF09595),
    success: rgb(0x5DCAA5),
    warning: rgb(0xEF9F27),
    accent: rgb(0x7F77DD),
    accent_soft: rgba(0x7F77DD, 0x2E),
    accent_faint: rgba(0x7F77DD, 0x17),
    accent_strong: rgb(0xBCAEE5),
    on_accent: rgb(0xFFFFFF),
    button: rgb(0x262624),
    button_hover: rgb(0x31312E),
    button_press: rgb(0x3A3A36),
    button_border: rgba(0xFFFFFF, 0x38),
    button_border_hover: rgba(0xFFFFFF, 0x4D),
    shadow: rgba(0x000000, 0x40),
};

pub fn pal(ui: &Ui) -> &'static Pal {
    if ui.visuals().dark_mode { &DARK } else { &LIGHT }
}

pub const RADIUS: u8 = 16;
pub const RADIUS_SMALL: u8 = 9;

/// Segoe Fluent Icons／Segoe MDL2 Assets 的字碼。
pub mod icon {
    pub const ADD: &str = "\u{E710}";
    pub const SETTINGS: &str = "\u{E713}";
    pub const MIC: &str = "\u{E720}";
    pub const PEOPLE: &str = "\u{E716}";
    pub const DOCUMENT: &str = "\u{E8A5}";
    pub const PLAY: &str = "\u{E768}";
    pub const PAUSE: &str = "\u{E769}";
    pub const STOP: &str = "\u{E71A}";
    pub const SEARCH: &str = "\u{E721}";
    pub const FOLDER: &str = "\u{E8B7}";
    pub const DELETE: &str = "\u{E74D}";
    pub const COPY: &str = "\u{E8C8}";
    pub const EDIT: &str = "\u{E70F}";
    pub const UPLOAD: &str = "\u{E898}";
    pub const DOWNLOAD: &str = "\u{E896}";
    pub const INFO: &str = "\u{E946}";
    pub const REFRESH: &str = "\u{E72C}";
    pub const CANCEL: &str = "\u{E711}";
    pub const CHECK: &str = "\u{E73E}";
    pub const CLOCK: &str = "\u{E823}";
    pub const WARNING: &str = "\u{E7BA}";
    pub const KEY: &str = "\u{E8D7}";
    pub const LIST: &str = "\u{E8FD}";
    pub const SPARKLE: &str = "\u{E945}";
    pub const OPEN: &str = "\u{E8A7}";
    pub const MONEY: &str = "\u{E8C7}";
}

/// 使用者設定金鑰、儲值會用到的外部頁面（未登入時對方會先導到登入頁再回來）。
pub mod links {
    pub const OPENROUTER_CREDITS: &str = "https://openrouter.ai/settings/credits";
    pub const OPENROUTER_KEYS: &str = "https://openrouter.ai/settings/keys";
    pub const GOOGLE_AI_STUDIO_KEYS: &str = "https://aistudio.google.com/apikey";
}

/// 一般按鈕樣式，按下用預設瀏覽器開網址。
pub fn link_button(ui: &mut Ui, glyph: &str, text: &str, url: &str) -> egui::Response {
    let r = button(ui, Some(glyph), &format!("{text} ↗")).on_hover_text(url);
    if r.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
    r
}

pub fn icons_family() -> FontFamily {
    FontFamily::Name("icons".into())
}
pub fn bold_family() -> FontFamily {
    FontFamily::Name("bold".into())
}

/// 圖示文字。
pub fn ic(glyph: &str, size: f32) -> RichText {
    RichText::new(glyph).family(icons_family()).size(size)
}

/// 粗體文字。
pub fn bold(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).family(bold_family()).size(size)
}

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.all_styles_mut(|style| {
        use egui::TextStyle;
        style.text_styles = [
            (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(24.0, bold_family())),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(14.0, 7.0);
        style.spacing.interact_size.y = 32.0;
        style.spacing.combo_height = 320.0;
        style.spacing.text_edit_width = 260.0;
        style.spacing.slider_width = 200.0;
        style.spacing.icon_width = 18.0;
        style.spacing.icon_width_inner = 10.0;
        style.spacing.menu_margin = Margin::same(6);
        style.spacing.window_margin = Margin::same(18);
    });
    ctx.set_visuals_of(Theme::Light, visuals(&LIGHT, false));
    ctx.set_visuals_of(Theme::Dark, visuals(&DARK, true));
}

fn visuals(p: &Pal, dark: bool) -> Visuals {
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    let small = CornerRadius::same(RADIUS_SMALL);
    v.panel_fill = p.page;
    v.window_fill = p.surface;
    v.extreme_bg_color = p.input;
    v.text_edit_bg_color = Some(p.input);
    v.faint_bg_color = if dark { rgb(0x2B2B28) } else { rgb(0xF8F7F4) };
    v.code_bg_color = p.surface_alt;
    v.hyperlink_color = p.accent_strong;
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.danger;
    v.weak_text_color = Some(p.text_muted);
    v.window_corner_radius = CornerRadius::same(RADIUS);
    v.menu_corner_radius = CornerRadius::same(12);
    v.window_stroke = Stroke::new(1.0, p.border);
    let shadow = Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: p.shadow,
    };
    v.window_shadow = shadow;
    v.popup_shadow = shadow;
    v.selection.bg_fill = p.accent_soft;
    v.selection.stroke = Stroke::new(1.0, p.accent_strong);
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);
    v.slider_trailing_fill = true;
    v.striped = false;
    v.indent_has_left_vline = false;
    v.collapsing_header_frame = false;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = small;

    w.inactive.bg_fill = p.surface_alt;
    w.inactive.weak_bg_fill = p.button;
    w.inactive.bg_stroke = Stroke::new(1.0, p.button_border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = small;
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = p.button_hover;
    w.hovered.weak_bg_fill = p.button_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.button_border_hover);
    w.hovered.fg_stroke = Stroke::new(1.5, p.text);
    w.hovered.corner_radius = small;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.button_press;
    w.active.weak_bg_fill = p.button_press;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.5, p.text);
    w.active.corner_radius = small;
    w.active.expansion = 0.0;

    w.open.bg_fill = p.surface_alt;
    w.open.weak_bg_fill = p.surface_alt;
    w.open.bg_stroke = Stroke::new(1.0, p.accent);
    w.open.fg_stroke = Stroke::new(1.0, p.text);
    w.open.corner_radius = small;
    v
}

fn install_fonts(ctx: &egui::Context) {
    let dir = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into())
        .join("Fonts");
    let mut fonts = egui::FontDefinitions::default();
    let mut add = |key: &str, file: &str, index: u32| -> bool {
        match std::fs::read(dir.join(file)) {
            Ok(bytes) => {
                let mut data = egui::FontData::from_owned(bytes);
                data.index = index;
                fonts.font_data.insert(key.into(), Arc::new(data));
                true
            }
            Err(_) => false,
        }
    };
    // msjh.ttc：0＝微軟正黑體、1＝微軟正黑體 UI（行距較緊，介面用）
    let regular: Vec<&str> = [
        ("segoe", "segoeui.ttf", 0),
        ("jhenghei", "msjh.ttc", 1),
        ("yahei", "msyh.ttc", 0),
        ("symbols", "seguisym.ttf", 0),
    ]
    .into_iter()
    .filter(|(k, f, i)| add(k, f, *i))
    .map(|(k, _, _)| k)
    .collect();
    let bold: Vec<&str> = [("segoe-bold", "seguisb.ttf", 0), ("jhenghei-bold", "msjhbd.ttc", 1)]
        .into_iter()
        .filter(|(k, f, i)| add(k, f, *i))
        .map(|(k, _, _)| k)
        .collect();
    let icons: Vec<&str> = [("fluent", "SegoeIcons.ttf", 0), ("mdl2", "segmdl2.ttf", 0)]
        .into_iter()
        .filter(|(k, f, i)| add(k, f, *i))
        .map(|(k, _, _)| k)
        .collect();
    if regular.is_empty() {
        tracing::warn!("找不到 Windows 字型，介面中文會顯示成方塊");
    }
    let prop = fonts.families.entry(FontFamily::Proportional).or_default();
    for (i, k) in regular.iter().enumerate() {
        prop.insert(i, k.to_string());
    }
    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    for k in regular.iter().skip(1) {
        mono.push(k.to_string());
    }
    let mut bold_chain: Vec<String> = bold.iter().map(|s| s.to_string()).collect();
    bold_chain.extend(regular.iter().map(|s| s.to_string()));
    fonts.families.insert(bold_family(), bold_chain);
    let mut icon_chain: Vec<String> = icons.iter().map(|s| s.to_string()).collect();
    icon_chain.extend(regular.iter().map(|s| s.to_string()));
    fonts.families.insert(icons_family(), icon_chain);
    ctx.set_fonts(fonts);
}

// ---------- 元件 ----------

/// 白色卡片（Filee `Border.card`）。
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = pal(ui);
    let r = egui::Frame::NONE
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(RADIUS)
        .inner_margin(Margin::symmetric(20, 18))
        .shadow(Shadow {
            offset: [0, 2],
            blur: 10,
            spread: 0,
            color: p.shadow,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner;
    ui.add_space(14.0);
    r
}

/// 帶標題的卡片。
pub fn section<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    card(ui, |ui| {
        ui.label(bold(title, 16.0));
        ui.add_space(6.0);
        add(ui)
    })
}

/// 頁面大標題＋副標。
pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.label(bold(title, 26.0));
    if !subtitle.is_empty() {
        ui.label(RichText::new(subtitle).color(pal(ui).text_muted));
    }
    ui.add_space(14.0);
}

/// 欄位標籤（放在輸入框上方，Filee `TextBlock.label`）。
pub fn label(ui: &mut Ui, text: &str) -> egui::Response {
    ui.label(RichText::new(text).size(12.5).color(pal(ui).text_muted))
}

/// 說明小字。
pub fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(pal(ui).text_muted));
}

/// 實心主按鈕。
pub fn primary(ui: &mut Ui, glyph: Option<&str>, text: &str, enabled: bool) -> egui::Response {
    let p = pal(ui);
    let mut job = egui::text::LayoutJob::default();
    let color = p.on_accent;
    if let Some(g) = glyph {
        job.append(
            g,
            0.0,
            egui::TextFormat {
                font_id: FontId::new(14.0, icons_family()),
                color,
                valign: egui::Align::Center,
                ..Default::default()
            },
        );
        job.append(
            "  ",
            0.0,
            egui::TextFormat {
                font_id: FontId::new(14.0, FontFamily::Proportional),
                color,
                ..Default::default()
            },
        );
    }
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: FontId::new(14.5, bold_family()),
            color,
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    let fill = if enabled {
        p.accent
    } else {
        p.accent.gamma_multiply(0.45)
    };
    ui.add_enabled(
        enabled,
        egui::Button::new(job)
            .fill(fill)
            .stroke(Stroke::NONE)
            .corner_radius(RADIUS_SMALL)
            .min_size(egui::vec2(0.0, 36.0)),
    )
}

/// 一般按鈕（白底細框），可帶圖示。
pub fn button(ui: &mut Ui, glyph: Option<&str>, text: &str) -> egui::Response {
    ui.add(egui::Button::new(icon_text(ui, glyph, text, None)))
}

/// 透明背景的按鈕（工具列、清單操作）。
pub fn ghost(ui: &mut Ui, glyph: Option<&str>, text: &str, color: Option<Color32>) -> egui::Response {
    ui.add(egui::Button::new(icon_text(ui, glyph, text, color)).frame(false))
}

pub fn icon_text(ui: &Ui, glyph: Option<&str>, text: &str, color: Option<Color32>) -> egui::text::LayoutJob {
    styled_text(ui, glyph, text, color, false)
}

fn styled_text(
    ui: &Ui,
    glyph: Option<&str>,
    text: &str,
    color: Option<Color32>,
    strong: bool,
) -> egui::text::LayoutJob {
    let color = color.unwrap_or(pal(ui).text);
    let family = if strong {
        bold_family()
    } else {
        FontFamily::Proportional
    };
    let mut job = egui::text::LayoutJob::default();
    if let Some(g) = glyph {
        job.append(
            g,
            0.0,
            egui::TextFormat {
                font_id: FontId::new(14.0, icons_family()),
                color,
                valign: egui::Align::Center,
                ..Default::default()
            },
        );
        if !text.is_empty() {
            job.append(
                "  ",
                0.0,
                egui::TextFormat {
                    font_id: FontId::new(14.0, FontFamily::Proportional),
                    color,
                    ..Default::default()
                },
            );
        }
    }
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: FontId::new(14.0, family),
            color,
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    job
}

/// 側欄導覽項（Filee `ListBox.nav`）：選中時淡紫底、主色字。
pub fn nav_item(ui: &mut Ui, glyph: &str, text: &str, selected: bool) -> egui::Response {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::click());
    let fill = if selected {
        p.accent_soft
    } else if resp.hovered() {
        p.accent_faint
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, RADIUS_SMALL, fill);
    let color = if selected { p.accent_strong } else { p.text };
    let y = rect.center().y;
    ui.painter().text(
        egui::pos2(rect.left() + 14.0, y),
        egui::Align2::LEFT_CENTER,
        glyph,
        FontId::new(16.0, icons_family()),
        color,
    );
    let font = if selected {
        FontId::new(14.5, bold_family())
    } else {
        FontId::new(14.5, FontFamily::Proportional)
    };
    ui.painter().text(
        egui::pos2(rect.left() + 44.0, y),
        egui::Align2::LEFT_CENTER,
        text,
        font,
        color,
    );
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, text));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 分頁切換（膠囊樣式）。
pub fn tab<T: PartialEq + Copy>(ui: &mut Ui, current: &mut T, value: T, glyph: &str, text: &str) {
    let p = pal(ui);
    let selected = *current == value;
    let color = if selected { p.accent_strong } else { p.text_muted };
    let job = styled_text(ui, Some(glyph), text, Some(color), selected);
    let fill = if selected { p.accent_soft } else { Color32::TRANSPARENT };
    let r = ui.add(
        egui::Button::new(job)
            .fill(fill)
            .stroke(Stroke::NONE)
            .corner_radius(99)
            .min_size(egui::vec2(0.0, 34.0)),
    );
    if r.clicked() {
        *current = value;
    }
}

/// 小膠囊標籤（狀態、推測）。
pub fn badge(ui: &mut Ui, text: &str, fg: Color32) -> egui::Response {
    egui::Frame::NONE
        .fill(fg.gamma_multiply(0.14))
        .corner_radius(99)
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| ui.label(RichText::new(text).size(12.0).color(fg)))
        .response
}

/// 圓形底的大圖示（拖放區用）。
pub fn bubble_icon(ui: &mut Ui, glyph: &str) {
    let p = pal(ui);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(64.0, 64.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 32.0, p.accent_soft);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        FontId::new(26.0, icons_family()),
        p.accent_strong,
    );
}
