//! 用程式內建的更新檢查問真的 GitHub：假裝是某版、某台裝置，看會挑到哪個新版（只查不下載）。
//! `cargo run -p meetscribe-core --example update_check -- <目前版本> [識別碼] [windows|android]`
use meetscribe_core::update::{self, Platform};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let current = args.next().unwrap_or_else(|| "0.0.0".into());
    let device = args.next().unwrap_or_else(|| "AAAA-AAAA".into());
    let platform = match args.next().as_deref() {
        Some("android") => Platform::Android,
        _ => Platform::Windows,
    };
    let http = meetscribe_core::http::client(60).expect("http");
    let key = update::public_key().expect("公鑰");
    match update::check(&http, "", &key, &current, &device, platform).await {
        Ok(Some(o)) => println!("找到新版 {}（{} {} bytes，指定推送：{}）\n{}", o.version, o.asset.file, o.asset.size, o.targeted, o.url),
        Ok(None) => println!("{current} 已是最新版"),
        Err(e) => println!("檢查失敗：{e}"),
    }
}
