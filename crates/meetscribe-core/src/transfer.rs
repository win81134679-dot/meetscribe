//! 傳到自己的其他裝置（2026-10-31 使用者：「我可不可以用識別碼 User 可以傳給自己別台裝置用識別碼傳 要有資安防護 傳完線上馬上刪除」）。
//! 電腦與手機共用；伺服器是 `relay/`（Cloudflare Workers＋R2＋D1），只轉手密文。
//!
//! 資安：
//! - 每台裝置一把 P-256 金鑰（`transfer/identity.json`，App 私有資料夾），識別碼只是地址、不是密碼。
//! - 配對：A 輸入 B 的識別碼 → B 畫面顯示由**雙方公鑰**算出的 6 位數配對碼（[`pair_code`]）→ A 輸入它。
//!   伺服器若偷換公鑰，兩邊算出的碼就對不上（百萬分之一）。配對後雙方記住對方的公鑰（`transfer/peers.json`），之後只信它。
//! - 傳送：每次產生一把臨時金鑰，跟收件端公鑰做 ECDH → HKDF-SHA256 → AES-256-GCM，32 MB 一段各自加密（AAD 綁傳送 ID 與段號）。
//!   標題、檔名也加密；寄件端用自己的金鑰簽中繼資料，收件端用配對時記住的公鑰驗，伺服器冒充不了。
//! - 伺服器的請求都簽章（裝置金鑰），收件端收完就刪；沒收的 3 小時後刪。

use crate::model::{Project, ProjectStatus};
use crate::store::ProjectStore;
use crate::{Error, Result};
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chrono::{DateTime, Local};
use hkdf::Hkdf;
use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::elliptic_curve::sec1::ToSec1Point;
use p256::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// 每段明文大小（Workers 免費方案每個請求上限 100 MB；手機記憶體也要顧）。
pub const CHUNK: usize = 32 * 1024 * 1024;
/// 單次傳送上限（同伺服器 `MAX_FILE_BYTES`）。
pub const MAX_BYTES: u64 = 500 * 1024 * 1024;
const TAG: u64 = 16;
const KDF_SALT: &[u8] = b"meetscribe-transfer-v1";

/// 傳什麼。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// 整場會議（專案資料夾：逐字稿、摘要、說話者、錄音）。
    Meeting,
    /// 還沒轉錄的錄音。
    Recording,
    /// 匯出檔（Word、PDF…）。
    File,
    /// API 金鑰（2026-10-31 使用者：「配對後是否可以共享API KEY」）。只在記憶體裡加解密，不落地。
    Keys,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Meeting => "會議",
            Kind::Recording => "錄音",
            Kind::File => "檔案",
            Kind::Keys => "API 金鑰",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Peer {
    pub id: String,
    pub name: String,
    /// 配對時記住的公鑰（未壓縮 SEC1，base64）。
    pub pubkey: String,
    pub paired_at: DateTime<Local>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    /// 包裹內的相對路徑（`/` 分隔）。
    pub path: String,
    pub size: u64,
}

/// 加密的中繼資料（伺服器看不到）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Meta {
    pub kind: Kind,
    pub title: String,
    pub files: Vec<FileEntry>,
    pub total: u64,
    /// 所有檔案依序串起來的 SHA-256（hex）。
    pub sha256: String,
    pub from_name: String,
    /// 會議：寄件端的專案 ID（收件端判斷「已經有這場會議」）。
    pub project_id: Option<Uuid>,
    pub sent_at: DateTime<Local>,
}

/// 收件匣裡一筆等著收的東西（中繼資料已解密、寄件端簽章已驗過）。
#[derive(Clone, Debug)]
pub struct Incoming {
    pub id: String,
    pub from: String,
    pub from_name: String,
    pub meta: Meta,
    pub size: u64,
    pub chunks: u64,
    pub chunk_size: u64,
    eph: String,
    pub expires_in_secs: i64,
}

/// 別台送來的配對請求（我是 B）：畫面顯示 `code` 給使用者唸給 A。
#[derive(Clone, Debug, PartialEq)]
pub struct PairRequest {
    pub id: String,
    pub from: String,
    pub from_name: String,
    pub from_pubkey: String,
    pub code: String,
}

/// 我送出的配對請求（我是 A）的狀態。
#[derive(Clone, Debug, PartialEq)]
pub struct MyPairRequest {
    pub id: String,
    pub to: String,
    pub to_name: String,
    /// pending／accepted／declined
    pub status: String,
    pub to_pubkey: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    Paired { peer: String },
    Unpaired { peer: String },
    PairCancelled { peer: String },
    Received { peer: String, transfer: String },
    Declined { peer: String, transfer: String },
}

#[derive(Clone, Debug, Default)]
pub struct Inbox {
    pub pair_requests: Vec<PairRequest>,
    pub my_pair_requests: Vec<MyPairRequest>,
    pub incoming: Vec<Incoming>,
    pub notices: Vec<Notice>,
    /// 配對清單有變（對方取消配對、我這邊接受的配對完成了）。
    pub peers_changed: bool,
    /// 這次自動收下、存好的 API 金鑰（[`Hub::poll`] 處理，不會出現在 `incoming`）。
    pub keys_synced: Vec<KeysSynced>,
}

/// 從哪台同步來了哪幾把金鑰（畫面提示用）。
#[derive(Clone, Debug, PartialEq)]
pub struct KeysSynced {
    pub peer_name: String,
    pub kinds: Vec<crate::secrets::KeyKind>,
    /// 例如「OpenRouter …a1b2」。
    pub title: String,
}

/// 金鑰同步（2026-10-31 使用者：「配對後就同步喔！」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySync {
    /// 剛配對完：對方只補上自己沒有的（兩台各有不同的金鑰時不會互相蓋掉）。
    Fill,
    /// 使用者改了金鑰：取代對方的。
    Replace,
}

/// 要送出去的東西。
#[derive(Clone, Debug)]
pub struct Outgoing {
    pub kind: Kind,
    pub title: String,
    pub project_id: Option<Uuid>,
    /// (本機檔案, 包裹內路徑)
    pub files: Vec<(PathBuf, String)>,
    /// 只在記憶體裡的內容（API 金鑰）：有的話不讀 `files` 的本機檔，`files` 只有一筆、只用它的包裹內路徑。
    pub secret: Option<SecretBytes>,
}

/// API 金鑰的位元組：Debug 不會印出內容，丟掉時清成 0。
#[derive(Clone, Default, PartialEq)]
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    pub fn new(b: Vec<u8>) -> Self {
        Self(b)
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretBytes({} bytes)", self.0.len())
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        for b in self.0.iter_mut() {
            // SAFETY: 寫自己擁有的記憶體；volatile 讓編譯器不能把清零省掉
            unsafe { std::ptr::write_volatile(b, 0) };
        }
    }
}

/// 金鑰包裹裡的檔名（收件端只認這個）。
const KEYS_FILE: &str = "api-keys.json";
/// 金鑰包裹的大小上限（防呆：真的金鑰才幾百位元組）。
const KEYS_MAX: u64 = 64 * 1024;

fn key_name(k: crate::secrets::KeyKind) -> &'static str {
    match k {
        crate::secrets::KeyKind::OpenRouter => "openrouter",
        crate::secrets::KeyKind::Google => "google",
    }
}

/// 畫面上的短名稱。
pub fn key_short_label(k: crate::secrets::KeyKind) -> &'static str {
    match k {
        crate::secrets::KeyKind::OpenRouter => "OpenRouter",
        crate::secrets::KeyKind::Google => "Google AI Studio",
    }
}

/// 末 4 碼（畫面上辨認是哪一把）。
pub fn key_tail(key: &str) -> String {
    let c: Vec<char> = key.chars().collect();
    c[c.len().saturating_sub(4)..].iter().collect()
}

impl Outgoing {
    /// 整場會議：專案資料夾裡的所有檔案。
    pub fn meeting(store: &ProjectStore, p: &Project) -> Result<Self> {
        let dir = store.dir(p.id);
        let mut files = Vec::new();
        collect(&dir, &dir, &mut files)?;
        files.sort_by(|a, b| a.1.cmp(&b.1));
        Ok(Self { kind: Kind::Meeting, title: p.title.clone(), project_id: Some(p.id), files, secret: None })
    }

    /// API 金鑰（一次性手動推送）。標題寫哪幾把與末 4 碼，例如「OpenRouter …a1b2、Google AI Studio …x9y8」；
    /// 標題在加密過的中繼資料裡，只有收件那台看得到。
    pub fn api_keys(keys: &[(crate::secrets::KeyKind, String)], mode: KeySync) -> Result<Self> {
        if keys.is_empty() {
            return Err(Error::Other("沒有選要傳的金鑰".into()));
        }
        let mut map: std::collections::BTreeMap<&str, &str> = keys.iter().map(|(k, v)| (key_name(*k), v.trim())).collect();
        map.insert("_mode", if mode == KeySync::Fill { "fill" } else { "replace" });
        let title = keys.iter().map(|(k, v)| format!("{} …{}", key_short_label(*k), key_tail(v.trim()))).collect::<Vec<_>>().join("、");
        Ok(Self {
            kind: Kind::Keys,
            title,
            project_id: None,
            files: vec![(PathBuf::new(), KEYS_FILE.into())],
            secret: Some(SecretBytes(serde_json::to_vec(&map)?)),
        })
    }

    /// 一個檔（錄音或匯出檔），包裹內用原檔名。
    pub fn file(kind: Kind, title: &str, path: &Path) -> Self {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "檔案".into());
        Self { kind, title: title.to_string(), project_id: None, files: vec![(path.to_path_buf(), name)], secret: None }
    }

    pub fn total(&self) -> u64 {
        if let Some(s) = &self.secret {
            return s.0.len() as u64;
        }
        self.files.iter().map(|(f, _)| std::fs::metadata(f).map(|m| m.len()).unwrap_or(0)).sum()
    }
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, String)>) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            collect(root, &p, out)?;
        } else if p.is_file() {
            let rel = p.strip_prefix(root).unwrap_or(&p).components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>();
            out.push((p.clone(), rel.join("/")));
        }
    }
    Ok(())
}

/// 收完、驗過雜湊的包裹：檔案放在 `dir` 裡（依 `meta.files` 的相對路徑）。
#[derive(Clone, Debug)]
pub struct Received {
    pub meta: Meta,
    pub from: String,
    pub dir: PathBuf,
    /// API 金鑰：內容只在記憶體裡（`dir` 是空的）。
    pub secret: Option<SecretBytes>,
}

impl Received {
    pub fn paths(&self) -> Vec<PathBuf> {
        self.meta.files.iter().map(|f| self.dir.join(&f.path)).collect()
    }
}

#[derive(Serialize, Deserialize)]
struct IdentityFile {
    device_id: String,
    secret: String,
}

#[derive(Serialize, Deserialize)]
struct SignedMeta {
    meta: String,
    sig: String,
}

/// 一台裝置的傳送功能：金鑰、配對清單、跟中轉站講話。
pub struct Transfer {
    dir: PathBuf,
    device_id: String,
    secret: SecretKey,
    base: String,
    http: reqwest::Client,
}

impl Transfer {
    /// `app_root`：程式資料夾（電腦 `%APPDATA%\MeetScribe`、手機 `files/data`）；`relay_url` 空白＝還沒設定中轉站。
    pub fn open(app_root: &Path, relay_url: &str) -> Result<Self> {
        let base = relay_url.trim().trim_end_matches('/').to_string();
        if base.is_empty() {
            return Err(Error::Other("還沒設定中轉站網址".into()));
        }
        let dir = app_root.join("transfer");
        std::fs::create_dir_all(&dir)?;
        let device_id = crate::update::device_id(app_root);
        let secret = load_or_create_identity(&dir, &device_id)?;
        Ok(Self { dir, device_id, secret, base, http: crate::http::client(600)? })
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn public_key(&self) -> String {
        pub_b64(&self.secret.public_key())
    }

    // ---------- 配對清單（本機） ----------

    pub fn peers(&self) -> Vec<Peer> {
        std::fs::read(self.dir.join("peers.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_peers(&self, peers: &[Peer]) -> Result<()> {
        crate::store::write_atomic(&self.dir.join("peers.json"), &serde_json::to_vec_pretty(peers)?)
    }

    /// 我接受了別人的配對、等對方輸入配對碼：先記著公鑰，等伺服器說配對完成才放進清單。
    fn pending(&self) -> Vec<Peer> {
        std::fs::read(self.dir.join("pending.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_pending(&self, list: &[Peer]) -> Result<()> {
        crate::store::write_atomic(&self.dir.join("pending.json"), &serde_json::to_vec_pretty(list)?)
    }

    pub fn rename_peer(&self, id: &str, name: &str) -> Result<()> {
        let mut peers = self.peers();
        if let Some(p) = peers.iter_mut().find(|p| p.id == id) {
            p.name = name.trim().to_string();
        }
        self.save_peers(&peers)
    }

    // ---------- 中轉站 ----------

    fn sign_headers(&self, method: &str, path: &str, body_hash: &str) -> Vec<(&'static str, String)> {
        let time = chrono::Utc::now().timestamp();
        let msg = format!("{method}\n{path}\n{time}\n{body_hash}");
        let sig: Signature = SigningKey::from(&self.secret).sign(msg.as_bytes());
        vec![("x-device", self.device_id.clone()), ("x-time", time.to_string()), ("x-sig", B64.encode(sig.to_bytes()))]
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Option<serde_json::Value>) -> Result<serde_json::Value> {
        let bytes = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
        let hash = hex(&Sha256::digest(&bytes));
        let mut rb = self.http.request(method.clone(), format!("{}{path}", self.base));
        for (k, v) in self.sign_headers(method.as_str(), path, &hash) {
            rb = rb.header(k, v);
        }
        if !bytes.is_empty() {
            rb = rb.header("content-type", "application/json").body(bytes);
        }
        let resp = rb.send().await.map_err(net_error)?;
        decode(resp).await
    }

    /// 第一次（與改名後）向中轉站登記這台的公鑰與名稱。
    pub async fn register(&self, name: &str) -> Result<()> {
        let body = serde_json::json!({ "pubkey": self.public_key(), "name": name });
        self.call(reqwest::Method::POST, "/v1/register", Some(body)).await.map(|_| ())
    }

    /// 查收件匣，並依伺服器的配對清單同步本機（對方取消配對就移除；我接受的配對完成就加入）。
    pub async fn inbox(&self) -> Result<Inbox> {
        let v = self.call(reqwest::Method::GET, "/v1/inbox", None).await?;
        let now = v["now"].as_i64().unwrap_or_else(|| chrono::Utc::now().timestamp());
        let mut out = Inbox::default();
        let my_pub = self.public_key();

        // 配對清單：以伺服器為準決定「誰還在」，公鑰只信本機記住的
        let server: Vec<(String, String)> = arr(&v["peers"])
            .iter()
            .filter_map(|p| Some((p["id"].as_str()?.to_string(), p["name"].as_str().unwrap_or("").to_string())))
            .collect();
        let mut peers = self.peers();
        let before = peers.len();
        peers.retain(|p| server.iter().any(|(id, _)| *id == p.id));
        let mut pending = self.pending();
        let pending_before = pending.len();
        for (id, name) in &server {
            if !peers.iter().any(|p| p.id == *id)
                && let Some(i) = pending.iter().position(|p| p.id == *id)
            {
                let mut p = pending.remove(i);
                if p.name.is_empty() {
                    p.name = name.clone();
                }
                peers.push(p);
            }
        }
        if peers.len() != before || pending.len() != pending_before {
            out.peers_changed = true;
            self.save_peers(&peers)?;
            self.save_pending(&pending)?;
        }

        for r in arr(&v["pair_requests"]) {
            let (Some(id), Some(from), Some(pk)) = (r["id"].as_str(), r["from_id"].as_str(), r["from_pubkey"].as_str()) else {
                continue;
            };
            out.pair_requests.push(PairRequest {
                id: id.into(),
                from: from.into(),
                from_name: r["from_name"].as_str().unwrap_or("").into(),
                from_pubkey: pk.into(),
                code: pair_code(pk, &my_pub),
            });
        }
        for r in arr(&v["my_pair_requests"]) {
            out.my_pair_requests.push(MyPairRequest {
                id: r["id"].as_str().unwrap_or("").into(),
                to: r["to_id"].as_str().unwrap_or("").into(),
                to_name: r["to_name"].as_str().unwrap_or("").into(),
                status: r["status"].as_str().unwrap_or("").into(),
                to_pubkey: r["to_pubkey"].as_str().unwrap_or("").into(),
            });
        }
        for n in arr(&v["notices"]) {
            let peer = n["from_id"].as_str().unwrap_or("").to_string();
            let transfer = n["ref"].as_str().unwrap_or("").to_string();
            out.notices.push(match n["kind"].as_str().unwrap_or("") {
                "paired" => Notice::Paired { peer },
                "unpaired" => Notice::Unpaired { peer },
                "pair_cancelled" => Notice::PairCancelled { peer },
                "received" => Notice::Received { peer, transfer },
                "declined" => Notice::Declined { peer, transfer },
                _ => continue,
            });
        }
        for t in arr(&v["transfers"]) {
            let id = t["id"].as_str().unwrap_or("").to_string();
            let from = t["from_id"].as_str().unwrap_or("").to_string();
            let eph = t["eph"].as_str().unwrap_or("").to_string();
            let opened = peers
                .iter()
                .find(|p| p.id == from)
                .ok_or_else(|| Error::Other("不是配對過的裝置".into()))
                .and_then(|peer| self.open_meta(&id, peer, &eph, t["meta"].as_str().unwrap_or("")).map(|m| (peer.name.clone(), m)));
            match opened {
                Ok((from_name, meta)) => out.incoming.push(Incoming {
                    id,
                    from,
                    from_name,
                    meta,
                    size: t["size"].as_u64().unwrap_or(0),
                    chunks: t["chunks"].as_u64().unwrap_or(0),
                    chunk_size: t["chunk_size"].as_u64().unwrap_or(0),
                    eph,
                    expires_in_secs: t["expires"].as_i64().unwrap_or(now) - now,
                }),
                Err(e) => {
                    // 解不開或簽章不對：不是配對過的裝置送的（或被竄改），直接刪掉
                    tracing::warn!("略過無法驗證的傳送 {id}：{e}");
                    let _ = self.finish(&id, "declined").await;
                }
            }
        }
        Ok(out)
    }

    // ---------- 配對 ----------

    /// A：送出配對請求給識別碼 `to`。
    pub async fn pair_start(&self, to: &str, my_name: &str) -> Result<String> {
        let to = to.trim().to_uppercase();
        if !crate::update::is_device_id(&to) {
            return Err(Error::Other("識別碼格式錯誤（例如 K7Q2-9XMA，在對方的「設定」裡）".into()));
        }
        let v = self.call(reqwest::Method::POST, "/v1/pair", Some(serde_json::json!({ "to": to, "name": my_name }))).await?;
        v["id"].as_str().map(str::to_string).ok_or_else(|| Error::Other("中轉站沒有回配對編號".into()))
    }

    /// B：接受配對（之後等 A 輸入配對碼）。
    pub async fn pair_accept(&self, req: &PairRequest, my_name: &str) -> Result<()> {
        let path = format!("/v1/pair/{}/accept", req.id);
        self.call(reqwest::Method::POST, &path, Some(serde_json::json!({ "name": my_name }))).await?;
        let mut pending = self.pending();
        pending.retain(|p| p.id != req.from);
        pending.push(Peer { id: req.from.clone(), name: req.from_name.clone(), pubkey: req.from_pubkey.clone(), paired_at: Local::now() });
        self.save_pending(&pending)
    }

    pub async fn pair_decline(&self, req: &PairRequest) -> Result<()> {
        self.call(reqwest::Method::POST, &format!("/v1/pair/{}/decline", req.id), Some(serde_json::json!({}))).await.map(|_| ())
    }

    /// A：輸入 B 畫面上的配對碼。對了就完成配對、記住 B 的公鑰，回 true；不對回 false（呼叫端記次數，錯 3 次 [`Self::pair_cancel`]）。
    pub async fn pair_confirm(&self, req: &MyPairRequest, code: &str) -> Result<bool> {
        if req.status != "accepted" {
            return Err(Error::Other("對方還沒按「接受」".into()));
        }
        let want = pair_code(&self.public_key(), &req.to_pubkey);
        let typed: String = code.chars().filter(char::is_ascii_digit).collect();
        if typed != want {
            return Ok(false);
        }
        self.call(reqwest::Method::POST, &format!("/v1/pair/{}/confirm", req.id), Some(serde_json::json!({}))).await?;
        let mut peers = self.peers();
        peers.retain(|p| p.id != req.to);
        peers.push(Peer { id: req.to.clone(), name: req.to_name.clone(), pubkey: req.to_pubkey.clone(), paired_at: Local::now() });
        self.save_peers(&peers)?;
        Ok(true)
    }

    pub async fn pair_cancel(&self, id: &str) -> Result<()> {
        self.call(reqwest::Method::POST, &format!("/v1/pair/{id}/cancel"), Some(serde_json::json!({}))).await.map(|_| ())
    }

    /// 取消配對：兩邊都收不到對方的東西，伺服器上還沒收的一起刪。
    pub async fn unpair(&self, peer: &str) -> Result<()> {
        self.call(reqwest::Method::POST, "/v1/unpair", Some(serde_json::json!({ "peer": peer }))).await?;
        let mut peers = self.peers();
        peers.retain(|p| p.id != peer);
        self.save_peers(&peers)
    }

    // ---------- 傳送 ----------

    /// 加密、分段上傳給 `peer`。`progress(已送, 總共)`。斷線時每段重試（最多 6 次、間隔漸長）；失敗或取消會把線上那份刪掉。
    pub async fn send(
        &self,
        peer: &Peer,
        out: &Outgoing,
        from_name: &str,
        progress: &mut (dyn FnMut(u64, u64) + Send),
        cancel: &CancellationToken,
    ) -> Result<String> {
        let total = out.total();
        if total == 0 {
            return Err(Error::Other("沒有可以傳的內容".into()));
        }
        if total > MAX_BYTES {
            return Err(Error::Other(format!(
                "太大了（{} MB），一次最多傳 {} MB",
                total.div_ceil(1 << 20),
                MAX_BYTES >> 20
            )));
        }
        let peer_pub = parse_pub(&peer.pubkey)?;
        let (files, sha) = if let Some(secret) = &out.secret {
            (vec![FileEntry { path: KEYS_FILE.into(), size: total }], hex(&Sha256::digest(secret.as_slice())))
        } else {
            let files: Vec<FileEntry> = out
                .files
                .iter()
                .map(|(f, rel)| Ok(FileEntry { path: rel.clone(), size: std::fs::metadata(f)?.len() }))
                .collect::<Result<_>>()?;
            let paths: Vec<PathBuf> = out.files.iter().map(|(f, _)| f.clone()).collect();
            let sha = tokio::task::spawn_blocking(move || hash_files(&paths)).await.map_err(|e| Error::Other(e.to_string()))??;
            (files, sha)
        };
        let meta = Meta {
            kind: out.kind,
            title: out.title.clone(),
            files,
            total,
            sha256: sha,
            from_name: from_name.to_string(),
            project_id: out.project_id,
            sent_at: Local::now(),
        };
        let eph = SecretKey::from_slice(&random32()).map_err(|_| Error::Other("產生金鑰失敗".into()))?;
        let eph_b64 = pub_b64(&eph.public_key());
        let key = derive_key(&eph, &peer_pub, &eph_b64, &peer.pubkey);
        let meta_ct = self.seal_meta(&key, &meta, &eph_b64, &peer.id)?;
        let chunks = total.div_ceil(CHUNK as u64);
        let size = total + TAG * chunks;
        let body = serde_json::json!({
            "to": peer.id, "size": size, "chunks": chunks, "chunk_size": CHUNK, "eph": eph_b64, "meta": meta_ct,
        });
        let v = self.call(reqwest::Method::POST, "/v1/transfers", Some(body)).await?;
        let id = v["id"].as_str().ok_or_else(|| Error::Other("中轉站沒有回傳送編號".into()))?.to_string();

        let result = self.upload(&id, &key, out, total, chunks, progress, cancel).await;
        match result {
            Ok(()) => {
                self.call(reqwest::Method::POST, &format!("/v1/transfers/{id}/ready"), Some(serde_json::json!({}))).await?;
                Ok(id)
            }
            Err(e) => {
                let _ = self.finish(&id, "cancelled").await;
                Err(e)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn upload(
        &self,
        id: &str,
        key: &[u8; 32],
        out: &Outgoing,
        total: u64,
        chunks: u64,
        progress: &mut (dyn FnMut(u64, u64) + Send),
        cancel: &CancellationToken,
    ) -> Result<()> {
        let mut reader = match &out.secret {
            Some(s) => PackageReader::memory(s.clone()),
            None => PackageReader::new(out.files.iter().map(|(f, _)| f.clone()).collect()),
        };
        let mut sent = 0u64;
        progress(0, total);
        for n in 0..chunks {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let (plain, r) = tokio::task::spawn_blocking(move || {
                let mut buf = vec![0u8; CHUNK];
                let got = reader.fill(&mut buf);
                (got.map(|len| {
                    buf.truncate(len);
                    buf
                }), reader)
            })
            .await
            .map_err(|e| Error::Other(e.to_string()))?;
            reader = r;
            let plain = plain?;
            let len = plain.len() as u64;
            let ct = encrypt_chunk(key, id, n, chunks, &plain)?;
            drop(plain);
            let path = format!("/v1/transfers/{id}/{n}");
            self.with_retry(cancel, || async {
                let mut rb = self.http.put(format!("{}{path}", self.base));
                for (k, v) in self.sign_headers("PUT", &path, "UNSIGNED-PAYLOAD") {
                    rb = rb.header(k, v);
                }
                let resp = rb.header("content-type", "application/octet-stream").body(ct.clone()).send().await.map_err(net_error)?;
                decode(resp).await.map(|_| ())
            })
            .await?;
            sent += len;
            progress(sent, total);
        }
        Ok(())
    }

    /// 下載、解密、驗雜湊，檔案放進 `staging`（之後由呼叫端搬到該放的地方）。中途斷線再叫一次會從斷掉的那段繼續。
    /// 收完通知中轉站刪掉線上那份。
    pub async fn receive(
        &self,
        item: &Incoming,
        staging: &Path,
        progress: &mut (dyn FnMut(u64, u64) + Send),
        cancel: &CancellationToken,
    ) -> Result<Received> {
        let peer = self
            .peers()
            .into_iter()
            .find(|p| p.id == item.from)
            .ok_or_else(|| Error::Other("已經不是配對的裝置".into()))?;
        let my_pub = self.public_key();
        let key = derive_key_recv(&self.secret, &item.eph, &my_pub)?;
        if item.meta.kind == Kind::Keys {
            return self.receive_keys(item, &key, peer, staging, progress, cancel).await;
        }
        let parts = self.dir.join("incoming").join(&item.id);
        std::fs::create_dir_all(&parts)?;
        let total = item.meta.total;
        let mut done = 0u64;
        progress(0, total);
        for n in 0..item.chunks {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let part = parts.join(n.to_string());
            if let Ok(m) = std::fs::metadata(&part) {
                done += m.len();
                progress(done, total);
                continue;
            }
            let path = format!("/v1/transfers/{}/{n}", item.id);
            let ct = self
                .with_retry(cancel, || async {
                    let mut rb = self.http.get(format!("{}{path}", self.base));
                    let hash = hex(&Sha256::digest([]));
                    for (k, v) in self.sign_headers("GET", &path, &hash) {
                        rb = rb.header(k, v);
                    }
                    let resp = rb.send().await.map_err(net_error)?;
                    if !resp.status().is_success() {
                        return decode(resp).await.map(|_| Vec::new());
                    }
                    Ok(resp.bytes().await.map_err(net_error)?.to_vec())
                })
                .await?;
            let plain = decrypt_chunk(&key, &item.id, n, item.chunks, &ct)?;
            let tmp = parts.join(format!("{n}.tmp"));
            std::fs::write(&tmp, &plain)?;
            std::fs::rename(&tmp, &part)?;
            done += plain.len() as u64;
            progress(done, total);
        }
        // 串起來切回各個檔案，邊寫邊算雜湊
        let meta = item.meta.clone();
        let chunks = item.chunks;
        let out_dir = staging.to_path_buf();
        let parts2 = parts.clone();
        tokio::task::spawn_blocking(move || unpack(&parts2, chunks, &meta, &out_dir))
            .await
            .map_err(|e| Error::Other(e.to_string()))??;
        let _ = std::fs::remove_dir_all(&parts);
        // 收完就刪線上那份（2026-10-31 使用者：「傳完線上馬上刪除」）
        self.finish(&item.id, "received").await?;
        Ok(Received { meta: item.meta.clone(), from: peer.id, dir: staging.to_path_buf(), secret: None })
    }

    /// API 金鑰：下載、解密、驗雜湊都在記憶體裡，不寫任何檔。
    async fn receive_keys(
        &self,
        item: &Incoming,
        key: &[u8; 32],
        peer: Peer,
        staging: &Path,
        progress: &mut (dyn FnMut(u64, u64) + Send),
        cancel: &CancellationToken,
    ) -> Result<Received> {
        let total = item.meta.total;
        if total > KEYS_MAX || item.meta.files.len() != 1 || item.meta.files[0].path != KEYS_FILE {
            return Err(Error::Other("金鑰包裹格式不對".into()));
        }
        progress(0, total);
        let mut plain = SecretBytes::default();
        for n in 0..item.chunks {
            let path = format!("/v1/transfers/{}/{n}", item.id);
            let ct = self
                .with_retry(cancel, || async {
                    let mut rb = self.http.get(format!("{}{path}", self.base));
                    let hash = hex(&Sha256::digest([]));
                    for (k, v) in self.sign_headers("GET", &path, &hash) {
                        rb = rb.header(k, v);
                    }
                    let resp = rb.send().await.map_err(net_error)?;
                    if !resp.status().is_success() {
                        return decode(resp).await.map(|_| Vec::new());
                    }
                    Ok(resp.bytes().await.map_err(net_error)?.to_vec())
                })
                .await?;
            let part = SecretBytes(decrypt_chunk(key, &item.id, n, item.chunks, &ct)?);
            plain.0.extend_from_slice(part.as_slice());
        }
        if plain.0.len() as u64 != total || hex(&Sha256::digest(plain.as_slice())) != item.meta.sha256 {
            return Err(Error::Other("金鑰內容的雜湊不符".into()));
        }
        progress(total, total);
        self.finish(&item.id, "received").await?;
        Ok(Received { meta: item.meta.clone(), from: peer.id, dir: staging.to_path_buf(), secret: Some(plain) })
    }

    /// 拒收：線上那份立刻刪掉。
    pub async fn decline(&self, item: &Incoming) -> Result<()> {
        let _ = std::fs::remove_dir_all(self.dir.join("incoming").join(&item.id));
        self.finish(&item.id, "declined").await
    }

    async fn finish(&self, id: &str, result: &str) -> Result<()> {
        self.call(reqwest::Method::DELETE, &format!("/v1/transfers/{id}"), Some(serde_json::json!({ "result": result })))
            .await
            .map(|_| ())
    }

    async fn with_retry<T, F, Fut>(&self, cancel: &CancellationToken, mut f: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut attempt = 0u32;
        loop {
            let r = tokio::select! {
                _ = cancel.cancelled() => return Err(Error::Cancelled),
                r = f() => r,
            };
            match r {
                Ok(v) => return Ok(v),
                Err(e) if attempt < 6 && retryable(&e) => {
                    attempt += 1;
                    let wait = Duration::from_secs(2u64.pow(attempt.min(5)));
                    tracing::warn!("傳送重試（第 {attempt} 次，{} 秒後）：{e}", wait.as_secs());
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(Error::Cancelled),
                        _ = tokio::time::sleep(wait) => {}
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    // ---------- 中繼資料 ----------

    fn seal_meta(&self, key: &[u8; 32], meta: &Meta, eph: &str, to: &str) -> Result<String> {
        let json = serde_json::to_string(meta)?;
        let sig: Signature = SigningKey::from(&self.secret).sign(&meta_digest(eph, to, &json));
        let signed = serde_json::to_vec(&SignedMeta { meta: json, sig: B64.encode(sig.to_bytes()) })?;
        let ct = cipher(key)?
            .encrypt(&meta_nonce(), Payload { msg: &signed, aad: b"meta" })
            .map_err(|_| Error::Other("加密失敗".into()))?;
        Ok(B64.encode(ct))
    }

    fn open_meta(&self, _id: &str, peer: &Peer, eph: &str, ct: &str) -> Result<Meta> {
        let my_pub = self.public_key();
        let key = derive_key_recv(&self.secret, eph, &my_pub)?;
        let ct = B64.decode(ct).map_err(|_| Error::Other("中繼資料格式錯誤".into()))?;
        let plain = cipher(&key)?
            .decrypt(&meta_nonce(), Payload { msg: &ct, aad: b"meta" })
            .map_err(|_| Error::Other("解不開（不是送給這台的）".into()))?;
        let signed: SignedMeta = serde_json::from_slice(&plain)?;
        let vk = VerifyingKey::from_sec1_bytes(&B64.decode(&peer.pubkey).unwrap_or_default())
            .map_err(|_| Error::Other("配對的公鑰格式錯誤".into()))?;
        let sig = Signature::from_slice(&B64.decode(&signed.sig).unwrap_or_default()).map_err(|_| Error::Other("簽章格式錯誤".into()))?;
        vk.verify(&meta_digest(eph, &self.device_id, &signed.meta), &sig)
            .map_err(|_| Error::Other("寄件裝置的簽章不符".into()))?;
        Ok(serde_json::from_str(&signed.meta)?)
    }
}

// ---------- 匯入 ----------

/// 收到的會議放進專案庫。這台沒有同 ID 的會議就照原樣放進來；有的話看 `replace`：取代，
/// 或另存一份（新 ID，標題加「（來自 <裝置>）」）。
/// `export`：收件端這台的輸出選項（寄件端的資料夾路徑在這台沒有意義）。
pub fn import_meeting(
    received: &Received,
    store: &ProjectStore,
    replace: bool,
    export: crate::export::ExportOptions,
) -> Result<Project> {
    let mut p: Project = serde_json::from_slice(&std::fs::read(received.dir.join("project.json"))?)?;
    let exists = store.dir(p.id).join("project.json").is_file();
    if exists && replace {
        store.delete(p.id)?;
    } else if exists {
        p.id = Uuid::new_v4();
        p.title = format!("{}（來自 {}）", p.title, received.meta.from_name);
    }
    let dir = store.dir(p.id);
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(&received.dir, &dir).is_err() {
        copy_dir(&received.dir, &dir)?;
        let _ = std::fs::remove_dir_all(&received.dir);
    }
    // 原始錄音的路徑是寄件端的；改指這台的播放檔，續跑、重新辨識才找得到音訊
    if let Some(f) = p.playback_audio().map(|rel| dir.join(rel)).filter(|f| f.is_file()) {
        p.source_file = f;
    }
    p.exported.clear();
    p.options.export = Some(export);
    if matches!(p.status, ProjectStatus::Running | ProjectStatus::Queued) {
        p.status = ProjectStatus::Cancelled;
    }
    store.save(&p)?;
    Ok(p)
}

/// 收到的單一檔案（錄音、匯出檔）搬到 `dir`，同名就加「(2)」。回傳最後的路徑。
pub fn place_file(received: &Received, dir: &Path) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let mut out = Vec::new();
    for f in &received.meta.files {
        let src = received.dir.join(&f.path);
        let name = Path::new(&f.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "檔案".into());
        let target = unique(dir, &name);
        if std::fs::rename(&src, &target).is_err() {
            std::fs::copy(&src, &target)?;
            let _ = std::fs::remove_file(&src);
        }
        out.push(target);
    }
    let _ = std::fs::remove_dir_all(&received.dir);
    Ok(out)
}

/// 收到的 API 金鑰存進這台的安全位置（電腦：Windows 認證管理員；手機：App 私有空間），回傳存了哪幾把。
pub fn save_api_keys(received: &Received) -> Result<Vec<crate::secrets::KeyKind>> {
    use crate::secrets::KeyKind;
    let secret = received.secret.as_ref().ok_or_else(|| Error::Other("不是金鑰包裹".into()))?;
    let map: std::collections::BTreeMap<String, String> =
        serde_json::from_slice(secret.as_slice()).map_err(|_| Error::Other("金鑰包裹格式不對".into()))?;
    let fill = map.get("_mode").is_some_and(|m| m == "fill");
    let mut saved = Vec::new();
    for kind in [KeyKind::OpenRouter, KeyKind::Google] {
        if let Some(k) = map.get(key_name(kind)).map(|k| k.trim()).filter(|k| !k.is_empty()) {
            // 補缺模式：這台已經有這把就不動
            if fill && crate::secrets::load(kind).is_some() {
                continue;
            }
            if crate::secrets::load(kind).is_some_and(|(cur, _)| cur == k) {
                continue;
            }
            crate::secrets::save(kind, k)?;
            saved.push(kind);
        }
    }
    Ok(saved)
}

/// 這台存著的金鑰（只算存在金鑰存放處的；環境變數的不同步）。
fn stored_keys() -> Vec<(crate::secrets::KeyKind, String)> {
    use crate::secrets::{KeyKind, KeySource};
    [KeyKind::OpenRouter, KeyKind::Google]
        .into_iter()
        .filter_map(|k| match crate::secrets::load(k) {
            Some((v, KeySource::CredentialStore)) => Some((k, v)),
            _ => None,
        })
        .collect()
}

fn unique(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|f| !f.exists()).expect("unbounded")
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let p = e?.path();
        let t = to.join(p.file_name().unwrap_or_default());
        if p.is_dir() {
            copy_dir(&p, &t)?;
        } else {
            std::fs::copy(&p, &t)?;
        }
    }
    Ok(())
}

// ---------- 加密與編碼 ----------

/// 配對碼：雙方公鑰（請求者在前）的 SHA-256 取 6 位數。兩邊各自算，對得上才代表中間沒人換過公鑰。
pub fn pair_code(requester_pub: &str, responder_pub: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"meetscribe-pair-v1");
    h.update(requester_pub.as_bytes());
    h.update(b"|");
    h.update(responder_pub.as_bytes());
    let d = h.finalize();
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{n:06}")
}

fn random32() -> [u8; 32] {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).expect("系統亂數");
    b
}

fn load_or_create_identity(dir: &Path, device_id: &str) -> Result<SecretKey> {
    let file = dir.join("identity.json");
    if let Ok(bytes) = std::fs::read(&file)
        && let Ok(id) = serde_json::from_slice::<IdentityFile>(&bytes)
        && let Ok(raw) = B64.decode(&id.secret)
        && let Ok(k) = SecretKey::from_slice(&raw)
    {
        return Ok(k);
    }
    let k = loop {
        if let Ok(k) = SecretKey::from_slice(&random32()) {
            break k;
        }
    };
    let id = IdentityFile { device_id: device_id.to_string(), secret: B64.encode(k.to_bytes()) };
    crate::store::write_atomic(&file, &serde_json::to_vec_pretty(&id)?)?;
    Ok(k)
}

fn pub_b64(k: &PublicKey) -> String {
    B64.encode(k.to_sec1_point(false).as_bytes())
}

fn parse_pub(b64: &str) -> Result<PublicKey> {
    PublicKey::from_sec1_bytes(&B64.decode(b64).unwrap_or_default()).map_err(|_| Error::Other("公鑰格式錯誤".into()))
}

fn hkdf_key(shared: &[u8], eph_pub: &str, recipient_pub: &str) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(KDF_SALT), shared);
    let mut okm = [0u8; 32];
    let info = format!("{eph_pub}|{recipient_pub}");
    hk.expand(info.as_bytes(), &mut okm).expect("32 bytes");
    okm
}

/// 寄件端：臨時私鑰 × 收件端公鑰。
fn derive_key(eph: &SecretKey, recipient: &PublicKey, eph_pub: &str, recipient_pub: &str) -> [u8; 32] {
    let shared = p256::ecdh::diffie_hellman(eph.to_nonzero_scalar(), recipient.as_affine());
    hkdf_key(shared.raw_secret_bytes(), eph_pub, recipient_pub)
}

/// 收件端：自己的私鑰 × 臨時公鑰（結果同 [`derive_key`]）。
fn derive_key_recv(mine: &SecretKey, eph_pub: &str, my_pub: &str) -> Result<[u8; 32]> {
    let eph = parse_pub(eph_pub)?;
    let shared = p256::ecdh::diffie_hellman(mine.to_nonzero_scalar(), eph.as_affine());
    Ok(hkdf_key(shared.raw_secret_bytes(), eph_pub, my_pub))
}

fn cipher(key: &[u8; 32]) -> Result<Aes256Gcm> {
    Aes256Gcm::new_from_slice(key).map_err(|_| Error::Other("金鑰長度錯誤".into()))
}

fn chunk_nonce(n: u64) -> Nonce<aes_gcm::aead::consts::U12> {
    let mut b = [0u8; 12];
    b[..8].copy_from_slice(&n.to_be_bytes());
    Nonce::from(b)
}

fn meta_nonce() -> Nonce<aes_gcm::aead::consts::U12> {
    Nonce::from([0xFFu8; 12])
}

fn chunk_aad(id: &str, n: u64, chunks: u64) -> Vec<u8> {
    format!("{id}:{n}/{chunks}").into_bytes()
}

fn encrypt_chunk(key: &[u8; 32], id: &str, n: u64, chunks: u64, plain: &[u8]) -> Result<Vec<u8>> {
    cipher(key)?
        .encrypt(&chunk_nonce(n), Payload { msg: plain, aad: &chunk_aad(id, n, chunks) })
        .map_err(|_| Error::Other("加密失敗".into()))
}

fn decrypt_chunk(key: &[u8; 32], id: &str, n: u64, chunks: u64, ct: &[u8]) -> Result<Vec<u8>> {
    cipher(key)?
        .decrypt(&chunk_nonce(n), Payload { msg: ct, aad: &chunk_aad(id, n, chunks) })
        .map_err(|_| Error::Other(format!("第 {} 段解密失敗（內容被改過或金鑰不符）", n + 1)))
}

fn meta_digest(eph: &str, to: &str, json: &str) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(b"meetscribe-meta-v1|");
    h.update(eph.as_bytes());
    h.update(b"|");
    h.update(to.as_bytes());
    h.update(b"|");
    h.update(json.as_bytes());
    h.finalize().to_vec()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hash_files(paths: &[PathBuf]) -> Result<String> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    for p in paths {
        let mut f = std::fs::File::open(p)?;
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
    }
    Ok(hex(&h.finalize()))
}

/// 依序讀多個檔，像一條串流。
struct PackageReader {
    files: std::collections::VecDeque<PathBuf>,
    current: Option<std::fs::File>,
    /// 只在記憶體裡的內容（API 金鑰）與讀到哪裡。
    memory: Option<(SecretBytes, usize)>,
}

impl PackageReader {
    fn new(files: Vec<PathBuf>) -> Self {
        Self { files: files.into(), current: None, memory: None }
    }

    fn memory(data: SecretBytes) -> Self {
        Self { files: Default::default(), current: None, memory: Some((data, 0)) }
    }

    /// 盡量填滿 `buf`，回傳讀到的長度（0＝讀完）。
    fn fill(&mut self, buf: &mut [u8]) -> Result<usize> {
        if let Some((data, pos)) = &mut self.memory {
            let n = (data.0.len() - *pos).min(buf.len());
            buf[..n].copy_from_slice(&data.0[*pos..*pos + n]);
            *pos += n;
            return Ok(n);
        }
        let mut filled = 0;
        while filled < buf.len() {
            if self.current.is_none() {
                match self.files.pop_front() {
                    Some(p) => self.current = Some(std::fs::File::open(p)?),
                    None => break,
                }
            }
            let n = self.current.as_mut().expect("file").read(&mut buf[filled..])?;
            if n == 0 {
                self.current = None;
            } else {
                filled += n;
            }
        }
        Ok(filled)
    }
}

/// 包裹內路徑只收一般的相對路徑（不准 `..`、絕對路徑、磁碟代號），免得被寫到資料夾外面。
fn safe_rel(path: &str) -> Result<PathBuf> {
    let p = Path::new(path);
    if path.is_empty() || p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(Error::Other(format!("檔名不安全：{path}")));
    }
    Ok(p.to_path_buf())
}

fn unpack(parts: &Path, chunks: u64, meta: &Meta, out: &Path) -> Result<()> {
    let targets: Vec<(PathBuf, u64)> = meta.files.iter().map(|f| Ok((out.join(safe_rel(&f.path)?), f.size))).collect::<Result<_>>()?;
    if meta.files.iter().map(|f| f.size).sum::<u64>() != meta.total {
        return Err(Error::Other("檔案清單與大小不符".into()));
    }
    std::fs::create_dir_all(out)?;
    let mut h = Sha256::new();
    let mut idx = 0usize;
    let mut left = targets.first().map(|t| t.1).unwrap_or(0);
    let mut file: Option<std::fs::File> = None;
    let open = |i: usize| -> Result<std::fs::File> {
        if let Some(parent) = targets[i].0.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(std::fs::File::create(&targets[i].0)?)
    };
    if !targets.is_empty() {
        file = Some(open(0)?);
    }
    for n in 0..chunks {
        let data = std::fs::read(parts.join(n.to_string()))?;
        h.update(&data);
        let mut pos = 0usize;
        while pos < data.len() {
            while left == 0 {
                idx += 1;
                if idx >= targets.len() {
                    return Err(Error::Other("內容比檔案清單長".into()));
                }
                file = Some(open(idx)?);
                left = targets[idx].1;
            }
            let take = (left as usize).min(data.len() - pos);
            file.as_mut().expect("file").write_all(&data[pos..pos + take])?;
            pos += take;
            left -= take as u64;
        }
    }
    // 結尾的空檔案也要建出來
    while idx + 1 < targets.len() {
        idx += 1;
        open(idx)?;
    }
    if hex(&h.finalize()) != meta.sha256 {
        return Err(Error::Other("檔案不完整（雜湊不符），請重新傳送".into()));
    }
    Ok(())
}

fn arr(v: &serde_json::Value) -> Vec<serde_json::Value> {
    v.as_array().cloned().unwrap_or_default()
}

fn net_error(e: reqwest::Error) -> Error {
    if e.is_connect() || e.is_timeout() {
        Error::Other(format!("連不上中轉站：{e}"))
    } else {
        Error::Http(e)
    }
}

fn retryable(e: &Error) -> bool {
    match e {
        Error::Http(_) => true,
        Error::Other(m) => m.starts_with("連不上中轉站") || m.starts_with("中轉站暫時"),
        Error::Api { status, .. } => *status >= 500 || *status == 429,
        _ => false,
    }
}

async fn decode(resp: reqwest::Response) -> Result<serde_json::Value> {
    let status = resp.status();
    let text = resp.text().await.map_err(net_error)?;
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    if status.is_success() {
        return Ok(v);
    }
    let msg = v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("中轉站回應 {status}"));
    if status.as_u16() >= 500 && status.as_u16() != 503 {
        return Err(Error::Other(format!("中轉站暫時有問題：{msg}")));
    }
    Err(Error::Api { status: status.as_u16(), message: msg })
}

// ---------- 介面用的狀態（電腦 worker 與手機 bridge 共用） ----------

/// 輸入配對碼的結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairResult {
    Paired,
    /// 碼不對，還可以再試幾次。
    Wrong { left: u32 },
    /// 錯 3 次，這次配對作廢（要重新送出）。
    Cancelled,
}

/// 配對碼最多錯幾次（2026-10-31 使用者確認：錯 3 次作廢）。
pub const PAIR_ATTEMPTS: u32 = 3;

/// 包一層給介面用：記住最近一次收件匣（收件要用裡面的臨時公鑰）、配對碼錯幾次、名稱有沒有登記過。
pub struct Hub {
    t: Transfer,
    name: std::sync::Mutex<String>,
    registered: std::sync::Mutex<Option<String>>,
    incoming: std::sync::Mutex<Vec<Incoming>>,
    requests: std::sync::Mutex<Vec<PairRequest>>,
    my_requests: std::sync::Mutex<Vec<MyPairRequest>>,
    attempts: std::sync::Mutex<std::collections::HashMap<String, u32>>,
}

impl Hub {
    pub fn open(app_root: &Path, relay_url: &str, name: &str) -> Result<Self> {
        Ok(Self {
            t: Transfer::open(app_root, relay_url)?,
            name: std::sync::Mutex::new(name.trim().to_string()),
            registered: Default::default(),
            incoming: Default::default(),
            requests: Default::default(),
            my_requests: Default::default(),
            attempts: Default::default(),
        })
    }

    pub fn transfer(&self) -> &Transfer {
        &self.t
    }

    pub fn device_id(&self) -> &str {
        self.t.device_id()
    }

    pub fn name(&self) -> String {
        self.name.lock().expect("name").clone()
    }

    /// 改名：下次跟中轉站講話時重新登記。
    pub fn set_name(&self, name: &str) {
        *self.name.lock().expect("name") = name.trim().to_string();
    }

    pub fn peers(&self) -> Vec<Peer> {
        self.t.peers()
    }

    async fn ensure_registered(&self) -> Result<()> {
        let name = self.name();
        if self.registered.lock().expect("registered").as_deref() == Some(name.as_str()) {
            return Ok(());
        }
        self.t.register(&name).await?;
        *self.registered.lock().expect("registered") = Some(name);
        Ok(())
    }

    /// 查收件匣（程式開著時每分鐘一次、一打開先一次）。
    pub async fn poll(&self) -> Result<Inbox> {
        self.ensure_registered().await?;
        let before: std::collections::HashSet<String> = self.t.peers().into_iter().map(|p| p.id).collect();
        let mut inbox = self.t.inbox().await?;
        // 金鑰自動收下、存好（只有配對過的裝置送得進來，中繼資料的簽章已驗過）
        let (keys, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut inbox.incoming).into_iter().partition(|i| i.meta.kind == Kind::Keys);
        inbox.incoming = rest;
        for item in keys {
            let none = CancellationToken::new();
            match self.t.receive(&item, &std::env::temp_dir(), &mut |_, _| {}, &none).await {
                Ok(got) => match save_api_keys(&got) {
                    Ok(kinds) => {
                        let title = kinds.iter().map(|k| key_short_label(*k)).collect::<Vec<_>>().join("、");
                        if !kinds.is_empty() {
                            inbox.keys_synced.push(KeysSynced { peer_name: item.from_name.clone(), kinds, title });
                        }
                    }
                    Err(e) => tracing::warn!("存同步來的金鑰失敗：{e}"),
                },
                Err(e) => tracing::warn!("收同步來的金鑰失敗（下次再試）：{e}"),
            }
        }
        // 剛配對完成的裝置（我是接受的那台）：把我的金鑰補給它
        let new: Vec<String> = self.t.peers().into_iter().map(|p| p.id).filter(|id| !before.contains(id)).collect();
        for id in new {
            if let Err(e) = self.sync_keys(Some(&id), KeySync::Fill).await {
                tracing::warn!("配對後同步金鑰失敗：{e}");
            }
        }
        *self.incoming.lock().expect("incoming") = inbox.incoming.clone();
        *self.requests.lock().expect("requests") = inbox.pair_requests.clone();
        *self.my_requests.lock().expect("my") = inbox.my_pair_requests.clone();
        Ok(inbox)
    }

    pub fn incoming(&self, id: &str) -> Option<Incoming> {
        self.incoming.lock().expect("incoming").iter().find(|i| i.id == id).cloned()
    }

    pub async fn pair_start(&self, to: &str) -> Result<String> {
        self.ensure_registered().await?;
        let id = self.t.pair_start(to, &self.name()).await?;
        self.attempts.lock().expect("attempts").insert(id.clone(), 0);
        Ok(id)
    }

    pub async fn pair_accept(&self, id: &str) -> Result<()> {
        let req = self.request(id)?;
        self.t.pair_accept(&req, &self.name()).await
    }

    pub async fn pair_decline(&self, id: &str) -> Result<()> {
        let req = self.request(id)?;
        self.t.pair_decline(&req).await
    }

    fn request(&self, id: &str) -> Result<PairRequest> {
        self.requests
            .lock()
            .expect("requests")
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| Error::Other("配對請求已過期，請對方重新送出".into()))
    }

    /// A：輸入配對碼。先重查一次收件匣拿到對方最新狀態（是否已接受、公鑰）。
    pub async fn pair_confirm(&self, id: &str, code: &str) -> Result<PairResult> {
        self.poll().await?;
        let req = self
            .my_requests
            .lock()
            .expect("my")
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| Error::Other("配對請求已過期，請重新配對".into()))?;
        if self.t.pair_confirm(&req, code).await? {
            self.attempts.lock().expect("attempts").remove(id);
            // 配對完成（我是輸入配對碼的那台）：把我的金鑰補給對方
            if let Err(e) = self.sync_keys(Some(&req.to), KeySync::Fill).await {
                tracing::warn!("配對後同步金鑰失敗：{e}");
            }
            return Ok(PairResult::Paired);
        }
        let used = {
            let mut a = self.attempts.lock().expect("attempts");
            let n = a.entry(id.to_string()).or_insert(0);
            *n += 1;
            *n
        };
        if used >= PAIR_ATTEMPTS {
            self.attempts.lock().expect("attempts").remove(id);
            let _ = self.t.pair_cancel(id).await;
            return Ok(PairResult::Cancelled);
        }
        Ok(PairResult::Wrong { left: PAIR_ATTEMPTS - used })
    }

    pub async fn pair_cancel(&self, id: &str) -> Result<()> {
        self.attempts.lock().expect("attempts").remove(id);
        self.t.pair_cancel(id).await
    }

    pub async fn unpair(&self, peer: &str) -> Result<()> {
        self.ensure_registered().await?;
        self.t.unpair(peer).await
    }

    pub async fn send(
        &self,
        peer_id: &str,
        out: &Outgoing,
        progress: &mut (dyn FnMut(u64, u64) + Send),
        cancel: &CancellationToken,
    ) -> Result<String> {
        self.ensure_registered().await?;
        let peer = self
            .t
            .peers()
            .into_iter()
            .find(|p| p.id == peer_id)
            .ok_or_else(|| Error::Other("還沒跟這台裝置配對".into()))?;
        self.t.send(&peer, out, &self.name(), progress, cancel).await
    }

    pub async fn receive(
        &self,
        id: &str,
        staging: &Path,
        progress: &mut (dyn FnMut(u64, u64) + Send),
        cancel: &CancellationToken,
    ) -> Result<Received> {
        let item = self.incoming(id).ok_or_else(|| Error::Other("這筆已經不在收件匣（可能過期或被取消）".into()))?;
        let r = self.t.receive(&item, staging, progress, cancel).await?;
        self.incoming.lock().expect("incoming").retain(|i| i.id != id);
        Ok(r)
    }

    /// 把這台存著的金鑰送給一台（`Some`）或所有配對的裝置；回傳送了幾台。沒有金鑰就什麼都不做。
    pub async fn sync_keys(&self, peer: Option<&str>, mode: KeySync) -> Result<usize> {
        let keys = stored_keys();
        if keys.is_empty() {
            return Ok(0);
        }
        self.ensure_registered().await?;
        let out = Outgoing::api_keys(&keys, mode)?;
        let mut sent = 0;
        for p in self.t.peers().into_iter().filter(|p| peer.is_none_or(|id| id == p.id)) {
            self.t.send(&p, &out, &self.name(), &mut |_, _| {}, &CancellationToken::new()).await?;
            sent += 1;
        }
        Ok(sent)
    }

    pub async fn decline(&self, id: &str) -> Result<()> {
        let item = self.incoming(id).ok_or_else(|| Error::Other("這筆已經不在收件匣".into()))?;
        self.t.decline(&item).await?;
        self.incoming.lock().expect("incoming").retain(|i| i.id != id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SecretKey {
        SecretKey::from_slice(&random32()).unwrap()
    }

    #[test]
    fn pair_code_is_six_digits_and_depends_on_both_keys_and_order() {
        let (a, b, c) = (pub_b64(&key().public_key()), pub_b64(&key().public_key()), pub_b64(&key().public_key()));
        let code = pair_code(&a, &b);
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|ch| ch.is_ascii_digit()));
        assert_eq!(code, pair_code(&a, &b), "兩邊算出一樣的碼");
        assert_ne!(code, pair_code(&a, &c), "公鑰被換掉就對不上");
        assert_ne!(code, pair_code(&b, &a));
    }

    #[test]
    fn ecdh_keys_match_and_chunks_roundtrip() {
        let recipient = key();
        let rpub = pub_b64(&recipient.public_key());
        let eph = key();
        let epub = pub_b64(&eph.public_key());
        let k1 = derive_key(&eph, &recipient.public_key(), &epub, &rpub);
        let k2 = derive_key_recv(&recipient, &epub, &rpub).unwrap();
        assert_eq!(k1, k2);
        let ct = encrypt_chunk(&k1, "t1", 0, 2, b"hello").unwrap();
        assert_eq!(decrypt_chunk(&k2, "t1", 0, 2, &ct).unwrap(), b"hello");
        assert!(decrypt_chunk(&k2, "t1", 1, 2, &ct).is_err(), "段號換了解不開（防調換順序）");
        assert!(decrypt_chunk(&k2, "t2", 0, 2, &ct).is_err(), "別筆傳送的段落解不開");
        let mut bad = ct.clone();
        bad[0] ^= 1;
        assert!(decrypt_chunk(&k2, "t1", 0, 2, &bad).is_err(), "內容被改過解不開");
        let other = key();
        assert!(derive_key_recv(&other, &epub, &pub_b64(&other.public_key())).unwrap() != k1, "別台算不出同一把");
    }

    #[test]
    fn meta_is_signed_by_sender_and_only_opens_for_recipient() {
        let dir = tempfile::tempdir().unwrap();
        let a = Transfer::open(&dir.path().join("a"), "http://x").unwrap();
        let b = Transfer::open(&dir.path().join("b"), "http://x").unwrap();
        let c = Transfer::open(&dir.path().join("c"), "http://x").unwrap();
        let meta = Meta {
            kind: Kind::File,
            title: "週會".into(),
            files: vec![FileEntry { path: "週會.pdf".into(), size: 3 }],
            total: 3,
            sha256: "x".into(),
            from_name: "辦公室電腦".into(),
            project_id: None,
            sent_at: Local::now(),
        };
        let bpub = parse_pub(&b.public_key()).unwrap();
        let eph = key();
        let epub = pub_b64(&eph.public_key());
        let k = derive_key(&eph, &bpub, &epub, &b.public_key());
        let ct = a.seal_meta(&k, &meta, &epub, b.device_id()).unwrap();
        let peer_a = Peer { id: a.device_id().into(), name: "A".into(), pubkey: a.public_key(), paired_at: Local::now() };
        assert_eq!(b.open_meta("t", &peer_a, &epub, &ct).unwrap(), meta);
        assert!(c.open_meta("t", &peer_a, &epub, &ct).is_err(), "不是收件裝置解不開");
        let impostor = Peer { pubkey: c.public_key(), ..peer_a };
        assert!(b.open_meta("t", &impostor, &epub, &ct).is_err(), "簽章不是配對時記住的那把就拒收");
    }

    #[test]
    fn identity_is_stable_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let a = Transfer::open(dir.path(), "http://x").unwrap();
        let again = Transfer::open(dir.path(), "http://x/").unwrap();
        assert_eq!(a.public_key(), again.public_key());
        assert_eq!(a.device_id(), again.device_id());
        assert!(Transfer::open(dir.path(), "  ").is_err(), "沒設定中轉站");
    }

    #[test]
    fn unpack_splits_files_checks_hash_and_rejects_unsafe_paths() {
        let dir = tempfile::tempdir().unwrap();
        let parts = dir.path().join("parts");
        std::fs::create_dir_all(&parts).unwrap();
        // 兩段：「abc」「defg」，對應三個檔 2＋0＋5
        std::fs::write(parts.join("0"), b"abc").unwrap();
        std::fs::write(parts.join("1"), b"defg").unwrap();
        let mut meta = Meta {
            kind: Kind::Meeting,
            title: "t".into(),
            files: vec![
                FileEntry { path: "project.json".into(), size: 2 },
                FileEntry { path: "clips/empty.mp3".into(), size: 0 },
                FileEntry { path: "chunks/0.mp3".into(), size: 5 },
            ],
            total: 7,
            sha256: hex(&Sha256::digest(b"abcdefg")),
            from_name: "A".into(),
            project_id: None,
            sent_at: Local::now(),
        };
        let out = dir.path().join("out");
        unpack(&parts, 2, &meta, &out).unwrap();
        assert_eq!(std::fs::read(out.join("project.json")).unwrap(), b"ab");
        assert_eq!(std::fs::read(out.join("chunks/0.mp3")).unwrap(), b"cdefg");
        assert!(out.join("clips/empty.mp3").is_file());
        meta.sha256 = "0".repeat(64);
        assert!(unpack(&parts, 2, &meta, &dir.path().join("o2")).is_err(), "雜湊不符");
        for bad in ["../evil", "/etc/passwd", "C:\\Windows\\x", "a/../../b", ""] {
            assert!(safe_rel(bad).is_err(), "{bad}");
        }
        assert!(safe_rel("chunks/0001.mp3").is_ok());
    }

    #[test]
    fn package_reader_streams_across_files() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::write(&a, b"12345").unwrap();
        std::fs::write(&b, b"678").unwrap();
        let mut r = PackageReader::new(vec![a, b]);
        let mut buf = [0u8; 4];
        assert_eq!(r.fill(&mut buf).unwrap(), 4);
        assert_eq!(&buf, b"1234");
        assert_eq!(r.fill(&mut buf).unwrap(), 4);
        assert_eq!(&buf, b"5678");
        assert_eq!(r.fill(&mut buf).unwrap(), 0);
    }
}
