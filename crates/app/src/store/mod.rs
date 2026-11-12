//! 存储后端：`core` 定义契约，本模块提供 JSON / redb 实现、后端选择与迁移。
//!
//! 选择顺序：
//! 1. `SYLVA_STORE=json`：强制 JSON（排查/回退用）；
//! 2. 默认 redb；若 redb 为空且存在 `desk.json`，自动导入一次（先备份 `desk.json.bak`）；
//! 3. redb 打不开：回退 JSON 并记录错误，保证应用仍可启动。

mod json;
mod redb_store;

pub use json::JsonStore;
pub use redb_store::RedbStore;

use std::path::Path;

use sylva_core::storage::{DeskStore, StoreKind};

pub const REDB_FILE: &str = "sylva.redb";
pub const JSON_FILE: &str = "desk.json";

/// 打开默认后端（数据目录为 exe 同级 `data`）。
pub fn open(data_dir: &Path) -> Box<dyn DeskStore> {
    let requested = std::env::var("SYLVA_STORE")
        .ok()
        .and_then(|v| StoreKind::from_name(&v));

    if requested == Some(StoreKind::Json) {
        tracing::info!(dir = %data_dir.display(), "使用 JSON 存储后端（SYLVA_STORE=json）");
        return Box::new(JsonStore::new(data_dir.to_path_buf()));
    }

    match RedbStore::new(data_dir.join(REDB_FILE)) {
        Ok(store) => {
            if !store.exists() {
                migrate_json(data_dir, &store);
            }
            tracing::info!(path = %store.location(), "使用 redb 存储后端");
            Box::new(store)
        }
        Err(e) => {
            tracing::error!("打开 redb 失败，回退 JSON 后端: {e}");
            Box::new(JsonStore::new(data_dir.to_path_buf()))
        }
    }
}

/// 首次启用 redb 时把 `desk.json` 导入一次。
///
/// 规则：
/// - 先备份为 `desk.json.bak`（已存在则不覆盖）；
/// - 写完后回读比对，不一致则保留 `desk.json` 供人工回退；
/// - 失败不阻断启动（调用方继续用空 redb，下次启动会再尝试）。
fn migrate_json(data_dir: &Path, target: &RedbStore) {
    let json_path = data_dir.join(JSON_FILE);
    if !json_path.exists() {
        return;
    }

    let backup = data_dir.join(format!("{JSON_FILE}.bak"));
    if !backup.exists()
        && let Err(e) = std::fs::copy(&json_path, &backup)
    {
        tracing::warn!("备份 desk.json 失败: {e}");
    }

    let source = JsonStore::new(data_dir.to_path_buf());
    match source.load() {
        Ok(Some(desk)) => match target.save(&desk).and_then(|_| target.load()) {
            Ok(Some(reloaded)) if reloaded == desk => {
                tracing::info!(
                    fences = desk.fences.len(),
                    backup = %backup.display(),
                    "desk.json 已迁移到 redb"
                );
            }
            Ok(_) => tracing::warn!("redb 迁移回读不一致，保留 desk.json 以便回退"),
            Err(e) => tracing::warn!("redb 迁移失败，保留 desk.json: {e}"),
        },
        Ok(None) => {}
        Err(e) => tracing::warn!("读取 desk.json 失败，跳过迁移: {e}"),
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use std::path::PathBuf;

    use sylva_core::config::AppSettings;
    use sylva_core::model::{Desk, Fence, Rect};

    pub(crate) fn tmp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sylva-app-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    pub(crate) fn desk_with_fence(title: &str) -> Desk {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_util::{desk_with_fence, tmp_dir};

    #[test]
    fn migrate_json_imports_and_backs_up() {
        let dir = tmp_dir("migrate");
        std::fs::create_dir_all(&dir).unwrap();
        JsonStore::new(dir.clone())
            .save(&desk_with_fence("迁移"))
            .unwrap();

        let redb = RedbStore::new(dir.join(REDB_FILE)).unwrap();
        migrate_json(&dir, &redb);

        assert_eq!(
            redb.load().unwrap().unwrap().fences[0].title.as_deref(),
            Some("迁移")
        );
        assert!(dir.join(format!("{JSON_FILE}.bak")).exists());
    }

    #[test]
    fn migrate_json_is_noop_without_file() {
        let dir = tmp_dir("migrate-empty");
        let redb = RedbStore::new(dir.join(REDB_FILE)).unwrap();
        migrate_json(&dir, &redb);
        assert!(!redb.exists());
    }
}
