//! 專案存放：`projects\<uuid>\project.json`＋音訊、分段、聲音樣本。

use crate::model::Project;
use crate::{Error, Result};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct ProjectStore {
    root: PathBuf,
}

impl ProjectStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn dir(&self, id: Uuid) -> PathBuf {
        self.root.join(id.to_string())
    }

    pub fn path_in(&self, id: Uuid, relative: &str) -> PathBuf {
        self.dir(id).join(relative)
    }

    pub fn save(&self, project: &Project) -> Result<()> {
        let dir = self.dir(project.id);
        std::fs::create_dir_all(&dir)?;
        let json = serde_json::to_vec_pretty(project)?;
        write_atomic(&dir.join("project.json"), &json)
    }

    pub fn load(&self, id: Uuid) -> Result<Project> {
        let bytes = std::fs::read(self.dir(id).join("project.json"))?;
        let mut p: Project = serde_json::from_slice(&bytes)?;
        // 舊會議記住的匯出選項補勾後來新增的格式（PDF）；下次存檔時一起寫回
        if let Some(e) = p.options.export.as_mut() {
            e.upgrade();
        }
        Ok(p)
    }

    /// 所有專案，新的在前。讀不了的跳過（記在 log）。
    pub fn list(&self) -> Vec<Project> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.root) else {
            return out;
        };
        for e in rd.flatten() {
            let Some(id) = e.file_name().to_str().and_then(|s| Uuid::parse_str(s).ok()) else {
                continue;
            };
            match self.load(id) {
                Ok(p) => out.push(p),
                Err(err) => tracing::warn!("略過無法讀取的專案 {id}：{err}"),
            }
        }
        out.sort_by_key(|p| std::cmp::Reverse(p.created_at));
        out
    }

    pub fn delete(&self, id: Uuid) -> Result<()> {
        let dir = self.dir(id);
        // 只刪自己建立的 uuid 資料夾，且裡面要有 project.json
        if dir.parent() != Some(self.root.as_path()) || !dir.join("project.json").is_file() {
            return Err(Error::Other(format!("拒絕刪除不是專案的資料夾：{}", dir.display())));
        }
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}

/// 先寫暫存檔再改名，避免當機時留下半份 JSON。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::JobOptions;

    #[test]
    fn save_list_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectStore::new(dir.path().to_path_buf());
        let p = Project::new("會議.m4a".into(), JobOptions::default());
        store.save(&p).unwrap();
        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "會議");
        store.delete(p.id).unwrap();
        assert!(store.list().is_empty());
    }

    /// 1.2 之前建的會議（匯出選項沒有 rev）載入時補勾 PDF；已經是新版本、使用者自己取消了 PDF 的不再補。
    #[test]
    fn old_meetings_get_pdf_checked_once() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProjectStore::new(tmp.path().to_path_buf());
        let mut p = Project::new(PathBuf::from("a.m4a"), JobOptions::default());
        let mut opts = crate::export::ExportOptions { formats: vec!["md".into(), "docx".into()], ..Default::default() };
        p.options.export = Some(opts.clone());
        store.save(&p).unwrap();
        // 模擬舊檔：拿掉 rev 欄位
        let file = store.dir(p.id).join("project.json");
        let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        v["options"]["export"].as_object_mut().unwrap().remove("rev");
        std::fs::write(&file, serde_json::to_vec(&v).unwrap()).unwrap();
        let got = store.load(p.id).unwrap().options.export.unwrap();
        assert_eq!(got.formats, vec!["md", "docx", "pdf"]);
        assert_eq!(got.rev, crate::export::EXPORT_OPTIONS_REV);

        opts.formats = vec!["md".into()];
        p.options.export = Some(opts);
        store.save(&p).unwrap();
        assert_eq!(store.load(p.id).unwrap().options.export.unwrap().formats, vec!["md"], "新版本取消 PDF 就尊重");
    }
}
