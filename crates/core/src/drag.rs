//! 拖放语义类型（跨 shell / render / app 共享）。

use serde::{Deserialize, Serialize};

/// OLE 拖放效果的业务映射。虚拟栅栏只持有引用，默认返回 `Link`；
/// `None` = 不可放置（不支持的数据类型 / 光标不在栅栏内）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DropEffect {
    #[default]
    None,
    Copy,
    Move,
    Link,
}
