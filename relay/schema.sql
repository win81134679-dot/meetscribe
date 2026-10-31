-- 中轉站的資料表（D1）。只有公鑰、名稱、配對關係與傳送的大小／密文中繼資料；檔案內容（密文）在 R2。
CREATE TABLE IF NOT EXISTS devices (
  id TEXT PRIMARY KEY,          -- 裝置識別碼 XXXX-XXXX
  pubkey TEXT NOT NULL,         -- P-256 公鑰（未壓縮 SEC1，base64）
  name TEXT NOT NULL,
  created INTEGER NOT NULL,
  seen INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS pair_requests (
  id TEXT PRIMARY KEY,
  from_id TEXT NOT NULL,
  to_id TEXT NOT NULL,
  from_name TEXT NOT NULL,
  to_name TEXT NOT NULL,
  status TEXT NOT NULL,         -- pending／accepted／declined
  created INTEGER NOT NULL,
  expires INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS pair_requests_to ON pair_requests (to_id);
CREATE INDEX IF NOT EXISTS pair_requests_from ON pair_requests (from_id);

-- 已配對：兩個方向各一列
CREATE TABLE IF NOT EXISTS pairs (
  a TEXT NOT NULL,
  b TEXT NOT NULL,
  created INTEGER NOT NULL,
  PRIMARY KEY (a, b)
);

CREATE TABLE IF NOT EXISTS transfers (
  id TEXT PRIMARY KEY,
  from_id TEXT NOT NULL,
  to_id TEXT NOT NULL,
  size INTEGER NOT NULL,        -- 密文總大小
  chunks INTEGER NOT NULL,
  chunk_size INTEGER NOT NULL,  -- 每段明文大小（密文多 16 位元組）
  eph TEXT NOT NULL,            -- 寄件端這次的臨時公鑰（ECDH 用）
  meta TEXT NOT NULL,           -- 加密過的標題、檔名等（只有收件端解得開）
  status TEXT NOT NULL,         -- uploading／ready
  created INTEGER NOT NULL,
  expires INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS transfers_to ON transfers (to_id);
CREATE INDEX IF NOT EXISTS transfers_expires ON transfers (expires);

-- 每台每天的上傳量（UTC 日）
CREATE TABLE IF NOT EXISTS usage (
  device TEXT NOT NULL,
  day TEXT NOT NULL,
  bytes INTEGER NOT NULL,
  PRIMARY KEY (device, day)
);

-- 每段實際上傳了多少位元組：加起來不能超過 transfers.size（擋「宣告很小、實際傳很大」灌爆儲存空間）
CREATE TABLE IF NOT EXISTS chunks (
  transfer_id TEXT NOT NULL,
  n INTEGER NOT NULL,
  bytes INTEGER NOT NULL,
  PRIMARY KEY (transfer_id, n)
);

-- R2 每天的計費操作次數（a＝寫入類 PUT/LIST、b＝讀取類 GET）：近 31 天加總不超過預算，才不會產生費用
CREATE TABLE IF NOT EXISTS r2_ops (
  day TEXT PRIMARY KEY,
  a INTEGER NOT NULL DEFAULT 0,
  b INTEGER NOT NULL DEFAULT 0
);

-- 給對方的一次性通知：paired／unpaired／pair_cancelled／received／declined
CREATE TABLE IF NOT EXISTS notices (
  id TEXT PRIMARY KEY,
  to_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  from_id TEXT NOT NULL,
  ref TEXT NOT NULL,
  created INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS notices_to ON notices (to_id);
