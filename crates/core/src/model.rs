//! 领域模型：桌面、栅栏、图标等核心类型。
//!
//! 本模块只包含纯数据定义与默认值，不含任何平台相关逻辑，
//! 以便 `sylva-core` 保持零 Win32 依赖、可完全单元测试。

use serde::{Deserialize, Serialize};

use std::collections::HashMap;

use crate::history::UndoStack;

/// 图标的稳定标识符。
///
/// 由 Shell 层的项指纹（PIDL / 路径哈希）生成，跨重启稳定，
/// 用于持久化成员关系与排序。
pub type ItemId = String;

/// 二维点。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// 矩形（逻辑坐标，与 DPI 无关；渲染时由 Render 层按 DPI 换算像素）。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// 右边界。
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    /// 下边界。
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// 点是否在矩形内（含边界）。
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }

    /// 向内收缩 `d`（负值向外扩张）。不会缩到负宽高。
    pub fn inset(&self, d: f32) -> Self {
        Self {
            x: self.x + d,
            y: self.y + d,
            w: (self.w - 2.0 * d).max(0.0),
            h: (self.h - 2.0 * d).max(0.0),
        }
    }
}

/// 图标的类别。仅用于渲染表现（如快捷方式角标）与右键菜单，
/// **不参与任何自动归类**——归属完全由用户拖拽决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemKind {
    /// 应用程序。
    App,
    /// 文件夹。
    Folder,
    /// 文档。
    Doc,
    /// 驱动器/卷。
    Drive,
    /// 快捷方式。
    Link,
    /// 未知。
    Unknown,
}

/// 栅栏的折叠状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FenceState {
    /// 展开：显示栅栏区域与全部图标。
    #[default]
    Expanded,
    /// 折叠：仅显示标题栏。
    Folded,
}

/// 栅栏背景风格（决定背景填充与不透明度）。
///
/// v3 起替代「透明度滑块」：栅栏背景只分几种固定风格，不再用 0..1 滑块细调。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FenceStyle {
    /// 颜色：不透明纯色填充（色调来自「背景色调」；未选时用默认背景色）。
    Filled,
    /// 透明：内部完全透明，仅保留圆角描边。
    Outline,
    /// 玻璃：默认半透明玻璃感（可叠加「背景色调」着色，45% 混合）。
    #[default]
    Glass,
    /// 模糊：背景 = 栅栏底下桌面内容的高斯模糊（磨砂玻璃；CPU 侧降采样 + 高斯）。
    Blur,
}

impl FenceStyle {
    /// 菜单/设置显示名。
    pub fn label(&self) -> &'static str {
        match self {
            FenceStyle::Filled => "颜色",
            FenceStyle::Outline => "透明",
            FenceStyle::Glass => "玻璃",
            FenceStyle::Blur => "模糊",
        }
    }
}

/// 图标悬停效果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoverEffect {
    /// 不显示悬停反馈。
    None,
    /// 只显示悬停底色，不放大。
    Highlight,
    /// 放大 + 高亮。
    #[default]
    Scale,
}

impl HoverEffect {
    pub fn label(self) -> &'static str {
        match self {
            HoverEffect::None => "关闭",
            HoverEffect::Highlight => "高亮",
            HoverEffect::Scale => "放大",
        }
    }
}

/// 栅栏内图标的显示形状。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconShape {
    /// 方角。
    #[default]
    Square,
    /// 圆角方形。
    Rounded,
    /// 圆形。
    Circle,
}

impl IconShape {
    pub fn label(self) -> &'static str {
        match self {
            IconShape::Square => "方角",
            IconShape::Rounded => "圆角",
            IconShape::Circle => "圆形",
        }
    }
}

/// 栅栏的布局格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FenceLayout {
    /// 网格：图标自左向右、自上而下按列排布，标签在图标下方。
    Grid,
    /// 列表：图标单列纵向排布，标签在图标右侧。
    List,
    /// 侧边栏：仿 Mac Dock，仅显示图标，悬停放大 + 名称预览。
    Sidebar,
}

impl FenceLayout {
    /// 控制台/菜单显示名。
    pub fn label(&self) -> &'static str {
        match self {
            FenceLayout::Grid => "网格",
            FenceLayout::List => "列表",
            FenceLayout::Sidebar => "侧边栏",
        }
    }

    /// 循环切换到下一个格式。
    pub fn next(self) -> Self {
        match self {
            FenceLayout::Grid => FenceLayout::List,
            FenceLayout::List => FenceLayout::Sidebar,
            FenceLayout::Sidebar => FenceLayout::Grid,
        }
    }
}

/// 侧边栏停靠位置。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SidebarPosition {
    /// 屏幕左侧（纵向排列）。
    #[default]
    Left,
    /// 屏幕上侧（横向排列）。
    Top,
    /// 屏幕右侧（纵向排列）。
    Right,
}

impl SidebarPosition {
    pub fn label(&self) -> &'static str {
        match self {
            SidebarPosition::Left => "左侧",
            SidebarPosition::Top => "上侧",
            SidebarPosition::Right => "右侧",
        }
    }
}

/// 网格单元格档位（小 / 中 / 大）。
///
/// v4 起替代裸 `icon_size`：一个档位同时决定格宽、格高、图标边长与名称行，
/// 保证三档可预期，且切换「显示名称」只改图标大小、不重排格子。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CellSize {
    /// 紧凑：格 64×64，图标 32。
    Small,
    /// 默认：格 80×80，图标 48。
    #[default]
    Medium,
    /// 大：格 104×96，图标 64。
    Large,
}

impl CellSize {
    /// 三档（控制中心按此顺序渲染按钮）。
    pub const ALL: [CellSize; 3] = [Self::Small, Self::Medium, Self::Large];
    /// 格内上下留白（逻辑 px）。
    pub const PAD_V: f32 = 6.0;
    /// 图标与名称之间的间距（逻辑 px）。
    pub const CAPTION_GAP: f32 = 4.0;
    /// 名称行高（逻辑 px，12px 字 + 4 行距）。
    pub const CAPTION_H: f32 = 16.0;

    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "小",
            Self::Medium => "中",
            Self::Large => "大",
        }
    }

    /// 单元格宽（逻辑 px）。
    pub const fn cell_w(self) -> f32 {
        match self {
            Self::Small => 64.0,
            Self::Medium => 80.0,
            Self::Large => 104.0,
        }
    }

    /// 单元格高（逻辑 px）。与名称开关无关——这是"三档可预期"的前提。
    pub const fn cell_h(self) -> f32 {
        match self {
            Self::Small => 64.0,
            Self::Medium => 80.0,
            Self::Large => 96.0,
        }
    }

    /// 显示名称时的图标边长（逻辑 px）。
    pub const fn icon(self) -> f32 {
        match self {
            Self::Small => 32.0,
            Self::Medium => 48.0,
            Self::Large => 64.0,
        }
    }

    /// 隐藏名称时的图标边长：最多放大 25%，且不溢出色块。
    pub fn icon_without_labels(self) -> f32 {
        (self.icon() * 1.25)
            .min(self.cell_w() - 12.0)
            .min(self.cell_h() - 8.0)
    }

    /// 旧版 `icon_size`（32/48/64）→ 最近档位。
    pub fn nearest(icon: f32) -> Self {
        if icon < 40.0 {
            Self::Small
        } else if icon < 56.0 {
            Self::Medium
        } else {
            Self::Large
        }
    }
}

/// 网格容量不足时的处理方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverflowMode {
    /// 折叠成九宫格缩略图（点击展开）。
    #[default]
    Fold,
    /// 换行并滚动（不折叠）。
    Wrap,
}

impl OverflowMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fold => "九宫格",
            Self::Wrap => "换行",
        }
    }
}

/// 栅栏外观配置。
///
/// `#[serde(default)]`：旧版 `desk.json` 缺少新增字段时自动取默认值，
/// 保证配置向后兼容。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FenceAppearance {
    /// 背景色 RGBA（0.0..=1.0）；描边模式下不使用。
    pub bg_color: [f32; 4],
    /// 栅栏内边距（逻辑 px，四边同一份；标题与内容区共用）。
    pub padding: f32,
    /// 网格单元格档位（小 / 中 / 大）：决定格宽、格高、图标边长与名称行。
    #[serde(default)]
    pub cell: CellSize,
    /// 旧版图标尺寸（逻辑 px，v3 及更早）：加载时迁移为 `cell`，不再写回。
    #[serde(rename = "icon_size", default, skip_serializing_if = "Option::is_none")]
    pub legacy_icon_size: Option<f32>,
    /// 侧边栏图标间距（逻辑 px）。
    pub gap: f32,
    /// 布局格式（网格 / 列表）。
    pub layout: FenceLayout,
    /// 背景风格（玻璃 / 透明 / 颜色）。v3 起替代透明度滑块，决定填充与不透明度。
    #[serde(default = "default_bg_style")]
    pub bg_style: FenceStyle,
    /// 栅栏背景不透明度（0.0=完全透明，1.0=不透明；滑块调节）。
    ///
    /// v3 起滑块移除，「风格」取代它；字段保留仅供旧配置反序列化兼容，渲染不再读取。
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    /// 背景色调（RGB 0.0..=1.0）：None = 默认底色；Some = 着色。
    /// 「玻璃」风格下 45% 向色调靠拢；「颜色」风格下作为纯色填充。
    #[serde(default)]
    pub tint: Option<[f32; 3]>,
    /// 侧边栏停靠位置（仅 Sidebar 布局有效）。
    #[serde(default)]
    pub sidebar_pos: SidebarPosition,
    /// 是否显示栅栏标题文字；关闭后标题栏仍保留为拖动把手。
    #[serde(default = "default_true")]
    pub show_title: bool,
    /// 是否显示图标名称标签；关闭后只显示图标。
    #[serde(default = "default_true")]
    pub show_labels: bool,
    /// 网格容量不足时：折叠成九宫格缩略图 / 换行滚动。
    #[serde(default)]
    pub overflow: OverflowMode,
    /// 网格固定列数；0 = 按栅栏宽度自动计算。
    #[serde(default)]
    pub grid_cols: u32,
    /// 网格固定可见行数；0 = 按内容/栅栏高度自动计算。
    #[serde(default)]
    pub grid_rows: u32,
    /// 背景填充不透明度（0.0..=1.0）；1.0 = 使用当前风格默认值。
    #[serde(default = "default_fill_opacity")]
    pub fill_opacity: f32,
    /// 栅栏内图标的显示形状。
    #[serde(default)]
    pub icon_shape: IconShape,
    /// 图标悬停效果：关闭 / 仅高亮 / 放大。
    #[serde(default)]
    pub hover_effect: HoverEffect,
    /// 是否渲染该栅栏；false 时成员保留、可在控制中心重新显示。
    #[serde(default)]
    pub hidden: bool,
}

fn default_true() -> bool {
    true
}

fn default_fill_opacity() -> f32 {
    0.8
}

/// `bg_style` 未持久化时的默认值：玻璃（保持旧版滑块默认的半透明观感）。
fn default_bg_style() -> FenceStyle {
    FenceStyle::Glass
}

/// `opacity` 未持久化时的默认值（与旧版 `bg_color` 的 alpha 一致，迁移平滑）。
fn default_opacity() -> f32 {
    0.55
}

impl Default for FenceAppearance {
    fn default() -> Self {
        Self {
            bg_color: [0.08, 0.08, 0.12, 0.55],
            padding: 12.0,
            cell: CellSize::Medium,
            legacy_icon_size: None,
            gap: 10.0,
            layout: FenceLayout::Grid,
            bg_style: FenceStyle::Glass,
            opacity: 0.55,
            tint: None,
            sidebar_pos: SidebarPosition::Left,
            show_title: true,
            show_labels: true,
            overflow: OverflowMode::Fold,
            grid_cols: 0,
            grid_rows: 0,
            fill_opacity: 0.8,
            icon_shape: IconShape::Square,
            hover_effect: HoverEffect::Scale,
            hidden: false,
        }
    }
}

/// 栅栏类型：手动空白栅栏 / 桌面收件箱 / 文件夹门户。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FenceKind {
    /// 空白/手动栅栏：成员完全由用户拖入决定。
    #[default]
    Manual,
    /// 桌面收件箱：镜像桌面目录，作为未分类图标的入口（全局唯一）。
    DesktopInbox,
    /// 文件夹门户：镜像 `storage_path` 指定的真实文件夹（只读成员关系）。
    FolderPortal,
}

impl FenceKind {
    pub fn label(self) -> &'static str {
        match self {
            FenceKind::Manual => "普通栅栏",
            FenceKind::DesktopInbox => "桌面收件箱",
            FenceKind::FolderPortal => "文件夹门户",
        }
    }

    /// 只读镜像：桌面收件箱与文件夹门户；拖入/粘贴/移入均拒绝。
    pub fn is_read_only(self) -> bool {
        matches!(self, FenceKind::DesktopInbox | FenceKind::FolderPortal)
    }

    /// 是否由目录镜像维护成员列表（收件箱与文件夹门户）。
    pub fn is_mirrored(self) -> bool {
        matches!(self, FenceKind::DesktopInbox | FenceKind::FolderPortal)
    }
}

/// 栅栏。绑定「显式图标成员列表」，无分类语义。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fence {
    pub id: u64,
    /// 用户自拟标题，可留空。
    pub title: Option<String>,
    /// 所属显示器（逻辑坐标基准）。
    pub monitor_id: u32,
    /// 栅栏几何（物理像素；与 overlay 虚拟屏幕坐标一致，渲染直接用）。
    pub bounds: Rect,
    pub state: FenceState,
    /// 成员顺序即布局顺序。
    pub icon_ids: Vec<ItemId>,
    pub appearance: FenceAppearance,
    /// 栅栏类型（空白/桌面收件箱/文件夹门户）。旧配置缺省为 Manual，
    /// `normalize` 会把有 `storage_path` 的旧栅栏迁移为 FolderPortal。
    #[serde(default)]
    pub kind: FenceKind,
    /// 内容滚动偏移（物理像素；0 = 未滚动）。内容超出可视区时用滚轮滚动。
    #[serde(default)]
    pub scroll: f32,
    /// 镜像目录（绝对路径）。仅 `FolderPortal` / `DesktopInbox` 使用；
    /// 普通空白栅栏为 None。
    #[serde(default)]
    pub storage_path: Option<String>,
    /// 侧边栏是否折叠（仅 Sidebar 布局有效）。折叠后只显示一个小箭头。
    #[serde(default)]
    pub sidebar_collapsed: bool,
    /// 用户主动从链接文件夹栅栏移除、但文件仍留在磁盘上的路径（小写）。
    /// 文件夹镜像同步时跳过这些路径，实现“删除只移除引用、不删盘”且不会被
    /// 4 秒同步重新加回来；撤销删除时移除对应项即可恢复。
    #[serde(default)]
    pub suppressed_paths: Vec<String>,
}

/// 图标来源：桌面接管枚举项，还是用户手动拖入/粘贴的收集项。
///
/// 需求 §2.1 要求两类成员显式区分；这里用标记字段而不是两个独立结构体，
/// 避免把现有持久化格式一次性推倒重来。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemOrigin {
    /// 由 shell 层枚举真实桌面得到。
    #[default]
    DesktopIcon,
    /// 用户从桌面/资源管理器拖入或粘贴得到的路径引用。
    Collected,
}

/// 图标元数据。核心层只关心标识与展示信息。
///
/// 新增字段均带 `#[serde(default)]`，保证旧版 `desk.json` 能加载。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Icon {
    pub id: ItemId,
    pub display_name: String,
    pub kind: ItemKind,
    /// 文件系统路径；虚拟项为 None。持久化后重启可恢复图标/打开能力。
    #[serde(default)]
    pub path: Option<String>,
    /// 文件类型标签（"文件夹"、"文本文档"…）；空 = 未知/虚拟项。
    #[serde(default)]
    pub type_label: String,
    /// 最近修改时间（unix 秒）；无法读取为 None。
    #[serde(default)]
    pub modified_secs: Option<i64>,
    /// 文件大小（字节）；文件夹/未知为 None。
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// 旧版兼容字段：是否为拖入/粘贴新增的项。新代码请用 `origin`。
    /// `normalize` 会把旧数据的 `added=true` 迁移为 `origin=Collected`。
    #[serde(default)]
    pub added: bool,
    /// 来源类型（需求 §2.1）。旧数据缺省为 DesktopIcon，由 `normalize` 按 added 迁移。
    #[serde(default)]
    pub origin: ItemOrigin,
    /// 路径失效（文件被外部删除/移动/无权限）。失效项降级显示、可移除引用，
    /// 但不得导致渲染崩溃。
    #[serde(default)]
    pub invalid: bool,
}

impl Icon {
    /// 基础构造（详情字段留空，由 `details::enrich` 按路径补齐）。
    pub fn new(id: ItemId, display_name: String, kind: ItemKind) -> Self {
        Self {
            id,
            display_name,
            kind,
            path: None,
            type_label: String::new(),
            modified_secs: None,
            size_bytes: None,
            added: false,
            origin: ItemOrigin::DesktopIcon,
            invalid: false,
        }
    }

    /// 是否为用户收集项（拖入/粘贴/旧内部库项）。
    pub fn is_collected(&self) -> bool {
        self.origin == ItemOrigin::Collected
    }

    /// 标记为收集项，并同步旧版 `added` 字段，保证降级回旧版本时语义不丢。
    pub fn mark_collected(&mut self) {
        self.origin = ItemOrigin::Collected;
        self.added = true;
    }

    /// 标记为桌面枚举项（启动探测到的真实桌面图标）。
    pub fn mark_desktop(&mut self) {
        self.origin = ItemOrigin::DesktopIcon;
        self.added = false;
    }
}

/// 图标当前的归属位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconLocation {
    /// 未分组图标区。
    Free,
    /// 位于指定栅栏内。
    Fence(u64),
}

/// 待办事项条目（控制台第一个插件的数据）。二级结构：名称 + 详细信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TodoItem {
    /// 稳定 id（行动画/删除定位用；旧数据缺失时回退 0）。
    #[serde(default)]
    pub id: u64,
    /// 事项名称（一级标题）。旧配置字段名 `text`，反序列化时兼容。
    #[serde(alias = "text")]
    pub name: String,
    /// 详细信息（二级副标题）；可为空（只显示名称）。
    #[serde(default)]
    pub detail: String,
    /// 是否已完成。
    #[serde(default)]
    pub done: bool,
}

impl TodoItem {
    pub fn new(id: u64, name: String, detail: String) -> Self {
        Self {
            id,
            name,
            detail,
            done: false,
        }
    }
}

/// 桌面全局状态，唯一的持久化根。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Desk {
    /// 配置版本，用于结构迁移。
    pub version: u32,
    pub settings: crate::config::AppSettings,
    pub fences: Vec<Fence>,
    /// 未分组图标区：不属于任何栅栏的图标，顺序即布局顺序。
    pub free_icons: Vec<ItemId>,
    pub icons: HashMap<ItemId, Icon>,
    /// 待办插件数据（控制台第一个插件）。
    #[serde(default)]
    pub todos: Vec<TodoItem>,
    /// 下一个待办 id（`TodoItem.id` 分配；自增保证唯一）。
    #[serde(default = "default_next_todo_id")]
    pub next_todo_id: u64,
    /// 控制台面板是否显示（右上角插件面板）。
    /// 旧配置缺失该字段时默认打开（用户要求恢复控制台）；关闭后持久化 false。
    #[serde(default = "default_console_open")]
    pub console_open: bool,
    /// 控制台面板左上角（物理像素）。None = 未拖动过，按右上角自动摆放。
    #[serde(default)]
    pub console_pos: Option<Vec2>,
    /// 控制台面板宽高（物理像素）。None = 默认尺寸（宽固定，高随待办条数自适应）。
    /// 用户拖边缘/角缩放后落为具体值；之后高度固定、超出滚动。
    #[serde(default)]
    pub console_size: Option<(f32, f32)>,
    /// 插件注册表：内置插件 + 外部清单插件的统一启用状态与数据。
    /// 旧配置缺失时默认含「待办事项」（保持既有行为）。
    #[serde(default = "default_plugins")]
    pub plugins: Vec<PluginEntry>,
    /// 桌面模式：false = 栅栏接管（隐藏真实图标）；true = 原始桌面（恢复真实图标、
    /// 栅栏淡出隐藏）。控制中心「切换桌面」按钮切换。
    #[serde(default)]
    pub desktop_mode: bool,
    /// 删除/移出操作的撤销队列（持久化，重启后仍可撤销）。
    #[serde(default)]
    pub undo: UndoStack,
}

/// 插件种类：内置实现 / 外部清单（当前只有内置种类有界面实现）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    Todo,
    Notes,
    External,
}

impl PluginKind {
    pub fn label(self) -> &'static str {
        match self {
            PluginKind::Todo => "待办事项",
            PluginKind::Notes => "便签",
            PluginKind::External => "外部插件",
        }
    }
}

/// 插件注册项：内置插件与外部清单插件的统一持久化状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginEntry {
    pub id: String,
    pub name: String,
    pub kind: PluginKind,
    pub enabled: bool,
    pub version: String,
    pub desc: String,
    /// 便签插件内容（多行文本，持久化）。
    pub note_text: String,
}

impl PluginEntry {
    pub fn builtin_todo() -> Self {
        Self {
            id: "todo".into(),
            name: "待办事项".into(),
            kind: PluginKind::Todo,
            enabled: true,
            version: "1.0".into(),
            desc: "两级待办清单（名称 + 详情）".into(),
            note_text: String::new(),
        }
    }

    pub fn builtin_notes() -> Self {
        Self {
            id: "notes".into(),
            name: "便签".into(),
            kind: PluginKind::Notes,
            enabled: false,
            version: "1.0".into(),
            desc: "随手记多行文本，自动保存".into(),
            note_text: String::new(),
        }
    }
}

impl Default for PluginEntry {
    fn default() -> Self {
        Self::builtin_todo()
    }
}

/// `plugins` 未持久化时（旧配置）的默认注册表：待办事项启用 + 便签未启用。
fn default_plugins() -> Vec<PluginEntry> {
    vec![PluginEntry::builtin_todo(), PluginEntry::builtin_notes()]
}

/// `console_open` 未持久化时（旧配置）默认打开控制台。
fn default_console_open() -> bool {
    true
}

/// `next_todo_id` 未持久化时（旧配置）从 1 起（0 保留给无 id 的旧数据）。
fn default_next_todo_id() -> u64 {
    1
}

impl Desk {
    pub fn new(settings: crate::config::AppSettings) -> Self {
        Self {
            version: 4,
            settings,
            fences: Vec::new(),
            free_icons: Vec::new(),
            icons: HashMap::new(),
            todos: Vec::new(),
            // 首个版本默认打开控制台（用户要求恢复），关闭后持久化 false。
            next_todo_id: 1,
            console_open: true,
            console_pos: None,
            console_size: None,
            plugins: default_plugins(),
            desktop_mode: false,
            undo: UndoStack::default(),
        }
    }

    pub fn fence(&self, id: u64) -> Option<&Fence> {
        self.fences.iter().find(|f| f.id == id)
    }

    pub fn fence_mut(&mut self, id: u64) -> Option<&mut Fence> {
        self.fences.iter_mut().find(|f| f.id == id)
    }

    /// 下一个可用的栅栏 id（简单自增，稳定即可）。
    pub fn next_fence_id(&self) -> u64 {
        self.fences.iter().map(|f| f.id).max().unwrap_or(0) + 1
    }

    /// 查询图标的归属位置。
    pub fn icon_location(&self, id: &ItemId) -> Option<IconLocation> {
        for f in &self.fences {
            if f.icon_ids.contains(id) {
                return Some(IconLocation::Fence(f.id));
            }
        }
        if self.free_icons.contains(id) {
            Some(IconLocation::Free)
        } else {
            None
        }
    }

    /// 把一个图标从当前归属移到目标位置（None 表示未分组区）。
    /// 幂等：目标位置已包含则原样返回。图标不存在时不做任何事。
    pub fn move_icon(&mut self, id: &ItemId, to: Option<u64>) -> Option<IconLocation> {
        let from = self.icon_location(id)?;
        // 从旧位置移除
        if let Some(fid) = from.fence_id()
            && let Some(f) = self.fence_mut(fid)
        {
            f.icon_ids.retain(|x| x != id);
        }
        self.free_icons.retain(|x| x != id);
        // 加入新位置
        match to {
            None => {
                if !self.free_icons.contains(id) {
                    self.free_icons.push(id.clone());
                }
            }
            Some(fid) => {
                if let Some(f) = self.fence_mut(fid) {
                    if !f.icon_ids.contains(id) {
                        f.icon_ids.push(id.clone());
                    }
                } else {
                    // 目标栅栏不存在：退回未分组区
                    if !self.free_icons.contains(id) {
                        self.free_icons.push(id.clone());
                    }
                }
            }
        }
        Some(from)
    }

    /// 校验栅栏成员引用的完整性（存在但无元数据的图标会被剔除）。
    /// 用于加载配置后的防御性清理。
    pub fn validate(&mut self) {
        for f in &mut self.fences {
            f.icon_ids.retain(|id| self.icons.contains_key(id));
        }
        self.free_icons.retain(|id| self.icons.contains_key(id));
    }
}

impl IconLocation {
    /// 栅栏 id；未分组区返回 None。
    pub fn fence_id(&self) -> Option<u64> {
        match self {
            IconLocation::Free => None,
            IconLocation::Fence(id) => Some(*id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desk() -> Desk {
        Desk::new(crate::config::AppSettings::default())
    }

    fn icon(id: &str) -> Icon {
        Icon::new(id.to_string(), id.to_string(), ItemKind::Unknown)
    }

    #[test]
    fn move_icon_between_locations() {
        let mut d = desk();
        d.icons.insert("a".into(), icon("a"));
        d.icons.insert("b".into(), icon("b"));
        d.free_icons = vec!["a".into(), "b".into()];

        let f1 = d.next_fence_id();
        d.fences.push(Fence {
            id: f1,
            title: Some("工作".into()),
            monitor_id: 0,
            bounds: Rect::new(0.0, 0.0, 300.0, 200.0),
            state: FenceState::Expanded,
            icon_ids: Vec::new(),
            appearance: FenceAppearance::default(),
            kind: FenceKind::Manual,
            scroll: 0.0,
            storage_path: None,
            sidebar_collapsed: false,
            suppressed_paths: Vec::new(),
        });

        // 移到栅栏
        let from = d.move_icon(&"a".into(), Some(f1));
        assert_eq!(from, Some(IconLocation::Free));
        assert_eq!(d.icon_location(&"a".into()), Some(IconLocation::Fence(f1)));

        // 移回未分组区
        let from = d.move_icon(&"a".into(), None);
        assert_eq!(from, Some(IconLocation::Fence(f1)));
        assert_eq!(d.icon_location(&"a".into()), Some(IconLocation::Free));
    }

    #[test]
    fn move_icon_to_missing_fence_falls_back_to_free() {
        let mut d = desk();
        d.icons.insert("a".into(), icon("a"));
        d.free_icons = vec!["a".into()];
        let from = d.move_icon(&"a".into(), Some(999));
        assert_eq!(from, Some(IconLocation::Free));
        assert_eq!(d.icon_location(&"a".into()), Some(IconLocation::Free));
    }

    #[test]
    fn validate_drops_dangling_members() {
        let mut d = desk();
        d.icons.insert("a".into(), icon("a"));
        d.fences.push(Fence {
            id: 1,
            title: None,
            monitor_id: 0,
            bounds: Rect::default(),
            state: FenceState::Expanded,
            icon_ids: vec!["a".into(), "ghost".into()],
            appearance: FenceAppearance::default(),
            kind: FenceKind::Manual,
            scroll: 0.0,
            storage_path: None,
            sidebar_collapsed: false,
            suppressed_paths: Vec::new(),
        });
        d.free_icons = vec!["ghost2".into()];
        d.validate();
        assert_eq!(d.fences[0].icon_ids, vec!["a".to_string()]);
        assert!(d.free_icons.is_empty());
    }

    #[test]
    fn rect_contains_and_inset() {
        let r = Rect::new(10.0, 10.0, 100.0, 50.0);
        assert!(r.contains(Vec2 { x: 10.0, y: 10.0 }));
        assert!(!r.contains(Vec2 { x: 111.0, y: 10.0 }));
        let inner = r.inset(5.0);
        assert_eq!(inner, Rect::new(15.0, 15.0, 90.0, 40.0));
    }

    #[test]
    fn only_manual_fences_are_writable() {
        assert!(!FenceKind::Manual.is_read_only());
        assert!(FenceKind::DesktopInbox.is_read_only());
        assert!(FenceKind::FolderPortal.is_read_only());
    }

    #[test]
    fn fence_layout_cycles() {
        assert_eq!(FenceLayout::Grid.next(), FenceLayout::List);
        assert_eq!(FenceLayout::List.next(), FenceLayout::Sidebar);
        assert_eq!(FenceLayout::Sidebar.next(), FenceLayout::Grid);
    }

    #[test]
    fn sylva_appearance_serde_backward_compatible() {
        // 旧版 desk.json 的栅栏外观（无 style / border_width / layout）应能加载并取默认值
        let old = r#"{"bg_color":[0.08,0.08,0.12,0.55],"corner_radius":12.0,"acrylic":true,"title_bar_height":32.0,"padding":12.0,"icon_size":48.0,"gap":10.0}"#;
        let a: FenceAppearance = serde_json::from_str(old).expect("旧配置应可反序列化");
        assert_eq!(a.bg_style, FenceStyle::Glass);
        // 旧 icon_size 先进入迁移占位字段，再由 normalize 折算成档位
        assert_eq!(a.legacy_icon_size, Some(48.0));
        assert_eq!(a.cell, CellSize::Medium);
        assert_eq!(a.layout, FenceLayout::Grid);
        assert_eq!(a.opacity, 0.55);
        assert_eq!(a.tint, None);
        assert!(a.show_title);
        assert!(a.show_labels);
        assert_eq!(a.grid_cols, 0);
        assert_eq!(a.grid_rows, 0);
        assert_eq!(a.fill_opacity, 0.8);
        assert_eq!(a.icon_shape, IconShape::Square);
        assert_eq!(a.hover_effect, HoverEffect::Scale);
    }

    #[test]
    fn desk_console_fields_serde_backward_compatible() {
        // 旧版 desk.json 无 todos / console_open / console_pos：应取默认（打开控制台）
        let old = r#"{"version":1,"settings":{"show_free_area":true,"free_area_height":90.0,"hotkeys":{},"autostart":false},"fences":[],"free_icons":[],"icons":{}}"#;
        let d: Desk = serde_json::from_str(old).expect("旧配置应可反序列化");
        assert!(d.todos.is_empty());
        assert_eq!(d.next_todo_id, 1);
        assert!(d.console_open);
        assert_eq!(d.console_pos, None);
        assert_eq!(d.console_size, None);
        // 旧配置无插件/桌面模式字段：默认注册表（待办启用）+ 栅栏模式
        assert_eq!(d.plugins.len(), 2);
        assert!(d.plugins.iter().any(|p| p.id == "todo" && p.enabled));
        assert!(!d.desktop_mode);
    }

    #[test]
    fn old_added_flag_migrates_to_origin() {
        let old = r#"{"version":1,"settings":{"show_free_area":true,"free_area_height":90.0,"hotkeys":{},"autostart":false},"fences":[],"free_icons":[],"icons":{"a":{"id":"a","display_name":"A","kind":"Doc","added":true},"b":{"id":"b","display_name":"B","kind":"Doc"}}}"#;
        let d = crate::config::normalize(serde_json::from_str(old).expect("旧配置应可反序列化"));
        assert_eq!(d.icons["a"].origin, ItemOrigin::Collected);
        assert!(d.icons["a"].added);
        assert_eq!(d.icons["b"].origin, ItemOrigin::DesktopIcon);
        assert!(!d.icons["b"].invalid);
    }

    #[test]
    fn fence_suppressed_paths_are_backward_compatible() {
        let f: Fence = serde_json::from_str(
            r#"{"id":1,"title":null,"monitor_id":0,"bounds":{"x":0.0,"y":0.0,"w":10.0,"h":10.0},"state":"Expanded","icon_ids":[],"appearance":{"bg_color":[0.0,0.0,0.0,0.0],"corner_radius":0.0,"acrylic":false,"title_bar_height":0.0,"padding":0.0,"icon_size":0.0,"gap":0.0,"border_width":0.0,"opacity":0.0}}"#,
        )
        .expect("旧栅栏应可反序列化");
        assert!(f.suppressed_paths.is_empty());
    }

    #[test]
    fn plugin_entry_roundtrip() {
        let mut p = PluginEntry::builtin_notes();
        p.enabled = true;
        p.note_text = "买牛奶\n拿快递".into();
        let json = serde_json::to_string(&p).unwrap();
        let back: PluginEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "notes");
        assert!(back.enabled);
        assert_eq!(back.note_text, "买牛奶\n拿快递");
    }

    #[test]
    fn plugin_kind_labels() {
        assert_eq!(PluginKind::Todo.label(), "待办事项");
        assert_eq!(PluginKind::Notes.label(), "便签");
        assert_eq!(PluginKind::External.label(), "外部插件");
    }

    #[test]
    fn todo_item_roundtrip() {
        let t = TodoItem::new(7, "写周报".into(), "周五前提交".into());
        let json = serde_json::to_string(&t).unwrap();
        let back: TodoItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, 7);
        assert_eq!(back.name, "写周报");
        assert_eq!(back.detail, "周五前提交");
        assert!(!back.done);
    }

    #[test]
    fn todo_item_old_text_field_backward_compatible() {
        // 旧配置只有 `text`（无 detail）：应映射到 name，detail 取默认空串
        let old = r#"{"id":3,"text":"旧事项","done":true}"#;
        let t: TodoItem = serde_json::from_str(old).expect("旧待办应可反序列化");
        assert_eq!(t.name, "旧事项");
        assert_eq!(t.detail, "");
        assert!(t.done);
    }
}
