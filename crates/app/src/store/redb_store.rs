//! redb 后端：单一数据库文件，多键分片存储，`save` 用单个 write transaction 保证原子。
//!
//! 键设计：每个 `Desk` 字段一个键（JSON 编码的字节串），便于后续按字段局部读写，
//! 也比"整库一个 blob"更符合需求文档里"结构化键值 + 单事务"的约束。

use std::path::PathBuf;

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use sylva_core::config::normalize;
use sylva_core::error::CoreError;
use sylva_core::model::Desk;
use sylva_core::snapshot::{Snapshot, SnapshotId, SnapshotMeta};
use sylva_core::storage::{DeskStore, StoreKind};

/// 单表多键：键为字段名，值为该字段的 JSON 字节。
const TABLE: TableDefinition<'static, &'static str, &'static [u8]> = TableDefinition::new("desk");

/// 快照独立表：键为快照 id，值为 JSON 字节。与主配置分离，主配置损坏时仍可读取。
const SNAPSHOT_TABLE: TableDefinition<'static, u64, &'static [u8]> =
    TableDefinition::new("snapshots");

/// 数据库 schema 版本（与 `Desk.version` 独立，用于后端结构迁移）。
const SCHEMA: u32 = 2;

const K_SCHEMA: &str = "schema";
const K_VERSION: &str = "version";
const K_SETTINGS: &str = "settings";
const K_FENCES: &str = "fences";
const K_FREE_ICONS: &str = "free_icons";
const K_ICONS: &str = "icons";
const K_TODOS: &str = "todos";
const K_NEXT_TODO: &str = "next_todo_id";
const K_CONSOLE_OPEN: &str = "console_open";
const K_CONSOLE_POS: &str = "console_pos";
const K_CONSOLE_SIZE: &str = "console_size";
const K_PLUGINS: &str = "plugins";
const K_DESKTOP_MODE: &str = "desktop_mode";
const K_UNDO: &str = "undo";

const ALL_KEYS: &[&str] = &[
    K_SCHEMA,
    K_VERSION,
    K_SETTINGS,
    K_FENCES,
    K_FREE_ICONS,
    K_ICONS,
    K_TODOS,
    K_NEXT_TODO,
    K_CONSOLE_OPEN,
    K_CONSOLE_POS,
    K_CONSOLE_SIZE,
    K_PLUGINS,
    K_DESKTOP_MODE,
    K_UNDO,
];

pub struct RedbStore {
    db: Database,
    path: PathBuf,
}

impl RedbStore {
    pub fn new(path: PathBuf) -> Result<Self, CoreError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| CoreError::Storage(format!("创建数据目录失败: {e}")))?;
        }
        let db = Database::create(&path)
            .map_err(|e| CoreError::Storage(format!("打开 redb 失败: {e}")))?;
        Ok(Self { db, path })
    }
}

fn storage_err(context: &str, e: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("{context}: {e}"))
}

impl DeskStore for RedbStore {
    fn kind(&self) -> StoreKind {
        StoreKind::Redb
    }

    fn location(&self) -> String {
        self.path.display().to_string()
    }

    fn exists(&self) -> bool {
        let Ok(txn) = self.db.begin_read() else {
            return false;
        };
        let Ok(table) = txn.open_table(TABLE) else {
            return false;
        };
        matches!(table.get(K_SCHEMA), Ok(Some(_)))
    }

    fn load(&self) -> Result<Option<Desk>, CoreError> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| storage_err("redb 读事务失败", e))?;
        let table = match txn.open_table(TABLE) {
            Ok(t) => t,
            // 空库（首次运行）没有表
            Err(_) => return Ok(None),
        };

        let read = |key: &str| -> Result<Option<Vec<u8>>, CoreError> {
            Ok(table
                .get(key)
                .map_err(|e| storage_err("redb 读取失败", e))?
                .map(|v| v.value().to_vec()))
        };

        let Some(_schema) = read(K_SCHEMA)? else {
            return Ok(None);
        };
        let decode = |key: &str| -> Result<Vec<u8>, CoreError> {
            read(key)?.ok_or_else(|| CoreError::Storage(format!("redb 缺少键: {key}")))
        };

        let desk = Desk {
            version: serde_json::from_slice(&decode(K_VERSION)?)?,
            settings: serde_json::from_slice(&decode(K_SETTINGS)?)?,
            fences: serde_json::from_slice(&decode(K_FENCES)?)?,
            free_icons: serde_json::from_slice(&decode(K_FREE_ICONS)?)?,
            icons: serde_json::from_slice(&decode(K_ICONS)?)?,
            todos: serde_json::from_slice(&decode(K_TODOS)?)?,
            next_todo_id: serde_json::from_slice(&decode(K_NEXT_TODO)?)?,
            console_open: serde_json::from_slice(&decode(K_CONSOLE_OPEN)?)?,
            console_pos: serde_json::from_slice(&decode(K_CONSOLE_POS)?)?,
            console_size: serde_json::from_slice(&decode(K_CONSOLE_SIZE)?)?,
            plugins: serde_json::from_slice(&decode(K_PLUGINS)?)?,
            desktop_mode: serde_json::from_slice(&decode(K_DESKTOP_MODE)?)?,
            // 旧 schema 1 没有 undo 键：缺省为空队列，不阻断升级。
            undo: read(K_UNDO)?
                .map(|bytes| serde_json::from_slice(&bytes))
                .transpose()?
                .unwrap_or_default(),
        };
        Ok(Some(normalize(desk)))
    }

    fn save(&self, desk: &Desk) -> Result<(), CoreError> {
        let rows: Vec<(&str, Vec<u8>)> = vec![
            (K_SCHEMA, serde_json::to_vec(&SCHEMA)?),
            (K_VERSION, serde_json::to_vec(&desk.version)?),
            (K_SETTINGS, serde_json::to_vec(&desk.settings)?),
            (K_FENCES, serde_json::to_vec(&desk.fences)?),
            (K_FREE_ICONS, serde_json::to_vec(&desk.free_icons)?),
            (K_ICONS, serde_json::to_vec(&desk.icons)?),
            (K_TODOS, serde_json::to_vec(&desk.todos)?),
            (K_NEXT_TODO, serde_json::to_vec(&desk.next_todo_id)?),
            (K_CONSOLE_OPEN, serde_json::to_vec(&desk.console_open)?),
            (K_CONSOLE_POS, serde_json::to_vec(&desk.console_pos)?),
            (K_CONSOLE_SIZE, serde_json::to_vec(&desk.console_size)?),
            (K_PLUGINS, serde_json::to_vec(&desk.plugins)?),
            (K_DESKTOP_MODE, serde_json::to_vec(&desk.desktop_mode)?),
            (K_UNDO, serde_json::to_vec(&desk.undo)?),
        ];

        let txn = self
            .db
            .begin_write()
            .map_err(|e| storage_err("redb 写事务失败", e))?;
        {
            let mut table = txn
                .open_table(TABLE)
                .map_err(|e| storage_err("redb 打开表失败", e))?;
            for (key, bytes) in &rows {
                table
                    .insert(*key, bytes.as_slice())
                    .map_err(|e| storage_err("redb 写入失败", e))?;
            }
        }
        txn.commit().map_err(|e| storage_err("redb 提交失败", e))?;
        Ok(())
    }

    fn delete(&self) -> Result<(), CoreError> {
        let snapshot_ids: Vec<u64> = self.list_snapshots()?.into_iter().map(|m| m.id).collect();
        let txn = self
            .db
            .begin_write()
            .map_err(|e| storage_err("redb 写事务失败", e))?;
        {
            let mut table = txn
                .open_table(TABLE)
                .map_err(|e| storage_err("redb 打开表失败", e))?;
            for key in ALL_KEYS {
                table
                    .remove(*key)
                    .map_err(|e| storage_err("redb 删除失败", e))?;
            }
            let mut snapshots = txn
                .open_table(SNAPSHOT_TABLE)
                .map_err(|e| storage_err("redb 打开快照表失败", e))?;
            for id in snapshot_ids {
                snapshots
                    .remove(id)
                    .map_err(|e| storage_err("redb 删除快照失败", e))?;
            }
        }
        txn.commit().map_err(|e| storage_err("redb 提交失败", e))?;
        Ok(())
    }

    fn save_snapshot(&self, snapshot: &Snapshot) -> Result<(), CoreError> {
        let bytes = serde_json::to_vec(snapshot)?;
        let txn = self
            .db
            .begin_write()
            .map_err(|e| storage_err("redb 快照写事务失败", e))?;
        {
            let mut table = txn
                .open_table(SNAPSHOT_TABLE)
                .map_err(|e| storage_err("redb 打开快照表失败", e))?;
            table
                .insert(snapshot.id, bytes.as_slice())
                .map_err(|e| storage_err("redb 写入快照失败", e))?;
        }
        txn.commit()
            .map_err(|e| storage_err("redb 提交快照失败", e))?;
        Ok(())
    }

    fn list_snapshots(&self) -> Result<Vec<SnapshotMeta>, CoreError> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| storage_err("redb 读事务失败", e))?;
        let table = match txn.open_table(SNAPSHOT_TABLE) {
            Ok(t) => t,
            Err(_) => return Ok(Vec::new()),
        };
        let mut metas = Vec::new();
        let iter = table
            .iter()
            .map_err(|e| storage_err("redb 遍历快照失败", e))?;
        for item in iter {
            let (key, value) = item.map_err(|e| storage_err("redb 读取快照失败", e))?;
            match serde_json::from_slice::<Snapshot>(value.value()) {
                Ok(snapshot) => metas.push(snapshot.meta()),
                Err(e) => tracing::warn!(id = key.value(), "redb 快照解析失败，已跳过: {e}"),
            }
        }
        metas.sort_by_key(|m| m.id);
        Ok(metas)
    }

    fn load_snapshot(&self, id: SnapshotId) -> Result<Option<Snapshot>, CoreError> {
        let txn = self
            .db
            .begin_read()
            .map_err(|e| storage_err("redb 读事务失败", e))?;
        let table = match txn.open_table(SNAPSHOT_TABLE) {
            Ok(t) => t,
            Err(_) => return Ok(None),
        };
        let Some(value) = table
            .get(id)
            .map_err(|e| storage_err("redb 读取快照失败", e))?
        else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_slice(value.value())?))
    }

    fn prune_snapshots(&self, keep: usize) -> Result<(), CoreError> {
        let metas = self.list_snapshots()?;
        if metas.len() <= keep {
            return Ok(());
        }
        let excess = metas.len() - keep;
        let txn = self
            .db
            .begin_write()
            .map_err(|e| storage_err("redb 快照写事务失败", e))?;
        {
            let mut table = txn
                .open_table(SNAPSHOT_TABLE)
                .map_err(|e| storage_err("redb 打开快照表失败", e))?;
            for meta in metas.iter().take(excess) {
                table
                    .remove(meta.id)
                    .map_err(|e| storage_err("redb 删除快照失败", e))?;
            }
        }
        txn.commit()
            .map_err(|e| storage_err("redb 提交快照失败", e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_util::{desk_with_fence, tmp_dir};
    use sylva_core::storage::copy_all;

    #[test]
    fn empty_store_returns_none() {
        let store = RedbStore::new(tmp_dir("redb-empty").join("sylva.redb")).unwrap();
        assert!(!store.exists());
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn save_load_roundtrip() {
        let store = RedbStore::new(tmp_dir("redb-roundtrip").join("sylva.redb")).unwrap();
        let desk = desk_with_fence("工作");
        store.save(&desk).unwrap();

        assert!(store.exists());
        assert_eq!(store.load().unwrap().unwrap(), desk);
    }

    #[test]
    fn delete_clears_all_keys() {
        let store = RedbStore::new(tmp_dir("redb-delete").join("sylva.redb")).unwrap();
        store.save(&desk_with_fence("工作")).unwrap();
        store.delete().unwrap();
        assert!(!store.exists());
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn undo_queue_roundtrip() {
        let dir = tmp_dir("redb-undo");
        let store = RedbStore::new(dir.join("sylva.redb")).unwrap();
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

        let dir = tmp_dir("redb-snapshot");
        let store = RedbStore::new(dir.join("sylva.redb")).unwrap();
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
    fn copy_all_json_to_redb() {
        let source = crate::store::JsonStore::new(tmp_dir("redb-import-src"));
        source.save(&desk_with_fence("来源")).unwrap();

        let target = RedbStore::new(tmp_dir("redb-import-dst").join("sylva.redb")).unwrap();
        let n = copy_all(&source, &target).unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            target.load().unwrap().unwrap().fences[0].title.as_deref(),
            Some("来源")
        );
    }
}
