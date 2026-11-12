//! 撤销动作的 App 侧接线：捕获被删除项、恢复 DesktopItem/位图、持久化。
//!
//! 核心队列与纯恢复逻辑在 `sylva_core::history`；这里只处理 app 独占的运行时
//! 数据（`items` / `item_index` / `bitmap_ids` / `pending_uploads`）。

use sylva_core::history::{self, ItemRestore, UndoAction};

use crate::persist::commit_or_rollback;
use crate::*;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 捕获一组图标被删除前的完整状态（元数据、归属、抑制项）。
pub(crate) fn capture_items(rt: &Runtime, ids: &[String]) -> Vec<ItemRestore> {
    let mut out = Vec::new();
    for id in ids {
        let Some(icon) = rt.desk.icons.get(id).cloned() else {
            continue;
        };
        let mut locations = Vec::new();
        for fence in &rt.desk.fences {
            if let Some(index) = fence.icon_ids.iter().position(|x| x == id) {
                locations.push((Some(fence.id), index));
            }
        }
        if let Some(index) = rt.desk.free_icons.iter().position(|x| x == id) {
            locations.push((None, index));
        }
        let mut suppressed = Vec::new();
        if let Some(path) = icon.path.as_deref()
            && is_linked_path(rt, path)
        {
            let key = path.to_ascii_lowercase();
            for fence in &rt.desk.fences {
                if fence.icon_ids.iter().any(|x| x == id) {
                    suppressed.push((fence.id, key.clone()));
                }
            }
        }
        out.push(ItemRestore {
            icon,
            locations,
            suppressed,
        });
    }
    out
}

/// 捕获一个栅栏删除前的状态（栅栏本身 + 成员归属）。
pub(crate) fn capture_fence(
    rt: &Runtime,
    fence_idx: usize,
) -> Option<(Fence, usize, Vec<ItemRestore>)> {
    let fence = rt.desk.fences.get(fence_idx)?.clone();
    let items = capture_items(rt, &fence.icon_ids);
    Some((fence, fence_idx, items))
}

/// 压入一条撤销记录（调用方保证随后会 `commit_or_rollback`）。
pub(crate) fn record(rt: &mut Runtime, label: impl Into<String>, action: UndoAction) {
    rt.desk.undo.push(now_secs(), label, action);
}

/// 撤销最后一条记录。返回 false 表示没有可撤销记录或恢复失败。
pub(crate) fn undo_last(rt: &mut Runtime) -> bool {
    // 先克隆再弹出：save 失败回滚时，撤销记录也一并恢复，不会静默丢历史。
    let before = rt.desk.clone();
    let Some(entry) = rt.desk.undo.pop() else {
        return false;
    };
    let label = entry.label.clone();
    let action = entry.action.clone();
    let is_snapshot = matches!(action, UndoAction::RestoreSnapshot { .. });
    let result = match &action {
        UndoAction::RestoreSnapshot { snapshot_id } => {
            crate::snapshot::apply_snapshot(rt, *snapshot_id)
        }
        _ => history::apply(&mut rt.desk, &action),
    };
    if let Err(e) = result {
        tracing::warn!(%label, "撤销失败: {e}");
        // 放回记录，保持队列一致。
        rt.desk.undo.entries.push(entry);
        return false;
    }

    // 重建被恢复项对应的 DesktopItem/位图（路径失效的项只保留引用，降级显示）。
    // 快照恢复已在 apply_snapshot 内重建整个运行时池。
    if !is_snapshot {
        for id in restored_ids(&action) {
            ensure_item(rt, &id);
        }
    }

    if !commit_or_rollback(rt, before, "撤销") {
        return false;
    }
    tracing::info!(%label, "已撤销");
    true
}

/// 检查撤销队列是否有记录（托盘菜单置灰用）。
pub(crate) fn can_undo(rt: &Runtime) -> bool {
    !rt.desk.undo.is_empty()
}

fn restored_ids(action: &UndoAction) -> Vec<String> {
    match action {
        UndoAction::RestoreItems { items } => items.iter().map(|i| i.icon.id.clone()).collect(),
        UndoAction::RestoreFence { fence, .. } => fence.icon_ids.clone(),
        UndoAction::RestoreSnapshot { .. } => Vec::new(),
    }
}

/// 确保某个恢复的图标在 App 运行时池中有 DesktopItem/位图。
fn ensure_item(rt: &mut Runtime, id: &str) {
    if rt.item_index.contains_key(id) {
        return;
    }
    let Some(path) = rt.desk.icons.get(id).and_then(|ic| ic.path.clone()) else {
        return;
    };
    if !std::path::Path::new(&path).exists() {
        return;
    }
    let item = match sylva_shell::items::item_from_path(&path) {
        Ok(item) => item,
        Err(e) => {
            tracing::warn!(%path, "撤销恢复项失败: {e}");
            return;
        }
    };
    let bitmap_id = rt.items.len() as u64;
    rt.items.push(item);
    rt.item_index.insert(id.to_string(), rt.items.len() - 1);
    rt.bitmap_ids.insert(id.to_string(), bitmap_id);
    if let Some(index) = rt.item_index.get(id).copied() {
        match sylva_shell::icons::extract_icon(&rt.items[index], ICON_EXTRACT_SIZE) {
            Ok(data) => rt.pending_uploads.push((bitmap_id, data)),
            Err(e) => tracing::warn!(%path, "撤销恢复图标提取失败: {e}"),
        }
    }
}
