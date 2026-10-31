# 語音轉錄（MeetScribe）計畫書

會議錄音 → 完整逐字稿（含說話者）＋依內容決定格式的會議摘要。Rust 桌面工具＋ Android App（共用 Rust 核心），推送更新。
1.1 起顯示名稱是「語音轉錄」；程式內部、資料夾、安裝檔名、applicationId 維持 MeetScribe，舊版才能原地升級。

## 需求與對應做法

| 使用者原話 | 做法 |
|---|---|
| 「RUST UI，User 角度設計，正式工具不是玩具」 | egui/eframe 原生桌面程式；專案歷史、多檔排隊、取消、中斷續跑、逐句編輯、播放核對、匯出 7 種格式 |
| 「必須要有擴充性」 | core 函式庫與 UI 分離；`Transcriber`／`LlmClient`／`Exporter` trait；提示詞可外部覆寫；另有 CLI |
| 「接 OpenRouter API KEY，User 可以自己設定」 | 設定頁輸入，存 Windows 認證管理員；可測試餘額 |
| 「會議摘要（不固定格式，看內容）」 | 提示詞要求先判斷會議性質再選結構；超長稿 map-reduce |
| 「完整逐字稿」 | Nemotron 3.5 Transcribe；說話者模型只回「第幾句是誰」，文字不經 LLM，不會漏字 |
| 「辨識誰在說話，2~1000 人」 | 見〈說話者辨識〉 |
| 「Gemini 聽音訊分配說話者，Gemini 要免費；Nemotron 付費」 | 說話者分配預設 Google AI Studio Gemini 免費額度 |
| 「摘要靠我的 Claude Code CLI」→「會議摘要免費為主，手動 UI 可切換成 Claude Code CLI」 | 摘要預設 Gemini 免費額度；新增轉錄表單與摘要頁都有切換鈕可改用本機 `claude -p` |

## 費用分工（2026-10-30 定案）

| 工作 | 預設 | 備援／選項 | 費用 |
|---|---|---|---|
| 逐字稿 | OpenRouter `nvidia/nemotron-3.5-transcribe` | 任何 OpenRouter STT 模型 | 付費 ≈ US$0.12／小時 |
| 說話者分配（聽音訊） | Google `gemini-3.8-flash`（免費額度） | 無 Google Key → OpenRouter `nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free` | 免費 |
| 說話者整合、摘要 | Google `gemini-3.8-flash`（免費額度） | 無 Google Key → OpenRouter `nvidia/nemotron-3-ultra-550b-a55b:free`；手動切本機 Claude Code CLI | 免費 |

## 2026-10-30 查到的事實（會變，改程式前重查）

- OpenRouter `nvidia/nemotron-3.5-transcribe`：端點 `POST /api/v1/audio/transcriptions`；唯一供應商 DeepInfra，`supports_diarization: false`、`supported_parameters: []`、`prompt 0.0000333`／秒。
- 模型本身：單次最長 1 小時、16 kHz mono、40 語言（中文屬 Supported 級）；原生可輸出 `<spk:N>`，但 OpenRouter 沒開放 prompt。
- OpenRouter 目前沒有免費的 Gemini（最便宜 `gemini-2.5-flash-lite`）。真正免費的 Gemini 只有 Google AI Studio 免費額度：Flash 系列免綁卡，額度約每日 20～50 次、以 Google 專案計；免費方案的輸入可能被 Google 用於改善產品。
- Claude Code CLI 2.1.312：`--bare` 只認 ANTHROPIC_API_KEY，不能用；改用 `--setting-sources project --tools "" --strict-mcp-config --no-session-persistence --output-format json`，提示詞從 stdin 送。

## 處理流程

1. 探測長度 → ffmpeg 轉 16 kHz mono MP3 32 kbps。
2. `silencedetect` 找停頓，在目標長度（預設 10 分）±90 秒內挑最長停頓切段。
3. 各段並行送 Nemotron（預設 3 路），429/5xx 退避重試，每段結果立刻存檔。排隊逾時（504／524／本機逾時）不在 HTTP 層重試，
   整輪結束後改成一次一段、等 20 秒×輪數再重送，最多 6 輪（逾時不收費）；某段失敗時其他段照樣做完存檔才回報錯誤。
   2026-10-31 查證：逾時來自 DeepInfra 排隊（同一段時快時慢、跟長度無關），先前「逾時就對半切」的做法已拿掉。
4. 說話者分配（依序）：逐字稿切成編號句 → 連同音訊、已知說話者名單與聲音樣本送 Gemini → 回 `[句號, 說話者, 秒]`；新人 `N#` 由程式配全域 `S#`；漏標沿用前一位；時間取最長非遞減子序列當錨點再依字數內插。
5. 整合：文字模型看每位說話者的描述、發言樣本與與會者名單，保守合併、推測姓名（標「推測」）。
6. 摘要：依會議性質自選結構；超過 30 萬字分段筆記再合併。
7. 每步存 `project.json`（原子寫入）；中斷後續跑只做沒做完的部分。
8. 完成時依 `options.export`（建立時從設定複製；舊專案為 `None` 用目前設定）自動匯出，寫出的檔記在 `project.exported`。
   已完成的專案再跑一次、段落／說話者／摘要都沒變時不重複匯出。匯出失敗只記警告，不影響「完成」。

## 里程碑

- [x] M0 研究 API 與模型限制
- [x] M1 core：模型、設定、ffmpeg 分段、OpenRouter／Gemini／Claude Code 三個 client、pipeline、匯出
- [x] M2 桌面 App（新增轉錄、進度、摘要、逐字稿、說話者、匯出、紀錄、設定）
- [x] M3 CLI（transcribe／list／resume／summarize／export）
- [x] M4 驗證：clippy 0 警告；47 單元＋2 端對端（真 ffmpeg＋模擬 API）＋離屏 UI 渲染 7 畫面；本機 Claude Code 摘要與認證管理員實測
- [x] M5 文件（README、CLAUDE、本檔）
- [x] M6 外觀改用 Filee 的 Soft 主題（使用者：「UI 好醜，參考 Filee」）：白側欄導覽、白卡片、薰衣草紫、Fluent 圖示、跟隨深淺色
- [x] M7 好上手（使用者：「預設開啟新增轉錄頁，放上 OpenRouter 儲值頁與 API 頁，讓 User 輕鬆設置；根目錄放捷徑」）：
  新增轉錄頁三步驟設定卡＋帳戶列（餘額、儲值、API Keys）；根目錄 `開啟 MeetScribe.cmd`
- [x] M8 轉錄排隊逾時自動重送（2026-10-31，見處理流程第 3 點；最初誤判為段落太長而做的自動切段已拿掉）
- [x] M9 輸出選項（使用者：「可以設定逐字稿的格式 Markdown docx txt…等跟可自訂輸出位置，幫我想完整」）：
  `export::ExportOptions`（格式、資料夾、子資料夾、檔名樣式、內容四選一、逐字稿樣式、同名覆蓋、自動匯出），
  設定頁／新增轉錄／匯出頁共用 `widgets::export_options`，CLI 的 `--formats`／`--out` 蓋在上面
- [x] M10 Android App（2026-10-31，使用者：「我要製作成 android APP」「幫我完整建置 Android APP 輸出成 APK」）：
  - 核心不分叉：`meetscribe-core` 加 `native-audio` feature（symphonia 解碼＋自寫重取樣／靜音偵測＋LAME 編 MP3，介面同 ffmpeg 的 `AudioTool`）；
    Android 上 TLS 改 rustls＋ring＋webpki-roots，金鑰改存 App 私有檔（`secrets::use_file_store`）。
  - `crates/meetscribe-android`：JNI 橋，`call`（同步 JSON 請求）／`poll`（事件佇列），一次跑一個專案，同桌面 worker 的分工。
  - `android/`：Kotlin＋Jetpack Compose（新增轉錄＋三步驟設定、會議記錄、專案頁四分頁、設定），前景服務（dataSync＋通知進度），
    接收「分享／開啟方式」的錄音，匯出檔經 FileProvider 分享。Opus／AMR 等 symphonia 不支援的格式用 MediaCodec 轉 WAV 再交給核心。
  - 驗證：電腦上原生音訊後端跑完整兩段式流程（模擬 API）；Android 16 模擬器實機閉環：系統檔案選擇器／開啟方式匯入 m4a 與 opus、
    手機端編出的 MP3 經 ffprobe 驗證（16 kHz mono 32 kbps，長度相符）、說話者分配、摘要、自動匯出 docx/md/srt、通知列進度、
    取消→續跑、改說話者名、改句子、從時間點播放、刪除；手機連 openrouter.ai 的 HTTPS 實測可用。過程中修掉：英文句子併段少空格（桌面也有）、
    匯出檔顯示 App 內部路徑、未開始的匯入檔殘留、短錄音費用顯示 US$0.00、專案頁標題列多一段狀態列空白。

- [x] M11 語音轉錄 1.1（2026-10-31，使用者：「用User的角度設計程式UI及功能各種貼心設計 程式名稱改成語音轉錄 會議預設開啟每場會議子資料夾
  手機版支援開啟該會議資料夾功能…支援推送升級功能且每台裝置有識別碼可以獨立推送更新 電腦版可自動更新，手機版看你怎麼做」「電腦版加入預設開啟程式UI置中」）：
  - 改名：視窗、側欄、安裝程式（AppName／捷徑；舊捷徑 `[InstallDelete]`）、Android 標籤。AppId、安裝資料夾、`%APPDATA%\MeetScribe`、applicationId 不改。
  - 匯出預設「每場會議一個子資料夾」：`Settings.schema` 2，舊設定檔載入時打開一次（之後使用者關掉就尊重）。
  - 推送更新（照 Filee 的做法）：`meetscribe-core/src/update.rs`。公開 repo 的 Release 帶 `update.json`＋`update.sig`（ECDSA P-256，公鑰編進程式）；
    每台裝置隨機 `XXXX-XXXX` 識別碼，`devices` 名單可只推給部分裝置；`工具/publish.py` 簽章、上傳、`open` 開放給全部。
    使用者選擇把 meetscribe 改成公開 repo（2026-10-31）；公開前掃過整段歷史（金鑰形狀、email、本機路徑）。
  - 電腦：每天背景檢查下載；側欄「新版已備妥」→「立即更新」（Inno `/VERYSILENT /SELFUPDATE=1 /RELAUNCH=1`，安裝程式等 `MeetScribe-Running` mutex 消失再換檔、裝完開回來）
    或關閉程式時靜默安裝。只在安裝版（exe 旁有 `unins000.exe`）做。
  - 手機：每天背景檢查下載＋通知；一鍵「更新」走 PackageInstaller（第一次要允許安裝不明應用程式；Android 12+ `USER_ACTION_NOT_REQUIRED`，更新自己可免確認）；
    `MY_PACKAGE_REPLACED` 通知「已更新」。商店外的 App 做不到完全無聲，這是 Android 的規定。
  - 手機匯出改到公開「文件/語音轉錄/<會議>」；會議頁「開啟資料夾」用系統檔案 App（DocumentsUI 的 externalstorage URI），打不開時檔案清單仍在頁面上。
  - 貼心設計：電腦視窗置中（小螢幕自動縮）、處理完成時工作列閃爍、設定頁「關於與更新」（版本、識別碼、自動更新、檢查更新）；
    手機 App 內錄音（前景服務 microphone、暫停／繼續、停止就帶入）、處理完成通知（點了開那場會議）、會議清單搜尋（五場以上出現）、
    逐字稿搜尋反白＋複製全文、摘要複製。
  - 跟隨播放（使用者：「我發現跟隨撥放功能根本不準」）：原因是時間戳，不是播放器。真實 2.6 小時會議 18 段全部不準：說話者模型回假等差秒數
    （整段 9 分鐘的字擠在前 30 秒）、供應商 300 秒一整段、最後一窗時間拉長超出音訊。新 `timing` 模組驗證時間、不可信就依「有聲音的時間」分配，
    長段切成句子；舊專案啟動時重新對齊一次（不呼叫 API）。同一場會議重新對齊後 16/18 段合格，剩 2 段是長時間有聲音但轉錄只出十幾個字（沒有字可以對）。
    手機播放改 SEEK_CLOSEST、跟著捲動。
  - 音訊品質（使用者：「音訊格式是否可以改成 aac 高品質避免品質變差」）：送轉錄的 MP3 32→64 kbps；另做播放用高品質副本（桌面 AAC 128 kbps、Android 保留原檔），
    播放、從某句播放、跟隨播放都用它。

- 發佈：v1.1.0 Release（2026-10-31，repo 已改公開）；模擬器實測 1.1.0 → 1.1.1 自動下載、驗簽、一鍵安裝（第一次允許後無確認視窗）、
  指定給別台的 1.2.0 被略過；1.0 會議升級後匯出資料夾搬到公開位置、時間重新對齊。

- [x] M12（2026-10-31 同日追加）：
  - 手機刪除會議時詢問是否一併刪除匯出檔（使用者：「大部分User並不會開啟資料夾手動刪除」）：`bridge` 的 `export_files`／`delete_exports`，
    只刪記在專案上的與專屬子資料夾裡檔名以這場會議開頭的匯出格式；手動匯出也記進 `exported`。模擬器實測刪掉 3 個檔與資料夾。
  - 說話者辨識不卡（使用者：「辨識不出來就趕快接下去辨識不要卡這麼久」）：單次 180 秒上限（含 HTTP 重試與 Retry-After）、最多 2 次且逾時／400 不重試、
    連續 3 段失敗後其餘段落不呼叫模型。以前一段最壞可卡數十分鐘（3 次 × HTTP 重試 4 次 × 單次 900 秒）。測試：模型不回應時整場 4.4 秒做完。
  - Word 排版（使用者：「docx也要修 我希望把排版...等變得更好」）：見 README〈輸出選項〉的 Word 版面；`examples/docx_sample.rs` 產範例看版面。

- [x] M13（2026-10-31，1.2.0）：
  - 錄音管理（使用者：「我想多做一個頁面 錄音管理 不管有沒有轉錄 我希望該頁面做完善 有轉錄可以支援跳轉」）：Android 第三個分頁。
    App 錄音改存錄音庫 `recordings/<uuid>/`，永久保存（1.1 存在匯入區，沒轉錄就在下次開 App 被清掉，等於會掉錄音）；錄到一半被殺的開 App 時補登記、標「不完整」。
    清單＝未轉錄的錄音＋每場會議；試聽迷你播放器、篩選、搜尋、統計；詳細頁有完整播放器（±15 秒、0.75–2×）、資訊、轉錄／開啟逐字稿、分享、存到手機、改名、刪除。
    轉錄完成後錄音併進會議，不存兩份。
  - 匯出附錄音（使用者：「匯出時一併存一份錄音：會議資料夾裡多放 會議名稱.m4a」）：`ExportOptions.audio`，預設開；刪除會議的勾選框算進去。
  - 會議頁「分享錄音檔」（使用者：「直接傳到 LINE、雲端硬碟」）：以會議名稱複製到 cache/share 再分享。
  - Word 只放摘要、新增 PDF 並預設勾（使用者：「docx更新成不放逐字稿，PDF是從Docx轉成PDF 輸出檔案也加入PDF並預設勾選」）：
    手機上沒有 Word／LibreOffice 能做 docx→PDF 轉檔，所以 PDF 由 `export/pdf.rs` 照 Word 同一份內容、色票與版面直接畫（krilla，系統中文字型子集化），
    電腦與手機都能產；舊設定檔（schema 2）補勾 PDF 一次。
  - 模擬器閉環（mock API）：錄音管理錄一段 → 列為未轉錄 → 試聽 → 詳細頁改名、1.25×、存到手機（`Documents/語音轉錄/錄音/`）、分享（分享表單顯示「<名稱>.m4a」、有雲端硬碟）
    → 轉錄這段錄音（新增轉錄頁自動帶入）→ 完成後錄音庫清空、會議資料夾有 docx／pdf／md／srt／m4a（手機產的 PDF 用 Noto Sans CJK TC，版面與 Word 相同）
    → 錄音詳細頁「開啟逐字稿與摘要」跳會議 → 刪除會議（勾選 5 個檔）資料夾整個刪掉。新增轉錄頁錄音停止後自動帶入；刪除未轉錄錄音後回清單。
    測試中修掉：從會議頁刪掉會議回到錄音詳細頁時卡在「載入中」；刪掉新增轉錄頁正選著的錄音時表單沒清掉。

## 尚未驗證（需要使用者的金鑰）

- 用真的 OpenRouter Key 跑 Nemotron 轉錄（目前只對模擬伺服器測過；`verbose_json`／`keyterms` 若供應商不收，程式會自動改送精簡請求）。
- 用真的 Google AI Studio Key 跑 Gemini 說話者分配（請求格式依官方 `generateContent` 文件，模擬伺服器測過）。
- 實機啟動桌面程式（依防毒規則由使用者自己開）；**電腦版自動更新的「執行安裝檔」那一步**（下載驗證有模擬伺服器測試，
  實際靜默安裝要在使用者電腦上由程式自己跑，AI 不在這台執行自編的安裝檔）。
- Android 版用真的金鑰跑一次（模擬器閉環用的是模擬 API；自動化測試不得從認證管理員取出金鑰）、裝到實體手機（目前只在 x86_64 模擬器上跑過，
  實體手機用的 arm64 函式庫有編出、符號檢查過，但沒有在 ARM 裝置上執行過）。

## 不做的事

- 本機部署 Nemotron（vLLM 等）：使用者 2026-10-30 決定不做——「我的 GPU RAM 不夠」（模型 BF16 需約 80 GB 顯示記憶體級的卡）。逐字稿一律走 OpenRouter。

## 之後可以做（未排程）

- 即時錄音（麥克風／系統音訊）直接進 pipeline；Android 版尤其適合（App 內錄音→直接轉錄）。
- Android：電腦與手機之間同步會議資料；一次匯入多個檔；自訂提示詞。
- 程式碼簽章（電腦版安裝檔沒有簽章，第一次執行會跳 SmartScreen）。
- 本機聲紋嵌入做跨段比對，取代聲音樣本送 LLM。
- 安裝程式（Inno Setup）與 ffmpeg 一起打包。
