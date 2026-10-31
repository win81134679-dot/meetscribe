//! 推送更新（2026-10-31 使用者：「支援推送升級功能且每台裝置有識別碼可以獨立推送更新 電腦版可自動更新」）。
//!
//! 新版放在公開 repo [`REPO`] 的 GitHub Release，每個 Release 帶三樣：安裝檔（電腦 `.exe`、手機 `.apk`，可只有其中一個）、
//! `update.json`（[`Manifest`]：版本、說明、各平台檔名大小 SHA-256、指定裝置）、`update.sig`（ECDSA P-256／SHA-256 簽
//! `update.json` 的原始位元組，DER 再 base64）。公鑰 `packaging/update.pub` 編進程式；私鑰只在發佈者電腦（`工具/publish.py`）。
//! 程式只接受：簽章對得上、比目前新、這個平台有檔、給所有裝置或名單裡有本機 [`device_id`] 的 Release，挑其中最新的。
//! 下載後比對大小與 SHA-256 才算數。安裝由各平台自己做（電腦：靜默跑 Inno Setup；Android：PackageInstaller）。

use crate::{Error, Result};
use base64::Engine;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

/// 放新版的公開 repo。
pub const REPO: &str = "win81134679-dot/meetscribe";
pub const DEFAULT_API: &str = "https://api.github.com";
/// 驗 `update.json` 簽章的公鑰。換金鑰＝已安裝的每一台都收不到更新，所以 `keys/` 絕不重產。
pub const PUBLIC_KEY_PEM: &str = include_str!("../../../packaging/update.pub");
/// 好念的識別碼字元：沒有 0/O、1/I/L、U。
const ID_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Android,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "android") { Platform::Android } else { Platform::Windows }
    }
}

/// `update.json`。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub version: String,
    /// 這版改了什麼（給使用者看，幾行就好）。
    #[serde(default)]
    pub notes: String,
    /// `null`＝所有裝置；否則只有這些識別碼收得到。
    #[serde(default)]
    pub devices: Option<Vec<String>>,
    #[serde(default)]
    pub windows: Option<Asset>,
    #[serde(default)]
    pub android: Option<Asset>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Asset {
    pub file: String,
    pub size: u64,
    pub sha256: String,
}

impl Manifest {
    pub fn asset(&self, platform: Platform) -> Option<&Asset> {
        match platform {
            Platform::Windows => self.windows.as_ref(),
            Platform::Android => self.android.as_ref(),
        }
    }
    pub fn is_for(&self, device: &str) -> bool {
        self.devices
            .as_ref()
            .is_none_or(|list| list.iter().any(|d| d.trim().eq_ignore_ascii_case(device)))
    }
}

/// 找到可以裝的新版。
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub version: String,
    pub notes: String,
    pub asset: Asset,
    /// 安裝檔的下載網址。
    pub url: String,
    /// 只推給部分裝置的版本（介面上會註明「指定推送」）。
    pub targeted: bool,
}

// ------------------------------------------------------------------------------------------------ 裝置識別碼

/// `XXXX-XXXX`（40 位元，[`ID_ALPHABET`]）。
pub fn format_device_id(seed: &[u8]) -> String {
    let mut n = seed.iter().take(5).fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
    let base = ID_ALPHABET.len() as u64;
    let mut chars = Vec::with_capacity(8);
    for _ in 0..8 {
        chars.push(ID_ALPHABET[(n % base) as usize] as char);
        n /= base;
    }
    let s: String = chars.into_iter().collect();
    format!("{}-{}", &s[..4], &s[4..])
}

pub fn is_device_id(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 9 && b[4] == b'-' && b.iter().enumerate().all(|(i, c)| i == 4 || ID_ALPHABET.contains(c))
}

/// 這台裝置的識別碼：第一次用時隨機產生，存在資料夾的 `device-id.txt`（不從硬體或帳號推算，也不送到任何地方，
/// 只有檢查更新時在本機比對 Release 的名單）。
pub fn device_id(root: &Path) -> String {
    let file = root.join("device-id.txt");
    if let Ok(text) = std::fs::read_to_string(&file)
        && is_device_id(text.trim())
    {
        return text.trim().to_string();
    }
    let id = format_device_id(uuid::Uuid::new_v4().as_bytes());
    if let Err(e) = std::fs::write(&file, &id) {
        tracing::warn!("無法儲存裝置識別碼：{e}");
    }
    id
}

// ------------------------------------------------------------------------------------------------ 版本與簽章

/// `1.2.3` → `[1,2,3]`（前面的 v 與後面的 -xxx 不算）。
pub fn parse_version(text: &str) -> Option<Vec<u32>> {
    let t = text.trim().trim_start_matches(['v', 'V']);
    let t = t.split(['-', '+']).next()?;
    let mut v = t.split('.').map(|p| p.parse::<u32>().ok()).collect::<Option<Vec<_>>>()?;
    while v.len() < 3 {
        v.push(0);
    }
    Some(v)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

pub fn public_key() -> Result<VerifyingKey> {
    VerifyingKey::from_str(PUBLIC_KEY_PEM.trim()).map_err(|_| Error::Other("程式內建的更新公鑰壞了".into()))
}

/// 驗 `update.sig` 並解析 `update.json`。簽章不對就是錯誤（不管內容寫什麼）。
pub fn verify(manifest: &[u8], sig_base64: &str, key: &VerifyingKey) -> Result<Manifest> {
    let der = base64::engine::general_purpose::STANDARD
        .decode(sig_base64.trim())
        .map_err(|_| Error::Other("update.sig 不是 base64".into()))?;
    let sig = Signature::from_der(&der).map_err(|_| Error::Other("update.sig 格式不對".into()))?;
    key.verify(manifest, &sig)
        .map_err(|_| Error::Other("update.json 的簽章不符，已略過".into()))?;
    Ok(serde_json::from_slice(manifest)?)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ------------------------------------------------------------------------------------------------ 檢查與下載

/// GitHub releases API 的一筆（只取用得到的欄位）。
#[derive(Debug, Deserialize)]
struct Release {
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

/// 從已驗證的 Release 裡挑：比 `current` 新、給這台、有這個平台的檔，取最新的。
pub fn pick(
    candidates: impl IntoIterator<Item = (Manifest, Vec<(String, String)>)>,
    current: &str,
    device: &str,
    platform: Platform,
) -> Option<Offer> {
    candidates
        .into_iter()
        .filter(|(m, _)| is_newer(&m.version, current) && m.is_for(device))
        .filter_map(|(m, assets)| {
            let asset = m.asset(platform)?.clone();
            let url = assets.iter().find(|(name, _)| *name == asset.file)?.1.clone();
            Some(Offer {
                version: m.version.clone(),
                notes: m.notes.clone(),
                targeted: m.devices.is_some(),
                asset,
                url,
            })
        })
        .max_by(|a, b| parse_version(&a.version).cmp(&parse_version(&b.version)))
}

/// 問 GitHub 有沒有給這台的新版。`api` 空白＝[`DEFAULT_API`]（測試用模擬伺服器）。
/// 某個 Release 的 update.json 抓不到或簽章不對只記警告、略過它。
pub async fn check(
    http: &reqwest::Client,
    api: &str,
    key: &VerifyingKey,
    current: &str,
    device: &str,
    platform: Platform,
) -> Result<Option<Offer>> {
    let api = if api.trim().is_empty() { DEFAULT_API } else { api.trim().trim_end_matches('/') };
    let url = format!("{api}/repos/{REPO}/releases?per_page=30");
    let resp = http
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(Error::Other(format!("查詢新版失敗（GitHub 回 {}）", resp.status().as_u16())));
    }
    let releases: Vec<Release> = resp.json().await?;
    let mut verified = Vec::new();
    for r in releases.into_iter().filter(|r| !r.draft) {
        let find = |name: &str| r.assets.iter().find(|a| a.name == name).map(|a| a.browser_download_url.clone());
        let (Some(json_url), Some(sig_url)) = (find("update.json"), find("update.sig")) else {
            continue;
        };
        let fetched = async {
            let json = http.get(&json_url).send().await?.error_for_status()?.bytes().await?;
            let sig = http.get(&sig_url).send().await?.error_for_status()?.text().await?;
            verify(&json, &sig, key)
        };
        match fetched.await {
            Ok(m) => verified.push((
                m,
                r.assets.iter().map(|a| (a.name.clone(), a.browser_download_url.clone())).collect(),
            )),
            Err(e) => tracing::warn!("略過 Release（{json_url}）：{e}"),
        }
    }
    Ok(pick(verified, current, device, platform))
}

/// 下載安裝檔到 `dir`，比對大小與 SHA-256。同名且內容已正確的檔直接沿用。`progress(已下載, 總數)`。
pub async fn download(
    http: &reqwest::Client,
    offer: &Offer,
    dir: &Path,
    cancel: &CancellationToken,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf> {
    tokio::fs::create_dir_all(dir).await?;
    let name = Path::new(&offer.asset.file)
        .file_name()
        .ok_or_else(|| Error::Other("update.json 的檔名不對".into()))?;
    let target = dir.join(name);
    if file_matches(&target, &offer.asset).await {
        return Ok(target);
    }
    let part = target.with_extension("part");
    let mut resp = http.get(&offer.url).send().await?.error_for_status()?;
    let mut file = tokio::fs::File::create(&part).await?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => {
                drop(file);
                let _ = tokio::fs::remove_file(&part).await;
                return Err(Error::Cancelled);
            }
            c = resp.chunk() => c?,
        };
        let Some(chunk) = chunk else { break };
        done += chunk.len() as u64;
        if done > offer.asset.size {
            break;
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        progress(done, offer.asset.size);
    }
    file.flush().await?;
    drop(file);
    let digest = hex(&hasher.finalize());
    if done != offer.asset.size || !digest.eq_ignore_ascii_case(offer.asset.sha256.trim()) {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(Error::Other("下載的更新檔大小或 SHA-256 不符，已丟棄".into()));
    }
    let _ = tokio::fs::remove_file(&target).await;
    tokio::fs::rename(&part, &target).await?;
    // 舊版的安裝檔用不到了
    if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            if e.path() != target {
                let _ = tokio::fs::remove_file(e.path()).await;
            }
        }
    }
    Ok(target)
}

async fn file_matches(path: &Path, asset: &Asset) -> bool {
    match tokio::fs::read(path).await {
        Ok(bytes) => bytes.len() as u64 == asset.size && sha256_hex(&bytes).eq_ignore_ascii_case(asset.sha256.trim()),
        Err(_) => false,
    }
}

// ------------------------------------------------------------------------------------------------ 狀態

/// 存在資料夾的 `update-state.json`：上次檢查時間、已下載好等著安裝的版本。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UpdateState {
    pub last_check: Option<chrono::DateTime<chrono::Utc>>,
    pub ready: Option<Ready>,
    /// 上一次執行的版本：跟目前不同＝剛更新完（顯示「已更新到 x」一次）。
    pub last_version: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Ready {
    pub version: String,
    pub notes: String,
    pub path: PathBuf,
    #[serde(default)]
    pub targeted: bool,
}

impl UpdateState {
    fn file(root: &Path) -> PathBuf {
        root.join("update-state.json")
    }
    pub fn load(root: &Path) -> Self {
        std::fs::read_to_string(Self::file(root))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, root: &Path) -> Result<()> {
        crate::store::write_atomic(&Self::file(root), serde_json::to_string_pretty(self)?.as_bytes())
    }
    /// 距離上次檢查超過 `hours` 小時（或從沒檢查過）。
    pub fn due(&self, hours: i64) -> bool {
        self.last_check
            .is_none_or(|t| chrono::Utc::now() - t >= chrono::Duration::hours(hours))
    }
    /// 還能用的已下載版本：比目前新、檔案還在。
    pub fn ready_for(&self, current: &str) -> Option<&Ready> {
        self.ready
            .as_ref()
            .filter(|r| is_newer(&r.version, current) && r.path.is_file())
    }
    /// 記下這次執行的版本；回傳「剛從哪一版更新上來」。
    pub fn note_version(&mut self, current: &str) -> Option<String> {
        let previous = std::mem::replace(&mut self.last_version, current.to_string());
        if self.ready.as_ref().is_some_and(|r| !is_newer(&r.version, current))
            && let Some(r) = self.ready.take()
        {
            let _ = std::fs::remove_file(&r.path);
        }
        (!previous.is_empty() && previous != current).then_some(previous)
    }
}

// ------------------------------------------------------------------------------------------------ 電腦與手機共用的一次檢查

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// 已是最新版。
    Latest,
    /// 新版已下載、驗證好，等著安裝。
    Ready(Ready),
}

/// 進度：(版本, 已下載, 總數)。
pub type Progress<'a> = &'a mut (dyn FnMut(&str, u64, u64) + Send);

/// 檢查 → 有就下載 → 記進 [`UpdateState`]。`root` 是程式資料夾（識別碼、狀態、`updates\` 下載處）。
pub async fn check_and_download(
    root: &Path,
    settings: &crate::config::Settings,
    current: &str,
    platform: Platform,
    cancel: &CancellationToken,
    progress: Progress<'_>,
) -> Result<Outcome> {
    let http = crate::http::client(1800)?;
    let device = device_id(root);
    let mut state = UpdateState::load(root);
    let found = check(&http, &settings.update_api, &public_key()?, current, &device, platform).await;
    state.last_check = Some(chrono::Utc::now());
    let offer = match found {
        Ok(Some(o)) => o,
        Ok(None) => {
            let _ = state.save(root);
            return Ok(match state.ready_for(current) {
                Some(r) => Outcome::Ready(r.clone()),
                None => Outcome::Latest,
            });
        }
        Err(e) => {
            let _ = state.save(root);
            return Err(e);
        }
    };
    if let Some(r) = state.ready_for(current)
        && r.version == offer.version
    {
        let r = r.clone();
        let _ = state.save(root);
        return Ok(Outcome::Ready(r));
    }
    let version = offer.version.clone();
    let path = download(&http, &offer, &root.join("updates"), cancel, |d, t| progress(&version, d, t)).await?;
    let ready = Ready { version: offer.version, notes: offer.notes, path, targeted: offer.targeted };
    state.ready = Some(ready.clone());
    state.save(root)?;
    Ok(Outcome::Ready(ready))
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::SigningKey;
    use p256::ecdsa::signature::Signer;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_key() -> SigningKey {
        SigningKey::from_slice(&[7u8; 32]).unwrap()
    }

    fn sign(bytes: &[u8], key: &SigningKey) -> String {
        let sig: Signature = key.sign(bytes);
        base64::engine::general_purpose::STANDARD.encode(sig.to_der().as_bytes())
    }

    #[test]
    fn embedded_public_key_parses() {
        public_key().unwrap();
    }

    #[test]
    fn device_ids_are_readable_and_stable() {
        let id = format_device_id(&[1, 2, 3, 4, 5, 6]);
        assert!(is_device_id(&id), "{id}");
        assert_ne!(id, format_device_id(&[9, 2, 3, 4, 5]));
        assert!(!is_device_id("K7Q2-9XM0"));
        assert!(!is_device_id("K7Q29XMA"));
        assert!(is_device_id("K7Q2-9XMA"));
        let dir = tempfile::tempdir().unwrap();
        let a = device_id(dir.path());
        assert_eq!(a, device_id(dir.path()));
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("1.10.0", "1.9.3"));
        assert!(is_newer("v1.1", "1.0.9"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.0", "1.0.0"));
        assert!(!is_newer("abc", "1.0.0"));
    }

    #[test]
    fn signature_must_match_the_exact_bytes() {
        let key = test_key();
        let body = br#"{"version":"1.1.0"}"#;
        let sig = sign(body, &key);
        assert_eq!(verify(body, &sig, key.verifying_key()).unwrap().version, "1.1.0");
        assert!(verify(br#"{"version":"9.9.9"}"#, &sig, key.verifying_key()).is_err());
        let other = SigningKey::from_slice(&[8u8; 32]).unwrap();
        assert!(verify(body, &sign(body, &other), key.verifying_key()).is_err());
    }

    #[test]
    fn signatures_from_the_python_publisher_verify() {
        // 工具/publish.py 用 cryptography 簽，S 不一定是 low-S；一半以上是 high-S 的也要收
        let fx: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/python_signatures.json")).unwrap();
        let key = VerifyingKey::from_str(fx["public_pem"].as_str().unwrap().trim()).unwrap();
        let msg = fx["message"].as_str().unwrap().as_bytes();
        assert!(fx["high_s"].as_u64().unwrap() > 0);
        for sig in fx["signatures"].as_array().unwrap() {
            verify(msg, sig.as_str().unwrap(), &key).unwrap();
        }
    }

    fn manifest(version: &str, devices: Option<&[&str]>, win: bool, android: bool) -> (Manifest, Vec<(String, String)>) {
        let asset = |f: &str| Asset { file: f.into(), size: 1, sha256: "00".into() };
        let m = Manifest {
            version: version.into(),
            notes: String::new(),
            devices: devices.map(|d| d.iter().map(|s| s.to_string()).collect()),
            windows: win.then(|| asset(&format!("w{version}.exe"))),
            android: android.then(|| asset(&format!("a{version}.apk"))),
        };
        let urls = [format!("w{version}.exe"), format!("a{version}.apk")]
            .into_iter()
            .map(|n| (n.clone(), format!("https://x/{n}")))
            .collect();
        (m, urls)
    }

    #[test]
    fn pick_takes_newest_release_meant_for_this_device_and_platform() {
        let me = "AAAA-BBBB";
        let list = vec![
            manifest("1.1.0", None, true, true),
            manifest("1.3.0", Some(&["ZZZZ-ZZZZ"]), true, true),
            manifest("1.2.0", Some(&["aaaa-bbbb"]), true, false),
            manifest("1.2.5", None, false, true),
            manifest("0.9.0", None, true, true),
        ];
        let w = pick(list.clone(), "1.0.0", me, Platform::Windows).unwrap();
        assert_eq!((w.version.as_str(), w.targeted), ("1.2.0", true));
        assert_eq!(w.url, "https://x/w1.2.0.exe");
        let a = pick(list.clone(), "1.0.0", me, Platform::Android).unwrap();
        assert_eq!((a.version.as_str(), a.targeted), ("1.2.5", false));
        assert!(pick(list, "1.3.0", me, Platform::Windows).is_none());
    }

    #[test]
    fn state_tracks_ready_installer_and_version_changes() {
        let dir = tempfile::tempdir().unwrap();
        let installer = dir.path().join("setup.exe");
        std::fs::write(&installer, b"x").unwrap();
        let mut s = UpdateState::default();
        assert!(s.due(24));
        assert_eq!(s.note_version("1.0.0"), None);
        s.ready = Some(Ready { version: "1.1.0".into(), path: installer.clone(), ..Default::default() });
        assert!(s.ready_for("1.0.0").is_some());
        s.save(dir.path()).unwrap();
        let mut s = UpdateState::load(dir.path());
        // 裝好之後第一次啟動：回報從哪版升上來、清掉用過的安裝檔
        assert_eq!(s.note_version("1.1.0").as_deref(), Some("1.0.0"));
        assert!(s.ready.is_none() && !installer.exists());
        assert_eq!(s.note_version("1.1.0"), None);
    }

    #[tokio::test]
    async fn check_and_download_against_a_fake_github() {
        let server = MockServer::start().await;
        let key = test_key();
        let installer = b"fake installer bytes".to_vec();
        let good = serde_json::to_vec(&Manifest {
            version: "1.2.0".into(),
            notes: "修正".into(),
            devices: None,
            windows: Some(Asset {
                file: "Setup-v1.2.0.exe".into(),
                size: installer.len() as u64,
                sha256: sha256_hex(&installer),
            }),
            android: None,
        })
        .unwrap();
        // 1.9.0 的簽章是別把鑰匙簽的：要被略過
        let forged = serde_json::to_vec(&Manifest { version: "1.9.0".into(), ..Manifest::default() }).unwrap();
        let other = SigningKey::from_slice(&[9u8; 32]).unwrap();
        let base = server.uri();
        let release = |tag: &str, extra: &[&str]| {
            let mut assets = vec![
                serde_json::json!({"name": "update.json", "browser_download_url": format!("{base}/dl/{tag}/update.json")}),
                serde_json::json!({"name": "update.sig", "browser_download_url": format!("{base}/dl/{tag}/update.sig")}),
            ];
            for n in extra {
                assets.push(serde_json::json!({"name": n, "browser_download_url": format!("{base}/dl/{tag}/{n}")}));
            }
            serde_json::json!({"tag_name": tag, "draft": false, "assets": assets})
        };
        Mock::given(method("GET"))
            .and(path(format!("/repos/{REPO}/releases")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                release("v1.9.0", &[]),
                release("v1.2.0", &["Setup-v1.2.0.exe"]),
            ])))
            .mount(&server)
            .await;
        for (p, body) in [
            ("/dl/v1.2.0/update.json", good.clone()),
            ("/dl/v1.2.0/update.sig", sign(&good, &key).into_bytes()),
            ("/dl/v1.9.0/update.json", forged.clone()),
            ("/dl/v1.9.0/update.sig", sign(&forged, &other).into_bytes()),
            ("/dl/v1.2.0/Setup-v1.2.0.exe", installer.clone()),
        ] {
            Mock::given(method("GET"))
                .and(path(p))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
                .mount(&server)
                .await;
        }
        let http = reqwest::Client::new();
        let offer = check(&http, &base, key.verifying_key(), "1.0.0", "AAAA-BBBB", Platform::Windows)
            .await
            .unwrap()
            .expect("1.2.0");
        assert_eq!(offer.version, "1.2.0");
        assert_eq!(offer.notes, "修正");
        assert!(
            check(&http, &base, key.verifying_key(), "1.2.0", "AAAA-BBBB", Platform::Windows)
                .await
                .unwrap()
                .is_none()
        );

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old-setup.exe"), b"old").unwrap();
        let mut seen = 0;
        let file = download(&http, &offer, dir.path(), &CancellationToken::new(), |d, _| seen = d)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), installer);
        assert_eq!(seen, installer.len() as u64);
        assert!(!dir.path().join("old-setup.exe").exists());

        // 內容被換掉（SHA-256 不符）：不留檔
        let mut bad = offer.clone();
        bad.asset.sha256 = "00".repeat(32);
        bad.asset.file = "other.exe".into();
        assert!(download(&http, &bad, dir.path(), &CancellationToken::new(), |_, _| ()).await.is_err());
        assert!(!dir.path().join("other.exe").exists());
    }
}
