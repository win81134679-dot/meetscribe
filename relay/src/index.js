// 語音轉錄的中轉站（2026-10-31 使用者：「用識別碼傳給自己別台裝置 要有資安防護 傳完線上馬上刪除」）。
//
// 只轉手「看不懂的亂碼」：檔案在手機／電腦上就用收件裝置的金鑰加密（端對端，見 crates/meetscribe-core/src/transfer.rs），
// 這裡只存密文、裝置的公鑰與配對關係。收件端下載完就刪；沒人收的 3 小時後刪（cron 每 15 分清一次，過期也直接拒絕下載）。
//
// 每個請求都要帶裝置簽章：X-Device（識別碼）、X-Time（Unix 秒）、X-Sig（ECDSA P-256 / SHA-256 簽
// 「方法\n路徑\n時間\n內容的 SHA-256」，原始 r||s 的 base64）。分段上傳的內容不算雜湊（太大，Worker 免費方案每次只有 10 ms CPU），
// 改簽 "UNSIGNED-PAYLOAD"；內容被竄改的話收件端 AES-GCM 解不開。
//
// 用量上限（伺服器記在部署者名下，但所有裝了 App 的人都會用到）：單檔、每台每天、暫存總量、每分鐘請求數，見 wrangler.toml 的 [vars]。
//
// 絕不產生費用（2026-10-31 使用者：「要限制喔 絕對不可以讓我付到錢」）：Workers、D1 的免費方案超量只會失敗，
// 會自動計費的只有 R2。所以 R2 的三樣計費項目都在這裡硬擋在免費額度內：
// - 儲存（免費 10 GB-月）：暫存總量上限 8 GB，而且每段實際上傳的大小記在 D1（chunks），超過宣告的大小就拒收；
// - 寫入類操作（PUT、LIST，免費每月 100 萬次）與讀取類（GET，每月 1000 萬次）：每次先記進 r2_ops，
//   近 31 天加總到預算（預設免費額度的八成）就拒絕，計費週期不一定對齊月初所以用滾動 31 天；
// - 不用 LIST（確認上傳完、刪除都靠 D1 記的分段），刪除不收費。
// 漏刪的保險（2026-10-31 使用者：「要設成6hr自動刪除 1天有點太長了」）：每小時掃一次儲存桶，上傳超過 6 小時的一律刪；
// R2 的生命週期規則最短只能 1 天，deploy.ps1 另設那條當最後一道。

const ID_RE = /^[23456789ABCDEFGHJKMNPQRSTVWXYZ]{4}-[23456789ABCDEFGHJKMNPQRSTVWXYZ]{4}$/;
const UUID_RE = /^[0-9a-f-]{36}$/;
const CLOCK_SKEW = 300; // 秒
const MAX_PAIRS = 5;
const MAX_PENDING_TO = 20;
const PAIR_TTL = 300; // 配對請求 5 分鐘有效

export default {
  async fetch(request, env, ctx) {
    try {
      return await route(request, env);
    } catch (e) {
      if (e instanceof HttpError) return json({ error: e.message }, e.status);
      console.error(e);
      return json({ error: "伺服器錯誤" }, 500);
    }
  },
  async scheduled(event, env, ctx) {
    ctx.waitUntil(cleanup(env, event.scheduledTime));
  },
};

class HttpError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

const fail = (status, message) => {
  throw new HttpError(status, message);
};

function json(data, status = 200) {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" },
  });
}

const now = () => Math.floor(Date.now() / 1000);
const num = (v, d) => (Number.isFinite(Number(v)) ? Number(v) : d);

function limits(env) {
  return {
    maxFile: num(env.MAX_FILE_BYTES, 500 * 1024 * 1024),
    maxChunk: num(env.MAX_CHUNK_BYTES, 64 * 1024 * 1024),
    daily: num(env.DAILY_BYTES, 2 * 1024 * 1024 * 1024),
    total: num(env.TOTAL_BYTES, 8 * 1024 * 1024 * 1024),
    ttl: num(env.TRANSFER_TTL_SECONDS, 3 * 3600),
    r2ClassA: num(env.R2_CLASS_A_BUDGET, 800_000),
    r2ClassB: num(env.R2_CLASS_B_BUDGET, 8_000_000),
  };
}

const today = () => new Date().toISOString().slice(0, 10);

/** R2 的計費操作先記帳：近 31 天加總到預算就拒絕（kind：a＝寫入類、b＝讀取類）。 */
async function spendR2(env, kind) {
  const L = limits(env);
  const budget = kind === "a" ? L.r2ClassA : L.r2ClassB;
  const since = new Date(Date.now() - 30 * 86400000).toISOString().slice(0, 10);
  const col = kind === "a" ? "a" : "b";
  const row = await env.DB.prepare(`SELECT COALESCE(SUM(${col}), 0) AS n FROM r2_ops WHERE day >= ?`).bind(since).first();
  if (row.n >= budget) {
    console.warn(`R2 ${col} 類操作近 31 天 ${row.n} 次，已達預算 ${budget}`);
    fail(503, "中轉站這個月的免費額度用完了，為了不產生任何費用先暫停，過幾天會自動恢復");
  }
  await env.DB.prepare(`INSERT INTO r2_ops (day, ${col}) VALUES (?, 1) ON CONFLICT(day) DO UPDATE SET ${col} = ${col} + 1`)
    .bind(today())
    .run();
}

async function route(request, env) {
  const url = new URL(request.url);
  const path = url.pathname;
  const method = request.method;

  if (method === "GET" && path === "/v1/health") {
    return json({ ok: true, service: "meetscribe-relay", ttl_seconds: limits(env).ttl });
  }
  if (!path.startsWith("/v1/")) fail(404, "找不到");

  // 先讀內容（分段上傳除外），簽章要算它的雜湊
  const isChunkPut = method === "PUT" && /^\/v1\/transfers\/[^/]+\/\d+$/.test(path);
  const raw = isChunkPut ? null : new Uint8Array(await request.arrayBuffer());
  const body = raw && raw.length ? parseJson(raw) : {};
  const device = await authenticate(request, env, url, raw, path === "/v1/register" ? body : null);

  if (env.LIMITER) {
    const { success } = await env.LIMITER.limit({ key: device.id });
    if (!success) fail(429, "請求太頻繁，請稍後再試");
  }

  if (method === "POST" && path === "/v1/register") return register(env, device, body);
  if (method === "GET" && path === "/v1/inbox") return inbox(env, device);
  if (method === "POST" && path === "/v1/pair") return pairRequest(env, device, body);
  let m;
  if ((m = path.match(/^\/v1\/pair\/([^/]+)\/(accept|decline|confirm|cancel)$/)) && method === "POST") {
    return pairAction(env, device, m[1], m[2], body);
  }
  if (method === "POST" && path === "/v1/unpair") return unpair(env, device, body);
  if (method === "POST" && path === "/v1/transfers") return createTransfer(env, device, body);
  if ((m = path.match(/^\/v1\/transfers\/([^/]+)\/(\d+)$/))) {
    if (method === "PUT") return putChunk(request, env, device, m[1], Number(m[2]));
    if (method === "GET") return getChunk(env, device, m[1], Number(m[2]));
  }
  if ((m = path.match(/^\/v1\/transfers\/([^/]+)\/ready$/)) && method === "POST") return markReady(env, device, m[1]);
  if ((m = path.match(/^\/v1\/transfers\/([^/]+)$/)) && method === "DELETE") return finishTransfer(env, device, m[1], body);
  fail(404, "找不到");
}

function parseJson(raw) {
  try {
    return JSON.parse(new TextDecoder().decode(raw));
  } catch {
    fail(400, "內容格式錯誤");
  }
}

// ---------- 簽章 ----------

const b64 = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
const hex = (buf) => [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");

async function importKey(pubB64) {
  try {
    return await crypto.subtle.importKey("raw", b64(pubB64), { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
  } catch {
    fail(400, "公鑰格式錯誤");
  }
}

/** 驗證簽章，回傳 { id, pubkey }。註冊時用內容裡的公鑰（第一次見到這台），其他請求用註冊過的公鑰。 */
async function authenticate(request, env, url, raw, registerBody) {
  const id = request.headers.get("x-device") || "";
  const time = Number(request.headers.get("x-time"));
  const sig = request.headers.get("x-sig") || "";
  if (!ID_RE.test(id)) fail(401, "識別碼格式錯誤");
  if (!Number.isFinite(time) || Math.abs(now() - time) > CLOCK_SKEW) fail(401, "裝置時間與伺服器差太多，請校正時間");
  let pubkey;
  if (registerBody) {
    pubkey = String(registerBody.pubkey || "");
  } else {
    const row = await env.DB.prepare("SELECT pubkey FROM devices WHERE id = ?").bind(id).first();
    if (!row) fail(401, "這台裝置還沒註冊");
    pubkey = row.pubkey;
  }
  const bodyHash = raw ? hex(await crypto.subtle.digest("SHA-256", raw)) : "UNSIGNED-PAYLOAD";
  const message = `${request.method}\n${url.pathname}${url.search}\n${time}\n${bodyHash}`;
  let ok = false;
  try {
    ok = await crypto.subtle.verify(
      { name: "ECDSA", hash: "SHA-256" },
      await importKey(pubkey),
      b64(sig),
      new TextEncoder().encode(message),
    );
  } catch (e) {
    if (e instanceof HttpError) throw e;
  }
  if (!ok) fail(401, "簽章不符");
  return { id, pubkey };
}

// ---------- 裝置 ----------

const cleanName = (s) => String(s || "").replace(/[\u0000-\u001f]/g, "").trim().slice(0, 40) || "未命名裝置";

async function register(env, device, body) {
  const name = cleanName(body.name);
  const row = await env.DB.prepare("SELECT pubkey FROM devices WHERE id = ?").bind(device.id).first();
  if (row && row.pubkey !== device.pubkey) fail(409, "這個識別碼已被另一台裝置註冊");
  const t = now();
  if (row) {
    await env.DB.prepare("UPDATE devices SET name = ?, seen = ? WHERE id = ?").bind(name, t, device.id).run();
  } else {
    await env.DB.prepare("INSERT INTO devices (id, pubkey, name, created, seen) VALUES (?, ?, ?, ?, ?)")
      .bind(device.id, device.pubkey, name, t, t)
      .run();
  }
  return json({ ok: true });
}

async function paired(env, a, b) {
  return !!(await env.DB.prepare("SELECT 1 FROM pairs WHERE a = ? AND b = ?").bind(a, b).first());
}

async function pairCount(env, id) {
  return (await env.DB.prepare("SELECT COUNT(*) AS n FROM pairs WHERE a = ?").bind(id).first()).n;
}

// ---------- 收件匣 ----------

async function inbox(env, device) {
  const t = now();
  const id = device.id;
  await env.DB.prepare("UPDATE devices SET seen = ? WHERE id = ?").bind(t, id).run();
  const incomingPairs = await env.DB.prepare(
    `SELECT r.id, r.from_id, r.from_name, d.pubkey AS from_pubkey, r.created FROM pair_requests r
     JOIN devices d ON d.id = r.from_id WHERE r.to_id = ? AND r.status = 'pending' AND r.expires > ?`,
  ).bind(id, t).all();
  const myPairs = await env.DB.prepare(
    `SELECT r.id, r.to_id, r.to_name, r.status, d.pubkey AS to_pubkey FROM pair_requests r
     JOIN devices d ON d.id = r.to_id WHERE r.from_id = ? AND r.expires > ?`,
  ).bind(id, t).all();
  const transfers = await env.DB.prepare(
    `SELECT id, from_id, size, chunks, chunk_size, eph, meta, created, expires FROM transfers
     WHERE to_id = ? AND status = 'ready' AND expires > ? ORDER BY created`,
  ).bind(id, t).all();
  const notices = await env.DB.prepare("SELECT id, kind, from_id, ref, created FROM notices WHERE to_id = ?").bind(id).all();
  if (notices.results.length) {
    // 通知只送一次
    await env.DB.prepare("DELETE FROM notices WHERE to_id = ?").bind(id).run();
  }
  const peers = await env.DB.prepare(
    "SELECT p.b AS id, d.name, d.seen FROM pairs p JOIN devices d ON d.id = p.b WHERE p.a = ?",
  ).bind(id).all();
  return json({
    now: t,
    pair_requests: incomingPairs.results,
    my_pair_requests: myPairs.results,
    transfers: transfers.results,
    notices: notices.results,
    peers: peers.results,
  });
}

// ---------- 配對 ----------
// A 輸入 B 的識別碼 → B 收到請求，畫面顯示由雙方公鑰算出的 6 位數配對碼 → B 按接受 → A 輸入那 6 位數，
// A 的程式比對自己算的碼（防中間人換公鑰）→ 對了才送 confirm，這裡才記成已配對。輸錯 3 次 A 會送 cancel。

async function pairRequest(env, device, body) {
  const to = String(body.to || "").toUpperCase();
  if (!ID_RE.test(to)) fail(400, "識別碼格式錯誤（例如 K7Q2-9XMA）");
  if (to === device.id) fail(400, "這就是這台裝置的識別碼");
  const target = await env.DB.prepare("SELECT id FROM devices WHERE id = ?").bind(to).first();
  if (!target) fail(404, "找不到這個識別碼：請先在那台裝置打開「傳到其他裝置」");
  if (await paired(env, device.id, to)) fail(409, "已經配對過了");
  if ((await pairCount(env, device.id)) >= MAX_PAIRS) fail(409, `最多配對 ${MAX_PAIRS} 台，先取消一台`);
  if ((await pairCount(env, to)) >= MAX_PAIRS) fail(409, `那台裝置已配對 ${MAX_PAIRS} 台`);
  const t = now();
  // 同一對裝置只留最新的一個請求
  await env.DB.prepare("DELETE FROM pair_requests WHERE from_id = ? AND to_id = ?").bind(device.id, to).run();
  const id = crypto.randomUUID();
  await env.DB.prepare(
    "INSERT INTO pair_requests (id, from_id, to_id, from_name, to_name, status, created, expires) VALUES (?, ?, ?, ?, '', 'pending', ?, ?)",
  ).bind(id, device.id, to, cleanName(body.name), t, t + PAIR_TTL).run();
  return json({ id, expires: t + PAIR_TTL });
}

async function pairAction(env, device, id, action, body) {
  const r = await env.DB.prepare("SELECT * FROM pair_requests WHERE id = ? AND expires > ?").bind(id, now()).first();
  if (!r) fail(404, "配對請求已過期，請重新配對");
  if (action === "accept" || action === "decline") {
    if (r.to_id !== device.id || r.status !== "pending") fail(403, "不能處理這個配對請求");
    if (action === "decline") {
      await env.DB.prepare("UPDATE pair_requests SET status = 'declined' WHERE id = ?").bind(id).run();
    } else {
      await env.DB.prepare("UPDATE pair_requests SET status = 'accepted', to_name = ? WHERE id = ?")
        .bind(cleanName(body.name), id)
        .run();
    }
    return json({ ok: true });
  }
  if (r.from_id !== device.id) fail(403, "不能處理這個配對請求");
  if (action === "cancel") {
    await env.DB.prepare("DELETE FROM pair_requests WHERE id = ?").bind(id).run();
    await notice(env, r.to_id, "pair_cancelled", device.id, id);
    return json({ ok: true });
  }
  // confirm
  if (r.status !== "accepted") fail(409, "對方還沒接受配對");
  const t = now();
  await env.DB.batch([
    env.DB.prepare("INSERT OR IGNORE INTO pairs (a, b, created) VALUES (?, ?, ?)").bind(r.from_id, r.to_id, t),
    env.DB.prepare("INSERT OR IGNORE INTO pairs (a, b, created) VALUES (?, ?, ?)").bind(r.to_id, r.from_id, t),
    env.DB.prepare("DELETE FROM pair_requests WHERE id = ?").bind(id),
  ]);
  await notice(env, r.to_id, "paired", device.id, id);
  return json({ ok: true });
}

async function unpair(env, device, body) {
  const peer = String(body.peer || "").toUpperCase();
  if (!ID_RE.test(peer)) fail(400, "識別碼格式錯誤");
  await env.DB.batch([
    env.DB.prepare("DELETE FROM pairs WHERE (a = ? AND b = ?) OR (a = ? AND b = ?)").bind(device.id, peer, peer, device.id),
  ]);
  // 兩台之間還沒收的東西一起刪
  const rows = await env.DB.prepare(
    "SELECT id FROM transfers WHERE (from_id = ? AND to_id = ?) OR (from_id = ? AND to_id = ?)",
  ).bind(device.id, peer, peer, device.id).all();
  for (const t of rows.results) await deleteTransfer(env, t.id);
  await notice(env, peer, "unpaired", device.id, "");
  return json({ ok: true, deleted: rows.results.length });
}

async function notice(env, to, kind, from, ref) {
  await env.DB.prepare("INSERT INTO notices (id, to_id, kind, from_id, ref, created) VALUES (?, ?, ?, ?, ?, ?)")
    .bind(crypto.randomUUID(), to, kind, from, ref || "", now())
    .run();
}

// ---------- 傳送 ----------

async function createTransfer(env, device, body) {
  const L = limits(env);
  const to = String(body.to || "").toUpperCase();
  const size = Math.floor(Number(body.size));
  const chunks = Math.floor(Number(body.chunks));
  const chunkSize = Math.floor(Number(body.chunk_size));
  const eph = String(body.eph || "");
  const meta = String(body.meta || "");
  if (!ID_RE.test(to)) fail(400, "識別碼格式錯誤");
  if (!(await paired(env, device.id, to))) fail(403, "還沒跟這台裝置配對");
  if (!(size > 0) || !(chunks > 0) || !(chunkSize > 0) || chunks > 10000) fail(400, "傳送資訊錯誤");
  if (chunkSize > L.maxChunk) fail(400, "分段太大");
  if (size > L.maxFile) fail(413, `檔案太大（上限 ${Math.round(L.maxFile / 1048576)} MB）`);
  if (size > chunks * (chunkSize + 16)) fail(400, "傳送資訊錯誤");
  if (eph.length > 200 || meta.length > 64 * 1024) fail(400, "傳送資訊太大");
  const pending = await env.DB.prepare("SELECT COUNT(*) AS n FROM transfers WHERE to_id = ?").bind(to).first();
  if (pending.n >= MAX_PENDING_TO) fail(429, "對方還有很多沒收的東西，請對方先接收");
  const day = new Date().toISOString().slice(0, 10);
  const used = await env.DB.prepare("SELECT bytes FROM usage WHERE device = ? AND day = ?").bind(device.id, day).first();
  if ((used ? used.bytes : 0) + size > L.daily) fail(429, `今天傳送量已達上限（每天 ${Math.round(L.daily / 1073741824)} GB），明天再傳`);
  const stored = await env.DB.prepare("SELECT COALESCE(SUM(size), 0) AS n FROM transfers").first();
  if (stored.n + size > L.total) {
    console.warn(`暫存空間接近上限：${stored.n} + ${size} > ${L.total}`);
    fail(503, "中轉站暫時滿了，請稍後再傳");
  }
  const id = crypto.randomUUID();
  const t = now();
  await env.DB.batch([
    env.DB.prepare(
      `INSERT INTO transfers (id, from_id, to_id, size, chunks, chunk_size, eph, meta, status, created, expires)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'uploading', ?, ?)`,
    ).bind(id, device.id, to, size, chunks, chunkSize, eph, meta, t, t + L.ttl),
    env.DB.prepare(
      "INSERT INTO usage (device, day, bytes) VALUES (?, ?, ?) ON CONFLICT(device, day) DO UPDATE SET bytes = bytes + excluded.bytes",
    ).bind(device.id, day, size),
  ]);
  return json({ id, expires: t + L.ttl });
}

async function transferFor(env, id, check) {
  if (!UUID_RE.test(id)) fail(404, "找不到這筆傳送");
  const t = await env.DB.prepare("SELECT * FROM transfers WHERE id = ?").bind(id).first();
  if (!t) fail(404, "找不到這筆傳送（可能已過期或已被刪除）");
  if (t.expires <= now()) {
    await deleteTransfer(env, id);
    fail(410, "這筆傳送已過期（超過 3 小時沒有接收）");
  }
  if (!check(t)) fail(403, "不能存取這筆傳送");
  return t;
}

async function putChunk(request, env, device, id, n) {
  const t = await transferFor(env, id, (t) => t.from_id === device.id && t.status === "uploading");
  if (n < 0 || n >= t.chunks) fail(400, "分段編號錯誤");
  const len = Number(request.headers.get("content-length"));
  if (!(len > 0) || len > t.chunk_size + 16) fail(400, "分段大小錯誤");
  // 實際上傳的總量不能超過宣告的大小（暫存總量上限是照宣告的大小算的）
  const others = await env.DB.prepare("SELECT COALESCE(SUM(bytes), 0) AS n FROM chunks WHERE transfer_id = ? AND n != ?")
    .bind(id, n)
    .first();
  if (others.n + len > t.size) fail(400, "分段加起來超過宣告的檔案大小");
  await spendR2(env, "a");
  await env.BUCKET.put(`t/${id}/${n}`, request.body);
  await env.DB.prepare(
    "INSERT INTO chunks (transfer_id, n, bytes) VALUES (?, ?, ?) ON CONFLICT(transfer_id, n) DO UPDATE SET bytes = excluded.bytes",
  )
    .bind(id, n, len)
    .run();
  return json({ ok: true });
}

async function markReady(env, device, id) {
  const t = await transferFor(env, id, (t) => t.from_id === device.id);
  if (t.status === "ready") return json({ ok: true });
  const got = await env.DB.prepare("SELECT COUNT(*) AS count, COALESCE(SUM(bytes), 0) AS total FROM chunks WHERE transfer_id = ?")
    .bind(id)
    .first();
  const { count, total } = got;
  if (count !== t.chunks || total !== t.size) fail(409, `還有分段沒上傳完（${count}/${t.chunks}）`);
  await env.DB.prepare("UPDATE transfers SET status = 'ready' WHERE id = ?").bind(id).run();
  return json({ ok: true });
}

async function getChunk(env, device, id, n) {
  const t = await transferFor(env, id, (t) => t.to_id === device.id && t.status === "ready");
  if (n < 0 || n >= t.chunks) fail(400, "分段編號錯誤");
  await spendR2(env, "b");
  const obj = await env.BUCKET.get(`t/${id}/${n}`);
  if (!obj) fail(404, "分段不見了");
  return new Response(obj.body, {
    headers: { "content-type": "application/octet-stream", "content-length": String(obj.size), "cache-control": "no-store" },
  });
}

/** 收件端收完（received）或拒收（declined）、寄件端取消（cancelled）：刪掉線上的檔案，通知對方。 */
async function finishTransfer(env, device, id, body) {
  if (!UUID_RE.test(id)) fail(404, "找不到這筆傳送");
  const t = await env.DB.prepare("SELECT * FROM transfers WHERE id = ?").bind(id).first();
  if (!t) return json({ ok: true, deleted: false });
  const result = String(body.result || "");
  if (t.to_id === device.id && (result === "received" || result === "declined")) {
    await deleteTransfer(env, id);
    await notice(env, t.from_id, result, device.id, id);
  } else if (t.from_id === device.id && result === "cancelled") {
    await deleteTransfer(env, id);
  } else {
    fail(403, "不能刪除這筆傳送");
  }
  return json({ ok: true, deleted: true });
}

/** 刪掉線上的檔案（刪除不收費；分段的檔名是固定的 t/<id>/<n>，不用 LIST）。 */
async function deleteTransfer(env, id) {
  const t = await env.DB.prepare("SELECT chunks FROM transfers WHERE id = ?").bind(id).first();
  const n = t ? t.chunks : 0;
  for (let i = 0; i < n; i += 1000) {
    const keys = [];
    for (let k = i; k < Math.min(n, i + 1000); k++) keys.push(`t/${id}/${k}`);
    await env.BUCKET.delete(keys);
  }
  await env.DB.batch([
    env.DB.prepare("DELETE FROM chunks WHERE transfer_id = ?").bind(id),
    env.DB.prepare("DELETE FROM transfers WHERE id = ?").bind(id),
  ]);
}

// ---------- 定時清理（每 15 分鐘） ----------

async function cleanup(env, scheduledTime = Date.now()) {
  const t = now();
  const expired = await env.DB.prepare("SELECT id FROM transfers WHERE expires <= ?").bind(t).all();
  for (const row of expired.results) await deleteTransfer(env, row.id);
  const day = new Date(Date.now() - 2 * 86400000).toISOString().slice(0, 10);
  await env.DB.batch([
    env.DB.prepare("DELETE FROM pair_requests WHERE expires <= ?").bind(t),
    env.DB.prepare("DELETE FROM notices WHERE created <= ?").bind(t - 7 * 86400),
    env.DB.prepare("DELETE FROM usage WHERE day < ?").bind(day),
    env.DB.prepare("DELETE FROM r2_ops WHERE day < ?").bind(new Date(Date.now() - 40 * 86400000).toISOString().slice(0, 10)),
  ]);
  if (expired.results.length) console.log(`清掉 ${expired.results.length} 筆過期傳送`);
  // 每小時一次（cron 每 15 分鐘，挑整點那一輪）
  if (new Date(scheduledTime).getUTCMinutes() < 15 || env.SWEEP_EVERY_RUN === "1") {
    try {
      await sweep(env);
    } catch (e) {
      // 額度用完時 spendR2 會擋下（不掃）；還有 1 天的生命週期規則兜底
      console.warn(`掃描儲存桶沒做：${e.message}`);
    }
  }
}

/** 刪掉儲存桶裡上傳超過 6 小時的物件（正常 3 小時內就刪了，這只接漏刪的）。LIST 算寫入類操作，照樣記帳。 */
async function sweep(env) {
  const cutoff = Date.now() - num(env.SWEEP_AGE_SECONDS, 6 * 3600) * 1000;
  let cursor;
  let removed = 0;
  do {
    await spendR2(env, "a");
    const page = await env.BUCKET.list({ prefix: "t/", cursor, limit: 1000 });
    const old = page.objects.filter((o) => o.uploaded.getTime() < cutoff).map((o) => o.key);
    if (old.length) await env.BUCKET.delete(old);
    removed += old.length;
    cursor = page.truncated ? page.cursor : undefined;
  } while (cursor);
  if (removed) console.warn(`掃到 ${removed} 個超過 6 小時的物件，已刪除`);
}

export { cleanup };
