//! JSON 文件后端：`<data_dir>/desk.json`，write temp → rename 原子写。

use std::path::PathBuf;

use sylva_core::config::normalize;
use sylva_core::error::CoreError;
use sylva_core::model::Desk;
use sylva_core::snapshot::{Snapshot, SnapshotId, SnapshotMeta};
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

    pub fn snapshots_dir(&self) -> PathBuf {
        self.dir.join("snapshots")
    }

    fn snapshot_path(&self, id: SnapshotId) -> PathBuf {
        self.snapshots_dir().join(format!("{id}.json"))
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
        let snapshots = self.snapshots_dir();
        if snapshots.exists() {
            std::fs::remove_dir_all(snapshots)?;
        }
        Ok(())
    }

    fn save_snapshot(&self, snapshot: &Snapshot) -> Result<(), CoreError> {
        let dir = self.snapshots_dir();
        std::fs::create_dir_all(&dir)?;
        let path = self.snapshot_path(snapshot.id);
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(snapshot)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn list_snapshots(&self) -> Result<Vec<SnapshotMeta>, CoreError> {
        let dir = self.snapshots_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut metas = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match std::fs::read_to_string(&path)
                .map_err(CoreError::from)
                .and_then(|raw| serde_json::from_str::<Snapshot>(&raw).map_err(CoreError::from))
            {
                Ok(snapshot) => metas.push(snapshot.meta()),
                Err(e) => tracing::warn!(path = %path.display(), "读取快照失败，已跳过: {e}"),
            }
        }
        metas.sort_by_key(|m| m.id);
        Ok(metas)
    }

    fn load_snapshot(&self, id: SnapshotId) -> Result<Option<Snapshot>, CoreError> {
        let path = self.snapshot_path(id);
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(path)?;
        Ok(Some(serde_json::from_str(&raw)?))
    }

    fn prune_snapshots(&self, keep: usize) -> Result<(), CoreError> {
        let metas = self.list_snapshots()?;
        if metas.len() <= keep {
            return Ok(());
        }
        let excess = metas.len() - keep;
        for meta in metas.iter().take(excess) {
            let _ = std::fs::remove_file(self.snapshot_path(meta.id));
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
    fn undo_queue_roundtrip() {
        let dir = tmp_dir("json-undo");
        let store = JsonStore::new(dir.clone());
        let mut desk = desk_with_fence("撤销");
        desk.undo.push(
            123,
            "删除 1 项",
            sylva_core::history::UndoAction::RestoreSnapshot { snapshot_id: 7 },
        );
        store.save(&desk).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.undo.len(), 1);
        assert_eq!(loaded.undo.entries[0].label, "删除 1 项");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn snapshot_roundtrip_prune_and_delete() {
        use sylva_core::snapshot::Snapshot;

        let dir = tmp_dir("json-snapshot");
        let store = JsonStore::new(dir.clone());
        for id in 1..=3 {
            let snapshot = Snapshot::new(id, id, format!("reason{id}"), desk_with_fence("快照"));
            store.save_snapshot(&snapshot).unwrap();
        }
        assert_eq!(store.list_snapshots().unwrap().len(), 3);
        assert_eq!(store.load_snapshot(2).unwrap().unwrap().reason, "reason2");
        store.prune_snapshots(1).unwrap();
        let metas = store.list_snapshots().unwrap();
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].id, 3);
        store.delete().unwrap();
        assert!(store.list_snapshots().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
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
