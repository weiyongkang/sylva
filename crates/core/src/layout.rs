//! 栅栏内网格布局计算。纯函数，可单测。

use crate::model::{CellSize, Rect, Vec2};

/// 网格单元格的运行时度量（物理像素，已乘 DPI 缩放）。
///
/// 布局（App）、绘制（Render）、就地编辑框三处共用同一份数值，避免"格宽一个
/// 公式、行高另一个公式"导致的标签错位。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellMetrics {
    /// 格宽。
    pub cell_w: f32,
    /// 格高（与名称开关无关）。
    pub cell_h: f32,
    /// 图标边长。
    pub icon: f32,
    /// 名称行高（0 = 不显示名称）。
    pub caption_h: f32,
    /// 图标与名称之间的间距（0 = 不显示名称）。
    pub caption_gap: f32,
    /// 格内上下留白。
    pub pad_v: f32,
}

impl CellMetrics {
    /// 图标在格内的纵向起点（格顶为 0）。
    pub fn icon_top(&self) -> f32 {
        if self.caption_h > 0.0 {
            self.pad_v
        } else {
            ((self.cell_h - self.icon) / 2.0).max(0.0)
        }
    }

    /// 名称行的纵向起点（格顶为 0）；不显示名称时返回格底。
    pub fn caption_top(&self) -> f32 {
        if self.caption_h > 0.0 {
            self.pad_v + self.icon + self.caption_gap
        } else {
            self.cell_h
        }
    }
}

/// 标题带高度：**栅栏顶部到内容区顶边 = 半格**。
///
/// 上半格由「顶部内边距 + 标题带」共同占据，因此标题带本身要减去 `pad`；
/// 下限是放得下标题字（1.5 倍字号）。
///
/// 布局（App）、就地编辑框、命中模型共用，避免三处各写一份公式。
pub fn header_height(cell_h: f32, title_size: f32, pad: f32, show_title: bool) -> f32 {
    if show_title {
        (cell_h * 0.5 - pad).max(title_size * 1.5)
    } else {
        0.0
    }
}

/// 按档位 + 名称开关算出单元格度量（逻辑尺寸 × `scale`）。
pub fn cell_metrics(cell: CellSize, show_labels: bool, scale: f32) -> CellMetrics {
    let icon = if show_labels {
        cell.icon()
    } else {
        cell.icon_without_labels()
    };
    CellMetrics {
        cell_w: cell.cell_w() * scale,
        cell_h: cell.cell_h() * scale,
        icon: icon * scale,
        caption_h: if show_labels {
            CellSize::CAPTION_H * scale
        } else {
            0.0
        },
        caption_gap: if show_labels {
            CellSize::CAPTION_GAP * scale
        } else {
            0.0
        },
        pad_v: CellSize::PAD_V * scale,
    }
}

/// 网格布局参数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridLayout {
    pub icon_size: f32,
    pub gap: f32,
    pub padding: f32,
}

impl Default for GridLayout {
    fn default() -> Self {
        Self {
            icon_size: 48.0,
            gap: 10.0,
            padding: 12.0,
        }
    }
}

impl GridLayout {
    /// 计算指定内容宽度下能容纳的列数（至少 1 列）。
    pub fn cols_for_width(&self, inner_w: f32) -> usize {
        if inner_w <= self.icon_size {
            return 1;
        }
        ((inner_w + self.gap) / (self.icon_size + self.gap)).floor() as usize
    }

    /// 图标占满指定列数所需的行数。
    pub fn rows_for_count(&self, count: usize, cols: usize) -> usize {
        if count == 0 {
            0
        } else {
            count.div_ceil(cols.max(1))
        }
    }

    /// 为 `count` 个图标排布网格，返回各图标**中心点**（基于 `area` 内边距）。
    ///
    /// 从左到右、从上到下填充；行随内容宽度自适应。
    pub fn arrange(&self, count: usize, area: &Rect) -> Vec<Vec2> {
        let inner = area.inset(self.padding);
        let cols = self.cols_for_width(inner.w).max(1);
        let step = self.icon_size + self.gap;

        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let row = (i / cols) as f32;
            let col = (i % cols) as f32;
            let x = inner.x + col * step + self.icon_size / 2.0;
            let y = inner.y + row * step + self.icon_size / 2.0;
            out.push(Vec2 { x, y });
        }
        out
    }

    /// 内容占用的最小高度（供折叠动画推算目标高度）。
    pub fn content_height(&self, count: usize, cols: usize) -> f32 {
        let rows = self.rows_for_count(count, cols);
        if rows == 0 {
            0.0
        } else {
            rows as f32 * self.icon_size + (rows - 1) as f32 * self.gap
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> GridLayout {
        GridLayout {
            icon_size: 48.0,
            gap: 10.0,
            padding: 10.0,
        }
    }

    #[test]
    fn arrange_fills_left_to_right_top_to_bottom() {
        let area = Rect::new(0.0, 0.0, 200.0, 200.0);
        // 内宽 = 180，列数 = floor((180+10)/58) = 3
        let pos = g().arrange(4, &area);
        assert_eq!(pos.len(), 4);
        // 第一行: x = 10 + 0*58 + 24 = 34 ; 第二行: y = 10 + 1*58 + 24 = 92
        assert_eq!(pos[0], Vec2 { x: 34.0, y: 34.0 });
        assert_eq!(pos[1], Vec2 { x: 92.0, y: 34.0 });
        assert_eq!(pos[2], Vec2 { x: 150.0, y: 34.0 });
        assert_eq!(pos[3], Vec2 { x: 34.0, y: 92.0 });
    }

    #[test]
    fn cols_never_zero() {
        assert_eq!(g().cols_for_width(5.0), 1);
        assert_eq!(g().cols_for_width(48.0), 1);
    }

    #[test]
    fn cell_metrics_grow_with_tier() {
        let small = cell_metrics(CellSize::Small, true, 1.0);
        let medium = cell_metrics(CellSize::Medium, true, 1.0);
        let large = cell_metrics(CellSize::Large, true, 1.0);
        assert!(small.cell_w < medium.cell_w && medium.cell_w < large.cell_w);
        assert!(small.cell_h < medium.cell_h && medium.cell_h < large.cell_h);
        assert!(small.icon < medium.icon && medium.icon < large.icon);
        // 图标 + 名称必须装得进格子（名称行不压到图标）
        for m in [small, medium, large] {
            assert!(m.caption_top() + m.caption_h <= m.cell_h + 0.01);
            assert!(m.icon_top() + m.icon <= m.caption_top() + 0.01);
        }
    }

    #[test]
    fn hiding_labels_grows_icon_but_keeps_cell() {
        for cell in CellSize::ALL {
            let with = cell_metrics(cell, true, 1.0);
            let without = cell_metrics(cell, false, 1.0);
            // 格子不变（开关名称不重排）
            assert_eq!(with.cell_w, without.cell_w);
            assert_eq!(with.cell_h, without.cell_h);
            // 图标放大且不溢出
            assert!(without.icon > with.icon);
            assert!(without.icon <= without.cell_w + 0.01);
            assert!(without.icon <= without.cell_h + 0.01);
            // 图标纵向居中
            let top = without.icon_top();
            assert!((top - (without.cell_h - without.icon) / 2.0).abs() < 0.01);
        }
    }

    #[test]
    fn header_plus_pad_is_half_cell() {
        // 中档：格子 80、内边距 12 → 标题带 28，顶部 12 + 28 = 40 = 半格
        let pad = 12.0;
        let h = header_height(80.0, 13.0, pad, true);
        assert!((h - 28.0).abs() < 0.01, "header={h}");
        assert!((pad + h - 40.0).abs() < 0.01);
        // 半格放不下标题字时以字号为下限
        assert!((header_height(30.0, 13.0, pad, true) - 19.5).abs() < 0.01);
        // 无标题不占高
        assert_eq!(header_height(80.0, 13.0, pad, false), 0.0);
    }

    #[test]
    fn cell_metrics_scale_with_dpi() {
        let m1 = cell_metrics(CellSize::Medium, true, 1.0);
        let m2 = cell_metrics(CellSize::Medium, true, 1.5);
        assert!((m2.cell_w - m1.cell_w * 1.5).abs() < 0.01);
        assert!((m2.icon - m1.icon * 1.5).abs() < 0.01);
    }

    #[test]
    fn content_height_matches_rows() {
        assert_eq!(g().content_height(0, 3), 0.0);
        assert_eq!(g().content_height(1, 3), 48.0);
        assert_eq!(g().content_height(4, 3), 48.0 * 2.0 + 10.0);
    }
}
