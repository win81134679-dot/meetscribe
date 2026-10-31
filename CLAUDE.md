# 語音轉錄／MeetScribe（會議逐字稿＋摘要，Rust 桌面＋ Android，推送更新）

使用者看的說明在 `README.md`，設計決定與里程碑在 `PLAN.md`。這份只記改程式時查不到、會踩的事。

## Git

repo `win81134679-dot/meetscribe`（2026-10-30 建，分支 `main`；**2026-10-31 使用者選擇改成公開**，讓已安裝的程式不登入就抓得到 Release）。
因為是公開的：推之前除了祕密，也掃 email、本機路徑（`C:\Users\`）；`keys/`、`工具/devices.json`、`android/keystore*` 絕不進版控。這個資料夾是**獨立的 repo**，不是上層「一堆專案」repo 的一部分。
本 repo 的提交身分設在 local config：`win81134679-dot` ＋ GitHub noreply 信箱（全域沒設 user.email）。
`.gitattributes` 固定 LF，只有 `.cmd`、`.bat`（`android/gradlew.bat`）、`.url` 是 CRLF；`target/`、`dist/` 與 `android/` 的建置產物、簽章檔不進版控（`android/.gitignore`）。推之前掃一次祕密（`sk-or-`、`AIza`、`gh?_`）。

## 地圖

- `crates/meetscribe-core`：全部邏輯。`pipeline.rs` 串起五個階段（前處理→轉錄→說話者分配→整合→摘要），每階段完成就存檔、可續跑。
- `crates/meetscribe-app`：egui 桌面程式。`worker.rs` 自帶 tokio runtime，UI 只透過 `Cmd`／`Msg` 跟它講話；`views/` 每個畫面一檔，畫面上的動作先收成 `Action` 畫完再套用（避免同時借用 `MeetApp` 多個欄位）。
- `crates/meetscribe-cli`：同一個 core 的命令列入口。
- `crates/meetscribe-android`：Android 的 JNI 橋（`libmeetscribe_android.so`）。`bridge.rs` 平台無關、桌面可測；`lib.rs` 只做 JNI 轉接。
- `android/`：Kotlin＋Compose 的 App 外殼，所有邏輯都經 `core/Repo.kt` 呼叫 Rust。見下方〈Android〉。

名稱：使用者看得到的一律叫「語音轉錄」（2026-10-31 改名）。crate 名、`MeetScribe-*.exe/.apk` 檔名、Inno `AppId`、安裝資料夾 `MeetScribe`、
`%APPDATA%\MeetScribe`、Android applicationId／套件名、認證管理員的服務名 **都不改**：改了舊版升級會斷、資料與金鑰會找不到。

音訊經 `audio::AudioTool`：桌面是 ffmpeg，Android 是 `audio_native`（`native-audio` feature：symphonia 解碼、自寫重取樣與靜音偵測、LAME 編 MP3）。
兩份音訊（2026-10-31 使用者：「音訊格式是否可以改成 aac 高品質避免品質變差」）：`audio.mp3` 是**送轉錄**的 16 kHz 單聲道 64 kbps
（Nemotron 本來就以 16 kHz 處理，再高也聽不到；MP3 是 DeepInfra 確定收的格式，Android 的 Rust 端也只有 MP3 編碼器）；
`playback.*` 是**播放、核對**用的高品質副本（桌面 AAC 128 kbps 保留原取樣率；Android 直接保留原始檔），`Project::playback_audio()` 優先用它。
改 ffmpeg 那條路的行為（切段、正規化格式）時，兩個後端要一起改，`pipeline_e2e` 的兩個完整流程測試各驗一條。

外觀（2026-10-30 使用者：「UI 好醜，參考 Filee」）：`theme.rs` 照抄 Filee 上游 `Styles/Soft.axaml`／`SoftPalette.cs` 的 Soft 主題色票與尺寸
（暖灰頁 `#F6F5F1`、白卡片 16px 圓角、主色 `#7F77DD`、深色 `#1B1B19`），跟隨 Windows 深淺色。顏色一律從 `theme::pal(ui)` 取，
按鈕、卡片、導覽、分頁用 `theme::` 的元件，不要在畫面裡寫死色值。圖示是 Segoe Fluent Icons 字碼（`theme::icon`），新增前確認字型裡有。

使用者定的入口（2026-10-30）：程式一律從「新增轉錄」頁開始；沒 OpenRouter Key 時那頁頂端是三步驟設定卡（儲值 → 建立 Key → 貼上），
有 Key 後縮成帳戶列（餘額＋儲值、API Keys 按鈕；啟動時自動 `TestKey` 查餘額）。外部網址集中在 `theme::links`。
金鑰輸入框只有一份實作：`views::settings::key_box`，新增轉錄頁與設定頁共用同一組 `SettingsForm` 狀態。

發佈：`installer\build-installer.ps1`（測試 → release → Inno Setup）產出 `dist\MeetScribe-Setup-v<版本>.exe`；版本號在根 `Cargo.toml` 與三個 crate（app、cli 對 core 的相依版本也要一起改）。
安裝程式的 `AppId` 第一次發佈後不能再改。ffmpeg 不打包（完整版每個 exe 2 百多 MB），安裝最後一頁用 winget 補。1.0.0 於 2026-10-31 發佈到 GitHub Release；**1.1.0 同日用 `工具/publish.py` 發佈（給所有裝置）**，線上 update.json 驗簽、兩個安裝檔 SHA-256 已比對。
1.0.0 沒有更新功能，已裝 1.0.0 的電腦／手機要手動裝一次 1.1.0，之後才會自己更新。
建置時記憶體吃緊（可用虛擬記憶體不到 2 GB）連 `-j 1` 都會 OOM，請使用者先關程式，不要改 LTO 或另開 target 目錄硬編。
快速開啟：專案根目錄的 `開啟 語音轉錄.cmd`（`%~dp0` 相對路徑，不寫死中文路徑）指向 `target\release\meetscribe-app.exe`。
刻意不由 AI 建 `.lnk`：全域 CLAUDE.md〈防毒〉記過「claude.exe 建指向自編未簽章 exe 的捷徑」被 Bitdefender 判刑；要 `.lnk` 請使用者自己「傳送到桌面」。
程式開著時 exe 被鎖住，`cargo build --release` 會失敗——先關視窗再建。

擴充點：新供應商實作 `providers::Transcriber`／`LlmClient`，在 `Engine::from_settings` 接上；新匯出格式實作 `export::Exporter`，登記進 `export::registry()`
（會放摘要的格式讓 `has_summary()` 回 true，`ExportOptions` 的「內容」才對它有作用）。輸出選項的 UI 只有一份：`widgets::export_options`，設定頁、新增轉錄、匯出頁共用。提示詞在 `prompts.rs`，使用者可用 `%APPDATA%\MeetScribe\prompts\<名稱>.md` 覆寫——`attribution`／`reconcile` 的 JSON 格式是程式解析的契約，改提示詞時保持格式。

## 推送更新（2026-10-31）

使用者：「支援推送升級功能且每台裝置有識別碼可以獨立推送更新 電腦版可自動更新，手機版看你怎麼做」。做法照抄 Filee（`一堆專案\Filee 按住Ctrl拖移轉檔工具` 的 `core::self_update`＋`工具/publish.py`）。

- 檢查、下載、驗簽章、挑版本、識別碼、狀態檔都在 `meetscribe-core/src/update.rs`，電腦與手機共用；安裝各自做：`meetscribe-app/src/updater.rs`（Inno 靜默）、`android/.../core/Updater.kt`（PackageInstaller）。
- **簽章金鑰** `keys/update-signing.pem`＋`keys/update-signing-password.txt`（不進版控，靠每週完整備份）；公鑰 `packaging/update.pub` 以 `include_str!` 編進程式。
  **絕不重產**：換金鑰＝已安裝的每一台都收不到更新。`publish.py new-key` 在檔案已存在時會拒絕。
- 發佈：`build-installer.ps1`＋`android\build-apk.ps1` → `uv run 工具/publish.py publish --notes "…" [--devices …]` → 下載線上 update.json／sig 驗一次。
  版本號要比上一個 Release 大，否則裝好的程式不認為有新版。
- 安裝程式契約：`/SELFUPDATE=1` 時 `InitializeSetup` 等 mutex `MeetScribe-Running`（`updater::RUNNING_MUTEX`，程式啟動就建）消失；`/RELAUNCH=1` 時 `[Run]` 把程式開回來。改任一邊要一起改。
- 只在安裝版自動安裝（exe 旁有 `unins000.exe`）；`target\` 的開發版只檢查。**AI 不在這台執行下載來的安裝檔**（防毒規則）；實際靜默安裝留給使用者的電腦。
- 測試：`update::tests` 用 wiremock 假 GitHub＋固定測試金鑰；`tests/fixtures/python_signatures.json` 確認 Python `cryptography` 簽的（含 high-S）Rust 收得下。
  Android 實測：`MOCK_RELEASES=<資料夾> python -I android/test/mock_api.py …`＋ settings.json 的 `update_api`，新版 APK 用 `build-apk.ps1 -TestVersion 1.1.1` 產。

## 使用者定的費用分工（2026-10-30）

- 逐字稿：OpenRouter `nvidia/nemotron-3.5-transcribe`，**唯一付費**的部分。
- 說話者分配、整合、摘要：**預設免費**——Google AI Studio Gemini 免費額度；沒 Google Key 時 `Engine::from_settings` 退到 OpenRouter 的 `:free` 模型並寫 notice。
- 摘要可在 UI（新增轉錄表單、摘要頁）手動切成本機 Claude Code CLI。
- 新增任何預設模型都要維持「只有逐字稿收費」。
- 不做本機部署 Nemotron（vLLM 等）：使用者 GPU 記憶體不夠，逐字稿一律走雲端。

## 外部事實（會變，動相關程式前重查）

- 轉錄的 504／524 是 DeepInfra 排隊，**不是段落太長或內容問題**（2026-10-31 實測：同一段 10 分鐘音訊 20 秒成功、128 秒成功、181 秒 504 都出現過；
  多個請求在同一秒一起回來；OpenRouter 閘道約 180 秒切斷）。**逾時不收費**（對 `/key` 用量帳：幾十次逾時沒有計費）。
  所以 `pipeline::transcribe` 一輪輪原樣重送：出現逾時後改成一次一段、每輪多等 20 秒，6 輪後才停（可續跑）。曾經試過「逾時就對半切」，沒用，已拿掉。
  技術報告提到的「生成迴圈跑到 token 上限」也查過：temperature 0 下同一段時快時慢，不是主因。
  診斷工具：`cargo run -p meetscribe-core --example stt_probe -- <段落.mp3> verbose`（直接送 OpenRouter 量時間；`usage` 查金鑰用量），金鑰從認證管理員讀、不印出。
- **時間戳兩邊都不可信**（2026-10-31 使用者：「跟隨播放根本不準」，查一場 2.6 小時真實會議）：DeepInfra 以約 300 秒為一窗，
  沒有逐字時間的窗整窗回一段、最後一個不滿 300 秒的窗逐字時間被拉長到超出音訊結尾；說話者模型回的秒數常是假的等差數列（0,1,2… 或每句 +0.2）。
  所以所有時間都先過 `timing::plausible_anchors`／`provider_times_ok`，不可信就丟、改用 `timing::fill`（字數分配到「有聲音的時間」，靜音偵測 -35 dB／0.35 秒）。
  舊專案由 `pipeline::retime_project`（`Project.timing_rev`）在啟動時重新對齊一次。量測工具：`cargo run -p meetscribe-core --example retime -- <專案資料夾的複本>`。
  播放器本身是準的（`player::tests` 用嗶聲量過：MP3 誤差 0、AAC 固定晚 64 ms 是編碼器 priming）。
- OpenRouter 上 Nemotron 唯一供應商 DeepInfra：`supports_diarization: false`、不吃 prompt。所以說話者靠 Gemini 聽音訊分配；`stt_capabilities()` 查到支援時會自動改走原生 diarize。查法：`GET /api/v1/models/<id>/endpoints`。
- Gemini 模型名稱常改版：`gemini::pick_audio_model` 在設定的名字不存在時挑最新的穩定版 Flash，別把模型名寫死在別處。
- `claude -p`：不能用 `--bare`（只認 ANTHROPIC_API_KEY，訂閱登入會失敗）；用 `--setting-sources project`＋空工作目錄隔離使用者的 hook／plugin。`total_cost_usd` 是牌價換算，記為 0。

## 說話者分配的不變式

逐字稿文字只來自 STT。分配模型只回 `[句號, 說話者, 起始秒]`；新人用 `N#`，由 `attribution::map_to_global` 配全域 `S#`。漏標的句子沿用前一位、時間用 `fill_times`（最長非遞減子序列當錨點＋字數內插）。改這段時這條不變式要守住，否則逐字稿會漏字。

## 這台機器的建置坑

- 分頁檔小：rustc 會報 `os error 1455`／`STATUS_STACK_BUFFER_OVERRUN`。已用 `.cargo/config.toml` 的 `jobs = 3` 與 `[profile.dev] debug = 0` 壓住；別另開 `CARGO_TARGET_DIR`（整套重編會再撞記憶體）。
- reqwest 0.13 用 `native-tls`（`rustls` 預設走 aws-lc-rs 要 cmake）；`.query()` 要 `query` feature。
- 含反斜線或 `"#` 的補丁：用 Python 腳本檔（scratchpad），別用 Bash heredoc；Rust 原始字串內含 `"#` 時用 `r##"…"##`。

## Android

2026-10-31 使用者：「我要製作成 android APP」「幫我完整建置 Android APP 輸出成 APK」。發佈檔 `dist\MeetScribe-Android-v<版本>.apk`，版本號 Gradle 直接讀根 `Cargo.toml`。

- **建置一律走 `android\build-apk.ps1`**（正式版）或加 `-Debug -Abis arm64-v8a,x86_64`（模擬器用）。它用 `subst` 把專案掛成英數磁碟代號再建：
  AGP 拒絕中文路徑；NDK 的 `*-clang.cmd` 與 clang 交給 ld.lld 的回應檔都用 cp950 轉碼，中文路徑會變亂碼。`build-rust.ps1`（Gradle preBuild 會叫）同理，並直接叫 `clang.exe --target=…`。
- Rust 交叉編譯明確用 rustup 的 stable 工具鏈（`RUSTC` 指過去）：PATH 上先找到的是 MSI 版 rustc，沒有 Android 標準庫。
- `third_party/mp3lame-sys` 是改過 build.rs 的上游副本（根 `Cargo.toml` 的 `[patch.crates-io]`）：上游依「目標」挑 build-dependency，從 Windows 交叉編譯到 Android 拿不到 `cc`。
- `compileSdk = 37` 是 Compose BOM 2026.09 的要求；`targetSdk` 維持 36。Gradle 一律 `--no-daemon`（記憶體吃緊，daemon 也會佔住呼叫端的輸出管線）。
- 簽章：`android\keystore\meetscribe-release.jks`＋`android\keystore.properties`，不進版控、靠每週完整備份。遺失就不能再發佈同一個 App 的更新，不要重新產生。
- Kotlin ⇄ Rust 的契約：`bridge.rs` 的 op 名稱、`summary_view`／`full_view` 與事件 JSON 欄位，對應 `core/Models.kt`、`core/Repo.kt`；改一邊要改另一邊。
  JNI 函式名綁 `io.github.win81134679.meetscribe.core.Native`（`proguard-rules.pro` 保留它）。
- UI 慣例同桌面：顏色從 `Pal`／MaterialTheme 取（`ui/Theme.kt` 是桌面 `theme.rs` 的色票）；金鑰框只有 `ui/Components.kt` 的 `KeyBox` 一份；圖示是 `ui/Icons.kt` 的 Material 路徑（不引入 material-icons）。
- 手機沒有 Claude Code CLI，設定頁只給 Google／OpenRouter。
- 匯出預設在公開的「文件/語音轉錄」（`core/Storage.kt`；寫不進去才退回 App 的外部檔案區）。1.0 存的 `…/Android/data/…` 由 `bridge::migrate_export_dir` 換過去，使用者自選的資料夾不動。
  會議頁「開啟資料夾」用 `DocumentsContract` 的 externalstorage URI 開系統檔案 App（只對共用儲存空間有效）；分享經 FileProvider（`file_paths.xml` 開放 `Documents/`）。
- App 內錄音：`service/RecorderService.kt`（前景服務 microphone，AAC m4a 直接寫進 imports），錄完由 `Recording.finished` 帶進新增轉錄頁。
- 匯入檔放 `files/data/imports/<uuid>/<原檔名>`；完成或刪除專案時刪掉，App 啟動時清掉沒有專案在用的。

閉環測試（模擬器）：AVD `meetscribe_test`（API 36 x86_64）。主機跑 `python -I android/test/mock_api.py <資料夾> [延遲秒]`，用法在檔頭；
`android/test/ui.ps1` 用 uiautomator 依文字點擊、截圖。真的金鑰由使用者自己在 App 裡貼，測試流程不從認證管理員取出金鑰。
`adb push`／`/sdcard` 路徑用 PowerShell 跑（Git Bash 會把 `/sdcard` 改寫成 Windows 路徑）。

## 架構圖

`docs/架構圖/HTML/{執行架構圖,處理流程圖}.html`（diagram-design，eight-hue 配色；橘色只給「唯一付費」的 Nemotron）。
A2 橫式滿版：body 2245×1587、SVG viewBox 1280×904。改了 HTML 就重產：
`node 工具/render_diagram_a2.mjs docs/架構圖/HTML/<名稱>.html` → `PNG/`（7016×4959、pHYs 300 DPI）與 `PDF/`（向量單頁）。
需要本機 Chrome；渲染器、`cdp.mjs`、`set_png_dpi.py` 是從 Filee 專案複製來的。改了費用分工、流程階段或平台分工（電腦／Android 各用什麼），兩張圖要跟著改。
2026-10-31 起兩張圖都含 Android：App 節點是「電腦＋手機」同一個核心，音訊節點是「ffmpeg／內建音訊」，Claude Code 標「僅電腦版」。
1.1 起標題用「語音轉錄」，執行架構圖多一個 GitHub Releases（推送更新）節點。

## 驗證

```powershell
cargo clippy --workspace --all-targets      # 要 0 警告
cargo test --workspace                       # 單元＋端對端（真 ffmpeg＋wiremock 模擬 OpenRouter／Gemini）＋離屏 UI
cargo test -p meetscribe-core -p meetscribe-android --features meetscribe-core/native-audio   # Android 音訊後端＋JNI 橋
cargo clippy --workspace --all-targets --features meetscribe-core/native-audio                 # 也要 0 警告
```

- UI 看圖：`cargo test -p meetscribe-app` 會把 7 個畫面渲染到 `target/ui-shots/*.png`（egui_kittest，不開視窗）。它同時檢查缺字型與可及性標籤：新表單欄位要 `.labelled_by(label.id)` 或帶 `.text()`，新符號要在 Segoe UI Symbol 裡（表情符號會失敗）。
- 會花使用者額度的測試都標 `#[ignore]`：`claude_cli_live`（真的跑 `claude -p`）、`credential_store_roundtrip`（寫入認證管理員）。
- 啟動編好的 `meetscribe-app.exe` 前先問使用者（全域 CLAUDE.md 的防毒規則）；UI 驗證用上面的離屏渲染。
