# MeetScribe 計畫書

會議錄音 → 完整逐字稿（含說話者）＋依內容決定格式的會議摘要。Rust 桌面工具。

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
2. `silencedetect` 找停頓，在目標長度（預設 20 分）±90 秒內挑最長停頓切段。
3. 各段並行送 Nemotron（預設 3 路），429/5xx/逾時退避重試，每段結果立刻存檔。
4. 說話者分配（依序）：逐字稿切成編號句 → 連同音訊、已知說話者名單與聲音樣本送 Gemini → 回 `[句號, 說話者, 秒]`；新人 `N#` 由程式配全域 `S#`；漏標沿用前一位；時間取最長非遞減子序列當錨點再依字數內插。
5. 整合：文字模型看每位說話者的描述、發言樣本與與會者名單，保守合併、推測姓名（標「推測」）。
6. 摘要：依會議性質自選結構；超過 30 萬字分段筆記再合併。
7. 每步存 `project.json`（原子寫入）；中斷後續跑只做沒做完的部分。

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

## 尚未驗證（需要使用者的金鑰）

- 用真的 OpenRouter Key 跑 Nemotron 轉錄（目前只對模擬伺服器測過；`verbose_json`／`keyterms` 若供應商不收，程式會自動改送精簡請求）。
- 用真的 Google AI Studio Key 跑 Gemini 說話者分配（請求格式依官方 `generateContent` 文件，模擬伺服器測過）。
- 實機啟動桌面程式（依防毒規則由使用者自己開）。

## 不做的事

- 本機部署 Nemotron（vLLM 等）：使用者 2026-10-30 決定不做——「我的 GPU RAM 不夠」（模型 BF16 需約 80 GB 顯示記憶體級的卡）。逐字稿一律走 OpenRouter。

## 之後可以做（未排程）

- 即時錄音（麥克風／系統音訊）直接進 pipeline。
- 本機聲紋嵌入做跨段比對，取代聲音樣本送 LLM。
- 安裝程式（Inno Setup）與 ffmpeg 一起打包。
