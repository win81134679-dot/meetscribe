//! 帳戶檢查：金鑰有沒有效、OpenRouter 餘額。桌面與 Android 共用。

use crate::config::Settings;
use crate::gemini::{self, Gemini};
use crate::openrouter::{KeyInfo, OpenRouter};
use crate::secrets::{self, KeyKind};

/// 用存好的金鑰打一次 API；成功回一句給人看的說明（含餘額）。
pub async fn test_key(settings: &Settings, kind: KeyKind) -> Result<String, String> {
    let Some((key, _)) = secrets::load(kind) else {
        return Err(format!("尚未設定 {}", kind.label()));
    };
    match kind {
        KeyKind::OpenRouter => {
            let c = OpenRouter::new(settings, key).map_err(|e| e.to_string())?;
            let info: KeyInfo = c.key_info().await.map_err(|e| e.to_string())?;
            let mut s = String::from("金鑰有效");
            if let Some(c) = info.credits_remaining {
                s.push_str(&format!("，帳戶餘額 US${c:.2}"));
            }
            if let Some(r) = info.limit_remaining {
                s.push_str(&format!("，此金鑰剩餘額度 US${r:.2}"));
            }
            s.push_str(&format!("，累計用量 US${:.2}", info.usage));
            Ok(s)
        }
        KeyKind::Google => {
            let g = Gemini::new(settings, key).map_err(|e| e.to_string())?;
            let models = g.list_models().await.map_err(|e| e.to_string())?;
            let pick = gemini::pick_audio_model(&settings.attribution_model, &models);
            Ok(format!(
                "金鑰有效，可用 {} 個模型；說話者分配將使用 {}",
                models.len(),
                pick.unwrap_or_else(|| "（找不到 Flash 模型）".into())
            ))
        }
    }
}
