//! 用一場假的會議產生 Word 檔，看排版用（內容全是範例，不碰真的會議資料）：
//! `cargo run -p meetscribe-core --example docx_sample -- <輸出資料夾>`，再用 office_render.py 轉 PNG 看。
use meetscribe_core::export::{self, ExportOptions};
use meetscribe_core::model::{JobOptions, ModelsUsed, Project, Segment, Summary, Timing};

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("輸出資料夾"));
    let mut p = Project::new("D:\\錄音\\第四季行銷預算會議.m4a".into(), JobOptions::default());
    p.title = "第四季行銷預算會議".into();
    p.duration = 3725.0;
    p.models = ModelsUsed {
        stt: "nvidia/nemotron-3.5-transcribe".into(),
        attribution: "gemini-3.8-flash".into(),
        summary: "gemini-3.8-flash".into(),
    };
    let wang = p.add_speaker(Some("王經理".into()), "主持".into());
    let linda = p.add_speaker(Some("Linda".into()), "財務".into());
    let chen = p.add_speaker(Some("陳設計師".into()), String::new());
    let lines: [(&str, &str); 14] = [
        (&wang, "大家好，今天的會議主要討論第四季的行銷預算，還有展覽活動的分工。"),
        (&wang, "我們先看一下上一季的成果，再決定這一季的方向。"),
        (&linda, "Thanks. I am Linda from the finance team. Our total budget for the fourth quarter is two hundred thousand dollars."),
        (&linda, "That is about ten percent less than last quarter, so we need to prioritize."),
        (&wang, "好的，我建議把一半的預算放在社群廣告，另外一半用在展覽活動。"),
        (&chen, "展覽的攤位設計我這邊可以在兩週內出初稿，主視覺會沿用今年的品牌色。"),
        (&chen, "不過印刷品的數量要先確定，不然報價會差很多。"),
        (&wang, "印刷品先抓五百份，之後看報名人數再追加。"),
        (&linda, "That works for me. Please send me the detailed plan by next Friday."),
        (&wang, "沒問題，我下週五之前把企劃書寄給你。"),
        (&chen, "那我同一天把攤位設計的初稿也一起附上。"),
        (&linda, "Great. Let's also review the social ads performance every two weeks."),
        (&wang, "可以，每兩週的週一早上開一個十五分鐘的短會。"),
        (&wang, "那今天的會議就到這裡，謝謝大家。"),
    ];
    let mut t = 0.0;
    for (i, (who, text)) in lines.iter().enumerate() {
        let len = text.chars().count() as f64 / 5.0 + 2.0;
        p.segments.push(Segment {
            id: i as u64 + 1,
            start: t,
            end: t + len,
            timing: Timing::Model,
            speaker: Some(who.to_string()),
            text: text.to_string(),
            chunk: 0,
            edited: false,
        });
        t += len + 1.5;
    }
    p.summary = Some(Summary {
        markdown: "# 第四季行銷預算會議\n\n本次會議確認第四季總預算與分配方式，並分派展覽與企劃的工作。\n\n## 決議\n\n- 第四季總預算 **20 萬美元**（比上一季少約一成）\n- 一半用於社群廣告、一半用於展覽活動\n- 印刷品先印 **500 份**，依報名人數再追加\n\n## 待辦事項\n\n| 項目 | 負責人 | 期限 |\n|---|---|---|\n| 寄出第四季企劃書 | 王經理 | 下週五 |\n| 攤位設計初稿 | 陳設計師 | 下週五 |\n| 社群廣告成效檢討 | Linda | 每兩週一次 |\n\n## 後續追蹤\n\n- [ ] 確認展覽報名人數\n- [x] 敲定每兩週的檢討會時間\n\n1. 先確定印刷數量再請廠商報價\n2. 主視覺沿用今年品牌色\n\n> 備註：預算比上一季少，社群廣告的投放要先挑成效最好的平台。".into(),
        model: "gemini-3.8-flash".into(),
        created_at: chrono::Local::now(),
        instructions: String::new(),
    });
    let opts = ExportOptions {
        formats: vec!["docx".into()],
        dir: out.to_string_lossy().into_owned(),
        subfolder: false,
        overwrite: true,
        ..ExportOptions::default()
    };
    for f in export::export_project(&p, &opts).expect("匯出") {
        println!("{}", f.display());
    }
}
