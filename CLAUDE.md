# 語音轉錄／MeetScribe（會議逐字稿＋摘要，Rust 桌面＋ Android，推送更新）

使用者看的說明在 `README.md`，設計決定與里程碑在 `PLAN.md`。這份只記改程式時查不到、會踩的事。

## Git

repo `win81134679-dot/meetscribe`（2026-10-30 建，分支 `main`；**2026-10-31 使用者選擇改成公開**，讓已安裝的程式不登入就抓得到 Release）。
因為是公開的：推之前除了祕密，也掃 email、本機路徑（`C:\Users\`）；`keys/`、`工具/devices.json`、`android/keystore*` 絕不進版控。這個資料夾是**獨立的 repo**，不是上層「一堆專案」repo 的一部分。
本 repo 的提交身分設在 local config：`win81134679-dot` ＋ GitHub noreply 信箱（全域沒設 user.email）。
`.gitattributes` 固定 LF，只有 `.cmd`、`.bat`（`android/gradlew.bat`）、`.url` 是 CRLF；`target/`、`dist/` 與 `android/` 的建置產物、簽章檔不進版控（`android/.gitignore`）。推之前掃一次祕密（`sk-or-`、`AIza`、`gh?_`）。

## Y: 備份

2026-10-31 使用者：「做完備份到Y:」。NAS 的 `Y:\5. NVIDIA Nemotron 3.5 Transcribe 逐字稿轉錄模型`（Y: 的資料夾照編號，Filee 是 4）。
發佈新版或動到簽章檔之後跑一次（結束碼 0–3 是成功、≥8 才是失敗）：

```powershell
$src = "$PWD"; robocopy $src "Y:\5. NVIDIA Nemotron 3.5 Transcribe 逐字稿轉錄模型" /MIR /XD "$src\target" "$src\android\.gradle" "$src\android\.kotlin" "$src\android\build" "$src\android\app\build" "$src\android\app\src\main\jniLibs" "$src\relay\node_modules" "$src\relay\.wrangler" /R:2 /W:3 /NFL /NDL /NP /XJ
```

不備份的都能重建。`keys\`（更新簽章）與 `android\keystore*`（APK 簽章）的另一份就在 Y:；**只能本機 → Y:，絕不拿 Y: 那份蓋回本機**。

## 地圖

- `crates/meetscribe-core`：全部邏輯。`pipeline.rs` 串起五個階段（前處理→轉錄→說話者分配→整合→摘要），每階段完成就存檔、可續跑。
- `crates/meetscribe-app`：egui 桌面程式。`worker.rs` 自帶 tokio runtime，UI 只透過 `Cmd`／`Msg` 跟它講話；`views/` 每個畫面一檔，畫面上的動作先收成 `Action` 畫完再套用（避免同時借用 `MeetApp` 多個欄位）。
- `crates/meetscribe-cli`：同一個 core 的命令列入口。
- `crates/meetscribe-android`：Android 的 JNI 橋（`libmeetscribe_android.so`）。`bridge.rs` 平台無關、桌面可測；`recordings.rs` 是錄音庫；`lib.rs` 只做 JNI 轉接。
- `android/`：Kotlin＋Compose 的 App 外殼，所有邏輯都經 `core/Repo.kt` 呼叫 Rust。見下方〈Android〉。

名稱：使用者看得到的一律叫「語音轉錄」（2026-10-31 改名）。crate 名、`MeetScribe-*.exe/.apk` 檔名、Inno `AppId`、安裝資料夾 `MeetScribe`、
`%APPDATA%\MeetScribe`、Android applicationId／套件名、認證管理員的服務名 **都不改**：改了舊版升級會斷、資料與金鑰會找不到。

音訊經 `audio::AudioTool`：桌面是 ffmpeg，Android 是 `audio_native`（`native-audio` feature：symphonia 解碼、自寫重取樣與靜音偵測、LAME 編 MP3）。
兩份音訊（2026-10-31 使用者：「音訊格式是否可以改成 aac 高品質避免品質變差」）：`audio.mp3` 是**送轉錄**的 16 kHz 單聲道 64 kbps
（Nemotron 本來就以 16 kHz 處理，再高也聽不到；MP3 是 DeepInfra 確定收的格式，Android 的 Rust 端也只有 MP3 編碼器）；
`playback.*` 是**播放、核對**用的高品質副本（桌面 AAC 128 kbps 保留原取樣率；Android 直接保留原始檔），`Project::playback_audio()` 優先用它。
改 ffmpeg 那條路的行為（切段、正規化格式）時，兩個後端要一起改，`pipeline_e2e` 的兩個完整流程測試各驗一條。

圖示（2026-10-31 使用者：「為什麼電腦程式沒有圖示 幫我重新設計圖示電腦與手機要一致」；之前電腦版完全沒有圖示）：`工具/make_icons.py` 是唯一來源，
一份幾何（108 單位的 Android 自適應畫布，電腦版圓角方塊取看得到的 18～90）產出 `packaging/icon.ico`（`crates/meetscribe-app/build.rs` 用 `winresource` 嵌進 exe，
要 Windows SDK 的 rc.exe；Inno `SetupIconFile` 也用它）、`icon-256.png`（視窗圖示，`main.rs` 的 `with_icon`）、Android 的 `ic_launcher_foreground.xml`／`ic_notification.xml`。
改圖示只改那支、重跑，別手改產物。24 像素以下用簡化的 `SMALL` 形狀。

外觀（2026-10-30 使用者：「UI 好醜，參考 Filee」）：`theme.rs` 照抄 Filee 上游 `Styles/Soft.axaml`／`SoftPalette.cs` 的 Soft 主題色票與尺寸
（暖灰頁 `#F6F5F1`、白卡片 16px 圓角、主色 `#7F77DD`、深色 `#1B1B19`），跟隨 Windows 深淺色。顏色一律從 `theme::pal(ui)` 取，
按鈕、卡片、導覽、分頁用 `theme::` 的元件，不要在畫面裡寫死色值。圖示是 Segoe Fluent Icons 字碼（`theme::icon`），新增前確認字型裡有。

使用者定的入口（2026-10-30）：程式一律從「新增轉錄」頁開始；沒 OpenRouter Key 時那頁頂端是三步驟設定卡（儲值 → 建立 Key → 貼上），
有 Key 後縮成帳戶列（餘額＋儲值、API Keys 按鈕；啟動時自動 `TestKey` 查餘額）。外部網址集中在 `theme::links`。
金鑰輸入框只有一份實作：`views::settings::key_box`，新增轉錄頁與設定頁共用同一組 `SettingsForm` 狀態。

發佈：`installer\build-installer.ps1`（測試 → release → Inno Setup）產出 `dist\MeetScribe-Setup-v<版本>.exe`；版本號在根 `Cargo.toml` 與三個 crate（app、cli 對 core 的相依版本也要一起改）。
安裝程式的 `AppId` 第一次發佈後不能再改。ffmpeg 不打包（完整版每個 exe 2 百多 MB），安裝最後一頁用 winget 補。1.0.0 於 2026-10-31 發佈到 GitHub Release；**1.1.0 同日用 `工具/publish.py` 發佈（給所有裝置）**，線上 update.json 驗簽、兩個安裝檔 SHA-256 已比對。
1.0.0 沒有更新功能，已裝 1.0.0 的電腦／手機要手動裝一次 1.1.0，之後才會自己更新。
1.1.1（刪除會議可一併刪匯出檔、說話者辨識不卡、Word 排版）同日發佈給所有裝置，`update_check` 對真的 GitHub 確認 1.1.0 會挑到它。
1.2.0（錄音管理、匯出附錄音、分享錄音、PDF、Word 只放摘要）同日發佈給所有裝置：線上 update.json 驗簽、雜湊與建置輸出相同，`update_check` 從 1.1.1 兩個平台都挑到它。
1.2.1（手機會議頁四個分頁可左右滑動，`HorizontalPager`）只發 Android（`--only android`；`update::pick` 會跳過沒有該平台安裝檔的 Release，電腦版停在 1.2.0）。
1.3.0（傳到其他裝置＋API 金鑰自動同步，見〈傳到其他裝置〉）同日發佈給所有裝置：線上 update.json 的兩個雜湊與建置輸出相同，`update_check` 電腦從 1.2.0、手機從 1.2.1 都挑到它。
1.3.1（電腦版設定的「儲存設定」固定在底部、舊會議匯出頁補勾 PDF：`ExportOptions.rev`＋`upgrade()` 在 `ProjectStore::load`）同日發佈給所有裝置，`update_check` 兩個平台從 1.3.0 都挑到它。
1.3.2（手動同步會議內容：摘要頁「同步到其他裝置」、收件端自動套用＋還原上一版，見〈傳到其他裝置〉）同日發佈給所有裝置，`update_check` 兩個平台從 1.3.1 都挑到它。
1.3.3（摘要改 OpenRouter 下拉選單、預設免費、標免費／付費與費率、可開官網，見〈使用者定的費用分工〉）同日發佈給所有裝置，`update_check` 兩個平台從 1.3.2 都挑到它。
1.3.4（逐字稿點一下從那句播放：電腦雙擊修改、手機長按修改）同日發佈給所有裝置，`update_check` 兩個平台從 1.3.3 都挑到它。
1.3.5（跟隨播放：供應商段落開頭當錨點、舊會議重新對齊；轉錄每 60 秒一小段；新圖示）同日發佈給所有裝置：線上 update.json 驗簽、兩個雜湊與建置輸出相同，`update_check` 兩個平台從 1.3.4 都挑到它。
1.3.6（設定頁與處理紀錄顯示轉錄每約 60 秒一小段；說話者辨識段落改名）2026-11-02 發佈給所有裝置。`publish.py` 發佈成功後清 `dist/`，只留各平台最新一版
（2026-10-31 使用者：「dist 資料夾只留最新的版本」；舊的正式版要在 GitHub Release 找得到才刪，`--keep-old` 不清，`prune` 單獨跑）。
egui 離屏測試要驗雙擊時，直接往 `harness.input_mut().events` 送兩次 PointerButton 按下／放開；kittest 的 `click()` 走無障礙動作，永遠不會被判定成雙擊。
建 APK 時模擬器要先關（`adb emu kill`）：兩個一起開 Gradle daemon 會被系統砍掉（記憶體不夠）。
建置時記憶體吃緊（可用虛擬記憶體不到 2 GB）連 `-j 1` 都會 OOM，請使用者先關程式，不要改 LTO 或另開 target 目錄硬編。
快速開啟：專案根目錄的 `開啟 語音轉錄.cmd`（`%~dp0` 相對路徑，不寫死中文路徑）指向 `target\release\meetscribe-app.exe`。
刻意不由 AI 建 `.lnk`：全域 CLAUDE.md〈防毒〉記過「claude.exe 建指向自編未簽章 exe 的捷徑」被 Bitdefender 判刑；要 `.lnk` 請使用者自己「傳送到桌面」。
程式開著時 exe 被鎖住，`cargo build --release` 會失敗——先關視窗再建。

匯出（2026-10-31 使用者：「docx更新成不放逐字稿，PDF是從Docx轉成PDF 輸出檔案也加入PDF並預設勾選」）：Word、PDF 是 `summary_only()` 的格式，
固定只放摘要（＋會議資訊、發言者表），沒有摘要就不產；逐字稿在 Markdown、純文字、字幕。`ExportOptions.content` 只管 Markdown／純文字。
PDF 是 `export/pdf.rs` 照 docx 模組的版面直接畫（krilla，手機沒有 Word／LibreOffice 能轉）：色票、字級、標題區、表格都從 `docx::` 的常數與
共用的 `md_blocks` 來，**改 Word 版面時 PDF 要一起改**。字型讀系統的（Windows `msjh.ttc`、Android `/system/fonts/NotoSansCJK-Regular.ttc` 挑 TC），
補字用 Segoe UI Symbol／Noto Symbols（☐☑●）；`MEETSCRIBE_PDF_FONT` 可指定。看版面：`cargo test -p meetscribe-core --lib export::tests::pdf` 會寫
`target/pdf-shots/摘要.pdf`／`.docx`。設定檔 schema 3 對舊設定補勾 PDF 一次。
`ExportOptions.audio`（會議資料夾附一份錄音）只有 Android 的 bridge 用，core 不讀。

擴充點：新供應商實作 `providers::Transcriber`／`LlmClient`，在 `Engine::from_settings` 接上；新匯出格式實作 `export::Exporter`，登記進 `export::registry()`
（摘要＋逐字稿都能放的格式讓 `has_summary()` 回 true，`ExportOptions` 的「內容」才對它有作用；只放摘要的用 `summary_only()`）。輸出選項的 UI 只有一份：`widgets::export_options`，設定頁、新增轉錄、匯出頁共用。提示詞在 `prompts.rs`，使用者可用 `%APPDATA%\MeetScribe\prompts\<名稱>.md` 覆寫——`attribution`／`reconcile` 的 JSON 格式是程式解析的契約，改提示詞時保持格式。

## 推送更新（2026-10-31）

使用者：「支援推送升級功能且每台裝置有識別碼可以獨立推送更新 電腦版可自動更新，手機版看你怎麼做」。做法照抄 Filee（`一堆專案\Filee 按住Ctrl拖移轉檔工具` 的 `core::self_update`＋`工具/publish.py`）。

- 檢查、下載、驗簽章、挑版本、識別碼、狀態檔都在 `meetscribe-core/src/update.rs`，電腦與手機共用；安裝各自做：`meetscribe-app/src/updater.rs`（Inno 靜默）、`android/.../core/Updater.kt`（PackageInstaller）。
- **簽章金鑰** `keys/update-signing.pem`＋`keys/update-signing-password.txt`（不進版控，靠每週完整備份）；公鑰 `packaging/update.pub` 以 `include_str!` 編進程式。
  **絕不重產**：換金鑰＝已安裝的每一台都收不到更新。`publish.py new-key` 在檔案已存在時會拒絕。
- 發佈：`build-installer.ps1`＋`android\build-apk.ps1` → `uv run 工具/publish.py publish --notes "…" [--devices …]` → 下載線上 update.json／sig 驗一次 →
  `cargo run -p meetscribe-core --example update_check -- <上一版> AAAA-AAAA windows`（及 `android`）確認裝好的程式真的會挑到新版。
  版本號要比上一個 Release 大，否則裝好的程式不認為有新版。
- 安裝程式契約：`/SELFUPDATE=1` 時 `InitializeSetup` 等 mutex `MeetScribe-Running`（`updater::RUNNING_MUTEX`，程式啟動就建）消失；`/RELAUNCH=1` 時 `[Run]` 把程式開回來。改任一邊要一起改。
- 只在安裝版自動安裝（exe 旁有 `unins000.exe`）；`target\` 的開發版只檢查。**AI 不在這台執行下載來的安裝檔**（防毒規則）；實際靜默安裝留給使用者的電腦。
- 測試：`update::tests` 用 wiremock 假 GitHub＋固定測試金鑰；`tests/fixtures/python_signatures.json` 確認 Python `cryptography` 簽的（含 high-S）Rust 收得下。
  Android 實測：`MOCK_RELEASES=<資料夾> python -I android/test/mock_api.py …`＋ settings.json 的 `update_api`，新版 APK 用 `build-apk.ps1 -TestVersion 1.1.1` 產。

## 使用者定的費用分工（2026-10-30）

- 逐字稿：OpenRouter `nvidia/nemotron-3.5-transcribe`，**唯一付費**的部分。
- 說話者分配：**預設免費**——Google AI Studio Gemini 免費額度聽音訊；沒 Google Key 時 `Engine::from_settings` 退到 OpenRouter 的 `:free` 模型並寫 notice。
- 整合與摘要（2026-10-31 使用者：「摘要Gemini 模型預設拿掉 放上OPENROTUER 免費模型跟付費模型」）：OpenRouter，預設 `nvidia/nemotron-3-ultra-550b-a55b:free`；
  下拉選單（`widgets::summary_model_picker`、手機 `SummaryModelPicker`）列 `openrouter::summary_models`：免費組在前、付費組在後，各組照 `RECOMMENDED_FREE`／`RECOMMENDED_PAID`
  名次在前並標「推薦」，**一定標免費／付費**（付費附費率），旁邊「在 OpenRouter 看費率」開 `https://openrouter.ai/<模型 ID>`。推薦名單是 2026-10-31 依 OpenRouter 排行榜挑的，會過時：改之前重查。
  設定 schema 4 把舊的 Gemini 摘要改過來；`sanitized` 也不准摘要用 Google。電腦版另可選本機 Claude Code CLI。
- 新增任何預設模型都要維持「只有逐字稿收費」（付費模型只能是使用者自己在選單裡選的）。
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
  第二輪（同日，使用者：「逐字稿的跟隨撥放還是跟聲音有跟不上的狀況」，那場其實是音樂課）：**每窗的開頭是確定的時間**，`timing::provider_anchors` 把供應商段落的開頭對到句子當錨點
  （說話者辨識路徑原本把它丟了，第 9 段整段晚 28 秒→修後 3.6 秒；TIMING_REV 3）。窗裡面的段落開頭跟強制對齊比差幾秒，但比字數估算準（只用窗開頭反而 11.4→17.6 秒），
  所以也用；被拉長的最後一窗（段落結束超出音訊）只用它的開頭。模型的錨點由 `timing::merge_anchors` 補在中間、不准跟供應商的衝突。
  **送轉錄的小段（1.3.5，使用者同意：「可以 做完發佈」）**：說話者分配仍是 `chunk_minutes`（10 分鐘）一段，但每段再切成約 60 秒的 `PartRecord`
  （`STT_PART_SECS`，切在停頓、`STT_PART_WINDOW` 15 秒內找）送轉錄，每小段做完就存檔、全部回來才 `SttResult::join_parts` 合併並刪小段音檔。
  每小段一定留一個帶起訖的段落，`provider_anchors` 把每小段開頭當錨點。真實音樂課第 2 段重新轉錄：中位 23.8→3.1 秒、p90 52.4→11.4 秒；費用 US$0.0193，跟整段送一樣（照秒數計費）。
  不能把 `chunk_minutes` 直接調成 1 分鐘：Gemini 免費額度撐不住十倍的說話者分配請求，跨段配對說話者也會變差。舊會議要重新轉錄才有小段（沒有自動做，會花錢）。
  60 秒是固定值、不給使用者調；1.3.6 起兩邊設定頁都顯示「轉錄：每約 60 秒一小段送出」，`chunk_minutes` 標成「說話者辨識：每段分鐘數」，
  處理紀錄寫「第 i/N 段切成 K 小段送轉錄」（2026-11-02 使用者看到設定只有 10 分鐘，以為小段沒生效）。手機從 `state` 的 `stt_part_secs` 讀，桌面直接用 `pipeline::STT_PART_SECS`。
  端對端測試大多設 `engine.stt_part_secs = 0.0` 驗整段行為；小段由 `parts_give_each_minute_a_known_start`、`queue_timeouts_with_parts_still_finish`、原生音訊那個完整流程測試驗。
  量誤差：`工具/時間誤差/對齊標準答案.py`（本機 Qwen3-ForcedAligner 對齊自己的逐字稿當答案）→ `工具/時間誤差/量誤差.py`。供應商給的逐字時間不能當答案。
  播放器本身是準的（`player::tests` 用嗶聲量過：MP3 誤差 0、AAC 固定晚 64 ms 是編碼器 priming）。
- OpenRouter 上 Nemotron 唯一供應商 DeepInfra：`supports_diarization: false`、不吃 prompt。所以說話者靠 Gemini 聽音訊分配；`stt_capabilities()` 查到支援時會自動改走原生 diarize。查法：`GET /api/v1/models/<id>/endpoints`。
- Gemini 模型名稱常改版：`gemini::pick_audio_model` 在設定的名字不存在時挑最新的穩定版 Flash，別把模型名寫死在別處。
- `claude -p`：不能用 `--bare`（只認 ANTHROPIC_API_KEY，訂閱登入會失敗）；用 `--setting-sources project`＋空工作目錄隔離使用者的 hook／plugin。`total_cost_usd` 是牌價換算，記為 0。

## 說話者分配的等待上限

`pipeline.rs` 的 `ATTRIBUTION_TIMEOUT`（180 秒，含 HTTP 層重試與 Retry-After）、`ATTRIBUTION_ATTEMPTS`（2，逾時與不可重試的錯不重試）、
`ATTRIBUTION_GIVE_UP_AFTER`（連續 3 段失敗後其餘段落不呼叫模型）。使用者 2026-10-31：「辨識不出來就趕快接下去辨識不要卡這麼久」——調大這些數字前先想清楚。

## 說話者分配的不變式

逐字稿文字只來自 STT。分配模型只回 `[句號, 說話者, 起始秒]`；新人用 `N#`，由 `attribution::map_to_global` 配全域 `S#`。漏標的句子沿用前一位、時間用 `fill_times`（最長非遞減子序列當錨點＋字數內插）。改這段時這條不變式要守住，否則逐字稿會漏字。

## 這台機器的建置坑

- 分頁檔小：rustc 會報 `os error 1455`／`STATUS_STACK_BUFFER_OVERRUN`。已用 `.cargo/config.toml` 的 `jobs = 3` 與 `[profile.dev] debug = 0` 壓住；別另開 `CARGO_TARGET_DIR`（整套重編會再撞記憶體）。
- reqwest 0.13 用 `native-tls`（`rustls` 預設走 aws-lc-rs 要 cmake）；`.query()` 要 `query` feature。
- 含反斜線或 `"#` 的補丁：用 Python 腳本檔（scratchpad），別用 Bash heredoc；Rust 原始字串內含 `"#` 時用 `r##"…"##`。
  Python 讀寫要用 `read_bytes`／`write_bytes`：`write_text` 在 Windows 會把整檔寫成 CRLF；`"\u0000"` 寫進 Kotlin 原始碼前確認是跳脫字面、不是真的 NUL。

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
- 刪除會議可一併刪匯出檔：只刪 `bridge::export_files` 列的（`exported` 記錄＋專屬子資料夾裡檔名以會議開頭的匯出格式），不碰使用者自己放的檔。
- App 內錄音：`service/RecorderService.kt`（前景服務 microphone，AAC m4a 直接寫進錄音庫），停止後 `Repo.recordingStopped` 登記；從新增轉錄錄的再由 `Recording.finished` 帶進表單。
- 匯入檔放 `files/data/imports/<uuid>/<原檔名>`；完成或刪除專案時刪掉，App 啟動時清掉沒有專案在用的。
- 錄音管理（2026-10-31 使用者：「多做一個頁面 錄音管理 不管有沒有轉錄」）：App 錄的音檔放 `files/data/recordings/<uuid>/`＋`info.json`
  （`recordings.rs`），**永久保存**（1.1 錄在 imports，沒轉錄就被清掉）。錄音機停止 → `add_recording` 登記；開 App 時 `recover` 補登記錄到一半被殺的
  （讀不出長度標 `broken`）。拿去轉錄時專案的 `source_file` 直接指向錄音庫的檔；處理完成且專案資料夾有 `playback.*` 後 `after_done` 刪掉錄音庫那筆
  （併進會議，不存兩份）。`list_recordings` ＝還沒建會議的錄音庫錄音＋每場會議；已建會議的錄音由會議代表、不另外列。刪掉未完成的會議時錄音留在錄音庫
  （`keeps_recording`）。`Recording.start(ctx, transcribe)`：從新增轉錄錄的停止後帶進表單，從錄音管理錄的只存起來。
- 匯出附錄音：會議資料夾放 `<檔名>.<原副檔名>`（播放檔，不重壓），已有同大小的不再寫；記進 `exported`、刪除會議時可一起刪。
  分享錄音（`Files.shareAudio`）先以會議名稱複製到 `cache/share/` 再分享（FileProvider `cache-path`），對方收到的才不是 `playback.m4a`。

閉環測試（模擬器）：AVD `meetscribe_test`（API 36 x86_64）。主機跑 `python -I android/test/mock_api.py <資料夾> [延遲秒]`，用法在檔頭；
`android/test/ui.ps1` 用 uiautomator 依文字點擊、截圖。真的金鑰由使用者自己在 App 裡貼，測試流程不從認證管理員取出金鑰。
`adb push`／`/sdcard` 路徑用 PowerShell 跑（Git Bash 會把 `/sdcard` 改寫成 Windows 路徑）。

## 傳到其他裝置（2026-10-31，1.3.0）

設計與使用者原話在 `PLAN.md` 的 M15，使用者看的說明在 README〈傳到其他裝置〉。程式：`meetscribe-core/src/transfer.rs`（加密、配對碼、分段、`Hub`、金鑰同步）、
`relay/`（Cloudflare Worker＋R2＋D1）、電腦 `worker.rs` 的 `TransferCmd`／`TransferMsg`＋`views/transfer.rs`、手機 `bridge.rs` 的 `transfer_*`＋`core/Transfers.kt`＋`ui/TransferUi.kt`
（op 名稱與事件欄位是契約，改一邊要改另一邊）。

- **中轉站絕不能產生費用**（使用者：「絕對不可以讓我付到錢」）。Workers、D1 免費方案超量只會失敗；**會自動扣款的只有 R2**，所以：
  任何新的 R2 PUT／GET／LIST 都要先過 `spendR2`（近 31 天加總到 `R2_CLASS_A_BUDGET`／`R2_CLASS_B_BUDGET` 就 503）；請求路徑裡不准用 LIST（確認上傳完、刪除都靠 D1 的 `chunks`）；
  每段實際大小記帳、不能超過宣告的大小（暫存總量 8 GB 是照宣告大小算的）。刪除不收費。不要建議使用者升級 Workers 付費方案。
- 刪除的四層：收完／拒收立刻刪 → 3 小時過期（cron 每 15 分，過期也拒絕下載）→ 每小時掃描刪超過 6 小時的（`sweep`）→ R2 生命週期 1 天（最短只能 1 天）。
- 部署：`relay\deploy.ps1`（wrangler 已用使用者的 Cloudflare 帳號 OAuth 登入過，憑證在 `%APPDATA%\xdg.config\.wrangler`）。它會把真的 D1 ID 寫進 `wrangler.toml`（不是祕密）、
  遠端建表（`schema.sql` 全是 `IF NOT EXISTS`，改 schema 只能加不能改）、設生命週期、部署、健康檢查。正式網址寫在 `config.rs` 的 `DEFAULT_RELAY_URL`。
- `relay_url`：空白＝正式中轉站；`"off"`（`RELAY_OFF`）＝關閉。**測試一律用 off 或本機網址**，否則會真的連上正式中轉站登記假裝置（`ui_tests` 先寫一份 off 的設定檔）。
- 本機中轉站：`cd relay; npx wrangler dev --local --port 8787 --ip 0.0.0.0 --test-scheduled`（模擬器用 `http://10.0.2.2:8787`，App 的 settings.json 設 `relay_url`）。
  `wrangler.toml` 的 database_id 換過後本機資料庫是新的空庫，要先 `npx wrangler d1 execute meetscribe-relay --local --file schema.sql --yes`（症狀：`no such table: devices`）。
  `/__scheduled?cron=...` 觸發清理；`--var SWEEP_EVERY_RUN:1 --var SWEEP_AGE_SECONDS:0` 測掃描、`--var R2_CLASS_A_BUDGET:5` 測預算到頂。
- 端對端測試要設 `MEETSCRIBE_RELAY_TEST=<網址>` 才跑：`-p meetscribe-core --test transfer_relay`、`-p meetscribe-android transfer_ops`、`-p meetscribe-app desktop_workers`；
  對正式中轉站跑也可以（用量很小、收完就刪）。命令列對手 `cargo run -p meetscribe-core --example transfer_peer -- <資料夾> <網址> <指令>`（id／pair／accept／send／recv／inbox／unpair／setkey／keys／synckeys），
  金鑰存在該資料夾的 keys.json。`secrets::use_file_store` 是全程式一個（OnceLock），**同一個測試程式裡兩台裝置共用金鑰存放處**，金鑰同步的「補缺不覆蓋」要用兩個命令列程式測。
  `pair` 從標準輸入讀配對碼：`tail -f code.txt | transfer_peer … pair <ID>` 再 `echo 123456 >> code.txt`（PowerShell 的 Add-Content 寫不進被 tail 開著的檔）。
- API 金鑰同步（`Kind::Keys`）：只在記憶體裡加解密（`SecretBytes`、`receive_keys`），不准走寫暫存檔的路；`Hub::poll` 自動收存、`Hub::pair_confirm`／偵測到新配對時 `sync_keys(Fill)`、
  改金鑰時 `sync_keys(Replace)`（電腦：設定頁與新增轉錄頁的 `key_box` 改了就送 `SyncKeys`；手機：bridge 的 `save_key`）。只同步存在金鑰存放處的，環境變數的不送。
- 會議內容同步（`Kind::Update`，1.3.2）：`Hub::sync_meeting` 送給所有配對裝置，`Hub::poll` 自動 `apply_meeting_content`（有同一個會議 ID 才套用、先存 `previous-content.json`）並 `Transfer::ack`；
  寄件端用 `content_sent` 把對方的 Received／Declined 通知換成 `ContentResult`。同步的欄位在 `MeetingContent`，Project 新增可編輯的欄位時要一起加。整場會議傳送不帶 `previous-content.json`。
- 輪詢：程式開著時每 15 秒（使用者要金鑰改了馬上同步）、等配對另一邊時每 5 秒；電腦 `poll_wake`、手機 `Transfers.wake` 可以叫它馬上查。

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
