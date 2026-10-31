//! 「傳到其他裝置」的端對端測試：兩台虛擬裝置對真的中轉站程式（`relay/`）跑一輪。
//! 平常略過；要跑先開本機中轉站：`cd relay; npx wrangler d1 execute meetscribe-relay --local --file schema.sql; npm run dev`，
//! 再 `$env:MEETSCRIBE_RELAY_TEST="http://127.0.0.1:8787"; cargo test -p meetscribe-core --test transfer_relay -- --nocapture`。

use meetscribe_core::export::ExportOptions;
use meetscribe_core::model::{JobOptions, Project, ProjectStatus};
use meetscribe_core::store::ProjectStore;
use meetscribe_core::transfer::{Kind, Notice, Outgoing, Transfer, import_meeting, place_file};
use tokio_util::sync::CancellationToken;

fn relay() -> Option<String> {
    std::env::var("MEETSCRIBE_RELAY_TEST").ok().filter(|s| !s.trim().is_empty())
}

async fn paired(dir: &std::path::Path, url: &str) -> (Transfer, Transfer) {
    let a = Transfer::open(&dir.join("a"), url).unwrap();
    let b = Transfer::open(&dir.join("b"), url).unwrap();
    a.register("辦公室電腦").await.unwrap();
    b.register("我的手機").await.unwrap();

    // A 送出配對 → B 看到請求與配對碼 → B 接受 → A 先輸錯一次、再輸對
    a.pair_start(b.device_id(), "辦公室電腦").await.unwrap();
    let req = b.inbox().await.unwrap().pair_requests.into_iter().find(|r| r.from == a.device_id()).expect("B 收到配對請求");
    assert_eq!(req.from_name, "辦公室電腦");
    b.pair_accept(&req, "我的手機").await.unwrap();
    let mine = a.inbox().await.unwrap().my_pair_requests.into_iter().find(|r| r.to == b.device_id()).unwrap();
    assert_eq!(mine.status, "accepted");
    let wrong = if req.code == "000000" { "111111" } else { "000000" };
    assert!(!a.pair_confirm(&mine, wrong).await.unwrap(), "錯的配對碼不能過");
    assert!(a.pair_confirm(&mine, &req.code).await.unwrap());
    assert_eq!(a.peers().len(), 1);
    // B 從伺服器得知配對完成、把 A 放進清單
    let inbox = b.inbox().await.unwrap();
    assert!(inbox.peers_changed);
    assert!(inbox.notices.iter().any(|n| matches!(n, Notice::Paired { peer } if peer == a.device_id())));
    assert_eq!(b.peers()[0].id, a.device_id());
    (a, b)
}

#[tokio::test]
async fn pair_send_receive_and_delete_online_copy() {
    let Some(url) = relay() else {
        eprintln!("略過：沒有設定 MEETSCRIBE_RELAY_TEST");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = paired(dir.path(), &url).await;
    let peer_b = a.peers().into_iter().next().unwrap();

    // 70 MB 的檔案：三段（32＋32＋6 MB）
    let src = dir.path().join("週會 摘要.pdf");
    let data: Vec<u8> = (0..70 * 1024 * 1024u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    std::fs::write(&src, &data).unwrap();
    let mut last = (0, 0);
    let out = Outgoing::file(Kind::File, "週會 摘要", &src);
    let id = a.send(&peer_b, &out, "辦公室電腦", &mut |d, t| last = (d, t), &CancellationToken::new()).await.unwrap();
    assert_eq!(last, (data.len() as u64, data.len() as u64), "進度回報到 100%");

    let inbox = b.inbox().await.unwrap();
    let item = inbox.incoming.iter().find(|i| i.id == id).expect("B 收件匣有這筆");
    assert_eq!(item.meta.title, "週會 摘要");
    assert_eq!(item.from_name, "辦公室電腦");
    assert_eq!(item.chunks, 3);
    assert!(item.expires_in_secs > 3 * 3600 - 120 && item.expires_in_secs <= 3 * 3600, "3 小時後過期：{}", item.expires_in_secs);
    let got = b.receive(item, &dir.path().join("staging1"), &mut |_, _| {}, &CancellationToken::new()).await.unwrap();
    let placed = place_file(&got, &dir.path().join("收到的檔案")).unwrap();
    assert_eq!(std::fs::read(&placed[0]).unwrap(), data);

    // 收完線上馬上刪：收件匣沒了，再下載也拿不到
    let inbox = b.inbox().await.unwrap();
    assert!(inbox.incoming.iter().all(|i| i.id != id));
    assert!(b.receive(item, &dir.path().join("staging2"), &mut |_, _| {}, &CancellationToken::new()).await.is_err());
    // 寄件端收到「已送達」
    assert!(a.inbox().await.unwrap().notices.iter().any(|n| matches!(n, Notice::Received { transfer, .. } if *transfer == id)));

    // 拒收：線上那份也刪
    let small = dir.path().join("rec.m4a");
    std::fs::write(&small, b"audio").unwrap();
    let id2 = a.send(&peer_b, &Outgoing::file(Kind::Recording, "會議錄音", &small), "辦公室電腦", &mut |_, _| {}, &CancellationToken::new()).await.unwrap();
    let item2 = b.inbox().await.unwrap().incoming.into_iter().find(|i| i.id == id2).unwrap();
    assert_eq!(item2.meta.kind, Kind::Recording);
    b.decline(&item2).await.unwrap();
    assert!(b.inbox().await.unwrap().incoming.is_empty());
    assert!(a.inbox().await.unwrap().notices.iter().any(|n| matches!(n, Notice::Declined { .. })));

    // 取消配對：兩邊都不能再傳，對方清單同步移除
    a.unpair(b.device_id()).await.unwrap();
    assert!(a.peers().is_empty());
    let inbox = b.inbox().await.unwrap();
    assert!(inbox.peers_changed && b.peers().is_empty());
    assert!(a.send(&peer_b, &Outgoing::file(Kind::File, "x", &small), "辦公室電腦", &mut |_, _| {}, &CancellationToken::new()).await.is_err());
}

#[tokio::test]
async fn send_meeting_and_import_on_other_device() {
    let Some(url) = relay() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = paired(dir.path(), &url).await;
    let peer_b = a.peers().into_iter().next().unwrap();

    // A 的一場會議：project.json＋播放檔＋子資料夾
    let store_a = ProjectStore::new(dir.path().join("a/projects"));
    let mut p = Project::new("C:/錄音/週會.m4a".into(), JobOptions::default());
    p.title = "週會".into();
    p.status = ProjectStatus::Done;
    p.playback_file = Some("playback.m4a".into());
    store_a.save(&p).unwrap();
    std::fs::write(store_a.path_in(p.id, "playback.m4a"), vec![7u8; 1234]).unwrap();
    std::fs::create_dir_all(store_a.path_in(p.id, "chunks")).unwrap();
    std::fs::write(store_a.path_in(p.id, "chunks/0001.mp3"), b"chunk").unwrap();

    let out = Outgoing::meeting(&store_a, &p).unwrap();
    let id = a.send(&peer_b, &out, "辦公室電腦", &mut |_, _| {}, &CancellationToken::new()).await.unwrap();
    let item = b.inbox().await.unwrap().incoming.into_iter().find(|i| i.id == id).unwrap();
    assert_eq!(item.meta.project_id, Some(p.id));
    let got = b.receive(&item, &dir.path().join("b/staging"), &mut |_, _| {}, &CancellationToken::new()).await.unwrap();

    let store_b = ProjectStore::new(dir.path().join("b/projects"));
    let imported = import_meeting(&got, &store_b, true, ExportOptions::default()).unwrap();
    assert_eq!(imported.id, p.id, "取代：同一個 ID");
    assert_eq!(imported.title, "週會");
    assert_eq!(imported.source_file, store_b.path_in(p.id, "playback.m4a"), "原始檔改指這台的播放檔");
    assert_eq!(std::fs::read(store_b.path_in(p.id, "chunks/0001.mp3")).unwrap(), b"chunk");
    assert_eq!(store_b.load(p.id).unwrap().segments, p.segments);

    // 再傳一次、這次另存一份
    let id = a.send(&peer_b, &out, "辦公室電腦", &mut |_, _| {}, &CancellationToken::new()).await.unwrap();
    let item = b.inbox().await.unwrap().incoming.into_iter().find(|i| i.id == id).unwrap();
    let got = b.receive(&item, &dir.path().join("b/staging2"), &mut |_, _| {}, &CancellationToken::new()).await.unwrap();
    let copy = import_meeting(&got, &store_b, false, ExportOptions::default()).unwrap();
    assert_ne!(copy.id, p.id);
    assert_eq!(copy.title, "週會（來自 辦公室電腦）");
    assert_eq!(store_b.list().len(), 2);
}

/// API 金鑰：只在記憶體裡加解密，收件端不寫任何檔；存進金鑰存放處（測試用檔案版，不碰認證管理員）。
#[tokio::test]
async fn api_keys_travel_in_memory_only() {
    use meetscribe_core::secrets::{self, KeyKind};
    use meetscribe_core::transfer::save_api_keys;
    let Some(url) = relay() else {
        eprintln!("略過：沒有設定 MEETSCRIBE_RELAY_TEST");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    secrets::use_file_store(dir.path().join("keys.json"));
    let (a, b) = paired(dir.path(), &url).await;
    let peer_b = a.peers().into_iter().next().unwrap();
    let out = Outgoing::api_keys(&[
        (KeyKind::OpenRouter, "sk-or-v1-0123456789abcdefa1b2".into()),
        (KeyKind::Google, "AIzaSyTEST0000000000000x9y8".into()),
    ], meetscribe_core::transfer::KeySync::Replace)
    .unwrap();
    assert_eq!(out.title, "OpenRouter …a1b2、Google AI Studio …x9y8");
    assert!(!format!("{out:?}").contains("sk-or-v1"), "Debug 不能印出金鑰");
    let id = a.send(&peer_b, &out, "辦公室電腦", &mut |_, _| {}, &CancellationToken::new()).await.unwrap();

    let item = b.inbox().await.unwrap().incoming.into_iter().find(|i| i.id == id).unwrap();
    assert_eq!(item.meta.kind, Kind::Keys);
    assert_eq!(item.meta.title, "OpenRouter …a1b2、Google AI Studio …x9y8");
    let staging = dir.path().join("staging-keys");
    let got = b.receive(&item, &staging, &mut |_, _| {}, &CancellationToken::new()).await.unwrap();
    assert!(!staging.exists(), "金鑰不落地");
    let incoming = dir.path().join("b").join("transfer").join("incoming");
    assert!(!incoming.join(&id).exists(), "也沒有暫存的分段");
    assert_eq!(save_api_keys(&got).unwrap(), vec![KeyKind::OpenRouter, KeyKind::Google]);
    assert_eq!(secrets::load(KeyKind::OpenRouter).unwrap().0, "sk-or-v1-0123456789abcdefa1b2");
    assert_eq!(secrets::load(KeyKind::Google).unwrap().0, "AIzaSyTEST0000000000000x9y8");
    assert!(b.inbox().await.unwrap().incoming.iter().all(|i| i.id != id), "收完線上馬上刪");
}
