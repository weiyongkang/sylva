//! 存储后端实现：`core` 只定义 `DeskStore` 契约，本模块提供具体后端与选择逻辑。
//!
//! 当前只有 JSON；redb 后端就绪后新增 `RedbStore` 并改 `open` 的分发即可，
//! 上层（`Runtime` / 各业务模块）不需要任何改动。

use std::path::{Path, PathBuf};

use sylva_core::config::normalize;
use sylva_core::error::CoreError;
use sylva_core::model::Desk;
use sylva_core::storage::{DeskStore, StoreKind};

/// JSON 文件后端：`<data_dir>/desk.json`。
///
/// 写入采用 **write temp → rename**，避免崩溃产生半写文件。
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

/// 打开默认后端。
///
/// 可用 `SYLVA_STORE=json|redb` 显式指定；redb 尚未实现，当前回退 JSON 并记录告警。
pub fn open(data_dir: &Path) -> Box<dyn DeskStore> {
    match std::env::var("SYLVA_STORE")
        .ok()
        .and_then(|v| StoreKind::from_name(&v))
    {
        Some(StoreKind::Redb) => {
            tracing::warn!("SYLVA_STORE=redb 尚未实现，回退到 JSON 后端");
            Box::new(JsonStore::new(data_dir.to_path_buf()))
        }
        _ => Box::new(JsonStore::new(data_dir.to_path_buf())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sylva_core::config::AppSettings;
    use sylva_core::model::{Fence, Rect};
    use sylva_core::storage::copy_all;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sylva-app-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn desk_with_fence(title: &str) -> Desk {
        let mut desk = Desk::new(AppSettings::default());
        desk.fences.push(Fence {
            id: 1,
            title: Some(title.into()),
            monitor_id: 0,
            bounds: Rect::new(0.0, 0.0, 300.0, 200.0),
            state: sylva_core::model::FenceState::Expanded,
            icon_ids: Vec::new(),
            appearance: sylva_core::model::FenceAppearance::default(),
            scroll: 0.0,
            storage_path: None,
            sidebar_collapsed: false,
        });
        desk
    }

    #[test]
    fn missing_file_returns_none() {
        let store = JsonStore::new(tmp_dir("missing"));
        assert!(!store.exists());
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn save_load_roundtrip() {
        let store = JsonStore::new(tmp_dir("roundtrip"));
        let desk = desk_with_fence("工作");
        store.save(&desk).unwrap();

        assert!(store.exists());
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded, desk);
        // 临时文件不应残留
        assert!(!store.desk_path().with_extension("json.tmp").exists());
    }

    #[test]
    fn delete_removes_file() {
        let store = JsonStore::new(tmp_dir("delete"));
        store.save(&desk_with_fence("工作")).unwrap();
        store.delete().unwrap();
        assert!(!store.exists());
    }

    /// JSON → JSON 的导出/迁移：验证 `copy_all` 不关心后端类型。
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
