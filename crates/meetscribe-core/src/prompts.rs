//! 提示詞。預設值內嵌；`prompts\<名稱>.md` 存在就用它（使用者可自行調整語氣、格式）。
//! 範本變數用 `{變數}`，見各常數上方說明。

use std::path::Path;

/// 說話者分配（系統提示）。
pub const ATTRIBUTION: &str = r#"你是會議錄音的說話者標註員。你會收到一段會議音訊，以及這段音訊已經由專業語音辨識轉好、切成編號句子的逐字稿。
你的工作只有一件事：聽音訊，判斷每一句是誰說的、從本段音訊第幾秒開始。不要改寫、不要重新轉錄文字。

規則：
1. 每一個句號都要出現在輸出，依原順序，一句一筆。
2. 「已知說話者」清單裡的人（可能附聲音樣本）沿用他們的 ID，例如 S3。
3. 只有確定是先前沒出現過的聲音才用新 ID：N1、N2、N3…（本段第一個新出現的人是 N1）。
4. 判斷依據：音色、性別、口音、語速等聲音特徵；對話脈絡（被點名、自我介紹、回答別人的問題）；輪流發言的節奏。
5. 若能從內容確定某人的姓名或職稱（自我介紹、被別人叫名字、與會者名單吻合），填在 speakers 的 name；不確定填 null，不要猜。
6. note 用一句話描述這個聲音與角色（例：「男聲、低沉，主持會議」），供之後比對。
7. t 是該句在本段音訊中的開始秒數（從 0 起算，可到小數一位）。
8. 一句話若由兩人接著說完，標給說得比較多的那位。

只輸出 JSON，不要任何其他文字：
{"speakers":[{"id":"S3","name":null,"note":"男聲，主持會議"},{"id":"N1","name":"王經理","note":"女聲，業務部"}],"a":[[0,"S3",0.0],[1,"N1",4.2],[2,"S3",9.8]]}"#;

/// 跨段整合說話者（系統提示）。
pub const RECONCILE: &str = r#"你會收到一場會議的說話者清單。這些 ID 是逐段標註出來的，同一個人有可能在不同段落被標成不同 ID。
請根據每位說話者的聲音描述、發言樣本、姓名線索與與會者名單：
1. 找出「確定是同一個人」的 ID 並合併。只合併有把握的；錯誤合併比漏合併更糟。
2. 推測姓名或職稱。只有內容中有明確證據（自我介紹、被點名、與會者名單吻合）才給；沒有就不要列。

只輸出 JSON，不要任何其他文字：
{"merges":[{"from":"S7","into":"S2","reason":"同樣自稱王經理"}],"names":[{"id":"S1","name":"王小明（主持人）","reason":"開場自我介紹"}]}"#;

/// 摘要（系統提示）。變數：`{language}`。
pub const SUMMARY: &str = r#"你是資深的會議記錄秘書。請根據逐字稿撰寫會議摘要，讓沒參加的人能快速掌握重點。

不要套用固定範本。先判斷這場會議的性質——例如決策會議、專案進度、腦力激盪、一對一、訪談、課程或講座、客戶洽談、審查會、大型研討會或座談、閒談——再選擇最適合的結構與篇幅：
- 有決議就列決議；有待辦就列待辦（負責人、期限只在逐字稿有提到時才寫）。
- 有不同意見就分別呈現各方立場與理由。
- 講座或課程就整理知識重點與例子；訪談就整理問答重點。
- 大型多人會議依議程或主題分段，標出主要發言人。
- 數字、日期、金額、專有名詞照逐字稿寫，不要四捨五入或改寫。

寫作規則：
- 開頭用 2～4 句話說明這場會議在談什麼、得出什麼結論。
- 提到人時使用逐字稿中的說話者名稱。
- 只寫逐字稿裡有的內容，不要補充或推測。語音辨識可能有錯字：明顯的可依上下文判斷，不確定的專有名詞保留原文。
- 必要時可引用關鍵時間點（例如［00:12:30］）方便回頭聽錄音。
- 使用 Markdown（標題、條列，適合時用表格）。不要用程式碼區塊包住整份輸出。
- 輸出語言：{language}"#;

/// 長會議分段筆記（使用者提示前綴）。變數：`{i}` `{n}` `{range}`。
pub const SUMMARY_MAP: &str = r#"以下是一場長會議逐字稿的第 {i}/{n} 部分（{range}）。請整理這部分的詳細重點筆記：討論主題、各方論點、決議、待辦、數字與人名，保留時間標記。這份筆記之後會和其他部分合併成完整摘要，所以不要寫開場白或總結。"#;

/// 長會議合併（使用者提示前綴）。
pub const SUMMARY_REDUCE: &str = r#"以下是同一場會議依時間順序各部分的重點筆記。請依系統指示，把它們整合成一份完整的會議摘要（不是把筆記接在一起，要重新組織）。"#;

#[derive(Clone, Debug)]
pub struct Prompts {
    pub attribution: String,
    pub reconcile: String,
    pub summary: String,
    pub summary_map: String,
    pub summary_reduce: String,
}

impl Default for Prompts {
    fn default() -> Self {
        Self {
            attribution: ATTRIBUTION.into(),
            reconcile: RECONCILE.into(),
            summary: SUMMARY.into(),
            summary_map: SUMMARY_MAP.into(),
            summary_reduce: SUMMARY_REDUCE.into(),
        }
    }
}

const FILES: [(&str, &str); 5] = [
    ("attribution", ATTRIBUTION),
    ("reconcile", RECONCILE),
    ("summary", SUMMARY),
    ("summary_map", SUMMARY_MAP),
    ("summary_reduce", SUMMARY_REDUCE),
];

impl Prompts {
    /// 讀 `dir\<名稱>.md` 覆寫；空檔或不存在用預設。
    pub fn load(dir: &Path) -> Self {
        let read = |name: &str, default: &str| -> String {
            std::fs::read_to_string(dir.join(format!("{name}.md")))
                .ok()
                .map(|s| s.trim_start_matches('\u{feff}').trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| default.to_string())
        };
        Self {
            attribution: read("attribution", ATTRIBUTION),
            reconcile: read("reconcile", RECONCILE),
            summary: read("summary", SUMMARY),
            summary_map: read("summary_map", SUMMARY_MAP),
            summary_reduce: read("summary_reduce", SUMMARY_REDUCE),
        }
    }

    /// 在資料夾寫出 `<名稱>.default.md` 範本與說明，方便使用者複製修改。
    pub fn write_examples(dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        for (name, text) in FILES {
            std::fs::write(dir.join(format!("{name}.default.md")), text)?;
        }
        std::fs::write(
            dir.join("說明.txt"),
            "把 xxx.default.md 複製一份改名為 xxx.md 再修改，MeetScribe 就會改用你的版本。\n\
             刪掉 xxx.md 即恢復預設。\n\
             summary.md 的 {language} 會換成設定的輸出語言；summary_map.md 的 {i} {n} {range} 是分段編號與時間範圍。\n\
             attribution.md 與 reconcile.md 要求的 JSON 格式請勿更動，程式依此解析。\n",
        )
    }
}

pub fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    vars.iter()
        .fold(template.to_string(), |acc, (k, v)| acc.replace(&format!("{{{k}}}"), v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_file_wins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("summary.md"), "\u{feff}自訂摘要 {language}").unwrap();
        let p = Prompts::load(dir.path());
        assert_eq!(p.summary, "自訂摘要 {language}");
        assert_eq!(p.attribution, ATTRIBUTION);
        assert_eq!(fill(&p.summary, &[("language", "English")]), "自訂摘要 English");
    }
}
