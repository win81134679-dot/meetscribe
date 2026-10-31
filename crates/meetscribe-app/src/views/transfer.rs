//! 傳到其他裝置（2026-10-31）：設定頁的「我的裝置」、配對、傳送對話框、收件詢問、進度。
//! 流程跟手機版 `ui/TransferUi.kt` 一樣；邏輯在 worker（[`TransferCmd`]）與 core 的 `transfer.rs`。

use crate::app::{MeetApp, View};
use crate::theme::{self, icon, pal};
use crate::worker::{Cmd, ReceivedView, SendWhat, TransferCmd, TransferJob, TransferMsg, Worker};
use egui::{Margin, RichText, Ui};
use meetscribe_core::transfer::{Inbox, Notice, PairRequest, PairResult, Peer};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// 最多配對幾台（2026-10-31 使用者確認）。
const MAX_PEERS: usize = 5;
/// 配對碼的有效時間。
const PAIR_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
pub struct TransferUi {
    pub enabled: bool,
    pub device_id: String,
    pub name: String,
    pub peers: Vec<Peer>,
    pub inbox: Inbox,
    pub jobs: BTreeMap<String, TransferJob>,
    /// 我按了「接受」、等對方輸入配對碼的請求（以對方識別碼為鍵）。配對完成或 5 分鐘後拿掉。
    accepted: HashMap<String, (PairRequest, Instant)>,
    /// 處理過或按了「稍後」的配對請求與收件（這次開程式不再問）。
    handled: HashSet<String>,
    pub last_error: Option<String>,
    name_buf: Option<String>,
    pairing: Option<Pairing>,
    sending: Option<Sending>,
    received: Option<ReceivedView>,
    confirm_unpair: Option<Peer>,
}

/// 這台發起的配對：輸入對方識別碼 → 等對方接受 → 輸入對方畫面上的配對碼。
#[derive(Default)]
struct Pairing {
    target: String,
    req: Option<String>,
    code: String,
    note: Option<String>,
    sent_at: Option<Instant>,
}

struct Sending {
    what: SendWhat,
    title: String,
    peer: Option<String>,
}

impl TransferUi {
    /// 「配對新裝置」對話框。
    pub fn open_pairing(&mut self) {
        self.pairing = Some(Pairing::default());
    }

    /// 會議頁、匯出檔的「傳到其他裝置」。
    pub fn open_send(&mut self, what: SendWhat, title: String) {
        let peer = (self.peers.len() == 1).then(|| self.peers[0].id.clone());
        self.sending = Some(Sending { what, title, peer });
    }
}

pub fn handle(app: &mut MeetApp, m: TransferMsg) {
    let t = &mut app.transfer;
    let mut toasts: Vec<(String, bool)> = Vec::new();
    let mut keys_synced = false;
    match m {
        TransferMsg::State { enabled, device_id, name, peers } => {
            t.accepted.retain(|from, _| peers.iter().all(|p| p.id != *from));
            (t.enabled, t.device_id, t.name, t.peers) = (enabled, device_id, name, peers);
        }
        TransferMsg::Inbox(inbox) => {
            t.last_error = None;
            for k in &inbox.keys_synced {
                toasts.push((format!("已從「{}」同步 API 金鑰：{}", k.peer_name, k.title), false));
            }
            if !inbox.keys_synced.is_empty() {
                keys_synced = true;
            }
            for n in &inbox.notices {
                let name_of = |id: &str| t.peers.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or(id.to_string());
                let text = match n {
                    Notice::Paired { peer } => {
                        t.accepted.remove(peer);
                        format!("已跟「{}」配對", name_of(peer))
                    }
                    Notice::Unpaired { peer } => format!("「{}」取消了配對", name_of(peer)),
                    Notice::PairCancelled { peer } => format!("「{}」取消了配對請求", name_of(peer)),
                    Notice::Received { peer, .. } => format!("「{}」已收到", name_of(peer)),
                    Notice::Declined { peer, .. } => format!("「{}」拒收了", name_of(peer)),
                };
                toasts.push((text, false));
            }
            let peers = &t.peers;
            t.accepted.retain(|from, (_, at)| peers.iter().all(|p| p.id != *from) && at.elapsed() < PAIR_TTL);
            t.inbox = *inbox;
        }
        TransferMsg::PairStarted { id, to } => {
            if let Some(p) = t.pairing.as_mut().filter(|p| p.target.trim().eq_ignore_ascii_case(to.trim())) {
                p.req = Some(id);
                p.note = None;
            }
        }
        TransferMsg::PairResult(result) => match result {
            PairResult::Paired => {
                let who = t.pairing.as_ref().map(|p| p.target.clone()).unwrap_or_default();
                t.pairing = None;
                toasts.push((format!("配對完成（{who}）"), false));
            }
            PairResult::Wrong { left } => {
                if let Some(p) = t.pairing.as_mut() {
                    p.code.clear();
                    p.note = Some(format!("配對碼不對，還可以試 {left} 次"));
                }
            }
            PairResult::Cancelled => {
                if let Some(p) = t.pairing.as_mut() {
                    p.req = None;
                    p.code.clear();
                    p.sent_at = None;
                    p.note = Some("錯太多次，這次配對作廢了。請重新送出配對請求。".into());
                }
            }
        },
        TransferMsg::Progress { key, job } => {
            t.jobs.insert(key, job);
        }
        TransferMsg::Ended { key } => {
            t.jobs.remove(&key);
        }
        TransferMsg::Received(v) => t.received = Some(*v),
        TransferMsg::Error(e) => t.last_error = Some(e),
    }
    for (text, error) in toasts {
        app.toast(text, error);
    }
    if keys_synced {
        app.refresh_key_flags();
        app.settings_form.or_status = None;
        app.settings_form.g_status = None;
        if app.has_openrouter_key {
            app.worker.send(Cmd::TestKey(meetscribe_core::secrets::KeyKind::OpenRouter));
        }
    }
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

/// 「123456」→「123 456」。
fn spaced(code: &str) -> String {
    if code.len() == 6 { format!("{} {}", &code[..3], &code[3..]) } else { code.to_string() }
}

/// 設定頁的「傳到其他裝置」。回傳要存的新名稱。
pub fn devices_section(ui: &mut Ui, t: &mut TransferUi, worker: &Worker) -> Option<String> {
    let p = pal(ui);
    let mut rename = None;
    theme::hint(
        ui,
        "會議、匯出檔加密後經中轉站傳到你自己的其他裝置（電腦或手機），只有對方解得開；對方收完線上馬上刪除，沒收的 3 小時後自動刪除。",
    );
    if !t.enabled {
        theme::hint(ui, "中轉站尚未啟用。");
        return None;
    }
    ui.add_space(6.0);
    let l = theme::label(ui, "這台的名稱（配對的裝置上看到的）");
    ui.horizontal(|ui| {
        let buf = t.name_buf.get_or_insert_with(|| t.name.clone());
        ui.add(egui::TextEdit::singleline(buf).desired_width(260.0)).labelled_by(l.id);
        let changed = buf.trim() != t.name && !buf.trim().is_empty();
        if theme::primary(ui, Some(icon::CHECK), "儲存名稱", changed).clicked() {
            rename = Some(buf.trim().to_string());
            t.name_buf = None;
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("本機識別碼");
        ui.label(theme::bold(&t.device_id, 15.0).monospace());
        if theme::ghost(ui, Some(icon::COPY), "複製", Some(p.accent_strong)).clicked() {
            ui.ctx().copy_text(t.device_id.clone());
        }
    });
    if let Some(e) = &t.last_error {
        ui.label(RichText::new(format!("連不上中轉站：{e}")).color(p.danger));
    }
    ui.add_space(8.0);
    ui.label(theme::bold(format!("配對的裝置（最多 {MAX_PEERS} 台）"), 14.0));
    if t.peers.is_empty() {
        theme::hint(ui, "還沒有配對的裝置。在另一台的設定頁看它的識別碼，再按「配對新裝置」輸入。");
    }
    for peer in &t.peers {
        ui.horizontal(|ui| {
            ui.label(theme::ic(icon::DEVICES, 15.0).color(p.text_muted));
            ui.label(theme::bold(&peer.name, 14.0));
            ui.label(
                RichText::new(format!("{}　{} 配對", peer.id, peer.paired_at.format("%Y-%m-%d")))
                    .size(12.0)
                    .color(p.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::ghost(ui, None, "取消配對", Some(p.danger)).clicked() {
                    t.confirm_unpair = Some(peer.clone());
                }
            });
        });
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if theme::primary(ui, Some(icon::ADD), "配對新裝置", t.peers.len() < MAX_PEERS).clicked() {
            t.open_pairing();
        }
        if theme::button(ui, Some(icon::REFRESH), "檢查收件").clicked() {
            worker.send(Cmd::Transfer(TransferCmd::Poll));
        }
        if !t.peers.is_empty()
            && theme::button(ui, Some(icon::KEY), "立即同步金鑰")
                .on_hover_text("把這台的 API 金鑰推到所有配對的裝置（取代對方的）")
                .clicked()
        {
            worker.send(Cmd::Transfer(TransferCmd::SyncKeys { manual: true }));
        }
    });
    theme::hint(
        ui,
        "配對的裝置之間會自動同步 API 金鑰：剛配對時互相補上對方沒有的，之後在任一台改了金鑰就推到其他台。一樣加密傳送、只給配對過的裝置。",
    );
    rename
}

/// 置中的對話框（同刪除會議的樣式）。
fn dialog(ctx: &egui::Context, id: &str, add: impl FnOnce(&mut Ui)) {
    egui::Window::new(id)
        .id(egui::Id::unique(("transfer-dialog", id)))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(400.0);
            add(ui);
        });
}

fn big_code(ui: &mut Ui, code: &str) {
    ui.add_space(4.0);
    ui.label(theme::bold(spaced(code), 40.0).monospace().color(pal(ui).text));
    ui.add_space(4.0);
}

fn danger_button(ui: &mut Ui, text: &str) -> egui::Response {
    let p = pal(ui);
    ui.add(
        egui::Button::new(theme::bold(text, 14.0).color(egui::Color32::WHITE))
            .fill(p.danger)
            .stroke(egui::Stroke::NONE)
            .min_size(egui::vec2(0.0, 34.0)),
    )
}

/// 每一幀畫：一次只開一個對話框（取消配對 > 配對 > 傳送 > 別台的配對請求 > 顯示配對碼 > 收件 > 收到了），再畫進度。
pub fn overlays(app: &mut MeetApp, ctx: &egui::Context) {
    let mut open_project: Option<Uuid> = None;
    {
        let projects: HashSet<Uuid> = app.projects.iter().map(|p| p.id).collect();
        let t = &mut app.transfer;
        let w = &app.worker;
        let send = |c: TransferCmd| w.send(Cmd::Transfer(c));
        if let Some(peer) = t.confirm_unpair.clone() {
            dialog(ctx, "取消配對", |ui| {
                ui.label(theme::bold(format!("取消跟「{}」的配對？", peer.name), 18.0));
                theme::hint(ui, "兩邊都會移除對方；伺服器上兩台之間還沒收的東西一起刪除。之後要互傳得重新配對。");
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if danger_button(ui, "取消配對").clicked() {
                        send(TransferCmd::Unpair(peer.id.clone()));
                        t.confirm_unpair = None;
                    }
                    if theme::button(ui, None, "不要").clicked() {
                        t.confirm_unpair = None;
                    }
                });
            });
        } else if t.pairing.is_some() {
            pairing_dialog(ctx, t, &send);
        } else if t.sending.is_some() {
            send_dialog(ctx, t, &send);
        } else if let Some(r) = t
            .inbox
            .pair_requests
            .iter()
            .find(|r| !t.accepted.contains_key(&r.from) && !t.handled.contains(&r.id))
            .cloned()
        {
            dialog(ctx, "配對請求", |ui| {
                ui.label(theme::bold(format!("「{}」想跟這台配對", r.from_name), 18.0));
                ui.label(format!("識別碼 {}。是你自己的裝置才接受；接受後要在那台輸入這組配對碼：", r.from));
                big_code(ui, &r.code);
                theme::hint(ui, "配對後它可以把東西傳到這台，這台也能傳給它。");
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if theme::primary(ui, Some(icon::CHECK), "接受", true).clicked() {
                        send(TransferCmd::PairAccept(r.id.clone()));
                        t.accepted.insert(r.from.clone(), (r.clone(), Instant::now()));
                    }
                    if theme::ghost(ui, None, "拒絕", Some(pal(ui).danger)).clicked() {
                        send(TransferCmd::PairDecline(r.id.clone()));
                        t.handled.insert(r.id.clone());
                    }
                });
            });
        } else if let Some((r, _)) = t.accepted.values().find(|(r, _)| !t.handled.contains(&r.id)).cloned() {
            dialog(ctx, "輸入配對碼", |ui| {
                ui.label(theme::bold(format!("在「{}」輸入配對碼", r.from_name), 18.0));
                big_code(ui, &r.code);
                ui.horizontal(|ui| {
                    ui.spinner();
                    theme::hint(ui, "對方輸入後這個視窗會自己關（5 分鐘內有效）。");
                });
                ui.add_space(12.0);
                if theme::button(ui, None, "關閉").clicked() {
                    t.handled.insert(r.id.clone());
                }
            });
        } else if let Some(item) = t
            .inbox
            .incoming
            .iter()
            .find(|i| !t.handled.contains(&i.id) && !t.jobs.contains_key(&i.id))
            .cloned()
        {
            let exists = item.meta.project_id.is_some_and(|id| projects.contains(&id));
            dialog(ctx, "收件", |ui| {
                ui.label(theme::bold(format!("「{}」傳來{}", item.from_name, item.meta.kind.label()), 18.0));
                ui.label(format!("「{}」　{}", item.meta.title, mb(item.meta.total)));
                let left = item.expires_in_secs.max(0);
                theme::hint(
                    ui,
                    &format!("{} 小時 {} 分內有效；不收的話時間到會自動刪除。", left / 3600, left % 3600 / 60),
                );
                if exists {
                    ui.label("這台已經有這場會議：要取代，還是另存一份？");
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let mut take = |replace: bool| {
                        send(TransferCmd::Receive { id: item.id.clone(), replace });
                        t.handled.insert(item.id.clone());
                    };
                    if exists {
                        if theme::primary(ui, None, "取代", true).clicked() {
                            take(true);
                        }
                        if theme::button(ui, None, "另存一份").clicked() {
                            take(false);
                        }
                    } else if theme::primary(ui, Some(icon::DOWNLOAD), "接收", true).clicked() {
                        take(false);
                    }
                    if theme::ghost(ui, None, "拒收", Some(pal(ui).danger)).clicked() {
                        send(TransferCmd::Decline(item.id.clone()));
                        t.handled.insert(item.id.clone());
                    }
                    if theme::ghost(ui, None, "稍後", None).clicked() {
                        t.handled.insert(item.id.clone());
                    }
                });
            });
        } else if let Some(v) = t.received.clone() {
            dialog(ctx, "已收到", |ui| {
                ui.label(theme::bold(format!("已收到「{}」", v.title), 18.0));
                theme::hint(ui, &format!("來自「{}」", v.peer_name));
                for f in &v.paths {
                    ui.label(RichText::new(f.display().to_string()).size(12.0));
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if let Some(p) = &v.project {
                        if theme::primary(ui, Some(icon::OPEN), "開啟會議", true).clicked() {
                            open_project = Some(p.id);
                            t.received = None;
                        }
                    } else if let Some(dir) = v.paths.first().and_then(|f| f.parent())
                        && theme::primary(ui, Some(icon::FOLDER), "開啟資料夾", true).clicked()
                    {
                        let _ = open::that(dir);
                        t.received = None;
                    }
                    if theme::button(ui, None, "關閉").clicked() {
                        t.received = None;
                    }
                });
            });
        }
        progress_area(ctx, t, &send);
        if t.pairing.is_some() || !t.accepted.is_empty() || !t.jobs.is_empty() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
    if let Some(id) = open_project {
        app.save_now();
        app.selected = Some(id);
        app.view = View::Project;
        app.project_ui.reset();
    }
}

fn pairing_dialog(ctx: &egui::Context, t: &mut TransferUi, send: &dyn Fn(TransferCmd)) {
    let Some(pr) = t.pairing.as_mut() else { return };
    let mine = pr.req.as_ref().and_then(|id| t.inbox.my_pair_requests.iter().find(|r| r.id == *id)).cloned();
    let mut close = false;
    dialog(ctx, "配對新裝置", |ui| {
        let p = pal(ui);
        ui.label(theme::bold("配對新裝置", 18.0));
        match (&pr.req, mine.as_ref().map(|r| r.status.as_str())) {
            (None, _) => {
                theme::hint(ui, "在另一台的「設定 → 傳到其他裝置」看它的識別碼，輸入在這裡：");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut pr.target)
                        .hint_text("例如 HPPT-5ZKF")
                        .font(egui::FontId::monospace(18.0))
                        .desired_width(220.0),
                );
                r.on_hover_text("對方的識別碼");
                let waiting = pr.sent_at.is_some_and(|t| t.elapsed() < Duration::from_secs(8));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let ok = pr.target.trim().len() >= 9 && !waiting;
                    if theme::primary(ui, Some(icon::SEND), "送出配對請求", ok).clicked() {
                        pr.target = pr.target.trim().to_uppercase();
                        pr.sent_at = Some(Instant::now());
                        pr.note = None;
                        send(TransferCmd::PairStart(pr.target.clone()));
                    }
                    if waiting {
                        ui.spinner();
                    }
                    if theme::button(ui, None, "取消").clicked() {
                        close = true;
                    }
                });
            }
            (Some(_), Some("accepted")) => {
                let who = mine.as_ref().map(|r| r.to_name.clone()).unwrap_or_default();
                ui.label(format!("對方已接受。輸入「{who}」畫面上的 6 位數配對碼："));
                ui.add(
                    egui::TextEdit::singleline(&mut pr.code)
                        .hint_text("000000")
                        .font(egui::FontId::monospace(24.0))
                        .char_limit(7)
                        .desired_width(160.0),
                )
                .on_hover_text("配對碼");
                theme::hint(ui, "兩邊的碼對得上，才代表中間沒有人掉包。錯 3 次這次配對作廢。");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let digits: String = pr.code.chars().filter(char::is_ascii_digit).collect();
                    if theme::primary(ui, Some(icon::CHECK), "確認", digits.len() == 6).clicked()
                        && let Some(id) = &pr.req
                    {
                        send(TransferCmd::PairConfirm { id: id.clone(), code: digits });
                    }
                    if theme::button(ui, None, "取消").clicked() {
                        if let Some(id) = &pr.req {
                            send(TransferCmd::PairCancel(id.clone()));
                        }
                        close = true;
                    }
                });
            }
            (Some(_), Some("declined")) => {
                ui.label(RichText::new("對方拒絕了這次配對。").color(p.danger));
                ui.add_space(10.0);
                if theme::button(ui, None, "關閉").clicked() {
                    close = true;
                }
            }
            (Some(id), _) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("已送出。請在「{}」按「接受」…", pr.target));
                });
                theme::hint(ui, "對方的程式開著時 15 秒內會跳出請求；也可以在對方的設定頁按「檢查收件」。");
                ui.add_space(10.0);
                if theme::button(ui, None, "取消").clicked() {
                    send(TransferCmd::PairCancel(id.clone()));
                    close = true;
                }
            }
        }
        if let Some(n) = &pr.note {
            ui.label(RichText::new(n).color(p.danger));
        }
    });
    if close {
        t.pairing = None;
    }
}

fn send_dialog(ctx: &egui::Context, t: &mut TransferUi, send: &dyn Fn(TransferCmd)) {
    let Some(s) = t.sending.as_mut() else { return };
    let mut close = false;
    dialog(ctx, "傳到其他裝置", |ui| {
        ui.label(theme::bold("傳到其他裝置", 18.0));
        ui.label(format!("「{}」", s.title));
        ui.add_space(6.0);
        if !t.enabled {
            theme::hint(ui, "中轉站尚未啟用。");
        } else if t.peers.is_empty() {
            theme::hint(ui, "還沒有配對的裝置：到「設定 → 傳到其他裝置」按「配對新裝置」。");
        } else {
            for peer in &t.peers {
                ui.radio_value(&mut s.peer, Some(peer.id.clone()), format!("{}（{}）", peer.name, peer.id));
            }
            theme::hint(ui, "在這台加密後才上傳，只有對方解得開；對方收完線上馬上刪除，沒收的 3 小時後自動刪除。");
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if theme::primary(ui, Some(icon::SEND), "傳送", s.peer.is_some()).clicked()
                && let Some(peer) = s.peer.clone()
            {
                send(TransferCmd::Send { peer, what: s.what.clone() });
                close = true;
            }
            if theme::button(ui, None, "取消").clicked() {
                close = true;
            }
        });
    });
    if close {
        t.sending = None;
    }
}

/// 右上角：進行中的傳送與接收。
fn progress_area(ctx: &egui::Context, t: &TransferUi, send: &dyn Fn(TransferCmd)) {
    if t.jobs.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::unique("transfer-progress"))
        .anchor(egui::Align2::RIGHT_TOP, [-20.0, 20.0])
        .show(ctx, |ui| {
            let p = pal(ui);
            for (key, j) in &t.jobs {
                egui::Frame::popup(ui.style())
                    .corner_radius(12)
                    .inner_margin(Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_width(320.0);
                        let what = if j.send {
                            format!("傳送「{}」給「{}」", j.title, j.peer_name)
                        } else {
                            format!("從「{}」接收「{}」", j.peer_name, j.title)
                        };
                        ui.add(egui::Label::new(RichText::new(what).color(p.text)).truncate());
                        let f = if j.total > 0 { j.done as f32 / j.total as f32 } else { 0.0 };
                        ui.add(
                            egui::ProgressBar::new(f)
                                .desired_height(6.0)
                                .fill(p.accent)
                                .corner_radius(99),
                        );
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{} / {}", mb(j.done), mb(j.total))).size(12.0).color(p.text_muted));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if theme::ghost(ui, None, "取消", Some(p.danger)).clicked() {
                                    send(TransferCmd::Cancel(key.clone()));
                                }
                            });
                        });
                    });
                ui.add_space(8.0);
            }
        });
}
