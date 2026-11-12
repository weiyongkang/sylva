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
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            show_free_area: true,
            free_area_height: 90.0,
            hotkeys: HotkeyConfig::default(),
            autostart: false,
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
    // v1：初始版本，暂无迁移逻辑。
    let _ = desk;
}
