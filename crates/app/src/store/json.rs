//! JSON 文件后端：`<data_dir>/desk.json`，write temp → rename 原子写。

use std::path::PathBuf;

use sylva_core::config::normalize;
use sylva_core::error::CoreError;
use sylva_core::model::Desk;
use sylva_core::storage::{DeskStore, StoreKind};

pub struct JsonStore {
    dir: PathBuf,
}

impl JsonStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn desk_path(&self) -> PathBuf {
        self.dir.join("desk.json")
    }
}

impl DeskStore for JsonStore {
    fn kind(&self) -> StoreKind {
        StoreKind::Json
    }

    fn location(&self) -> String {
        self.desk_path().display().to_string()
    }

    fn exists(&self) -> bool {
        self.desk_path().exists()
    }

    fn load(&self) -> Result<Option<Desk>, CoreError> {
        let path = self.desk_path();
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&path)?;
        let desk: Desk = serde_json::from_str(&raw)?;
        Ok(Some(normalize(desk)))
    }

    fn save(&self, desk: &Desk) -> Result<(), CoreError> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.desk_path();
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(desk)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn delete(&self) -> Result<(), CoreError> {
        let path = self.desk_path();
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_util::{desk_with_fence, tmp_dir};
    use sylva_core::storage::copy_all;

    #[test]
    fn missing_file_returns_none() {
        let store = JsonStore::new(tmp_dir("json-missing"));
        assert!(!store.exists());
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn save_load_roundtrip() {
        let store = JsonStore::new(tmp_dir("json-roundtrip"));
        let desk = desk_with_fence("工作");
        store.save(&desk).unwrap();

        assert!(store.exists());
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded, desk);
        assert!(!store.desk_path().with_extension("json.tmp").exists());
    }

    #[test]
    fn delete_removes_file() {
        let store = JsonStore::new(tmp_dir("json-delete"));
        store.save(&desk_with_fence("工作")).unwrap();
        store.delete().unwrap();
        assert!(!store.exists());
    }

    #[test]
    fn copy_all_exports_snapshot() {
        let source = JsonStore::new(tmp_dir("export-src"));
        source.save(&desk_with_fence("来源")).unwrap();

        let target = JsonStore::new(tmp_dir("export-dst"));
        let n = copy_all(&source, &target).unwrap();
        assert_eq!(n, 1);

        let copied = target.load().unwrap().unwrap();
        assert_eq!(copied.fences[0].title.as_deref(), Some("来源"));
    }
}
