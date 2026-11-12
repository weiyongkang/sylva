//! 布局快照：结构性变更前保存完整 `Desk`，用于误操作/配置损坏恢复。
//!
//! 需求 §3.4：快照包含栅栏布局、收纳项引用集合、规则配置、时间戳、快照 ID；
//! 快照可回滚，回滚本身也进入撤销链。快照存储与主 `Desk` 分离（JSON 独立文件 /
//! redb 独立表），这样主配置损坏时仍能读取最近快照。

use serde::{Deserialize, Serialize};

use crate::model::Desk;

pub type SnapshotId = u64;

/// 保留的最近快照数量。
pub const SNAPSHOT_LIMIT: usize = 10;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: SnapshotId,
    /// unix 秒。
    pub created_at: u64,
    /// 触发原因（"删除栅栏"/"批量删除"/"回滚前"…），用于 UI 与日志。
    pub reason: String,
    pub desk: Desk,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotMeta {
    pub id: SnapshotId,
    pub created_at: u64,
    pub reason: String,
}

impl Snapshot {
    /// 构造快照：清空撤销队列，避免快照体积随历史增长、也避免递归语义。
    pub fn new(id: SnapshotId, created_at: u64, reason: impl Into<String>, mut desk: Desk) -> Self {
        desk.undo = Default::default();
        Self {
            id,
            created_at,
            reason: reason.into(),
            desk,
        }
    }

    pub fn meta(&self) -> SnapshotMeta {
        SnapshotMeta {
            id: self.id,
            created_at: self.created_at,
            reason: self.reason.clone(),
        }
    }
}
