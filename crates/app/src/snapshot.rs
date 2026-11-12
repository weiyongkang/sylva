//! 快照的 App 侧接线：生成快照、启动回退、恢复上一份。
//!
//! 快照的存储契约在 `sylva_core::storage::DeskStore`，具体文件/表由 JSON/redb
//! 后端实现；这里只负责选择快照 id、重建运行时图标池与撤销链。

use sylva_core::snapshot::{SNAPSHOT_LIMIT, Snapshot, SnapshotId};
use sylva_core::storage::DeskStore;

use crate::persist::commit_or_rollback;
use crate::*;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 读取当前最新快照（无快照返回 None）。
pub(crate) fn latest(store: &dyn DeskStore) -> sylva_core::Result<Option<Snapshot>> {
    let metas = store.list_snapshots()?;
    let Some(meta) = metas.last() else {
        return Ok(None);
    };
    store.load_snapshot(meta.id)
}

/// 为当前状态生成一份快照并执行容量淘汰。返回快照 id。
pub(crate) fn snapshot_before(rt: &Runtime, reason: &str) -> sylva_core::Result<SnapshotId> {
    let metas = rt.store.list_snapshots()?;
    let id = metas.iter().map(|m| m.id).max().unwrap_or(0) + 1;
    let snapshot = Snapshot::new(id, now_secs(), reason, rt.desk.clone());
    rt.store.save_snapshot(&snapshot)?;
    rt.store.prune_snapshots(SNAPSHOT_LIMIT)?;
    Ok(id)
}

/// 恢复最新快照。恢复前把当前状态另存一份，并把“恢复快照”压入撤销链。
pub(crate) fn restore_latest(rt: &mut Runtime) -> bool {
    let latest = match latest(rt.store.as_ref()) {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            tracing::info!("没有可恢复的快照");
            return false;
        }
        Err(e) => {
            tracing::warn!("读取快照失败: {e}");
            return false;
        }
    };

    let current_id = match snapshot_before(rt, "恢复快照前") {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!("恢复前保存当前状态失败，取消恢复: {e}");
            return false;
        }
    };

    let before = rt.desk.clone();
    let undo = rt.desk.undo.clone();
    let mut restored = latest.desk.clone();
    restored.undo = undo;
    restored.undo.push(
        now_secs(),
        format!("恢复快照：{}", latest.reason),
        sylva_core::history::UndoAction::RestoreSnapshot {
            snapshot_id: current_id,
        },
    );
    rt.desk = restored;
    rebuild_runtime_items(rt);
    commit_or_rollback(rt, before, "恢复快照")
}

/// 撤销 `RestoreSnapshot` 动作：加载指定快照，保留当前剩余撤销栈。
pub(crate) fn apply_snapshot(rt: &mut Runtime, id: SnapshotId) -> sylva_core::Result<()> {
    let Some(snapshot) = rt.store.load_snapshot(id)? else {
        return Err(sylva_core::CoreError::Storage(format!("快照 {id} 不存在")));
    };
    let undo = rt.desk.undo.clone();
    rt.desk = snapshot.desk;
    rt.desk.undo = undo;
    rebuild_runtime_items(rt);
    Ok(())
}

/// 用 Desk 重建 App 独占的 DesktopItem/位图池（快照恢复后必须调用）。
pub(crate) fn rebuild_runtime_items(rt: &mut Runtime) {
    rt.items.clear();
    rt.item_index.clear();
    rt.bitmap_ids.clear();
    rt.pending_uploads.clear();
    rt.selected.clear();
    rt.hover = None;
    rt.sidebar_reorder = None;

    let icons: Vec<(String, Option<String>)> = rt
        .desk
        .icons
        .iter()
        .map(|(id, ic)| (id.clone(), ic.path.clone()))
        .collect();
    for (id, path) in icons {
        let Some(path) = path else {
            continue;
        };
        if !std::path::Path::new(&path).exists() {
            continue;
        }
        let item = match sylva_shell::items::item_from_path(&path) {
            Ok(item) => item,
            Err(e) => {
                tracing::warn!(%path, "快照恢复项失败: {e}");
                continue;
            }
        };
        let bitmap_id = rt.items.len() as u64;
        rt.items.push(item);
        rt.item_index.insert(id.clone(), rt.items.len() - 1);
        rt.bitmap_ids.insert(id.clone(), bitmap_id);
        if let Some(index) = rt.item_index.get(&id).copied() {
            match sylva_shell::icons::extract_icon(&rt.items[index], ICON_EXTRACT_SIZE) {
                Ok(data) => rt.pending_uploads.push((bitmap_id, data)),
                Err(e) => tracing::warn!(%path, "快照恢复图标提取失败: {e}"),
            }
        }
    }
}
