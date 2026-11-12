//! 领域命令：多栅栏分类移动/复制。
//!
//! 跨栅栏拖放必须通过这里，而不是让 app/render 直接改 `icon_ids`：
//! - Move：从源栅栏移除；源是桌面收件箱/文件夹门户时写入抑制列表；
//! - Copy：保留源，只加入目标（显式复制引用）；
//! - 目标为文件夹门户时拒绝（门户成员由目录决定）；
//! - 同一路径在目标已有引用时幂等，不产生重复。

use std::collections::HashSet;

use crate::error::CoreError;
use crate::model::{Desk, FenceKind, ItemId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifyMode {
    Move,
    Copy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifyChange {
    pub ids: Vec<ItemId>,
    pub to: u64,
}

/// 把图标从各自当前栅栏移动到/复制到目标栅栏。返回值只包含实际处理的 id。
pub fn classify_move(
    desk: &mut Desk,
    ids: &[ItemId],
    to: u64,
    mode: ClassifyMode,
) -> Result<ClassifyChange, CoreError> {
    let target_idx = desk
        .fences
        .iter()
        .position(|f| f.id == to)
        .ok_or_else(|| CoreError::Storage(format!("目标栅栏 {to} 不存在")))?;
    let target = &desk.fences[target_idx];
    if target.kind == FenceKind::FolderPortal {
        return Err(CoreError::Storage(
            "文件夹门户是只读镜像，不能作为拖入目标".into(),
        ));
    }

    // 目标现有路径集合（用于同路径去重；不同 id 也可能指向同一路径）。
    let mut target_paths: HashSet<String> = target
        .icon_ids
        .iter()
        .filter_map(|id| desk.icons.get(id))
        .filter_map(|ic| ic.path.as_ref())
        .map(|p| p.to_ascii_lowercase())
        .collect();

    let mut changed = Vec::new();
    for id in ids {
        let Some(icon) = desk.icons.get(id) else {
            continue;
        };
        let path_key = icon.path.as_ref().map(|p| p.to_ascii_lowercase());

        // 收集源栅栏（可能同时在多个栅栏；Move 全部移除，Copy 全部保留）。
        let sources: Vec<(usize, FenceKind)> = desk
            .fences
            .iter()
            .enumerate()
            .filter(|(_, f)| f.icon_ids.iter().any(|x| x == id))
            .map(|(i, f)| (i, f.kind))
            .collect();

        let already_in_target = desk
            .fences
            .get(target_idx)
            .map(|f| f.icon_ids.iter().any(|x| x == id))
            .unwrap_or(false);
        let duplicate_path = path_key.as_ref().is_some_and(|p| target_paths.contains(p));

        if mode == ClassifyMode::Move {
            for (src_idx, src_kind) in &sources {
                if *src_idx == target_idx {
                    continue;
                }
                if let Some(src) = desk.fences.get_mut(*src_idx) {
                    src.icon_ids.retain(|x| x != id);
                    if src_kind.is_mirrored()
                        && let Some(path) = &path_key
                        && !src.suppressed_paths.iter().any(|p| p == path)
                    {
                        src.suppressed_paths.push(path.clone());
                    }
                }
            }
        }

        if !already_in_target && !duplicate_path {
            if let Some(target) = desk.fence_mut(to) {
                target.icon_ids.push(id.clone());
            }
            if let Some(path) = &path_key {
                target_paths.insert(path.clone());
            }
        }
        // 目标若曾抑制该路径（从收件箱移回/重新加入），解除抑制。
        if let Some(path) = &path_key
            && let Some(target) = desk.fence_mut(to)
        {
            target.suppressed_paths.retain(|p| p != path);
        }

        changed.push(id.clone());
    }

    Ok(ClassifyChange { ids: changed, to })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppSettings;
    use crate::model::{Fence, FenceAppearance, FenceState, Icon, ItemKind, Rect};

    fn fence(id: u64, kind: FenceKind, storage: Option<&str>) -> Fence {
        Fence {
            id,
            title: Some(format!("f{id}")),
            monitor_id: 0,
            bounds: Rect::default(),
            state: FenceState::Expanded,
            icon_ids: Vec::new(),
            appearance: FenceAppearance::default(),
            kind,
            scroll: 0.0,
            storage_path: storage.map(str::to_string),
            sidebar_collapsed: false,
            suppressed_paths: Vec::new(),
        }
    }

    fn icon(id: &str, path: &str) -> Icon {
        let mut ic = Icon::new(id.into(), id.into(), ItemKind::Doc);
        ic.path = Some(path.into());
        ic
    }

    fn desk_two() -> Desk {
        let mut d = Desk::new(AppSettings::default());
        d.fences
            .push(fence(1, FenceKind::DesktopInbox, Some(r"D:\Desktop")));
        d.fences.push(fence(2, FenceKind::Manual, None));
        d.fences
            .push(fence(3, FenceKind::FolderPortal, Some(r"D:\Portal")));
        d.icons.insert("a".into(), icon("a", r"D:\Desktop\a.txt"));
        d.fences[0].icon_ids.push("a".into());
        d
    }

    #[test]
    fn move_from_inbox_suppresses_source_and_adds_target() {
        let mut d = desk_two();
        classify_move(&mut d, &["a".into()], 2, ClassifyMode::Move).unwrap();
        assert!(!d.fences[0].icon_ids.contains(&"a".to_string()));
        assert!(
            d.fences[0]
                .suppressed_paths
                .contains(&r"d:\desktop\a.txt".to_string())
        );
        assert!(d.fences[1].icon_ids.contains(&"a".to_string()));
    }

    #[test]
    fn copy_keeps_source() {
        let mut d = desk_two();
        classify_move(&mut d, &["a".into()], 2, ClassifyMode::Copy).unwrap();
        assert!(d.fences[0].icon_ids.contains(&"a".to_string()));
        assert!(d.fences[1].icon_ids.contains(&"a".to_string()));
        assert!(d.fences[0].suppressed_paths.is_empty());
    }

    #[test]
    fn portal_target_is_rejected() {
        let mut d = desk_two();
        let err = classify_move(&mut d, &["a".into()], 3, ClassifyMode::Move).unwrap_err();
        assert!(err.to_string().contains("门户"));
        assert!(d.fences[0].icon_ids.contains(&"a".to_string()));
    }

    #[test]
    fn move_removes_target_suppression() {
        let mut d = desk_two();
        d.fences[1]
            .suppressed_paths
            .push(r"d:\desktop\a.txt".into());
        classify_move(&mut d, &["a".into()], 2, ClassifyMode::Move).unwrap();
        assert!(d.fences[1].suppressed_paths.is_empty());
    }
}
