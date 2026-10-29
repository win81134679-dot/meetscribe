# MeetScribe 會議逐字稿

會議錄音（或影片）→ **完整逐字稿（標出誰說的）＋ 依內容決定格式的會議摘要**。Windows 桌面程式，Rust 寫成。

| 工作 | 預設用什麼 | 費用 |
|---|---|---|
| 逐字稿 | OpenRouter 上的 NVIDIA **Nemotron 3.5 Transcribe** | 付費，約 US$0.12／小時音訊 |
| 誰在說話 | Google AI Studio 的 **Gemini Flash 免費額度**（聽音訊判斷） | 免費 |
| 會議摘要 | 同上，Gemini 免費額度 | 免費 |
| 摘要（可選） | 你電腦上的 **Claude Code CLI**（`claude -p`，用你的 Claude 訂閱） | — |

沒設定 Google Key 時，說話者與摘要會自動改用 OpenRouter 上的免費模型（`nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free`、`nvidia/nemotron-3-ultra-550b-a55b:free`），一樣不花錢。

## 架構一覽

| 執行架構圖 | 處理流程圖 |
|---|---|
| ![執行架構圖](docs/架構圖/PNG/執行架構圖.png) | ![處理流程圖](docs/架構圖/PNG/處理流程圖.png) |

A2 橫式 300 DPI；同名向量 PDF 在 `docs/架構圖/PDF/`，原始檔在 `docs/架構圖/HTML/`。

## 需要準備

1. **ffmpeg**：`winget install Gyan.FFmpeg`（程式會自己找；找不到可在設定指定位置）。
2. **OpenRouter 帳戶**（必要，逐字稿用）：[儲值](https://openrouter.ai/settings/credits)、[建立 API Key](https://openrouter.ai/settings/keys)。程式的「新增轉錄」頁有按鈕直接帶你去。
3. **Google AI Studio API Key**（建議，免費）：<https://aistudio.google.com/apikey>。
4. **Claude Code**（選用）：要用它寫摘要時，先在終端機登入過一次 `claude`。

金鑰在「設定」頁貼上即可，存在 **Windows 認證管理員**，不會寫進任何設定檔。

## 使用

1. 雙擊專案資料夾裡的 **`開啟 MeetScribe.cmd`**（或直接開 `target\release\meetscribe-app.exe`）。程式一律從「新增轉錄」頁開始。
   想放到桌面：在 `target\release\meetscribe-app.exe` 上按右鍵 →「傳送到」→「桌面（建立捷徑）」。
2. 第一次使用，「新增轉錄」頁最上面會有三個步驟，照著按就好：
   ① **開啟 OpenRouter 儲值頁**儲值（頁面會顯示儲值 US$5 約可轉幾小時）→
   ② **開啟 OpenRouter API Keys 頁**建立金鑰 → ③ 把 `sk-or-…` 貼進框裡按「儲存」，程式會自動測試並顯示餘額。
   下方「選用：Gemini 免費金鑰」可以順便設定（不設也能用）。
   設定好之後這塊會縮成一列：顯示 OpenRouter 餘額，右邊隨時有「儲值」「API Keys」按鈕。
3. 把錄音檔拖進視窗（可一次多個，會依序處理）。
   - 有與會者名單就貼上（一行一位），模型比較能把「說話者 1」對上真名。
   - 專有名詞（人名、產品代號）填進去可提升辨識率。
   - 「摘要方式」可在這裡切成「本機 Claude Code CLI」。
4. 處理中可以看進度、取消；中斷（關程式、斷網、額度用完）後按「從中斷處繼續」，已完成的段落不會重做、不會重複付費。
5. 完成後：
   - **摘要**：格式由模型依會議性質決定（決策會議列決議與待辦、講座整理重點、訪談整理問答…）；可加額外要求重新產生、可直接編輯。
   - **逐字稿**：點時間從那裡播放；點說話者改成別人；點文字修改、合併上一句、刪除。搜尋、依說話者篩選、跟隨播放。
   - **說話者**：改名（標「推測」的是模型判斷的，請核對）、把被拆開的同一人合併、聽聲音樣本、重新辨識。
   - **匯出**：Word（摘要＋逐字稿）、Markdown、純文字、SRT／WebVTT 字幕、CSV（Excel）、JSON。不會覆蓋同名檔。

## 大型會議（2～1000 人）怎麼處理

- 錄音超過 20 分鐘（可調）會在**停頓處**切段，Nemotron 單次上限 1 小時，長度不限。
- 每段的說話者由 Gemini 聽音訊判斷；前面段落出現過的人會附上**聲音樣本**給下一段比對，讓同一人跨段保持同一個標籤。
- 全部完成後再做一次「整合」：看每位說話者的描述與發言，**保守地**合併重複的人、推測姓名。
- 逐字稿文字永遠以 Nemotron 的辨識結果為準；說話者模型只回「第幾句是誰」，所以不會因為它偷懶而漏字。
- 人非常多時，自動辨識難免把同一人拆成兩位或把聲音相近的人併在一起，請在「說話者」頁核對。

## 命令列（批次、自動化）

```powershell
meetscribe-cli transcribe 會議.m4a --participants 名單.txt --keyterms "MeetScribe,Nemotron" --formats md,docx,srt
meetscribe-cli transcribe *.mp3 --summary claude      # 摘要改用本機 Claude Code CLI
meetscribe-cli list
meetscribe-cli resume 1a2b3c4d                         # 從中斷處繼續
meetscribe-cli summarize 1a2b3c4d --instructions "列出每位主管的意見"
meetscribe-cli export 1a2b3c4d --formats docx --out D:\會議記錄
```

設定、金鑰、專案都跟桌面程式共用。也可以用環境變數 `OPENROUTER_API_KEY`、`GEMINI_API_KEY` 給金鑰。

## 資料放在哪

`%APPDATA%\MeetScribe\`（可用環境變數 `MEETSCRIBE_HOME` 改到別處，例如隨身碟）：

| 位置 | 內容 |
|---|---|
| `settings.json` | 設定（不含金鑰） |
| `projects\<ID>\project.json` | 逐字稿、說話者、摘要、費用 |
| `projects\<ID>\audio.mp3`、`chunks\`、`clips\` | 轉成 16 kHz 的音訊、分段、聲音樣本 |
| `prompts\` | 自訂提示詞（設定頁「自訂提示詞…」會放範本與說明） |
| `logs\meetscribe.log` | 執行紀錄 |

## 隱私

- 錄音會上傳到 OpenRouter（及其供應商 DeepInfra）轉錄；說話者與摘要會送到 Google 或 OpenRouter 的免費模型。
- **Google 免費方案的輸入可能被 Google 用來改善產品**，免費模型供應商也可能記錄內容。機密會議請：摘要改用本機 Claude Code CLI、在設定勾選「OpenRouter 只路由到不保留資料的供應商」，或改用付費方案。

## 從原始碼建置

```powershell
cargo build --release          # 產出 target\release\meetscribe-app.exe、meetscribe-cli.exe
cargo test --workspace         # 單元＋端對端（需 ffmpeg）＋離屏 UI 渲染
cargo test -p meetscribe-core --test claude_cli_live -- --ignored   # 實際呼叫本機 Claude Code
```

架構與擴充方式見 `CLAUDE.md`，設計決定與進度見 `PLAN.md`。
