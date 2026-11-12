//! 结构性变更的持久化事务边界。
//!
//! 需求 §3.3：跨多个键的写入必须原子；若提交失败，内存状态必须回滚到事务开始前，
//! 不得让 UI 显示未持久化的“幽灵收纳项”。redb 后端本身用单 write transaction
//! 保证原子，但 App 的内存模型在调用 save 之前已经改过，因此这里统一先克隆
//! 变更前状态，save 失败时整体回滚。

use crate::*;

/// 保存当前 Desk；失败时把内存状态恢复为 `before` 并返回 false。
pub(crate) fn commit_or_rollback(rt: &mut Runtime, before: Desk, context: &str) -> bool {
    match rt.store.save(&rt.desk) {
        Ok(()) => true,
        Err(e) => {
            rt.desk = before;
            // Desk 回滚后，App 独占的 items / bitmap_ids 也必须与模型重新对齐，
            // 否则会出现“模型已恢复但图标池仍缺项/多项”的幽灵状态。
            crate::snapshot::rebuild_runtime_items(rt);
            tracing::error!(%context, "持久化失败，内存状态已回滚: {e}");
            false
        }
    }
}
