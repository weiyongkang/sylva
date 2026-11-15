//! 场景模型：一次绘制所需的全部数据（应用无关）。
//!
//! 坐标全部为**物理像素**（虚拟屏幕坐标），与 overlay 窗口客户端坐标一致。

use sylva_core::model::{
    CellSize, FenceLayout, FenceStyle, HoverEffect, IconShape, SidebarPosition,
};

use crate::overlay::{ConsoleZone, RectF};

/// 溢出格：可视容量放不下的图标折叠成一格九宫格缩略图。
#[derive(Debug, Clone, Default)]
pub struct OverflowCell {
    /// 被折叠的图标数量。
    pub count: usize,
    /// 缩略图位图 ID（最多 9 个，按 3×3 平铺）。
    pub thumbs: Vec<u64>,
}

/// 栅栏内的一个图标（位置由 App 层按主题网格/列表排布后填入）。
#[derive(Debug, Clone)]
pub struct SceneIcon {
    /// 图标文字（网格=下方，列表=名称列）。
    pub label: String,
    /// 对应 `IconStore` 中的位图 ID（`IconStore::insert` 返回）。
    pub bitmap_id: u64,
    /// 图标左上角（物理像素，虚拟屏幕坐标）。
    pub x: f32,
    pub y: f32,
    pub size: f32,
    /// 列表详情列文本：类型 / 修改日期 / 大小（网格模式下为空串）。
    pub col_type: String,
    pub col_modified: String,
    pub col_size: String,
    /// 悬停缩放 1.0=常态，>1 = 放大中（App 层按 hover 补间填值，绘制时以中心放大）。
    pub scale: f32,
    /// 路径失效（文件被外部删除/移动）：降透明度显示，双击不打开。
    pub invalid: bool,
    /// OLE 拖出进行中：原位降透明度显示（拖放源视觉反馈）。
    pub dragging: bool,
    /// 溢出格（Some = 该格画九宫格缩略图 + 计数，点击展开）。
    pub overflow: Option<OverflowCell>,
}

/// 列表布局的详情列位置（绝对虚拟屏幕 x；名称列紧贴图标右侧，不入列）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ListColumns {
    pub type_x: f32,
    pub modified_x: f32,
    pub size_x: f32,
    /// 列表列头高度（绘制列头与滚动裁剪共用）。
    pub header_h: f32,
}

/// 一个栅栏。
#[derive(Debug, Clone)]
pub struct SceneFence {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub title: String,
    /// 是否显示标题文字（false 时标题栏仍是拖动把手）。
    pub show_title: bool,
    /// 栅栏四边内边距（物理像素，唯一来源：标题、内容区、滚动条、裁剪共用）。
    pub padding: f32,
    /// 标题带高度（物理像素）= 一个网格单元格高；无标题时为 0。
    pub header_h: f32,
    /// 网格名称行高（物理像素；不显示名称时为 0）。
    pub caption_h: f32,
    /// 图标与名称之间的间距（物理像素；不显示名称时为 0）。
    pub caption_gap: f32,
    /// 是否显示图标名称标签。
    pub show_labels: bool,
    /// 图标形状：方角 / 圆角 / 圆形。
    pub icon_shape: IconShape,
    /// 图标悬停效果。
    pub hover_effect: HoverEffect,
    /// 用户是否隐藏该栅栏。
    pub hidden: bool,
    /// 控制中心当前选中的栅栏：绘制高对比边框。
    pub console_selected: bool,
    pub icons: Vec<SceneIcon>,
    /// 布局格式：网格 / 列表。
    pub layout: FenceLayout,
    /// 列表布局的详情列位置；网格布局为 None。
    pub list_cols: Option<ListColumns>,
    /// 网格布局的格宽（物理像素）；列表/侧边栏为 0。网格标签两行排布与
    /// 悬停工具提示是否截断的判断共用（标签横跨整格绘制）。
    pub grid_cell_w: f32,
    /// 当前内容滚动偏移（物理像素）。
    pub scroll: f32,
    /// 可滚动范围（0 = 内容不超出可视区，无滚动条）。
    pub scroll_max: f32,
    /// 内容可视区高度（滚动条比例用）。
    pub scroll_view: f32,
    /// 内容区顶边（绝对 y）：列表列头与网格第一行从这里开始，绘制裁剪用。
    pub content_top: f32,
    /// 内容区左边（绝对 x）：行/列头/滚动条以此为左基准（与栅栏内边距一致）。
    pub content_left: f32,
    /// 当前鼠标悬停的图标下标（布局中的顺序，与 `icons` 一致）；None = 无悬停。
    pub hover_icon: Option<usize>,
    /// 当前选中的图标下标（多选：框选 / Ctrl 单击，与资源管理器一致；顺序与 `icons` 一致）。
    pub selected: Vec<usize>,
    /// 框选橡皮筋矩形（物理像素）；None = 未在框选。绘制时裁剪在本栅栏内容区内。
    pub select_band: Option<RectF>,
    /// 边框描边宽度（物理像素）；0 = 不描边。
    pub border_width: f32,
    /// 边框颜色（直通 alpha）。
    pub border_color: [f32; 4],
    /// 背景填充颜色（直通 alpha）；None = 内部完全透明（仅描边 / 模糊）。
    pub fill_color: Option<[f32; 4]>,
    /// 模糊背景（FenceStyle::Blur）：背景由合成器里独立的 GaussianBlurEffect 视觉
    /// 绘制（GPU，实时），本栅栏内容区保持透明以透出；合成器据此建/删模糊视觉。
    pub blur: bool,
    /// 整体透明度 0..1（桌面切换淡出/淡入用；绘制时乘到所有颜色上）。
    pub alpha: f32,
    /// 侧边栏悬停工具提示矩形（物理像素，由 App 层按屏幕边界计算好，
    /// 可延伸到栅栏之外）；None = 不显示。非侧边栏布局恒为 None。
    pub tooltip_rect: Option<RectF>,
    /// 侧边栏图标拖动排序状态：Some 时绘制拖动中的图标位置。
    pub reorder_drag: Option<ReorderDrag>,
}

/// 侧边栏图标拖动排序的渲染状态。
#[derive(Debug, Clone, Copy)]
pub struct ReorderDrag {
    /// 被拖动的图标在 `icon_ids` 中的下标。
    pub icon_idx: usize,
    /// 当前光标位置（虚拟屏幕物理像素，图标中心跟随此位置）。
    pub cursor_x: f32,
    pub cursor_y: f32,
}

/// 控制中心右侧内容页。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConsolePage {
    /// 系统控制：桌面切换、快速隐藏、开机自启。
    #[default]
    System,
    /// 当前选中栅栏的布局/外观设置。
    Fence,
}

impl SceneFence {
    /// 点是否落在栅栏矩形内（命中测试用）。
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.width && py >= self.y && py <= self.y + self.height
    }
}

/// 待办插件的一条（渲染用）。二级结构：名称 + 详细信息。
#[derive(Debug, Clone)]
pub struct SceneTodoRow {
    /// 事项名称（一级）。
    pub name: String,
    /// 详细信息（二级）；空串 = 无副标题。
    pub detail: String,
    pub done: bool,
    /// 行不透明度 0..1（入场淡入 / 删除幽灵淡出）。
    pub alpha: f32,
    /// 完成状态交叉淡化 0..1（0=旧状态，1=新状态）。
    pub done_progress: f32,
}

/// 待办插件渲染数据：条目 + 滚动状态 + 内容区几何。
#[derive(Debug, Clone, Default)]
pub struct SceneTodo {
    pub rows: Vec<SceneTodoRow>,
    /// 列表滚动偏移（物理像素，向上卷出顶部）。
    pub scroll: f32,
    /// 可滚动范围（0 = 不超出可视区）。
    pub scroll_max: f32,
    /// 首行内容的绝对 y（输入行之下）。
    pub rows_top: f32,
    /// 单行高（物理像素）。
    pub row_h: f32,
}

/// 栅栏管理页：可点选的一行（选中后在下方详情区显示控制项）。
#[derive(Debug, Clone)]
pub struct SceneFenceRow {
    pub rect: RectF,
    pub title: String,
    pub selected: bool,
}

/// 栅栏管理页：选中栅栏的详情控制区（分段按钮 + 色调色板）。
#[derive(Debug, Clone)]
pub struct SceneFenceDetail {
    pub rect: RectF,
    pub title: String,
    /// 栅栏重命名按钮。
    pub rename_btn: RectF,
    /// 当前值（绘制时高亮对应分段按钮 / 色板项）。
    pub layout: FenceLayout,
    /// 网格单元格档位（小 / 中 / 大）。
    pub cell: CellSize,
    pub style: FenceStyle,
    pub tint: Option<[f32; 3]>,
    /// 布局：网格 / 列表 / 侧边栏
    pub layout_grid: RectF,
    pub layout_list: RectF,
    pub layout_sidebar: RectF,
    /// 单元格档位：小 / 中 / 大
    pub size_s: RectF,
    pub size_m: RectF,
    pub size_l: RectF,
    /// 背景风格：玻璃 / 描边 / 颜色 / 模糊
    pub style_glass: RectF,
    pub style_outline: RectF,
    pub style_filled: RectF,
    pub style_blur: RectF,
    /// 色调：默认（恢复玻璃底色）+ 预设色板（与 App 层 TINT_PRESETS 平行）。
    pub tint_default: RectF,
    pub tints: Vec<RectF>,
    /// 「更改位置…」按钮（存储位置行）。
    pub storage_btn: RectF,
    /// 当前侧边栏停靠位置（仅 Sidebar 布局显示）。
    pub sidebar_pos: SidebarPosition,
    /// 侧边栏位置按钮：左 / 上 / 右
    pub sidebar_left: RectF,
    pub sidebar_top: RectF,
    pub sidebar_right: RectF,
    /// 标题/名称标签显示开关。
    pub show_title: bool,
    pub show_labels: bool,
    pub title_toggle: RectF,
    pub labels_toggle: RectF,
    /// 溢出处理：九宫格折叠 / 换行滚动。
    pub overflow: sylva_core::model::OverflowMode,
    pub overflow_fold: RectF,
    pub overflow_wrap: RectF,
    /// 网格固定列/行数（0 = 自动）。
    pub grid_cols: u32,
    pub grid_rows: u32,
    pub cols_minus: RectF,
    pub cols_plus: RectF,
    pub rows_minus: RectF,
    pub rows_plus: RectF,
    /// 网格选择器：8×8 个 32px 小格；点选第 (col,row) 格 = 列数/行数。
    pub grid_picker: RectF,
    pub grid_picker_cell: f32,
    /// 背景填充不透明度快捷档位。
    pub fill_opacity: f32,
    /// 透明度滑轨（整条）与 21 个点击分段（0..100%，5% 一步）。
    pub opacity_track: RectF,
    pub opacity_steps: Vec<RectF>,
    /// 图标形状：方角 / 圆角 / 圆形。
    pub icon_shape: IconShape,
    pub shape_square: RectF,
    pub shape_rounded: RectF,
    pub shape_circle: RectF,
    /// 悬停效果：关闭 / 高亮 / 放大。
    pub hover_effect: HoverEffect,
    pub hover_none: RectF,
    pub hover_highlight: RectF,
    pub hover_scale: RectF,
    /// 是否渲染该栅栏。
    pub hidden: bool,
    pub visibility_toggle: RectF,
    /// 自定义色调（系统颜色选择器）。
    pub custom_color: RectF,
}

/// 控制台面板（控制中心：栅栏管理）。
///
/// 几何字段同时供绘制（draw_console）与命中模型（hit_model_from）使用，
/// 保证点击区域与视觉一致。
#[derive(Debug, Clone)]
pub struct SceneConsole {
    /// 左侧导航：系统控制行。
    pub system_nav: RectF,
    /// 当前右侧内容页。
    pub page: ConsolePage,
    /// 当前选中的栅栏下标（导航高亮/详情）。
    pub selected_fence: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// 标题栏高（拖动把手；关闭按钮与桌面切换按钮位于其内）。
    pub title_h: f32,
    /// 关闭按钮矩形（标题栏右上）。
    pub close: RectF,
    /// 「切换桌面」按钮矩形（标题栏，关闭按钮左侧）。
    pub desktop_toggle: RectF,
    /// 系统控制页：切换桌面 / 快速隐藏 / 开机自启。
    pub system_desktop: RectF,
    pub system_quick_hide: RectF,
    pub system_autostart: RectF,
    pub system_overlap: RectF,
    pub system_reset_size: RectF,
    /// 通用常量：栅栏间距 / 桌面留白（行 + −/+ 按钮）。
    pub system_gap_row: RectF,
    pub system_gap_minus: RectF,
    pub system_gap_plus: RectF,
    pub system_inset_row: RectF,
    pub system_inset_minus: RectF,
    pub system_inset_plus: RectF,
    pub fence_gap: f32,
    pub desktop_inset: f32,
    /// 栅栏管理页：可点选栅栏行（与 `desk.fences` 平行）。
    pub fence_rows: Vec<SceneFenceRow>,
    /// 栅栏管理页：列表可视区（行超出部分被裁剪，命中模型据此跳过不可见行）。
    pub fence_list_view: RectF,
    /// 栅栏列表滚动偏移与可滚动范围（绘制滚动条 + 滚轮判定用）。
    pub fence_scroll: f32,
    pub fence_scroll_max: f32,
    /// 栅栏管理页：选中栅栏的详情控制区。
    pub fence_detail: Option<SceneFenceDetail>,
    /// 右侧详情的可视裁剪区与可滚动范围。
    pub detail_view: RectF,
    pub detail_scroll_max: f32,
    /// 栅栏管理页：「添加栅栏」按钮。
    pub add_fence: RectF,
    /// 栅栏管理页：「删除栅栏」按钮（添加按钮下方）。
    pub remove_btn: RectF,
    pub fill_color: [f32; 4],
    pub border_color: [f32; 4],
    /// 面板展开进度 0..1（0=折叠胶囊，1=完整面板）。
    pub panel: f32,
    /// 当前悬停的控制台控件（App 层经 ConsoleHover 事件写入；绘制高亮用）。
    pub hover_zone: Option<ConsoleZone>,
    /// 是否处于原始桌面模式（系统控制按钮文案与状态）。
    pub desktop_mode: bool,
    /// 开机自启当前设置（系统控制按钮状态）。
    pub autostart: bool,
    /// 是否允许栅栏叠加。
    pub fence_overlap: bool,
}

/// 内联文本编辑的渲染数据（App 层 InlineEdit 的只读快照，绘制用）。
#[derive(Debug, Clone)]
pub struct SceneEdit {
    pub rect: RectF,
    pub lines: Vec<String>,
    pub line: usize,
    pub col: usize,
    pub placeholder: String,
    pub single_line: bool,
    pub focused: bool,
    pub composing: bool,
    pub comp: String,
}

/// 对齐参考线（虚拟屏幕坐标）：拖动/缩放栅栏时与邻居或屏幕对齐的位置。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuideLine {
    /// true = 竖线（对齐 x），false = 横线（对齐 y）。
    pub vertical: bool,
    /// 线的位置（x 或 y，物理像素）。
    pub pos: f32,
    /// 线的起止（另一轴的区间）。
    pub from: f32,
    pub to: f32,
}

/// 拖拽/缩放时的尺寸徽标（如 `320 × 240`）。
#[derive(Debug, Clone, PartialEq)]
pub struct SceneHud {
    pub text: String,
    /// 徽标左上角（物理像素）。
    pub x: f32,
    pub y: f32,
    /// 徽标尺寸（物理像素；由 App 按文本估算，绘制与合成表面共用）。
    pub w: f32,
    pub h: f32,
}

/// 整屏场景（虚拟屏幕）。
#[derive(Debug, Default)]
pub struct Scene {
    /// 虚拟屏幕尺寸（物理像素）。
    pub width: f32,
    pub height: f32,
    pub fences: Vec<SceneFence>,
    /// 当前激活的内联文本编辑（None = 无）。
    pub edit: Option<SceneEdit>,
    /// 控制台面板（插件宿主）；None = 本帧不画。
    pub console: Option<SceneConsole>,
    /// 对齐参考线（拖动/缩放中；空闲时为空）。
    pub guides: Vec<GuideLine>,
    /// 尺寸徽标（拖动/缩放中；空闲时 None）。
    pub hud: Option<SceneHud>,
}

impl Scene {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            fences: Vec::new(),
            edit: None,
            console: None,
            guides: Vec::new(),
            hud: None,
        }
    }

    /// 全部内容的包围盒（虚拟屏幕坐标：全部栅栏 ∪ 控制台）。
    ///
    /// 合成器据此把合成表面缩到内容大小而非整屏（省内存）。没有内容时返回 None。
    pub fn content_rect(&self) -> Option<RectF> {
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        let mut any = false;
        for g in &self.guides {
            if g.vertical {
                min_x = min_x.min(g.pos);
                max_x = max_x.max(g.pos);
                min_y = min_y.min(g.from);
                max_y = max_y.max(g.to);
            } else {
                min_y = min_y.min(g.pos);
                max_y = max_y.max(g.pos);
                min_x = min_x.min(g.from);
                max_x = max_x.max(g.to);
            }
            any = true;
        }
        if let Some(h) = &self.hud {
            min_x = min_x.min(h.x);
            min_y = min_y.min(h.y);
            max_x = max_x.max(h.x + h.w);
            max_y = max_y.max(h.y + h.h);
            any = true;
        }
        for f in &self.fences {
            // 隐藏的栅栏不占合成表面：否则一个远处被隐藏的栅栏会把整块表面撑大。
            if f.hidden {
                continue;
            }
            min_x = min_x.min(f.x);
            min_y = min_y.min(f.y);
            max_x = max_x.max(f.x + f.width);
            max_y = max_y.max(f.y + f.height);
            any = true;
            // 侧边栏工具提示可延伸到栅栏之外，必须并入表面，否则区域外绘制不可见。
            if let Some(tt) = f.tooltip_rect {
                min_x = min_x.min(tt.x);
                min_y = min_y.min(tt.y);
                max_x = max_x.max(tt.x + tt.w);
                max_y = max_y.max(tt.y + tt.h);
            }
        }
        if let Some(c) = &self.console {
            min_x = min_x.min(c.x);
            min_y = min_y.min(c.y);
            max_x = max_x.max(c.x + c.width);
            max_y = max_y.max(c.y + c.height);
            any = true;
        }
        if !any {
            return None;
        }
        Some(RectF {
            x: min_x,
            y: min_y,
            w: max_x - min_x,
            h: max_y - min_y,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_fence() -> SceneFence {
        SceneFence {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 80.0,
            title: "测试".into(),
            show_title: true,
            padding: 12.0,
            header_h: 40.0,
            caption_h: 16.0,
            caption_gap: 4.0,
            show_labels: true,
            icon_shape: IconShape::Square,
            hover_effect: HoverEffect::Scale,
            hidden: false,
            console_selected: false,
            icons: vec![],
            layout: FenceLayout::Grid,
            list_cols: None,
            grid_cell_w: 72.0,
            scroll: 0.0,
            scroll_max: 0.0,
            scroll_view: 0.0,
            content_top: 0.0,
            content_left: 0.0,
            hover_icon: None,
            selected: vec![],
            select_band: None,
            border_width: 1.0,
            border_color: [1.0, 1.0, 1.0, 0.1],
            fill_color: Some([0.08, 0.08, 0.12, 0.55]),
            blur: false,
            alpha: 1.0,
            tooltip_rect: None,
            reorder_drag: None,
        }
    }

    #[test]
    fn fence_contains_checks_rectangle() {
        let f = test_fence();
        assert!(f.contains(10.0, 20.0));
        assert!(f.contains(110.0, 100.0)); // 右下角（含边界）
        assert!(!f.contains(9.0, 20.0));
        assert!(!f.contains(10.0, 101.0));
    }

    #[test]
    fn fence_style_fields_are_plumbed() {
        let f = test_fence();
        assert_eq!(f.border_width, 1.0);
        assert!(f.fill_color.is_some());
        // 描边模式：无填充
        let outline = SceneFence {
            fill_color: None,
            border_width: 2.5,
            ..test_fence()
        };
        assert!(outline.fill_color.is_none());
        assert_eq!(outline.border_width, 2.5);
    }

    #[test]
    fn fence_scroll_and_columns_fields_exist() {
        let f = test_fence();
        assert_eq!(f.layout, FenceLayout::Grid);
        assert_eq!(f.scroll, 0.0);
        assert_eq!(f.scroll_max, 0.0);
        assert!(f.list_cols.is_none());
        assert!(f.hover_icon.is_none());
    }

    #[test]
    fn list_columns_store_absolute_x() {
        let c = ListColumns {
            type_x: 200.0,
            modified_x: 300.0,
            size_x: 450.0,
            header_h: 26.0,
        };
        assert_eq!(c.type_x, 200.0);
        assert!(c.size_x > c.modified_x);
        assert_eq!(c.header_h, 26.0);
    }

    #[test]
    fn content_rect_skips_hidden_fences() {
        let mut s = Scene::new(3072.0, 1920.0);
        let visible = test_fence();
        let hidden = SceneFence {
            x: 2000.0,
            y: 1000.0,
            width: 500.0,
            height: 500.0,
            hidden: true,
            ..test_fence()
        };
        s.fences = vec![visible, hidden];
        let r = s.content_rect().expect("可见栅栏应产生包围盒");
        // 隐藏栅栏不占位：包围盒只覆盖可见那一个
        assert!((r.x - 10.0).abs() < 0.01);
        assert!((r.w - 100.0).abs() < 0.01);
        assert!((r.h - 80.0).abs() < 0.01);
    }

    #[test]
    fn scene_defaults_empty() {
        let s = Scene::new(1920.0, 1080.0);
        assert!(s.fences.is_empty());
        assert_eq!(s.width, 1920.0);
        assert!(s.content_rect().is_none());
    }

    #[test]
    fn content_rect_covers_fences() {
        let mut s = Scene::new(3072.0, 1920.0);
        let f1 = test_fence(); // (10,20,100,80)
        let f2 = SceneFence {
            x: 400.0,
            y: 300.0,
            width: 250.0,
            height: 120.0,
            ..test_fence()
        };
        s.fences = vec![f1, f2];
        let r = s.content_rect().expect("有内容应返回包围盒");
        assert_eq!(r.x, 10.0);
        assert_eq!(r.y, 20.0);
        assert_eq!(r.w, 640.0); // (400+250) - 10
        assert_eq!(r.h, 400.0); // (300+120) - 20
    }
}
