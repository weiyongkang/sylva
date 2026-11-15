//! 主题：配色与尺寸（全部物理像素）。
//!
//! 与应用无关：App 层负责把逻辑尺寸 × DPI 换算后填入。
//! 颜色为直通 alpha（straight alpha），绘制时由 D2D 内部转换为预乘。

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

/// 直通 alpha 的 RGBA 颜色（浮点，0..1）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// 转成 D2D 颜色。
    pub fn to_d2d(&self) -> D2D1_COLOR_F {
        D2D1_COLOR_F {
            r: self.r,
            g: self.g,
            b: self.b,
            a: self.a,
        }
    }
}

/// 文本样式。
#[derive(Debug, Clone, Copy)]
pub struct TextStyle {
    pub font_family: &'static str,
    pub size: f32,
    pub color: Color,
}

/// 主题：栅栏外观、标题、图标网格与文字。
///
/// 渲染目标固定为 96 DPI（1 DIP = 1 物理像素），因此**全部尺寸都是物理像素**。
/// App 层在 `Theme::default()` 后按 `scale = 系统DPI / 96` 把每个 DIP 度量放大；
/// `scale` 同时供绘制层的固定偏移（按钮留白、控制台边距等）按同一比例缩放，
/// 保证高 DPI 下布局比例一致、文字行列不重叠。
#[derive(Debug, Clone)]
pub struct Theme {
    /// DPI 缩放系数（系统 DPI / 96）；默认 1.0 = 100% 缩放。
    pub scale: f32,
    // 栅栏外观
    pub fence_bg: Color,
    pub fence_border: Color,
    pub fence_corner_radius: f32,
    /// 边框描边宽度（物理像素）：常态 1px 细线，选中时用强调色 2px。
    pub fence_border_width: f32,
    /// 模糊背景高斯标准偏差（物理像素，`GaussianBlurEffect::SetStandardDeviation`）。
    pub blur_stddev: f32,
    // 强调色（单一来源）：选中框、滑轨、按钮激活态共用，避免出现两种蓝。
    pub accent: [f32; 4],
    // 标题
    /// 栅栏标题（13 semibold）。
    pub fence_title: TextStyle,
    /// 控制中心标题（16）。
    pub title: TextStyle,
    // 图标
    pub list_row_gap: f32,
    pub list_label_gap: f32,
    // 文字
    pub label: TextStyle,
    /// 辅助说明 / 列头（11）。
    pub detail_size: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            scale: 1.0,
            fence_bg: Color::rgba(0.13, 0.15, 0.19, 0.60),
            fence_border: Color::rgba(1.0, 1.0, 1.0, 0.42),
            fence_corner_radius: 12.0,
            fence_border_width: 1.0,
            blur_stddev: 20.0,
            accent: [0.23, 0.51, 0.96, 0.92],
            fence_title: TextStyle {
                font_family: "Microsoft YaHei UI",
                size: 13.0,
                color: Color::rgba(1.0, 1.0, 1.0, 0.92),
            },
            title: TextStyle {
                font_family: "Microsoft YaHei UI",
                size: 16.0,
                color: Color::rgba(1.0, 1.0, 1.0, 0.90),
            },
            list_row_gap: 8.0,
            list_label_gap: 10.0,
            label: TextStyle {
                font_family: "Microsoft YaHei UI",
                size: 12.0,
                color: Color::rgba(1.0, 1.0, 1.0, 0.85),
            },
            detail_size: 11.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_metrics_are_sane() {
        let t = Theme::default();
        assert!(t.fence_border_width > 0.0);
        assert!(t.fence_corner_radius >= 0.0);
        assert!(t.fence_title.size > 0.0 && t.title.size > 0.0);
        assert!(t.detail_size >= 10.0);
        assert!(t.label.size >= t.detail_size);
        assert!(t.title.size > t.fence_title.size);
    }

    #[test]
    fn color_converts_to_d2d_matching_input() {
        let c = Color::rgba(0.2, 0.4, 0.6, 0.8);
        let d = c.to_d2d();
        assert!((d.r - 0.2).abs() < 1e-6);
        assert!((d.a - 0.8).abs() < 1e-6);
    }
}
