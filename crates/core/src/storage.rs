//! 存储抽象：领域层只依赖契约，不感知文件系统与具体后端。
//!
//! 分层约定（见 AGENTS.md §2.2 / §3）：
//! - `core` 定义 [`DeskStore`] 与内存实现，保证可脱离平台单测；
//! - `app` 提供具体后端（当前 JSON，后续 redb），负责文件 I/O 与原子写入；
//! - 迁移/导出通过 [`copy_all`] 在两个后端之间复制完整快照，不关心格式差异。

use std::cell::RefCell;

use crate::config::normalize;
use crate::error::CoreError;
use crate::model::Desk;

/// 存储后端种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    Json,
    /// 预留：redb 单一数据库文件。
    Redb,
}

impl StoreKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Redb => "redb",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "json" => Some(Self::Json),
            "redb" => Some(Self::Redb),
            _ => None,
        }
    }
}

/// 配置存储契约。
///
/// 约定：
/// - `load` 返回 `None` 表示"此后端尚无数据"，由调用方决定默认值；
/// - `load` 返回的 `Desk` 必须已归一化（版本迁移 + 校验），后端不得跳过；
/// - `save` 必须原子（崩溃不产生半写文件/半写事务）；
/// - 实现方必须是 `&self` 可重入调用的（内部同步即可）。
pub trait DeskStore {
    fn kind(&self) -> StoreKind;

    /// 人类可读的位置描述（日志/诊断用），不保证是文件路径。
    fn location(&self) -> String;

    /// 后端是否已有可读数据（迁移/首次运行判断用）。
    fn exists(&self) -> bool;

    fn load(&self) -> Result<Option<Desk>, CoreError>;

    fn save(&self, desk: &Desk) -> Result<(), CoreError>;

    /// 删除后端中的所有数据（"重置所有设置"用）。
    fn delete(&self) -> Result<(), CoreError>;
}

/// 把一个后端的完整快照复制到另一个后端。
///
/// 语义：源为空时清空目标；否则整体覆盖目标。返回写入的栅栏数量，便于日志与测试断言。
/// 这是 JSON → redb 迁移、redb → JSON 导出的统一入口，调用方不需要知道格式。
pub fn copy_all(from: &dyn DeskStore, to: &dyn DeskStore) -> Result<usize, CoreError> {
    match from.load()? {
        Some(desk) => {
            to.save(&desk)?;
            Ok(desk.fences.len())
        }
        None => {
            to.delete()?;
            Ok(0)
        }
    }
}

/// 内存后端：用于单元测试与"先加载后写回"的临时场景，不接触文件系统。
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: RefCell<Option<Desk>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(desk: Desk) -> Self {
        Self {
            inner: RefCell::new(Some(desk)),
        }
    }
}

impl DeskStore for MemoryStore {
    fn kind(&self) -> StoreKind {
        StoreKind::Json
    }

    fn location(&self) -> String {
        "memory".into()
    }

    fn exists(&self) -> bool {
        self.inner.borrow().is_some()
    }

    fn load(&self) -> Result<Option<Desk>, CoreError> {
        Ok(self.inner.borrow().clone().map(normalize))
    }

    fn save(&self, desk: &Desk) -> Result<(), CoreError> {
        *self.inner.borrow_mut() = Some(desk.clone());
        Ok(())
    }

    fn delete(&self) -> Result<(), CoreError> {
        *self.inner.borrow_mut() = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppSettings;
    use crate::model::{Fence, Rect};

    fn desk_with_fence(title: &str) -> Desk {
        let mut desk = Desk::new(AppSettings::default());
        desk.fences.push(Fence {
            id: 1,
            title: Some(title.into()),
            monitor_id: 0,
            bounds: Rect::new(0.0, 0.0, 300.0, 200.0),
            state: crate::model::FenceState::Expanded,
            icon_ids: Vec::new(),
            appearance: crate::model::FenceAppearance::default(),
            scroll: 0.0,
            storage_path: None,
            sidebar_collapsed: false,
        });
        desk
    }

    #[test]
    fn memory_store_roundtrip() {
        let store = MemoryStore::new();
        assert!(!store.exists());
        assert!(store.load().unwrap().is_none());

        store.save(&desk_with_fence("工作")).unwrap();
        assert!(store.exists());
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.fences[0].title.as_deref(), Some("工作"));
    }

    #[test]
    fn copy_all_moves_snapshot_between_backends() {
        let from = MemoryStore::with(desk_with_fence("来源"));
        let to = MemoryStore::new();

        let n = copy_all(&from, &to).unwrap();
        assert_eq!(n, 1);
        let copied = to.load().unwrap().unwrap();
        assert_eq!(copied.fences[0].title.as_deref(), Some("来源"));
    }

    #[test]
    fn copy_all_from_empty_clears_target() {
        let from = MemoryStore::new();
        let to = MemoryStore::with(desk_with_fence("旧数据"));

        assert_eq!(copy_all(&from, &to).unwrap(), 0);
        assert!(!to.exists());
    }
}
