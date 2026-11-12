//! 撤销队列：删除收纳项/栅栏后可在重启后恢复引用。
//!
//! 需求 §2.5：删除只移除引用、不得删除磁盘文件，且必须进入可撤销队列；
//! 撤销记录持久化，重启后仍可恢复。队列随 `Desk` 一起保存，写入仍是单个
//! redb write transaction（见 app 的 `commit_or_rollback`）。

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::model::{Desk, Fence, Icon};

/// 撤销队列上限：超出后淘汰最旧记录，避免配置无限膨胀。
pub const UNDO_LIMIT: usize = 50;

/// 持久化的撤销栈（最新记录在尾部）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UndoStack {
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub entries: Vec<UndoEntry>,
}

impl UndoStack {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 压入一条记录并执行容量淘汰。
    pub fn push(&mut self, at: u64, label: impl Into<String>, action: UndoAction) {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        self.entries.push(UndoEntry {
            id,
            at,
            label: label.into(),
            action,
        });
        if self.entries.len() > UNDO_LIMIT {
            let excess = self.entries.len() - UNDO_LIMIT;
            self.entries.drain(0..excess);
        }
    }

    pub fn pop(&mut self) -> Option<UndoEntry> {
        self.entries.pop()
    }

    /// 加载后裁剪历史遗留的超长队列。
    pub fn clamp(&mut self) {
        if self.entries.len() > UNDO_LIMIT {
            let excess = self.entries.len() - UNDO_LIMIT;
            self.entries.drain(0..excess);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UndoEntry {
    pub id: u64,
    /// unix 秒；由 app 层传入，core 不直接读系统时钟，便于测试。
    #[serde(default)]
    pub at: u64,
    pub label: String,
    pub action: UndoAction,
}

/// 可撤销动作。删除/移出记录的是被移除项的完整元数据与归属位置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UndoAction {
    RestoreItems {
        items: Vec<ItemRestore>,
    },
    RestoreFence {
        fence: Box<Fence>,
        index: usize,
        items: Vec<ItemRestore>,
    },
    /// 快照回滚：恢复前的快照 id 由 app 层加载（快照存储不在 core）。
    RestoreSnapshot {
        snapshot_id: u64,
    },
}

/// 单个收集项的恢复信息：元数据 + 原归属（栅栏 id/下标或未分组区）+ 抑制项。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemRestore {
    pub icon: Icon,
    /// (Some(栅栏 id) | None=未分组区, 原下标)。一个图标可能同时属于多个栅栏。
    pub locations: Vec<(Option<u64>, usize)>,
    /// 删除时加入的抑制项：(栅栏 id, 小写路径)。
    pub suppressed: Vec<(u64, String)>,
}

/// 应用一条撤销动作到 Desk。快照动作由 app 处理，这里返回明确错误。
pub fn apply(desk: &mut Desk, action: &UndoAction) -> Result<(), CoreError> {
    match action {
        UndoAction::RestoreItems { items } => {
            restore_items(desk, items);
            Ok(())
        }
        UndoAction::RestoreFence {
            fence,
            index,
            items,
        } => {
            if desk.fence(fence.id).is_none() {
                let at = (*index).min(desk.fences.len());
                desk.fences.insert(at, fence.as_ref().clone());
            }
            restore_items(desk, items);
            Ok(())
        }
        UndoAction::RestoreSnapshot { .. } => Err(CoreError::Storage(
            "快照回滚需要由 app 层加载快照后应用".into(),
        )),
    }
}

fn restore_items(desk: &mut Desk, items: &[ItemRestore]) {
    for item in items {
        let id = item.icon.id.clone();
        desk.icons
            .entry(id.clone())
            .or_insert_with(|| item.icon.clone());
        // 先按快照清除所有归属，再按记录恢复——删除栅栏后成员曾被挪到未分组区，
        // 直接追加会留下“既在栅栏又在未分组区”的重复归属。
        desk.free_icons.retain(|x| x != &id);
        for fence in &mut desk.fences {
            fence.icon_ids.retain(|x| x != &id);
        }
        for (fence_id, index) in &item.locations {
            match fence_id {
                Some(fid) => {
                    if let Some(fence) = desk.fence_mut(*fid)
                        && !fence.icon_ids.iter().any(|x| x == &id)
                    {
                        let at = (*index).min(fence.icon_ids.len());
                        fence.icon_ids.insert(at, id.clone());
                    }
                }
                None => {
                    if !desk.free_icons.iter().any(|x| x == &id) {
                        let at = (*index).min(desk.free_icons.len());
                        desk.free_icons.insert(at, id.clone());
                    }
                }
            }
        }
        for (fid, path) in &item.suppressed {
            if let Some(fence) = desk.fence_mut(*fid) {
                fence.suppressed_paths.retain(|p| p != path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppSettings;
    use crate::model::{ItemKind, ItemOrigin, Rect};

    fn desk_with_fence() -> Desk {
        let mut desk = Desk::new(AppSettings::default());
        desk.fences.push(Fence {
            id: 1,
            title: Some("测试".into()),
            monitor_id: 0,
            bounds: Rect::new(0.0, 0.0, 300.0, 200.0),
            state: crate::model::FenceState::Expanded,
            icon_ids: Vec::new(),
            appearance: crate::model::FenceAppearance::default(),
            kind: crate::model::FenceKind::Manual,
            scroll: 0.0,
            storage_path: None,
            sidebar_collapsed: false,
            suppressed_paths: Vec::new(),
        });
        desk
    }

    fn icon(id: &str) -> Icon {
        Icon::new(id.into(), id.into(), ItemKind::Doc)
    }

    #[test]
    fn push_pop_and_capacity() {
        let mut stack = UndoStack::default();
        for i in 0..(UNDO_LIMIT + 5) {
            stack.push(
                i as u64,
                format!("op{i}"),
                UndoAction::RestoreItems { items: vec![] },
            );
        }
        assert_eq!(stack.len(), UNDO_LIMIT);
        assert_eq!(stack.entries.first().unwrap().label, "op5");
        assert_eq!(stack.pop().unwrap().label, format!("op{}", UNDO_LIMIT + 4));
    }

    #[test]
    fn restore_items_returns_to_original_index() {
        let mut desk = desk_with_fence();
        desk.icons.insert("a".into(), icon("a"));
        let item = ItemRestore {
            icon: icon("a"),
            locations: vec![(Some(1), 0)],
            suppressed: vec![(1, "c:\\x\\a.txt".into())],
        };
        desk.fence_mut(1)
            .unwrap()
            .suppressed_paths
            .push("c:\\x\\a.txt".into());
        apply(&mut desk, &UndoAction::RestoreItems { items: vec![item] }).unwrap();
        assert_eq!(desk.fences[0].icon_ids, vec!["a".to_string()]);
        assert!(desk.fences[0].suppressed_paths.is_empty());
        assert_eq!(desk.icons["a"].origin, ItemOrigin::DesktopIcon);
    }

    #[test]
    fn restore_fence_reinserts_at_index() {
        let mut desk = desk_with_fence();
        let fence = desk.fences.remove(0);
        let item = ItemRestore {
            icon: icon("a"),
            locations: vec![(Some(1), 0)],
            suppressed: vec![],
        };
        apply(
            &mut desk,
            &UndoAction::RestoreFence {
                fence: Box::new(fence),
                index: 0,
                items: vec![item],
            },
        )
        .unwrap();
        assert_eq!(desk.fences.len(), 1);
        assert_eq!(desk.fences[0].icon_ids, vec!["a".to_string()]);
    }
}
