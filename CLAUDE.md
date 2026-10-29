# MeetScribe（會議逐字稿＋摘要，Rust）

使用者看的說明在 `README.md`，設計決定與里程碑在 `PLAN.md`。這份只記改程式時查不到、會踩的事。

## 地圖

- `crates/meetscribe-core`：全部邏輯。`pipeline.rs` 串起五個階段（前處理→轉錄→說話者分配→整合→摘要），每階段完成就存檔、可續跑。
- `crates/meetscribe-app`：egui 桌面程式。`worker.rs` 自帶 tokio runtime，UI 只透過 `Cmd`／`Msg` 跟它講話；`views/` 每個畫面一檔，畫面上的動作先收成 `Action` 畫完再套用（避免同時借用 `MeetApp` 多個欄位）。
- `crates/meetscribe-cli`：同一個 core 的命令列入口。

外觀（2026-10-30 使用者：「UI 好醜，參考 Filee」）：`theme.rs` 照抄 Filee 上游 `Styles/Soft.axaml`／`SoftPalette.cs` 的 Soft 主題色票與尺寸
（暖灰頁 `#F6F5F1`、白卡片 16px 圓角、主色 `#7F77DD`、深色 `#1B1B19`），跟隨 Windows 深淺色。顏色一律從 `theme::pal(ui)` 取，
按鈕、卡片、導覽、分頁用 `theme::` 的元件，不要在畫面裡寫死色值。圖示是 Segoe Fluent Icons 字碼（`theme::icon`），新增前確認字型裡有。

使用者定的入口（2026-10-30）：程式一律從「新增轉錄」頁開始；沒 OpenRouter Key 時那頁頂端是三步驟設定卡（儲值 → 建立 Key → 貼上），
有 Key 後縮成帳戶列（餘額＋儲值、API Keys 按鈕；啟動時自動 `TestKey` 查餘額）。外部網址集中在 `theme::links`。
金鑰輸入框只有一份實作：`views::settings::key_box`，新增轉錄頁與設定頁共用同一組 `SettingsForm` 狀態。

快速開啟：專案根目錄的 `開啟 MeetScribe.cmd`（`%~dp0` 相對路徑，不寫死中文路徑）指向 `target\release\meetscribe-app.exe`。
刻意不由 AI 建 `.lnk`：全域 CLAUDE.md〈防毒〉記過「claude.exe 建指向自編未簽章 exe 的捷徑」被 Bitdefender 判刑；要 `.lnk` 請使用者自己「傳送到桌面」。
程式開著時 exe 被鎖住，`cargo build --release` 會失敗——先關視窗再建。

擴充點：新供應商實作 `providers::Transcriber`／`LlmClient`，在 `Engine::from_settings` 接上；新匯出格式實作 `export::Exporter`，登記進 `export::registry()`。提示詞在 `prompts.rs`，使用者可用 `%APPDATA%\MeetScribe\prompts\<名稱>.md` 覆寫——`attribution`／`reconcile` 的 JSON 格式是程式解析的契約，改提示詞時保持格式。

## 使用者定的費用分工（2026-10-30）

- 逐字稿：OpenRouter `nvidia/nemotron-3.5-transcribe`，**唯一付費**的部分。
- 說話者分配、整合、摘要：**預設免費**——Google AI Studio Gemini 免費額度；沒 Google Key 時 `Engine::from_settings` 退到 OpenRouter 的 `:free` 模型並寫 notice。
- 摘要可在 UI（新增轉錄表單、摘要頁）手動切成本機 Claude Code CLI。
- 新增任何預設模型都要維持「只有逐字稿收費」。
- 不做本機部署 Nemotron（vLLM 等）：使用者 GPU 記憶體不夠，逐字稿一律走雲端。

## 外部事實（會變，動相關程式前重查）

- OpenRouter 上 Nemotron 唯一供應商 DeepInfra：`supports_diarization: false`、不吃 prompt。所以說話者靠 Gemini 聽音訊分配；`stt_capabilities()` 查到支援時會自動改走原生 diarize。查法：`GET /api/v1/models/<id>/endpoints`。
- Gemini 模型名稱常改版：`gemini::pick_audio_model` 在設定的名字不存在時挑最新的穩定版 Flash，別把模型名寫死在別處。
- `claude -p`：不能用 `--bare`（只認 ANTHROPIC_API_KEY，訂閱登入會失敗）；用 `--setting-sources project`＋空工作目錄隔離使用者的 hook／plugin。`total_cost_usd` 是牌價換算，記為 0。

## 說話者分配的不變式

逐字稿文字只來自 STT。分配模型只回 `[句號, 說話者, 起始秒]`；新人用 `N#`，由 `attribution::map_to_global` 配全域 `S#`。漏標的句子沿用前一位、時間用 `fill_times`（最長非遞減子序列當錨點＋字數內插）。改這段時這條不變式要守住，否則逐字稿會漏字。

## 這台機器的建置坑

- 分頁檔小：rustc 會報 `os error 1455`／`STATUS_STACK_BUFFER_OVERRUN`。已用 `.cargo/config.toml` 的 `jobs = 3` 與 `[profile.dev] debug = 0` 壓住；別另開 `CARGO_TARGET_DIR`（整套重編會再撞記憶體）。
- reqwest 0.13 用 `native-tls`（`rustls` 預設走 aws-lc-rs 要 cmake）；`.query()` 要 `query` feature。
- 含反斜線或 `"#` 的補丁：用 Python 腳本檔（scratchpad），別用 Bash heredoc；Rust 原始字串內含 `"#` 時用 `r##"…"##`。

## 驗證

```powershell
cargo clippy --workspace --all-targets      # 要 0 警告
cargo test --workspace                       # 單元＋端對端（真 ffmpeg＋wiremock 模擬 OpenRouter／Gemini）＋離屏 UI
```

- UI 看圖：`cargo test -p meetscribe-app` 會把 7 個畫面渲染到 `target/ui-shots/*.png`（egui_kittest，不開視窗）。它同時檢查缺字型與可及性標籤：新表單欄位要 `.labelled_by(label.id)` 或帶 `.text()`，新符號要在 Segoe UI Symbol 裡（表情符號會失敗）。
- 會花使用者額度的測試都標 `#[ignore]`：`claude_cli_live`（真的跑 `claude -p`）、`credential_store_roundtrip`（寫入認證管理員）。
- 啟動編好的 `meetscribe-app.exe` 前先問使用者（全域 CLAUDE.md 的防毒規則）；UI 驗證用上面的離屏渲染。
