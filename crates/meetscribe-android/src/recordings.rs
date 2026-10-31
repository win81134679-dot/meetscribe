//! 錄音庫：App 內錄的音檔（2026-10-31 使用者：「我想多做一個頁面 錄音管理 不管有沒有轉錄」）。
//!
//! 每段錄音一個資料夾 `recordings/<uuid>/`：音檔＋`info.json`（標題、建立時間、長度）。
//! 1.1 的錄音放在匯入區，沒按「開始轉錄」就會在下次開 App 時被清掉；現在錄完就是永久保存，使用者自己刪才刪。
//!
//! 轉錄時專案的原始檔就是這裡的音檔；處理完成、專案資料夾已有播放用副本後，這筆錄音併進會議（資料夾刪掉，
//! 錄音管理頁改列那場會議），同一段錄音不會在手機上存兩份。

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const INFO: &str = "info.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Info {
    pub title: String,
    /// 資料夾裡的音檔名。
    pub file: String,
    pub created_at: DateTime<Local>,
    /// 秒；讀不到時 0。
    pub duration: f64,
    /// 檔案不完整、讀不出長度（例如錄到一半 App 被系統強制結束）。
    #[serde(default)]
    pub broken: bool,
}

#[derive(Clone, Debug)]
pub struct Recording {
    pub id: Uuid,
    pub info: Info,
    pub path: PathBuf,
}

impl Recording {
    pub fn size(&self) -> u64 {
        std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0)
    }
}

pub struct Library {
    root: PathBuf,
}

impl Library {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn dir(&self, id: Uuid) -> PathBuf {
        self.root.join(id.to_string())
    }

    /// 有 info.json 的錄音（錄音中的那一段還沒有，不列）。新的在前。
    pub fn list(&self) -> Vec<Recording> {
        let mut out: Vec<Recording> = std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| Uuid::parse_str(e.file_name().to_str()?).ok())
            .filter_map(|id| self.get(id))
            .collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.info.created_at));
        out
    }

    pub fn get(&self, id: Uuid) -> Option<Recording> {
        let dir = self.dir(id);
        let info: Info = serde_json::from_slice(&std::fs::read(dir.join(INFO)).ok()?).ok()?;
        let path = dir.join(&info.file);
        Some(Recording { id, info, path })
    }

    /// 錄音機寫好的檔（`recordings/<uuid>/<名稱>.m4a`）登記進錄音庫。`duration` 是 None 表示讀不出長度（檔案壞了）。
    pub fn add(&self, file: &Path, title: &str, duration: Option<f64>) -> Result<Recording, String> {
        let id = self.id_of(file).ok_or_else(|| format!("不是錄音庫裡的檔：{}", file.display()))?;
        if !file.is_file() {
            return Err(format!("找不到錄音檔：{}", file.display()));
        }
        let created_at = std::fs::metadata(file)
            .and_then(|m| m.created().or_else(|_| m.modified()))
            .map(DateTime::<Local>::from)
            .unwrap_or_else(|_| Local::now());
        let info = Info {
            title: clean_title(title).unwrap_or_else(|| stem(file)),
            file: file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            created_at,
            duration: duration.unwrap_or(0.0),
            broken: duration.is_none(),
        };
        self.write(id, &info)?;
        Ok(Recording { id, info, path: file.to_path_buf() })
    }

    pub fn rename(&self, id: Uuid, title: &str) -> Result<Recording, String> {
        let mut r = self.get(id).ok_or("找不到這段錄音")?;
        r.info.title = clean_title(title).ok_or("名稱不能空白")?;
        self.write(id, &r.info)?;
        Ok(r)
    }

    pub fn delete(&self, id: Uuid) -> Result<(), String> {
        let dir = self.dir(id);
        if !dir.join(INFO).is_file() {
            return Err("找不到這段錄音".into());
        }
        std::fs::remove_dir_all(&dir).map_err(|e| format!("刪不掉錄音：{e}"))
    }

    /// 音檔在錄音庫裡的話，回它的錄音 ID（`recordings/<uuid>/<檔>`）。
    pub fn id_of(&self, file: &Path) -> Option<Uuid> {
        let rel = file.strip_prefix(&self.root).ok()?;
        let mut parts = rel.components();
        let id = Uuid::parse_str(parts.next()?.as_os_str().to_str()?).ok()?;
        parts.next()?;
        Some(id)
    }

    /// 開 App 時：有音檔卻沒有 info.json 的資料夾（錄音中 App 被關掉、或 1.1 錄完還沒登記）補登記。
    /// 讀得出長度就是正常的錄音；讀不出來標成不完整，留給使用者決定要不要刪。空資料夾直接刪。
    pub fn recover(&self, probe: impl Fn(&Path) -> Option<f64>) -> usize {
        let mut n = 0;
        for e in std::fs::read_dir(&self.root).into_iter().flatten().flatten() {
            let dir = e.path();
            if Uuid::parse_str(&e.file_name().to_string_lossy()).is_err() || dir.join(INFO).exists() {
                continue;
            }
            let audio = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .find(|p| p.is_file() && std::fs::metadata(p).is_ok_and(|m| m.len() > 0));
            match audio {
                Some(f) => {
                    let d = probe(&f);
                    if self.add(&f, &stem(&f), d).is_ok() {
                        n += 1;
                    }
                }
                None => {
                    let _ = std::fs::remove_dir_all(&dir);
                }
            }
        }
        n
    }

    fn write(&self, id: Uuid, info: &Info) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(info).map_err(|e| e.to_string())?;
        meetscribe_core::store::write_atomic(&self.dir(id).join(INFO), &json).map_err(|e| e.to_string())
    }
}

fn clean_title(t: &str) -> Option<String> {
    Some(t.trim().to_string()).filter(|t| !t.is_empty())
}

fn stem(f: &Path) -> String {
    f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "錄音".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_file(lib: &Library, name: &str) -> PathBuf {
        let f = lib.root().join(Uuid::new_v4().to_string()).join(name);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, b"audio").unwrap();
        f
    }

    #[test]
    fn add_list_rename_delete() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::new(dir.path().to_path_buf());
        let a = new_file(&lib, "會議錄音 2026-10-31 0900.m4a");
        let r = lib.add(&a, "  ", Some(12.5)).unwrap();
        assert_eq!(r.info.title, "會議錄音 2026-10-31 0900", "空白標題用檔名");
        assert_eq!(lib.id_of(&a), Some(r.id));
        assert_eq!(lib.list().len(), 1);
        assert_eq!(lib.rename(r.id, "週會").unwrap().info.title, "週會");
        assert!(lib.rename(r.id, " ").is_err());
        assert_eq!(lib.get(r.id).unwrap().info.title, "週會");
        lib.delete(r.id).unwrap();
        assert!(lib.list().is_empty());
        assert!(!a.exists());
        assert!(lib.add(Path::new("/elsewhere/x.m4a"), "x", None).is_err(), "錄音庫外的檔不收");
    }

    #[test]
    fn recover_registers_orphans_and_marks_broken() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::new(dir.path().to_path_buf());
        let good = new_file(&lib, "好的.m4a");
        let bad = new_file(&lib, "壞的.m4a");
        let empty = lib.root().join(Uuid::new_v4().to_string());
        std::fs::create_dir_all(&empty).unwrap();
        let n = lib.recover(|p| (p == good.as_path()).then_some(30.0));
        assert_eq!(n, 2);
        let list = lib.list();
        let g = list.iter().find(|r| r.path == good).unwrap();
        assert!(!g.info.broken && g.info.duration == 30.0);
        assert!(list.iter().find(|r| r.path == bad).unwrap().info.broken);
        assert!(!empty.exists());
        assert_eq!(lib.recover(|_| None), 0, "登記過的不再動");
    }
}
