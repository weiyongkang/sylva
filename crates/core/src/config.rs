//! 应用级设置与「加载后归一化」。
//!
//! 具体存储后端（JSON / redb）不在这里实现——见 `crate::storage` 的契约；
//! 本模块只保留与平台无关的设置类型和版本迁移逻辑。

use serde::{Deserialize, Serialize};

use crate::model::Desk;

/// 全局设置，随 `Desk` 一起持久化。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    /// 是否显示未分组图标区。
    pub show_free_area: bool,
    /// 未分组图标区高度（逻辑 px）。
    pub free_area_height: f32,
    pub hotkeys: HotkeyConfig,
    /// 是否开机自启。
    pub autostart: bool,
    /// 是否允许栅栏相互叠加（关闭时移动/缩放强制保持间距）。
    #[serde(default)]
    pub allow_fence_overlap: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            show_free_area: true,
            free_area_height: 90.0,
            hotkeys: HotkeyConfig::default(),
            autostart: false,
            allow_fence_overlap: false,
        }
    }
}

/// 全局热键配置。值为可解析的键位描述（如 `"Ctrl+Shift+F"`）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct HotkeyConfig {
    pub collapse_all: Option<String>,
    pub expand_all: Option<String>,
    pub search: Option<String>,
}

/// 加载后的统一归一化：版本迁移 + 数据校验。
///
/// 所有存储后端（JSON / redb / 内存）在反序列化后都必须调用，保证迁移与校验只有一处实现。
pub fn normalize(mut desk: Desk) -> Desk {
    migrate(&mut desk);
    desk.validate();
    desk
}

/// 版本迁移：结构变化时在此按版本推进。
///
/// 规则：永远向前迁移；每次发布新结构时新增一段 `if desk.version < N`。
fn migrate(desk: &mut Desk) {
    // v1 → v2：把旧版 `Icon.added` 迁移为显式来源字段；同时保持旧字段同步，
    // 让配置仍可被旧版本读取。同时初始化撤销队列/抑制路径的默认值。
    desk.version = desk.version.max(3);
    for icon in desk.icons.values_mut() {
        if icon.added {
            icon.origin = crate::model::ItemOrigin::Collected;
        } else if icon.origin == crate::model::ItemOrigin::Collected {
            icon.added = true;
        }
    }
    // 抑制路径按小写保存，保证 Windows 大小写不敏感匹配。
    for fence in &mut desk.fences {
        // v2 → v3：有链接目录的旧栅栏视为文件夹门户；空白栅栏保持 Manual。
        // 桌面收件箱由 app 层根据桌面路径识别（core 不依赖 Shell）。
        if fence.kind == crate::model::FenceKind::Manual && fence.storage_path.is_some() {
            fence.kind = crate::model::FenceKind::FolderPortal;
        }
        for path in &mut fence.suppressed_paths {
            *path = path.to_ascii_lowercase();
        }
    }
    desk.undo.clamp();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Fence, FenceAppearance, FenceKind, FenceState, Rect};

    fn fence(storage: Option<&str>) -> Fence {
        Fence {
            id: 1,
            title: Some("旧栅栏".into()),
            monitor_id: 0,
            bounds: Rect::default(),
            state: FenceState::Expanded,
            icon_ids: Vec::new(),
            appearance: FenceAppearance::default(),
            kind: FenceKind::Manual,
            scroll: 0.0,
            storage_path: storage.map(str::to_string),
            sidebar_collapsed: false,
            suppressed_paths: vec!["C:\\Temp\\X".into()],
        }
    }

    #[test]
    fn legacy_storage_path_becomes_folder_portal() {
        let mut desk = Desk::new(AppSettings::default());
        desk.fences.push(fence(Some(r"D:\Portal")));
        let desk = normalize(desk);
        assert_eq!(desk.version, 3);
        assert_eq!(desk.fences[0].kind, FenceKind::FolderPortal);
        assert_eq!(desk.fences[0].suppressed_paths[0], "c:\\temp\\x");
    }

    #[test]
    fn blank_fence_stays_manual() {
        let mut desk = Desk::new(AppSettings::default());
        desk.fences.push(fence(None));
        let desk = normalize(desk);
        assert_eq!(desk.fences[0].kind, FenceKind::Manual);
    }
}
