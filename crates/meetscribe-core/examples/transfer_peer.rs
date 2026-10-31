//! 「傳到其他裝置」的命令列對手：在這台電腦當一台虛擬裝置，跟模擬器或手機上的 App 互傳（閉環測試用）。
//!
//! cargo run -p meetscribe-core --example transfer_peer -- <資料夾> <中轉站網址> <指令> [參數]
//!   id                    印出這台的識別碼（第一次會產生金鑰）
//!   pair <對方識別碼>      送出配對請求，等對方接受後印出「請輸入對方畫面上的配對碼」，從標準輸入讀 6 位數
//!   accept                接受第一個別人送來的配對請求，印出配對碼，等對方輸入完成
//!   unpair <對方識別碼>    取消配對
//!   peers                 列出配對的裝置
//!   send <對方識別碼> <檔案>  傳一個檔案（kind＝file）
//!   recv <放到哪>          收下收件匣裡所有東西（會議只解開不匯入）
//!   inbox                 印出收件匣（同步來的金鑰會自動存好）
//!   setkey <openrouter|google> <金鑰>  存一把金鑰（這台虛擬裝置自己的 keys.json）
//!   keys                  印出這台的金鑰（遮住中間）
//!   synckeys              把這台的金鑰推到所有配對的裝置（取代對方的）

use meetscribe_core::secrets::{self, KeyKind};
use meetscribe_core::transfer::{Hub, KeySync, Kind, Outgoing, PairResult};
use std::io::BufRead;
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("用法見檔頭");
        std::process::exit(2);
    }
    let root = PathBuf::from(&args[0]);
    std::fs::create_dir_all(&root)?;
    // 金鑰存在這台虛擬裝置自己的檔案（不碰認證管理員）
    secrets::use_file_store(root.join("keys.json"));
    let hub = Hub::open(&root, &args[1], "測試電腦")?;
    match args[2].as_str() {
        "id" => {
            hub.poll().await?;
            println!("{}", hub.device_id());
        }
        "pair" => {
            let req = hub.pair_start(&args[3]).await?;
            println!("已送出配對請求，等對方接受…");
            loop {
                hub.poll().await?;
                let inbox = hub.transfer().inbox().await?;
                if let Some(r) = inbox.my_pair_requests.iter().find(|r| r.id == req) {
                    if r.status == "accepted" {
                        break;
                    }
                    if r.status == "declined" {
                        println!("對方拒絕了");
                        return Ok(());
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            for line in std::io::stdin().lock().lines() {
                println!("請輸入對方畫面上的配對碼：");
                match hub.pair_confirm(&req, line?.trim()).await? {
                    PairResult::Paired => {
                        println!("配對完成");
                        break;
                    }
                    PairResult::Wrong { left } => println!("不對，還可以試 {left} 次"),
                    PairResult::Cancelled => {
                        println!("錯太多次，作廢");
                        break;
                    }
                }
            }
        }
        "accept" => {
            let req = loop {
                let inbox = hub.poll().await?;
                if let Some(r) = inbox.pair_requests.into_iter().next() {
                    break r;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            };
            hub.pair_accept(&req.id).await?;
            println!("配對碼：{}（來自 {} {}）", req.code, req.from_name, req.from);
            for _ in 0..150 {
                hub.poll().await?;
                if hub.peers().iter().any(|p| p.id == req.from) {
                    println!("配對完成");
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            println!("對方沒有在 5 分鐘內輸入配對碼");
        }
        "unpair" => {
            hub.unpair(&args[3]).await?;
            println!("已取消配對");
        }
        "peers" => {
            for p in hub.peers() {
                println!("{}  {}  {}", p.id, p.name, p.paired_at.format("%Y-%m-%d %H:%M"));
            }
        }
        "send" => {
            let path = PathBuf::from(&args[4]);
            let title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let out = Outgoing::file(Kind::File, &title, &path);
            let id = hub
                .send(&args[3], &out, &mut |d, t| eprint!("\r{d}/{t}"), &CancellationToken::new())
                .await?;
            println!("\n已送出 {id}");
        }
        "recv" => {
            let dest = PathBuf::from(&args[3]);
            let inbox = hub.poll().await?;
            for n in &inbox.notices {
                println!("通知：{n:?}");
            }
            for item in inbox.incoming {
                println!("收：{}「{}」來自 {}（{} bytes）", item.meta.kind.label(), item.meta.title, item.from_name, item.meta.total);
                let staging = root.join("staging").join(&item.id);
                let got = hub.receive(&item.id, &staging, &mut |_, _| {}, &CancellationToken::new()).await?;
                let target = dest.join(&item.id);
                std::fs::create_dir_all(&target)?;
                for f in &got.meta.files {
                    let to = target.join(&f.path);
                    std::fs::create_dir_all(to.parent().unwrap_or(&target))?;
                    std::fs::rename(got.dir.join(&f.path), &to)?;
                    println!("  {}（{} bytes）", to.display(), f.size);
                }
            }
        }
        "setkey" => {
            let kind = if args[3] == "google" { KeyKind::Google } else { KeyKind::OpenRouter };
            secrets::save(kind, &args[4])?;
            println!("已存");
        }
        "keys" => {
            for kind in [KeyKind::OpenRouter, KeyKind::Google] {
                println!("{kind:?}: {}", secrets::load(kind).map(|(k, _)| secrets::mask(&k)).unwrap_or_else(|| "（沒有）".into()));
            }
        }
        "synckeys" => {
            println!("同步到 {} 台", hub.sync_keys(None, KeySync::Replace).await?);
        }
        "inbox" => {
            let inbox = hub.poll().await?;
            println!("{inbox:#?}");
        }
        other => {
            eprintln!("不認得的指令：{other}");
            std::process::exit(2);
        }
    }
    Ok(())
}
