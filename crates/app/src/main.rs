//! Sylva —— 桌面栅栏整理器入口。
//!
// 发布版不弹终端窗口（双击直接运行，关掉启动它的 cmd 不影响本进程）；
// 调试版保留控制台便于看日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//!
//! 启动流程：
//! 1. 进程 DPI 感知 + COM 初始化 + 日志；
//! 2. 壳层接管：探测层级，隐藏真实图标（只隐藏 `SysListView32`，不碰其他窗口树，WE 共存）；
//! 3. GPU 上下文 + overlay 窗口 + 合成器；
//! 4. 枚举真实桌面图标，提取全部图标位图；首次运行创建演示栅栏布局；
//! 5. 按主题网格排布多栅栏并呈现；设置命中模型（`SetWindowRgn` 把窗口区域裁剪为
//!    栅栏并集，区域外点击穿透到桌面——修复全屏死区）；
//! 6. 进入消息循环：标题栏拖动栅栏、右下角缩放（高度自适应内容）、双击图标打开，
//!    变更实时重绘并持久化；Ctrl+C / Ctrl+Shift+F10 恢复真实图标并干净退出。

mod anim;
mod autostart;
mod context_menu;
mod editing;
mod file_ops;
mod guard;
mod history;
mod logging;
mod memory;
mod persist;
mod scene;
mod shell_menu;
mod snapshot;
mod store;
mod watchdog;

pub(crate) use std::cell::RefCell;
pub(crate) use std::collections::HashMap;
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::rc::Rc;
pub(crate) use std::sync::OnceLock;
pub(crate) use std::time::Instant;

pub(crate) use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HGLOBAL, HWND, LPARAM, POINT, RECT, WPARAM,
};
pub(crate) use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
pub(crate) use windows::Win32::System::Console::{
    CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, SetConsoleCtrlHandler,
};
pub(crate) use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
pub(crate) use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
pub(crate) use windows::Win32::System::Ole::OleInitialize;
pub(crate) use windows::Win32::System::Threading::CreateMutexW;
pub(crate) use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
pub(crate) use windows::Win32::UI::Input::Ime::{
    CFS_POINT, COMPOSITIONFORM, ImmGetContext, ImmReleaseContext, ImmSetCompositionWindow,
};
pub(crate) use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_BACK, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_RETURN, VK_RIGHT, VK_UP,
};
pub(crate) use windows::Win32::UI::Shell::{
    ABE_BOTTOM, ABE_LEFT, ABE_RIGHT, ABE_TOP, ABM_GETSTATE, ABM_GETTASKBARPOS, ABS_AUTOHIDE,
    APPBARDATA, DragQueryFileW, FOS_ALLOWMULTISELECT, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS,
    FileOpenDialog, HDROP, IFileOpenDialog, IShellItem, IShellItemArray, SHAppBarMessage,
    SIGDN_FILESYSPATH,
};
pub(crate) use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetSystemMetrics, HMENU, MF_CHECKED,
    MF_GRAYED, MF_SEPARATOR, MF_STRING, PostMessageW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    SetProcessDPIAware, SystemParametersInfoW, TPM_NONOTIFY, TPM_RETURNCMD, TrackPopupMenu,
};
pub(crate) use windows::core::{BOOL, PCWSTR, PWSTR};

pub(crate) use sylva_core::config::AppSettings;
pub(crate) use sylva_core::drag::DropEffect;
pub(crate) use sylva_core::magnet::{
    FreeSides, SNAP_DIST, alignment_guides, settle_move, settle_resize, snap_move, snap_resize,
};
pub(crate) use sylva_core::model::{
    Desk, Fence, FenceAppearance, FenceKind, FenceLayout, FenceState, FenceStyle, HoverEffect,
    Icon, Rect, SidebarPosition, Vec2,
};
pub(crate) use sylva_core::storage::DeskStore;
pub(crate) use sylva_render::{
    Compositor, ConsoleHit, ConsolePage, ConsoleZone, FenceHit, GRIP_SIZE, GuideLine, HitModel,
    IconHit, ListColumns, OverlayEvent, OverlayWindow, RectF, RenderDevice, ResizeZone, Scene,
    SceneConsole, SceneEdit, SceneFence, SceneFenceDetail, SceneFenceRow, SceneHud, SceneIcon,
    Theme, WM_APP_QUIT, WM_SYLVA_INJECT, run_message_loop,
};
pub(crate) use sylva_shell::icons::IconData;
pub(crate) use sylva_shell::items::DesktopItem;
pub(crate) use sylva_shell::takeover::DesktopHierarchy;

use crate::anim::*;
use crate::context_menu::*;
use crate::editing::*;
use crate::file_ops::*;
use crate::history::{capture_fence, capture_items, record, undo_last};
use crate::persist::commit_or_rollback;
use crate::scene::*;
use sylva_core::commands::{self, ClassifyMode};
use sylva_core::history::UndoAction;

/// 栅栏最小宽度（缩放下限，物理像素）。
pub(crate) const MIN_FENCE_W: f32 = 200.0;

/// 栅栏最小高度（缩放下限，物理像素）。
pub(crate) const MIN_FENCE_H: f32 = 60.0;

/// 栅栏之间的最小间距（物理像素）= 全局设置 × DPI。
fn fence_gap(rt: &Runtime) -> f32 {
    rt.desk.settings.fence_gap * rt.theme.scale
}

/// 桌面安全区（物理像素）= 全局设置 × DPI。
fn desktop_inset(rt: &Runtime) -> f32 {
    rt.desk.settings.desktop_inset * rt.theme.scale
}

/// 图标提取边长（物理像素）。取高于所有渲染尺寸的值（最大图标 64、名称关闭时
/// 80、列表 20），渲染时向下采样才清晰；若按渲染尺寸提取再放大，高 DPI 下发糊。
pub(crate) const ICON_EXTRACT_SIZE: u32 = 96;

// 右键菜单项 ID（分段避免冲突）。

// ---- 控制中心（桌面组件 + 栅栏管理）布局常量（DIP，× scale 变物理像素）----
/// 控制台面板默认宽度（用户拖边缘缩放后由 `desk.console_size` 覆盖）。
/// 控制台面板最小宽/高（缩放钳制，避免缩到无法交互）。
pub(crate) const CONSOLE_MIN_W: f32 = 420.0;
pub(crate) const CONSOLE_MIN_H: f32 = 170.0;
/// 控制台与屏幕右/上边距（未拖动时的默认摆放位置）。
pub(crate) const CONSOLE_MARGIN: f32 = 24.0;
/// 标题栏高度（拖动把手；关闭按钮位于其中）。
pub(crate) const CONSOLE_TITLE_H: f32 = 40.0;
/// 关闭按钮边长。
pub(crate) const CONSOLE_CLOSE_W: f32 = 32.0;
/// 待办列表距面板左右的内边距。
pub(crate) const CONSOLE_PAD: f32 = 12.0;
/// 折叠胶囊高度（始终可见的控制中心入口）。
pub(crate) const CONSOLE_PILL_H: f32 = 34.0;
/// 组件页：添加按钮高度。
pub(crate) const CONSOLE_ADD_BTN_H: f32 = 34.0;
/// 栅栏管理页：每行高度。
pub(crate) const CONSOLE_FENCE_ROW_H: f32 = 36.0;
/// 栅栏管理页：最多同时显示的行数（超出滚动）。
/// 栅栏管理页：选中栅栏详情区高度。
pub(crate) const CONSOLE_FENCE_DETAIL_H: f32 = 660.0;
/// 背景色调预设（标签, RGB 0..1）：菜单项顺序即此处顺序（+1 起）。
pub(crate) const TINT_PRESETS: &[(&str, [f32; 3])] = &[
    ("蓝", [0.32, 0.55, 0.95]),
    ("青", [0.30, 0.80, 0.85]),
    ("绿", [0.40, 0.75, 0.45]),
    ("黄", [0.95, 0.85, 0.40]),
    ("橙", [0.98, 0.62, 0.30]),
    ("红", [0.92, 0.35, 0.35]),
    ("紫", [0.66, 0.45, 0.90]),
    ("白", [0.92, 0.93, 0.96]),
    ("灰", [0.56, 0.58, 0.62]),
];

/// RAII：隐藏的真实图标在退出（含错误路径）时无条件恢复。
/// 反冲突约束的兜底——任何退出路径都不能让桌面图标永久消失。
///
/// 同时启动独立恢复守卫进程，覆盖 TerminateProcess / 强杀这类 Drop 不会执行的路径：
/// 主进程崩溃后守卫会重新探测桌面层级并显示真实图标（见 guard.rs）。
struct IconGuard {
    hierarchy: DesktopHierarchy,
    marker: guard::RecoveryMarker,
    guard: Option<guard::GuardHandle>,
}

impl IconGuard {
    fn new(hierarchy: DesktopHierarchy, data_dir: &std::path::Path) -> Self {
        let marker = guard::RecoveryMarker::new(data_dir);
        let guard = if hierarchy.list_view.is_some() {
            guard::start_guard(data_dir)
        } else {
            None
        };
        if hierarchy.list_view.is_some()
            && let Err(e) = marker.begin()
        {
            tracing::warn!("写入崩溃恢复标记失败（下次启动兜底恢复将不可用）: {e}");
        }
        let hidden = hierarchy.hide_icons();
        if !hidden {
            // 没有真实图标视图可隐藏：不需要恢复守卫。
            marker.clear();
        }
        Self {
            hierarchy,
            marker,
            guard,
        }
    }
}

impl Drop for IconGuard {
    fn drop(&mut self) {
        self.hierarchy.restore_icons();
        self.marker.clear();
        if let Some(g) = self.guard.take() {
            g.shutdown();
        }
    }
}

/// Ctrl+C 通知主循环退出的 overlay 窗口句柄（仅信号，不做窗口访问）。
static OVERLAY_HWND: OnceLock<usize> = OnceLock::new();

/// App 运行时：领域模型 + 渲染 + 持久化的组合根。
///
/// 由 `OverlayEvent` 回调持有（`Rc<RefCell>`），事件在主线程 wnd_proc 中同步处理，
/// 无跨线程竞争。
pub(crate) struct Runtime {
    pub(crate) desk: Desk,
    pub(crate) items: Vec<DesktopItem>,
    /// item id → items 下标（双击打开时反查）。
    pub(crate) item_index: HashMap<String, usize>,
    /// item id → 已上传的位图 id。
    pub(crate) bitmap_ids: HashMap<String, u64>,
    pub(crate) compositor: Compositor,
    pub(crate) theme: Theme,
    pub(crate) vw: f32,
    pub(crate) vh: f32,
    /// overlay 窗口在屏幕上的位置 = 虚拟屏原点（客户端 (0,0) = 虚拟 (0,0)）。
    /// 多屏时副屏在左/上可为负；主显示器工作区（`SPI_GETWORKAREA` 为屏幕坐标）
    /// 转虚拟坐标需减去它。显示拓扑变化时随 `DisplayChange` 更新。
    pub(crate) origin: (f32, f32),
    pub(crate) store: Box<dyn DeskStore>,
    /// 当前悬停的图标（栅栏下标, 图标下标）；None = 无悬停。
    pub(crate) hover: Option<(usize, usize)>,
    /// 最近一次光标位置（虚拟屏幕物理坐标）；侧边栏 Dock 放大据此连续计算。
    pub(crate) cursor: Option<(f32, f32)>,
    /// 当前选中的图标集合（框选 / Ctrl 单击多选，如资源管理器）。空 = 无选中。
    pub(crate) selected: Vec<(usize, usize)>,
    /// 框选橡皮筋矩形（所属栅栏下标 + 物理像素矩形）；None = 未在框选。
    pub(crate) select_band: Option<(usize, RectF)>,
    /// overlay 窗口句柄（右键菜单 owner / 就地编辑框父窗口）。
    pub(crate) hwnd: HWND,
    /// 当前激活的 D2D 内联文本编辑（None = 无编辑；键盘/IME 事件直达）。
    pub(crate) edit: Option<InlineEdit>,
    /// 上次 WM_CHAR 的高位代理（emoji 等增补平面字符由两个 WM_CHAR 组成）。
    pub(crate) edit_high: Option<u16>,
    /// 控制台面板动画状态（`AnimTick` 推进；idle 时停止定时器保持 0% CPU）。
    pub(crate) console_anim: ConsoleAnim,
    /// 当前连续动画批次开始时间；超过安全上限强制停止，防止状态机卡死持续重绘。
    pub(crate) anim_started: Option<Instant>,
    /// 桌面切换时栅栏整体淡出/淡入补间（None = 无动画，按 `desk.desktop_mode` 取最终值）。
    pub(crate) desktop_fade: Option<PanelTween>,
    /// 栅栏管理页当前选中的栅栏下标。
    pub(crate) selected_fence: usize,
    /// 控制中心当前标签页。
    pub(crate) console_page: ConsolePage,
    /// 当前悬停的控制台控件（绘制高亮反馈用）。
    pub(crate) console_hover: Option<ConsoleZone>,
    /// 栅栏管理页列表滚动偏移（物理像素）。
    pub(crate) fence_scroll: f32,
    /// 右侧栅栏设置页滚动偏移/最大值（物理像素）。
    pub(crate) detail_scroll: f32,
    pub(crate) detail_scroll_max: f32,
    /// 各栅栏最近一次实际渲染高度（物理像素，`build_scene` 每帧回写）。
    /// 自动高度栅栏 `bounds.h == 0`，碰撞检测/夹屏需要真实高度入算；下标与 `fences` 对齐。
    pub(crate) last_layout_h: Vec<f32>,
    /// 栅栏拖动/缩放补间（多个栅栏可同时动；结束自动移除）。
    pub(crate) fence_tweens: Vec<FenceTween>,
    /// 图标悬停放大补间（一次只有一个悬停图标）。
    pub(crate) icon_hover: Option<IconHoverAnim>,
    /// 桌面层级（保留句柄副本；「切换桌面」时反复隐藏/恢复真实图标）。
    pub(crate) hierarchy: DesktopHierarchy,
    /// overlay 窗口原始指针（动画定时器的启停需要访问它；与进程存活期一致）。
    pub(crate) overlay_ptr: *mut OverlayWindow,
    /// 本次事件中新添加图标的位图（`handle_event` 末尾随场景一起上传）。
    pub(crate) pending_uploads: Vec<(u64, IconData)>,
    /// 最近一次用户交互时间（空闲时修剪工作集用；后台 `SyncLibrary` 不计）。
    pub(crate) last_activity: std::time::Instant,
    /// 最近一次工作集修剪时间（限频：空闲时最多每 60s 一次）。
    pub(crate) last_trim: std::time::Instant,
    /// 侧边栏图标拖动排序状态：Some((fence_idx, icon_idx, cursor_x, cursor_y))。
    pub(crate) sidebar_reorder: Option<(usize, usize, f32, f32)>,
    /// 拖动排序时的图标顺序（fence_idx → Vec<usize>），由 build_scene 每帧计算。
    pub(crate) reorder_slot_order: std::collections::HashMap<usize, Vec<usize>>,
    /// 当前帧各图标渲染位置（fence_idx → Vec<(x, y)>），每帧更新，用于 lerp 动画。
    pub(crate) current_icon_positions: std::collections::HashMap<usize, Vec<(f32, f32)>>,
    /// OLE 拖放目标（单 overlay 注册一次；Drop 时自动撤销）。
    pub(crate) drop_target: Option<sylva_shell::drag::DropHandle>,
    /// 当前拖放悬停目标与效果（绘制高亮；DragLeave/Drop 后清除）。
    pub(crate) drop_hover: Option<(usize, DropEffect)>,
    /// 正在 OLE 拖出的图标（fence_idx, icon_idx）；原位降透明度显示。
    pub(crate) dragging_icon: Option<(usize, usize)>,
    /// Quick hide：只隐藏本应用视觉，不持久化，重启默认显示。
    pub(crate) quick_hide: bool,
    /// 拖动/缩放中的对齐参考线（虚拟屏幕坐标；空闲时为空）。
    pub(crate) guides: Vec<GuideLine>,
    /// 拖动/缩放中的尺寸徽标（空闲时 None）。
    pub(crate) hud: Option<SceneHud>,
    /// 栅栏内拖拽排序预览：(栅栏, 原下标, 插入槽, 光标 x, 光标 y)。
    pub(crate) grid_reorder: Option<(usize, usize, usize, f32, f32)>,
    /// 展开溢出格的栅栏下标（点击九宫格缩略图后显示全部图标）。
    pub(crate) grid_peek: Option<usize>,
    /// 控制中心左侧栅栏列表的可视矩形与滚动范围（上一帧场景回写，滚轮判定用）。
    pub(crate) console_list_view: Option<RectF>,
    pub(crate) fence_scroll_max: f32,
}

// 事件处理器再入守卫：`handle_event` 打开模态菜单/属性页（`TrackPopupMenu`、Shell 动词
// 的对话框）期间，嵌套消息循环会派发定时器、悬停、注入等其它事件再入回调。外层仍持有
// `Runtime` 可变借用，再入必然 RefCell 借用冲突崩溃；守卫置位后，再入回调直接丢弃事件。
// `ReentryGuard` 在回调结束时（含 panic）自动复位，避免标志位残留卡死后续事件。
thread_local! {
    static HANDLING: RefCell<bool> = const { RefCell::new(false) };
}

struct ReentryGuard;

impl Drop for ReentryGuard {
    fn drop(&mut self) {
        HANDLING.with(|h| *h.borrow_mut() = false);
    }
}

fn main() {
    // 恢复守卫模式：同二进制的隐藏子进程，不进入 GUI/单实例初始化。
    // 必须在创建单实例 mutex 之前分流，否则守卫会被误判为“重复启动”。
    if let Some(parent_pid) = guard::guard_mode() {
        guard::run(parent_pid);
        return;
    }

    // 单实例锁：防止重复启动导致多个全屏 overlay 叠层拦截输入（历史故障根因）。
    // 互斥名不带命名空间前缀 = 当前登录会话命名空间，无需管理员特权。
    // 句柄 `_mutex` 保持到 main 退出（进程生命周期），期间重复启动会立即在此退出。
    let name: Vec<u16> = "Sylva.Desktop.Fences"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let _mutex = match unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) } {
        Ok(h) => h,
        Err(e) => {
            eprintln!("单实例互斥创建失败: {e}");
            return;
        }
    };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        eprintln!("Sylva 已在运行，本次启动退出（单实例）");
        return;
    }

    // 全部使用物理像素，必须声明进程 DPI 感知。
    // 首选 Per-Monitor v2：窗口按真实物理像素渲染，DWM 不再对整窗位图缩放
    // （消除高缩放下「整窗发糊 + 拖拽/动画重采样与合成竞争 → 闪烁」），
    // 并支持 WM_DPICHANGED 实时重排。旧系统不支持时回退系统感知。
    unsafe {
        let pmv2 = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if pmv2.is_err() {
            // Win8.1/部分环境不支持 PerMonitorV2：降级为系统感知（GetDpiForSystem 仍可用）。
            let _ = SetProcessDPIAware();
        }
    }
    // COM：图标枚举/提取需要（APARTMENTTHREADED）
    let _com = sylva_shell::com::init();
    // OLE 剪贴板：Shell 右键菜单的复制/剪切/粘贴依赖 OleInitialize
    // （IContextMenu::InvokeCommand 内部调用 OleSetClipboard，未初始化 OLE 时静默失败）
    let _ole = unsafe { OleInitialize(None) };

    // 数据目录：exe 同级的 data 文件夹（与安装器约定一致——安装器在 <安装目录>/data 创建
    // 空目录，卸载时随安装目录一并删除；不再用 %APPDATA%，否则卸载不保留数据也清不掉）。
    let data_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("data");
    // 从旧位置（%APPDATA%\Sylva）迁移：旧版数据不在 exe 同级时搬过来，一次完成。
    if !data_dir.join("desk.json").exists()
        && let Ok(appdata) = std::env::var("APPDATA")
    {
        let old_dir = PathBuf::from(appdata).join("Sylva");
        if old_dir.join("desk.json").exists() {
            let _ = std::fs::create_dir_all(&data_dir);
            // 逐文件迁移（跨卷 rename 会失败，fallback 到 copy）
            for entry in std::fs::read_dir(&old_dir).into_iter().flatten().flatten() {
                let src = entry.path();
                let dst = data_dir.join(entry.file_name());
                if std::fs::rename(&src, &dst).is_err() {
                    let _ = std::fs::copy(&src, &dst);
                    let _ = std::fs::remove_file(&src);
                }
            }
            tracing::info!(
                "已从 {} 迁移数据到 {}",
                old_dir.display(),
                data_dir.display()
            );
        }
    }

    // 界面线程卡死看门狗：早于任何窗口/COM 代码启动，卡住时能落盘。
    watchdog::init(&data_dir);
    let _guard = match logging::init(&data_dir.join("logs")) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("日志初始化失败: {e}");
            return;
        }
    };

    if let Err(e) = run(&data_dir) {
        tracing::error!("启动失败: {e:?}");
    }
}

fn run(data_dir: &std::path::Path) -> sylva_core::Result<()> {
    // M0 桌面状态：加载/校验/回写，确保配置目录就绪。
    // 存储后端由 `store::open` 选择（当前 JSON，后续 redb），加载结果已归一化。
    let store = store::open(data_dir);
    // 主配置损坏时回退最近快照（需求 §3.4 / backlog P0-03）；快照与主配置分离存储，
    // 因此 desk.json 解析失败 / redb 主表损坏时仍可能读出快照。
    let mut desk = match store.load() {
        Ok(Some(desk)) => desk,
        Ok(None) => match snapshot::latest(store.as_ref()) {
            Ok(Some(snapshot)) => {
                tracing::warn!(reason = %snapshot.reason, "主配置为空，回退最近快照");
                store.save(&snapshot.desk)?;
                snapshot.desk
            }
            Ok(None) => Desk::new(AppSettings::default()),
            Err(e) => {
                tracing::warn!("读取快照失败: {e}");
                Desk::new(AppSettings::default())
            }
        },
        Err(e) => {
            tracing::error!("主配置加载失败，尝试最近快照: {e}");
            match snapshot::latest(store.as_ref()) {
                Ok(Some(snapshot)) => {
                    tracing::warn!(reason = %snapshot.reason, "已回退最近快照并写回主配置");
                    store.save(&snapshot.desk)?;
                    snapshot.desk
                }
                Ok(None) => return Err(e),
                Err(snapshot_err) => {
                    return Err(sylva_core::CoreError::Storage(format!(
                        "主配置加载失败（{e}），快照读取也失败（{snapshot_err}）"
                    )));
                }
            }
        }
    };
    // 开机自启对账：安装器写入的 Run 项在首次启动时采纳为设置，已勾选则对齐当前 exe。
    if autostart::reconcile(&mut desk.settings) {
        let _ = store.save(&desk);
    }
    tracing::info!(
        fences = desk.fences.len(),
        icons = desk.icons.len(),
        "桌面状态已加载"
    );

    // 1) 壳层接管：探测层级并隐藏真实图标（反冲突约束：不重挂/不销毁他人窗口）。
    //    守卫确保后续任何失败都会恢复图标。
    let hierarchy = sylva_shell::takeover::probe()
        .ok_or_else(|| sylva_core::CoreError::Shell("未找到桌面根窗口 Progman".into()))?;
    // 上次异常退出（崩溃/强杀）可能留下隐藏的 SysListView32：先恢复再启动本次会话。
    let recovery = guard::RecoveryMarker::new(data_dir);
    if recovery.recover_if_needed(&hierarchy) {
        tracing::warn!("检测到上次异常退出，已先恢复真实桌面图标");
    }
    let _guard = IconGuard::new(hierarchy, data_dir);

    // 2) GPU 上下文 + overlay + 合成器
    let device = RenderDevice::new()
        .map_err(|e| sylva_core::CoreError::Render(format!("RenderDevice: {e}")))?;
    let overlay = OverlayWindow::create(hierarchy.overlay_parent())
        .map_err(|e| sylva_core::CoreError::Render(format!("OverlayWindow: {e}")))?;
    let (vw, vh) = (overlay.width, overlay.height);
    tracing::info!(vw, vh, "overlay 覆盖虚拟屏幕");

    // OLE 拖放目标：整个 overlay 注册一次，按 App 同步的栅栏矩形分发。
    let drop_target = sylva_shell::drag::register(overlay.hwnd)
        .map_err(|e| sylva_core::CoreError::Shell(format!("注册拖放目标失败: {e}")))?;
    let drop_hwnd = overlay.hwnd;
    drop_target.set_callback(Box::new(move |event| {
        use sylva_shell::drag::DropEvent;
        match event {
            DropEvent::Enter { fence, .. } | DropEvent::Over { fence, .. } => {
                if let Some(fence) = fence {
                    post_overlay_event(
                        drop_hwnd,
                        OverlayEvent::DragHover {
                            fence: Some(fence),
                            effect: DropEffect::Link,
                        },
                    );
                    DropEffect::Link
                } else {
                    DropEffect::None
                }
            }
            DropEvent::Leave => {
                post_overlay_event(drop_hwnd, OverlayEvent::DragLeave);
                DropEffect::None
            }
            DropEvent::Drop { fence, paths, .. } => {
                let Some(fence) = fence else {
                    post_overlay_event(drop_hwnd, OverlayEvent::DragLeave);
                    return DropEffect::None;
                };
                if paths.is_empty() {
                    post_overlay_event(drop_hwnd, OverlayEvent::DragLeave);
                    return DropEffect::None;
                }
                post_overlay_event(
                    drop_hwnd,
                    OverlayEvent::FilesDropped {
                        fence,
                        paths,
                        effect: DropEffect::Link,
                    },
                );
                DropEffect::Link
            }
        }
    }));

    // 主题字号按 DPI 放大：渲染层把渲染目标固定为 96 DPI（1 DIP = 1 物理像素），
    // 布局/字号全部走物理像素，200% 缩放下图标与文字才会以像素级分辨率渲染（清晰）。
    // 图标位图也按物理尺寸提取（见 ICON_EXTRACT_SIZE），不再把小图放大导致发糊。
    // 注意：所有 DIP 度量必须**一起**缩放（字号、图标、间距、内边距、列宽、控制台），
    // 只放大文字不放大行距/间距正是「行列间重叠」的根因。
    let mut theme = Theme::default();
    // Per-Monitor v2 下用 overlay 所在显示器的 DPI（系统感知回退则同 GetDpiForSystem）。
    // overlay 已在上一步创建，hwnd 可用。
    let dpi = unsafe { GetDpiForWindow(overlay.hwnd) };
    let (primary_w, primary_h) = primary_screen_size();
    let dpi_scale = theme_scale_for(dpi, primary_w as u32, primary_h as u32);
    apply_theme_scale(&mut theme, dpi_scale);
    tracing::info!(dpi, scale = dpi_scale, "主题按 DPI + 屏幕尺寸缩放");
    // 侧边栏栅栏启动归一化：旧数据可能存了超屏/离屏 bounds（图标多时曾按内容全高
    // 计算导致 top 为负），重新停靠到屏幕内。折叠按钮已移除，历史折叠状态强制展开。
    let mut dock_reanchor = 0;
    for f in desk.fences.iter_mut() {
        if f.appearance.layout == FenceLayout::Sidebar {
            if f.sidebar_collapsed {
                f.sidebar_collapsed = false;
                dock_reanchor += 1;
            }
            let wa = work_area_rect(overlay.x as f32, overlay.y as f32, vw as f32, vh as f32);
            let b = sidebar_dock_rect(theme.scale, f, &wa);
            // 夹在工作区内（任务栏扣除后），下沿不越过任务栏
            let b = clamp_sidebar_work_rect(b, wa, f.appearance.sidebar_pos);
            if f.bounds != b {
                f.bounds = b;
                dock_reanchor += 1;
            }
        }
    }
    if dock_reanchor > 0 {
        tracing::info!(dock_reanchor, "侧边栏栅栏边界已重新停靠到屏幕内");
        let _ = store.save(&desk);
    }
    let compositor = Compositor::new(device, overlay.hwnd, theme.clone())
        .map_err(|e| sylva_core::CoreError::Render(format!("Compositor: {e}")))?;

    // 3) 枚举真实桌面图标（IShellFolder 枚举，不依赖 DefView）
    let mut items = sylva_shell::items::enumerate_desktop_items()
        .map_err(|e| sylva_core::CoreError::Shell(e.to_string()))?;
    tracing::info!(count = items.len(), "桌面图标枚举完成");

    // 4) 首次运行：无栅栏布局时按稳定顺序创建演示栅栏并持久化。
    //    之后布局由用户拖动/缩放决定，栅栏是显式成员列表（无自动分类）。
    seed_fences(&mut desk, &items, &theme);
    // 旧数据可能只有“桌面”链接栅栏：升级为唯一桌面收件箱。
    if ensure_desktop_inbox(&mut desk, shell_desktop_path().as_deref()) {
        let _ = store.save(&desk);
    }
    store.save(&desk)?;

    // 5) 图标元数据补齐 + 添加项恢复：
    //    - 枚举项：登记 path/详情（added=false），缺失的补进 desk.icons；
    //    - 上次拖入/粘贴的项（desk.icons 里带 path 但不在枚举中）：从路径重建
    //      DesktopItem 追加进 items 池，重启后仍能显示图标、双击打开。
    for it in &items {
        let ic = desk.icons.entry(it.id.clone()).or_insert_with(|| {
            let mut i = Icon::new(it.id.clone(), it.display_name.clone(), it.kind);
            i.path = it.path.clone();
            i
        });
        ic.path = it.path.clone();
        ic.mark_desktop();
        if let Some(p) = it.path.as_deref() {
            sylva_core::details::enrich(ic, p);
        }
    }
    let mut item_index: HashMap<String, usize> = items
        .iter()
        .enumerate()
        .map(|(i, it)| (it.id.clone(), i))
        .collect();
    let mut restored = 0usize;
    for (id, ic) in &desk.icons {
        if item_index.contains_key(id) {
            continue;
        }
        if let Some(path) = ic.path.as_deref()
            && let Ok(dt) = sylva_shell::items::item_from_path(path)
        {
            item_index.insert(dt.id.clone(), items.len());
            items.push(dt);
            restored += 1;
        }
    }
    if restored > 0 {
        tracing::info!(restored, "恢复上次添加的图标");
    }

    // 图标索引 + 位图映射 + 首次上传数据（正式版放后台加载线程）
    let mut bitmap_ids = HashMap::new();
    let mut uploads = Vec::new();
    for (i, item) in items.iter().enumerate() {
        match sylva_shell::icons::extract_icon(item, ICON_EXTRACT_SIZE) {
            Ok(data) => {
                bitmap_ids.insert(item.id.clone(), i as u64);
                uploads.push((i as u64, data));
            }
            Err(e) => tracing::warn!(name = %item.display_name, "图标提取失败: {e}"),
        }
    }
    tracing::info!(loaded = uploads.len(), "图标位图提取完成");

    tracing::info!(icons = items.len(), "开始提取图标位图（初始化阶段）");
    memory::report("图标提取前");

    // 5) 初始场景 + 呈现 + 命中模型
    // 读取控制台开关（供动画器初始面板进度）——先取值再构造 Runtime，避免字段求值移动冲突。
    let console_open = desk.console_open;
    let overlay_ptr = &overlay as *const OverlayWindow as *mut OverlayWindow;
    // 上次退出时若处于「原始桌面」模式：恢复真实图标（IconGuard 刚隐藏过），
    // 栅栏保持隐藏（fence_alpha 取 0，不播放动画）。
    if desk.desktop_mode {
        hierarchy.restore_icons();
    }
    // 真实高度缓冲：与栅栏一一对应，`build_scene` 每帧回写（移动 desk 前取长度）。
    let last_layout_h = vec![0.0; desk.fences.len()];
    let mut rt = Runtime {
        desk,
        items,
        item_index,
        bitmap_ids,
        compositor,
        theme: theme.clone(),
        vw: vw as f32,
        vh: vh as f32,
        origin: (overlay.x as f32, overlay.y as f32),
        store,
        hover: None,
        cursor: None,
        selected: Vec::new(),
        select_band: None,
        hwnd: overlay.hwnd,
        edit: None,
        edit_high: None,
        console_anim: ConsoleAnim::new(console_open),
        anim_started: None,
        desktop_fade: None,
        selected_fence: 0,
        console_page: ConsolePage::System,
        console_hover: None,
        fence_scroll: 0.0,
        detail_scroll: 0.0,
        detail_scroll_max: 0.0,
        last_layout_h,
        fence_tweens: Vec::new(),
        icon_hover: None,
        hierarchy,
        overlay_ptr,
        pending_uploads: Vec::new(),
        last_activity: std::time::Instant::now(),
        last_trim: std::time::Instant::now(),
        sidebar_reorder: None,
        reorder_slot_order: std::collections::HashMap::new(),
        current_icon_positions: std::collections::HashMap::new(),
        drop_target: Some(drop_target),
        drop_hover: None,
        dragging_icon: None,
        quick_hide: false,
        guides: Vec::new(),
        hud: None,
        grid_reorder: None,
        grid_peek: None,
        console_list_view: None,
        fence_scroll_max: 0.0,
    };
    // 原始桌面模式/Quick hide 时不要劫持 Explorer 的桌面右键；栅栏模式才启用。
    unsafe {
        (*rt.overlay_ptr).set_desktop_menu_enabled(true);
    }
    // 启动先归一化布局：旧版本/异常退出可能留下越界 bounds（栅栏左侧被屏幕裁掉）。
    // 再做一次同步；若同步改变了图标数，侧边栏 bounds 需要按新内容重算。
    let mut layout_changed = reanchor_fences(&mut rt);
    if reconcile_fences(&mut rt) {
        layout_changed = true;
        // 链接文件夹同步后图标数变了，侧边栏 bounds 需要重新计算（reanchor 时是空的）
        reanchor_fences(&mut rt);
    }
    if layout_changed {
        let _ = rt.store.save(&rt.desk);
    }
    // 高度策略：`bounds.h == 0` 表示未手动缩放，按内容自适应（增删应用自动长高）。
    // 不在此冻结高度——用户拖边缘/角缩放后才落为具体值。
    let mut scene = build_scene(&mut rt, Instant::now());
    // 启动重叠消解：真实高度已由上面首帧 layout 写入 `last_layout_h`（自动高度栅栏
    // 的 0 高此时可换算成真实可见高度）。历史布局若存在重叠（朋友机「两个栅栏突然
    // 重叠」即由此而来）→ 推离并持久化，随后重排一帧供首帧呈现使用。
    if resolve_overlaps(&mut rt) {
        tracing::info!("启动重叠消解：存在重叠栅栏，已推离并保存");
        let _ = rt.store.save(&rt.desk);
        scene = build_scene(&mut rt, Instant::now());
    }
    // 首帧上传：图标位图（新增项的位图在 build_scene 内推入 pending_uploads）
    let mut upload_refs: Vec<(u64, &sylva_shell::icons::IconData)> =
        uploads.iter().map(|(id, d)| (*id, d)).collect();
    upload_refs.extend(rt.pending_uploads.iter().map(|(id, d)| (*id, d)));
    rt.compositor
        .present(&scene, &upload_refs)
        .map_err(|e| sylva_core::CoreError::Render(e.to_string()))?;
    let model = hit_model_from(&rt.theme, &scene, &rt.desk);
    memory::report("首帧呈现后");
    // Shell 右键菜单预热：**默认关闭**（`SYLVA_PRIME_SHELL=1` 可恢复）。
    //
    // 预热会把 Shell 上下文菜单扩展（网盘/压缩/云同步等）在本进程里加载并初始化。
    // 这些扩展不是线程安全的：在后台线程创建 `IContextMenu` 后，其 STA 对象可能与
    // 主线程的 Shell 调用互相等待，表现为整窗「未响应」（Event Log: AppHangB1,
    // 退出码 0xCFFFFFFF），UI 线程卡在 Shell/COM 调用里。
    //
    // 现在改为按需预热：右键时 `shell_menu::show` 会先 `ensure_primed`，等待期间
    // 泵送消息（窗口保持响应），代价只是**每个文件类型的第一次右键稍慢**。
    if std::env::var_os("SYLVA_PRIME_SHELL").is_some() {
        let paths: Vec<String> = rt
            .desk
            .icons
            .values()
            .filter_map(|ic| ic.path.clone())
            .collect();
        shell_menu::prime_startup(&paths);
        shell_menu::prime_desktop_startup();
        tracing::info!(
            types = paths.len(),
            "Shell 菜单预热已启用（SYLVA_PRIME_SHELL）"
        );
    }
    // 启动期一次性分配已就绪：把不再活跃的内存页换出工作集（D3D/场景构建等），
    // 降低常驻内存。GPU 侧资源由驱动管理不受影响，用到时自动换回。
    memory::trim();
    memory::report("工作集修剪后");

    // 6) 事件回路：App 处理交互 → 重绘 → 返回新命中模型（overlay 据此更新区域）
    sync_drop_target(&rt, &model);
    let runtime = Rc::new(RefCell::new(rt));
    let runtime2 = runtime.clone();
    overlay.set_event_handler(Box::new(move |ev| {
        // 模态菜单 / 属性页 / Shell 动词执行期间，嵌套消息循环会派发定时器、悬停、
        // 注入等其它事件再入本回调。此时外层 `handle_event` 仍持有 `Runtime` 的可变
        // 借用，再入必然 RefCell 借用冲突崩溃——一律丢弃再入事件，保持当前命中模型。
        if HANDLING.with(|h| h.replace(true)) {
            return None;
        }
        let _reentry = ReentryGuard;
        // edition 2024 会先析构尾表达式中的 RefMut 临时值、再析构局部变量。
        // 这里显式绑定借用，固定析构顺序（先释放借用，再复位再入标志）。
        let mut rt = runtime2.borrow_mut();
        handle_event(&mut rt, ev)
    }));
    overlay.set_model(model);

    // 7) Ctrl+C：通知消息循环干净退出（图标由 _guard 在返回时恢复）
    let _ = OVERLAY_HWND.set(overlay.hwnd.0 as usize);
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(ctrl_handler), true);
    }

    // 测试钩子：设置 SYLVA_AUTOSTOP_MS 后到点自动干净退出（CI/自动验证用）。
    if let Some(ms) = std::env::var("SYLVA_AUTOSTOP_MS")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        let hwnd = overlay.hwnd.0 as usize; // HWND 非 Send，转 usize 跨线程
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(ms));
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(hwnd as *mut core::ffi::c_void)),
                    WM_APP_QUIT,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        });
        tracing::info!(ms, "SYLVA_AUTOSTOP_MS 已设置，到点自动退出");
    }

    tracing::info!(
        "Sylva 已就绪：拖边缘/角缩放、标题栏拖动、双击打开、文件拖入/粘贴添加；Ctrl+Alt+T 控制中心，Ctrl+Shift+F10 退出"
    );
    run_message_loop();

    memory::report("退出前");
    tracing::info!("已退出");
    Ok(())
}

/// Ctrl+C / 关闭终端 / 注销 / 关机：恢复桌面图标并通知主循环退出。
unsafe extern "system" fn ctrl_handler(ctrl_type: u32) -> BOOL {
    unsafe {
        match ctrl_type {
            CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
                // 信号线程不能访问 Runtime，但可以直接探测当前桌面并显示真实图标。
                let restored = sylva_shell::takeover::restore_desktop_icons();
                tracing::warn!(restored, "注销/关机信号：已恢复真实桌面图标");
            }
            _ => {
                if let Some(hwnd) = OVERLAY_HWND.get() {
                    let _ = PostMessageW(
                        Some(HWND(*hwnd as *mut core::ffi::c_void)),
                        WM_APP_QUIT,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        }
        BOOL(1) // 已处理，阻止默认终止行为（让主循环干净退出）
    }
}

/// 栅栏用于碰撞/夹屏的真实矩形：自动高度（`bounds.h <= 0`）时用最近一次布局
/// 渲染高度。视觉矩形 = 模型矩形（拖拽已无补间），碰撞检测与实际可见区域一致。
fn fence_collision_rect(rt: &Runtime, i: usize) -> Rect {
    let f = &rt.desk.fences[i];
    Rect::new(f.bounds.x, f.bounds.y, f.bounds.w, fence_height(rt, i))
}

/// 除 `skip` 外其它栅栏的碰撞矩形（真实高度入算——自动高度栅栏不再以 0 高漏检，
/// 两个栅栏叠加到同一个自动高度栅栏上就是旧版「栅栏重叠」的根因之一）。
fn other_bounds(rt: &Runtime, skip: usize) -> Vec<Rect> {
    (0..rt.desk.fences.len())
        // 隐藏的栅栏不占位：不可见的东西不该挡住拖动/新建/对齐。
        .filter(|&i| i != skip && !rt.desk.fences[i].appearance.hidden)
        .map(|i| fence_collision_rect(rt, i))
        .collect()
}

/// 启动重叠消解：加载的布局可能因历史 bug / DPI 变化存在重叠（尤其自动高度栅栏，
/// 旧版碰撞检测把 0 高当真高）。按序把每个栅栏与其它栅栏推离（真实高度入算），
/// 保证互不重叠且都在屏幕内。返回是否有变化（有变化才持久化 + 重排一帧）。
/// 依赖 `last_layout_h` 已由首帧 `build_scene` 填充，故须在首帧布局之后调用。
fn resolve_overlaps(rt: &mut Runtime) -> bool {
    if rt.desk.settings.allow_fence_overlap {
        return false;
    }
    let mut changed = false;
    let screen = screen_rect(rt);
    for i in 0..rt.desk.fences.len() {
        // 隐藏的栅栏不参与启动消解：它不占位，也不该被可见栅栏推着跑。
        if rt.desk.fences[i].appearance.hidden {
            continue;
        }
        let others = other_bounds(rt, i);
        let cur = fence_collision_rect(rt, i);
        let out = settle_move(&cur, &others, &screen, fence_gap(rt));
        // 只修位置（不改变用户已设的尺寸），并圆整到整数物理像素。
        if (out.x - cur.x).abs() > 0.5 || (out.y - cur.y).abs() > 0.5 {
            rt.desk.fences[i].bounds.x = out.x.round();
            rt.desk.fences[i].bounds.y = out.y.round();
            changed = true;
        }
    }
    changed
}

/// 虚拟屏幕边界（物理像素；栅栏活动范围）。
fn screen_rect(rt: &Runtime) -> Rect {
    Rect::new(0.0, 0.0, rt.vw, rt.vh)
}

/// 主显示器工作区（任务栏扣除后；物理像素）。
/// `SPI_GETWORKAREA` 返回的是**屏幕坐标**（主显示器左上 = (0,0)），而栅栏布局用
/// 虚拟/客户端坐标（客户端 (0,0) = 虚拟屏 (0,0) = 屏幕 (ox, oy)）——转虚拟坐标
/// 需减去 `(ox, oy)`。单屏时 (0,0) 减了无感；多屏主屏非虚拟原点时少了这一步
/// 侧边栏夹屏会整体错位。失败回退整个虚拟屏幕。
/// 任务栏占用的屏幕边（仅自动隐藏时需要额外预留）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScreenEdge {
    Top,
    Right,
    Bottom,
    Left,
}

/// 任务栏为「自动隐藏」时，它的厚度仍会占用屏幕边缘：`rcWork` 此时等于整屏，
/// 栅栏贴边就会挡住任务栏弹出（鼠标移到边缘唤不出任务栏）。
///
/// 通过 `SHAppBarMessage` 读取任务栏矩形与状态；只有同时满足
/// 「自动隐藏」且「任务栏与当前显示器相交」才返回预留边与厚度。
/// 可见任务栏已由 `rcWork` 扣除，这里返回 None，避免重复扣减。
fn taskbar_reservation(origin: (f32, f32), work: &Rect) -> Option<(ScreenEdge, f32)> {
    let mut pos = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        ..Default::default()
    };
    if unsafe { SHAppBarMessage(ABM_GETTASKBARPOS, &mut pos) } == 0 {
        return None;
    }
    let mut st = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        ..Default::default()
    };
    let state = unsafe { SHAppBarMessage(ABM_GETSTATE, &mut st) };
    if state & (ABS_AUTOHIDE as usize) == 0 {
        return None;
    }
    let tb = Rect::new(
        pos.rc.left as f32 - origin.0,
        pos.rc.top as f32 - origin.1,
        (pos.rc.right - pos.rc.left) as f32,
        (pos.rc.bottom - pos.rc.top) as f32,
    );
    // 任务栏不在当前显示器：不预留
    if tb.right() <= work.x
        || tb.x >= work.right()
        || tb.bottom() <= work.y
        || tb.y >= work.bottom()
    {
        return None;
    }
    let (edge, thickness) = if pos.uEdge == ABE_BOTTOM {
        (ScreenEdge::Bottom, tb.h)
    } else if pos.uEdge == ABE_TOP {
        (ScreenEdge::Top, tb.h)
    } else if pos.uEdge == ABE_LEFT {
        (ScreenEdge::Left, tb.w)
    } else if pos.uEdge == ABE_RIGHT {
        (ScreenEdge::Right, tb.w)
    } else {
        return None;
    };
    (thickness > 1.0).then_some((edge, thickness))
}

/// 光标所在（或栅栏中心所在）显示器的**工作区**（任务栏扣除后），不含安全区。
///
/// 多显示器下按栅栏矩形做 MonitorFromRect，不会像主屏 SPI_GETWORKAREA 那样
/// 把副屏上的栅栏夹回主屏。失败时退回主屏工作区。
fn monitor_work_area(rt: &Runtime, rect: &Rect) -> Rect {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
    };
    let rc = RECT {
        left: (rect.x + rt.origin.0).round() as i32,
        top: (rect.y + rt.origin.1).round() as i32,
        right: (rect.right() + rt.origin.0).round() as i32,
        bottom: (rect.bottom() + rt.origin.1).round() as i32,
    };
    unsafe {
        let mon = MonitorFromRect(&rc, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(mon, &mut mi).as_bool() {
            let w = mi.rcWork;
            let mut out = Rect::new(
                w.left as f32 - rt.origin.0,
                w.top as f32 - rt.origin.1,
                (w.right - w.left) as f32,
                (w.bottom - w.top) as f32,
            );
            // 自动隐藏的任务栏不占 rcWork：在它所在的那条边上补预留，
            // 避免栅栏贴边后挡住任务栏弹出。可见任务栏已由 rcWork 扣除。
            if let Some((edge, thickness)) = taskbar_reservation(rt.origin, &out) {
                match edge {
                    ScreenEdge::Top => {
                        out.y += thickness;
                        out.h -= thickness;
                    }
                    ScreenEdge::Bottom => out.h -= thickness,
                    ScreenEdge::Left => {
                        out.x += thickness;
                        out.w -= thickness;
                    }
                    ScreenEdge::Right => out.w -= thickness,
                }
                out.w = out.w.max(1.0);
                out.h = out.h.max(1.0);
            }
            return out;
        }
    }
    work_area_rect(rt.origin.0, rt.origin.1, rt.vw, rt.vh)
}

/// 栅栏可放置区域 = 所在显示器工作区向内收 DESKTOP_INSET（四边留白）。
fn fence_placement(rt: &Runtime, rect: &Rect) -> Rect {
    monitor_work_area(rt, rect).inset(desktop_inset(rt))
}

fn work_area_rect(ox: f32, oy: f32, vw: f32, vh: f32) -> Rect {
    let mut rc = RECT::default();
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut rc as *mut RECT as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    if ok.is_err() || rc.right <= rc.left || rc.bottom <= rc.top {
        return Rect::new(0.0, 0.0, vw, vh);
    }
    Rect::new(
        rc.left as f32 - ox,
        rc.top as f32 - oy,
        (rc.right - rc.left) as f32,
        (rc.bottom - rc.top) as f32,
    )
}

/// 该事件是否代表「用户真正点/拖了某处」（内联编辑打开时据此提交/失焦）。
/// 悬停、滚轮、定时器、注入重绘等非交互事件不在此列——它们不该关掉刚打开的编辑框。
fn is_popup_dismiss_event(ev: &OverlayEvent) -> bool {
    matches!(
        ev,
        OverlayEvent::FenceMove { .. }
            | OverlayEvent::FenceResize { .. }
            | OverlayEvent::IconClicked { .. }
            | OverlayEvent::IconDoubleClicked { .. }
            | OverlayEvent::IconDragOut { .. }
            | OverlayEvent::IconClassifyDrop { .. }
            | OverlayEvent::SelectDrag { .. }
            | OverlayEvent::ContextMenu { .. }
            | OverlayEvent::FilesDropped { .. }
            | OverlayEvent::FenceScroll { .. }
            // 控制台交互（点按钮/滚待办/拖动面板/热键开关）都是真实交互，编辑期间应提交。
            | OverlayEvent::ConsoleClick { .. }
            | OverlayEvent::ConsoleScroll { .. }
            | OverlayEvent::ConsoleMove { .. }
            | OverlayEvent::ConsoleResize { .. }
            | OverlayEvent::ConsoleResizeEnd
            | OverlayEvent::ConsoleToggle
    )
}

/// 把一个 `OverlayEvent` 投递到 overlay 主线程（OLE 回调不在 App 的借用期内）。
fn post_overlay_event(hwnd: HWND, event: OverlayEvent) {
    let boxed = Box::new(event);
    unsafe {
        let _ = PostMessageW(
            Some(hwnd),
            WM_SYLVA_INJECT,
            WPARAM(0),
            LPARAM(Box::into_raw(boxed) as isize),
        );
    }
}

/// 立即按当前模型重建并呈现一帧（拖出前显示“拖拽中”状态）。
fn present_now(rt: &mut Runtime) {
    let scene = build_scene(rt, Instant::now());
    let uploads = std::mem::take(&mut rt.pending_uploads);
    let refs: Vec<(u64, &IconData)> = uploads.iter().map(|(id, data)| (*id, data)).collect();
    if let Err(e) = rt.compositor.present(&scene, &refs) {
        tracing::warn!("拖出前重绘失败: {e}");
    }
}

/// 处理一个用户交互事件：更新布局 → （按需）重绘 → 生成新命中模型。
///
/// 返回 `None` 表示本事件不改变任何可见状态，无需重绘（overlay 保持当前
/// 命中模型与窗口区域）。这是空闲 CPU/GPU 与桌面闪烁的关键门控：
/// 无侧边栏时鼠标扫过（`CursorMove`）与 4s 心跳（`SyncLibrary` 无变化）
/// 都不再触发全量重绘。
fn handle_event(rt: &mut Runtime, ev: OverlayEvent) -> Option<HitModel> {
    watchdog::event(watchdog::code(&ev));
    watchdog::phase(watchdog::ENTER);
    // 空闲修剪门控：除周期性的 `SyncLibrary` 外都算用户活动，刷新时间戳。
    // `SyncLibrary` 每 4s 触发一次，若算活动则永远「不空闲」，修剪永不执行。
    if !matches!(ev, OverlayEvent::SyncLibrary) {
        rt.last_activity = std::time::Instant::now();
    }
    // 就地重命名编辑期间，真正的用户交互事件先提交编辑（资源管理器行为：点击别处即确认）。
    // 只对「用户确实在别处点/拖/滚」的事件提交——定时器（SyncLibrary）与悬停高亮
    // （HoverEnter/Leave）不算，否则编辑框会被后台同步或鼠标扫过自动关掉，用户根本没
    // 来得及输入（曾导致重命名看起来完全坏掉）。`EditCommitted` 是注入的「仅重绘」
    // 事件，此时编辑会话已结束，同样不在此处理。判断复用下方透明度滑块的同一语义。
    if rt.edit.is_some() && is_popup_dismiss_event(&ev) {
        dismiss_edit(rt);
    }
    // 重绘门控：默认 true（一切改变可见状态的事件照常重绘），
    // 仅对「可能无事可画」的事件显式置 false。
    let mut redraw = true;
    match ev {
        OverlayEvent::FenceMove { fence, pos } => {
            // 拖动标题栏：不交叉（无重叠推挤）+ 磁吸吸附 + 限制在虚拟屏幕内。
            // 直接落到目标（无补间）：视觉 = 模型 = 鼠标位置，拖拽即时跟手；
            // 且鼠标坐标是整数，拖拽中不存在分数坐标导致的边框发虚。
            let (x, y) = pos;
            let cur = rt.desk.fences.get(fence).map(|f| (f.bounds.w, f.bounds.h));
            if let Some((w, h)) = cur {
                // 全局允许叠加时不再推挤其它栅栏，只做屏幕边界夹取。
                let others = if rt.desk.settings.allow_fence_overlap {
                    Vec::new()
                } else {
                    other_bounds(rt, fence)
                };
                let cand = Rect::new(x, y, w, h);
                // 侧边栏贴边停靠（dock 语义），普通栅栏留出桌面安全区。
                let wa = monitor_work_area(rt, &cand);
                let is_sidebar = rt
                    .desk
                    .fences
                    .get(fence)
                    .is_some_and(|f| f.appearance.layout == FenceLayout::Sidebar);
                let place = if is_sidebar {
                    wa
                } else {
                    fence_placement(rt, &cand)
                };
                // 对齐吸附：拖到邻居边/中线附近时轻轻贴上（Alt 临时关闭）。
                let threshold = if is_alt_down() {
                    0.0
                } else {
                    SNAP_DIST * rt.theme.scale
                };
                let snapped = snap_move(&cand, &others, &place, threshold);
                let mut out = settle_move(&snapped, &others, &place, fence_gap(rt));
                if is_sidebar && let Some(f) = rt.desk.fences.get(fence) {
                    out = clamp_sidebar_work_rect(out, wa, f.appearance.sidebar_pos);
                }
                set_drag_feedback(rt, &out, &others, &place);
                if let Some(f) = rt.desk.fences.get_mut(fence) {
                    f.bounds.x = out.x;
                    f.bounds.y = out.y;
                }
            }
        }
        OverlayEvent::FenceResize { fence, zone, rect } => {
            // 拖边缘/角标：只动可动边，被其它栅栏挡住时停在交界边（不侵入），
            // 边接近时吸附对齐；锚定边不动，最小尺寸/屏幕边界约束在算法内完成。
            // 直接落到目标（无补间），视觉 = 模型 = 鼠标，即时跟手。
            let (nx, ny, nw, nh) = rect;
            let others = if rt.desk.settings.allow_fence_overlap {
                Vec::new()
            } else {
                other_bounds(rt, fence)
            };
            let free = match zone {
                ResizeZone::Right => FreeSides::Right,
                ResizeZone::Bottom => FreeSides::Bottom,
                ResizeZone::BottomRight => FreeSides::BottomRight,
                ResizeZone::Left => FreeSides::Left,
                ResizeZone::BottomLeft => FreeSides::BottomLeft,
                ResizeZone::TopRight => FreeSides::TopRight,
            };
            let cand = Rect::new(nx, ny, nw, nh);
            let wa = monitor_work_area(rt, &cand);
            let place = fence_placement(rt, &cand);
            // 缩放吸附：只吸可动边（锚定边纹丝不动）。
            let threshold = if is_alt_down() {
                0.0
            } else {
                SNAP_DIST * rt.theme.scale
            };
            let snapped = snap_resize(&cand, &others, &place, free, threshold);
            let mut out = settle_resize(
                &snapped,
                &others,
                &place,
                free,
                MIN_FENCE_W,
                MIN_FENCE_H,
                fence_gap(rt),
            );
            // 侧边栏 dock：厚度轴锁定为「紧贴放大图标」的停靠值——纵向锁 x/w、
            // 横向锁 y/h，只保留用户拖动的那一轴，宽度/高度弹回锁定值，放大图标不再被裁。
            if let Some(f) = rt.desk.fences.get(fence)
                && f.appearance.layout == FenceLayout::Sidebar
            {
                let d = sidebar_dock_rect(rt.theme.scale, f, &wa);
                out = if f.appearance.sidebar_pos == SidebarPosition::Top {
                    Rect::new(out.x, d.y, out.w, d.h)
                } else {
                    Rect::new(d.x, out.y, d.w, out.h)
                };
                // 夹在工作区内（任务栏扣除后）：纵向 dock 下沿不越过任务栏
                out = clamp_sidebar_work_rect(out, wa, f.appearance.sidebar_pos);
            }
            set_drag_feedback(rt, &out, &others, &place);
            if let Some(f) = rt.desk.fences.get_mut(fence) {
                f.bounds = out;
            }
        }
        OverlayEvent::IconDoubleClicked { fence, icon } => {
            // 双击栅栏内图标：若属于多选集合则全部打开，否则打开双击项（资源管理器行为）
            if rt.selected.len() > 1 && rt.selected.contains(&(fence, icon)) {
                let targets = rt.selected.clone();
                for (f, i) in targets {
                    launch_fence_icon(rt, f, i);
                }
            } else {
                launch_fence_icon(rt, fence, icon);
            }
        }
        OverlayEvent::IconDragOut { fence, icon } => {
            let key = (fence, icon);
            let ids: Vec<String> = if rt.selected.len() > 1 && rt.selected.contains(&key) {
                selected_ids(rt)
            } else {
                rt.desk
                    .fences
                    .get(fence)
                    .and_then(|f| f.icon_ids.get(icon))
                    .cloned()
                    .into_iter()
                    .collect()
            };
            let paths: Vec<String> = ids
                .iter()
                .filter_map(|id| rt.desk.icons.get(id).and_then(|ic| ic.path.clone()))
                .collect();
            if paths.is_empty() {
                return None;
            }
            // 先画一帧“拖拽中”状态，再进入 OLE 模态循环（期间消息被再入守卫丢弃）。
            rt.dragging_icon = Some((fence, icon));
            present_now(rt);
            let copy_only = rt
                .desk
                .fences
                .get(fence)
                .map(|f| f.kind == FenceKind::DesktopInbox)
                .unwrap_or(false);
            let result = if copy_only {
                sylva_shell::drag::start_drag_copy(rt.hwnd, &paths)
            } else {
                sylva_shell::drag::start_drag(rt.hwnd, &paths)
            };
            rt.dragging_icon = None;
            match result {
                Ok(effect) => tracing::info!(count = paths.len(), ?effect, "拖出完成"),
                Err(e) => tracing::warn!("拖出失败: {e}"),
            }
            // 拖出可能被目标移动/删除：重新校验引用（失效项标记 invalid，不猜新路径）。
            let before = rt.desk.clone();
            if reconcile_fences(rt) {
                commit_or_rollback(rt, before, "拖出后同步引用");
            }
        }
        OverlayEvent::IconClassifyDrag {
            to,
            ctrl,
            from,
            icon,
        } => {
            let source_is_inbox = rt
                .desk
                .fences
                .get(from)
                .map(|f| f.kind == FenceKind::DesktopInbox)
                .unwrap_or(false);
            let effect = to.and_then(|idx| rt.desk.fences.get(idx)).map(|f| {
                if f.kind.is_read_only() {
                    DropEffect::None
                } else if ctrl || source_is_inbox {
                    DropEffect::Copy
                } else {
                    DropEffect::Move
                }
            });
            let _ = icon;
            rt.drop_hover = to.zip(effect);
        }
        OverlayEvent::IconClassifyCancel => {
            rt.drop_hover = None;
        }
        OverlayEvent::IconClassifyDrop {
            from,
            icon,
            to,
            ctrl,
        } => {
            rt.drop_hover = None;
            let target = rt.desk.fences.get(to)?;
            if target.kind.is_read_only() {
                tracing::info!("只读镜像栅栏（收件箱/文件夹门户）拒绝拖入分类");
                return None;
            }
            let target_id = target.id;
            let key = (from, icon);
            let ids: Vec<String> = if rt.selected.len() > 1 && rt.selected.contains(&key) {
                selected_ids(rt)
            } else {
                rt.desk
                    .fences
                    .get(from)
                    .and_then(|f| f.icon_ids.get(icon))
                    .cloned()
                    .into_iter()
                    .collect()
            };
            if ids.is_empty() {
                return None;
            }
            let before = rt.desk.clone();
            let action = UndoAction::RestoreItems {
                items: capture_items(rt, &ids),
            };
            let source_inbox = rt
                .desk
                .fences
                .get(from)
                .map(|f| f.kind == FenceKind::DesktopInbox)
                .unwrap_or(false);
            // 桌面收件箱是只读镜像：把它拖到分类栅栏只能是复制引用，不能移出收件箱。
            let copy = ctrl || source_inbox;
            let mode = if copy {
                ClassifyMode::Copy
            } else {
                ClassifyMode::Move
            };
            record(
                rt,
                if copy {
                    "复制到栅栏"
                } else {
                    "移动到栅栏"
                },
                action,
            );
            match commands::classify_move(&mut rt.desk, &ids, target_id, mode) {
                Ok(change) => {
                    if change.ids.is_empty() {
                        rt.desk = before;
                        return None;
                    }
                    commit_or_rollback(rt, before, "跨栅栏分类");
                    tracing::info!(
                        from_fence = from,
                        to_fence = to,
                        count = change.ids.len(),
                        ?mode,
                        "图标已分类"
                    );
                }
                Err(e) => {
                    rt.desk = before;
                    tracing::warn!("跨栅栏分类失败: {e}");
                }
            }
        }
        OverlayEvent::IconClicked { fence, icon, ctrl } => {
            // 单击选中（如资源管理器）：Ctrl+单击切换该图标（不连续多选），普通单击单选
            let key = (fence, icon);
            if ctrl {
                if rt.selected.contains(&key) {
                    rt.selected.retain(|&k| k != key);
                } else {
                    rt.selected.push(key);
                }
            } else {
                rt.selected = vec![key];
            }
        }
        OverlayEvent::SelectDrag {
            fence,
            rect,
            selected,
        } => {
            // 框选拖拽：更新选择集合 + 橡皮筋矩形（App 持有，供绘制）
            rt.selected = selected;
            rt.select_band = Some((
                fence,
                RectF {
                    x: rect.0,
                    y: rect.1,
                    w: rect.2,
                    h: rect.3,
                },
            ));
        }
        OverlayEvent::SelectEnd => {
            // 框选结束：清除橡皮筋显示（选择结果保留）
            rt.select_band = None;
        }
        OverlayEvent::CursorMove { x, y } => {
            // 侧边栏 Dock 放大的连续光标驱动（build_scene 据此每帧算缩放）。
            // 仅当存在 Dock 栅栏时光标才影响画面 → 无 Dock 时不重绘，
            // 桌面任意鼠标移动都不再触发全量重绘（闪烁根治）。
            rt.cursor = Some((x, y));
            redraw = has_sidebar_dock(rt);
        }
        OverlayEvent::CursorLeave => {
            // 光标离开窗口：清除 Dock 放大（下一帧全部恢复 1.0）。
            rt.cursor = None;
            redraw = has_sidebar_dock(rt);
        }
        OverlayEvent::HoverEnter { fence, icon } => {
            rt.hover = Some((fence, icon));
            // 悬停放大补间：从当前状态继续（进入 = 放大方向）
            let base = rt
                .icon_hover
                .filter(|h| h.fence == fence && h.icon == icon)
                .map(|h| h.progress(Instant::now()))
                .unwrap_or(0.0);
            rt.icon_hover = Some(IconHoverAnim {
                fence,
                icon,
                t0: Instant::now(),
                dur: 0.14,
                from: base,
                to: 1.0,
            });
            arm_anim_timer(rt);
        }
        OverlayEvent::HoverLeave => {
            rt.hover = None;
            if let Some(h) = rt.icon_hover {
                let p = h.progress(Instant::now());
                rt.icon_hover = Some(IconHoverAnim {
                    fence: h.fence,
                    icon: h.icon,
                    t0: Instant::now(),
                    dur: 0.18,
                    from: p,
                    to: 0.0,
                });
                arm_anim_timer(rt);
            }
        }
        OverlayEvent::ContextMenu { fence, icon, pos } => {
            // 右键菜单是模态的：菜单开着时 UI 线程本就停在 TrackPopupMenu 里，
            // 标记为 MODAL 让看门狗跳过告警（菜单一旦返回会立刻恢复阶段）。
            watchdog::phase(watchdog::MODAL);
            handle_context_menu(rt, fence, icon, pos);
            watchdog::phase(watchdog::HANDLE);
        }
        OverlayEvent::FilesDropped {
            fence,
            paths,
            effect,
        } => {
            rt.drop_hover = None;
            tracing::info!(fence, count = paths.len(), ?effect, "OLE 拖入");
            let target = rt.desk.fences.get(fence)?;
            if target.kind.is_read_only() {
                tracing::info!(fence, "只读镜像栅栏（收件箱/文件夹门户）拒绝拖入");
                return None;
            }
            // 单个文件夹拖入普通栅栏：让用户选择“文件夹图标”或“文件夹门户”。
            if target.kind == FenceKind::Manual && paths.len() == 1 && Path::new(&paths[0]).is_dir()
            {
                match drop_folder_menu(rt.hwnd) {
                    Some(DropFolderAction::FolderIcon) => {
                        let before = rt.desk.clone();
                        add_paths_to_fence(rt, fence, &paths);
                        commit_or_rollback(rt, before, "拖入文件夹图标");
                    }
                    Some(DropFolderAction::FolderPortal) => {
                        convert_fence_to_portal(rt, fence, &paths[0]);
                    }
                    None => {}
                }
                return None;
            }
            // 拖入任意文件/文件夹/快捷方式：只登记路径引用（位图进 pending_uploads）。
            // 结构性变更前保存回滚点，save 失败时内存回滚，避免“幽灵项”。
            let before = rt.desk.clone();
            add_paths_to_fence(rt, fence, &paths);
            commit_or_rollback(rt, before, "拖入添加");
        }
        OverlayEvent::DragHover { fence, effect } => {
            rt.drop_hover = fence.map(|fence| (fence, effect));
        }
        OverlayEvent::DragLeave => {
            rt.drop_hover = None;
        }
        OverlayEvent::FenceScroll { fence, delta } => {
            // 滚轮滚动：正=向上（回到开头），负=向下（看更多）。上界由 layout 钳制。
            if let Some(f) = rt.desk.fences.get_mut(fence) {
                let s = rt.theme.scale;
                let step = match f.appearance.layout {
                    FenceLayout::List => LIST_ICON_SIZE * s + rt.theme.list_row_gap,
                    // 侧边栏实际步距 = icon + (gap + icon/2)，与 sidebar_eff_gap 一致。
                    FenceLayout::Sidebar => (f.appearance.cell.icon() * 1.5 + f.appearance.gap) * s,
                    FenceLayout::Grid => f.appearance.cell.cell_h() * s,
                };
                f.scroll = (f.scroll - (delta as f32 / 120.0) * step).max(0.0);
            }
            if let Err(e) = rt.store.save(&rt.desk) {
                tracing::warn!("滚动位置持久化失败: {e}");
            }
        }
        OverlayEvent::FenceDragEnd { .. } => {
            // 拖动结束：清掉临时视觉（参考线/尺寸徽标/排序预览），再持久化布局
            rt.guides.clear();
            rt.hud = None;
            rt.grid_reorder = None;
            if let Err(e) = rt.store.save(&rt.desk) {
                tracing::warn!("布局持久化失败: {e}");
            }
        }
        OverlayEvent::OverflowClicked { fence } => {
            // 溢出格：点一下展开（忽略容量显示全部+滚动），再点收起。
            rt.grid_peek = if rt.grid_peek == Some(fence) {
                None
            } else {
                Some(fence)
            };
        }
        OverlayEvent::GridReorderDrag {
            fence,
            icon,
            mx,
            my,
        } => {
            let insert = rt
                .desk
                .fences
                .get(fence)
                .map(|f| slot_index_at(&rt.theme, f, mx, my))
                .unwrap_or(icon);
            // 只读镜像（收件箱/门户）顺序由目录决定，不显示排序预览。
            if rt
                .desk
                .fences
                .get(fence)
                .is_some_and(|f| !f.kind.is_read_only())
            {
                rt.grid_reorder = Some((fence, icon, insert, mx, my));
            }
        }
        OverlayEvent::GridReorderEnd {
            fence,
            icon,
            mx,
            my,
        } => {
            let insert = rt
                .desk
                .fences
                .get(fence)
                .map(|f| slot_index_at(&rt.theme, f, mx, my))
                .unwrap_or(icon);
            rt.grid_reorder = None;
            // 只读镜像（桌面收件箱 / 文件夹门户）的顺序由目录决定，拒绝重排。
            let read_only = rt
                .desk
                .fences
                .get(fence)
                .map(|f| f.kind.is_read_only())
                .unwrap_or(true);
            if !read_only && insert != icon {
                let before = rt.desk.clone();
                if let Some(f) = rt.desk.fences.get_mut(fence)
                    && icon < f.icon_ids.len()
                {
                    let id = f.icon_ids.remove(icon);
                    let adj =
                        (if insert > icon { insert - 1 } else { insert }).min(f.icon_ids.len());
                    f.icon_ids.insert(adj, id);
                }
                commit_or_rollback(rt, before, "栅栏内排序");
            }
        }
        OverlayEvent::FenceClicked { fence } => {
            // 单击栅栏标题：选中它；控制中心开着就切到该栅栏的设置页（联动）。
            rt.grid_peek = None;
            rt.selected_fence = fence.min(rt.desk.fences.len().saturating_sub(1));
            rt.console_page = ConsolePage::Fence;
            if rt.desk.console_open {
                // 列表自动滚动到选中项（行高 36，可视行数由面板高度决定）
                let s = rt.theme.scale;
                rt.fence_scroll = clamp_fence_list_scroll(rt, rt.selected_fence, s);
            }
        }
        OverlayEvent::FenceDoubleClicked { fence } => {
            // 双击标题：折叠 / 展开（折叠后高度 = 上下 padding + 一格标题带）。
            let before = rt.desk.clone();
            if let Some(f) = rt.desk.fences.get_mut(fence) {
                let was_expanded = f.state == FenceState::Expanded;
                f.state = if was_expanded {
                    FenceState::Folded
                } else {
                    FenceState::Expanded
                };
                f.scroll = 0.0;
            }
            // 折叠时保留用户原来的高度（bounds.h 不动），展开后尺寸原样恢复：
            // 折叠态的视觉高度由 layout 按"一格标题带"覆盖，不写回 bounds。
            commit_or_rollback(rt, before, "折叠/展开栅栏");
        }
        OverlayEvent::ConsoleClick { zone } => match zone {
            ConsoleZone::Close => {
                // 折叠为胶囊（面板始终可见，不再「消失」）；保存开关状态供重启恢复
                rt.desk.console_open = false;
                if let Err(e) = rt.store.save(&rt.desk) {
                    tracing::warn!("控制台状态持久化失败: {e}");
                }
                start_panel_tween(rt, 0.0);
            }
            ConsoleZone::Expand => {
                // 点击胶囊展开面板
                rt.desk.console_open = true;
                if let Err(e) = rt.store.save(&rt.desk) {
                    tracing::warn!("控制台状态持久化失败: {e}");
                }
                start_panel_tween(rt, 1.0);
            }
            ConsoleZone::DesktopToggle => {
                toggle_desktop(rt);
            }
            ConsoleZone::NavSystem => {
                rt.console_page = ConsolePage::System;
                rt.detail_scroll = 0.0;
            }
            ConsoleZone::FenceSelect(i) => {
                if i < rt.desk.fences.len() {
                    rt.selected_fence = i;
                    rt.console_page = ConsolePage::Fence;
                    rt.detail_scroll = 0.0;
                }
            }
            ConsoleZone::QuickHide => toggle_quick_hide(rt),
            ConsoleZone::ToggleFenceOverlap => {
                rt.desk.settings.allow_fence_overlap = !rt.desk.settings.allow_fence_overlap;
                let _ = rt.store.save(&rt.desk);
            }
            ConsoleZone::SettingGap(dir) => {
                let v = rt.desk.settings.fence_gap;
                let nv = (v + dir as f32 * 4.0).clamp(4.0, 48.0);
                if (nv - v).abs() > 0.01 {
                    rt.desk.settings.fence_gap = nv;
                    let _ = rt.store.save(&rt.desk);
                }
            }
            ConsoleZone::SettingInset(dir) => {
                let v = rt.desk.settings.desktop_inset;
                let nv = (v + dir as f32 * 4.0).clamp(0.0, 48.0);
                if (nv - v).abs() > 0.01 {
                    rt.desk.settings.desktop_inset = nv;
                    // 安全区变了：重新夹一遍现有栅栏位置（可能往里收），再统一保存。
                    reanchor_fences(rt);
                    let _ = rt.store.save(&rt.desk);
                }
            }
            ConsoleZone::ResetConsoleSize => {
                rt.desk.console_size = None;
                let _ = rt.store.save(&rt.desk);
                tracing::info!("控制中心尺寸已重置为屏幕 1/3");
            }
            ConsoleZone::ToggleAutostart => {
                let enable = !rt.desk.settings.autostart;
                match crate::autostart::set_enabled(enable) {
                    Ok(()) => {
                        rt.desk.settings.autostart = enable;
                        let _ = rt.store.save(&rt.desk);
                    }
                    Err(e) => tracing::warn!("开机自启设置失败: {e}"),
                }
            }
            ConsoleZone::RenameFence => {
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                start_inplace_rename(rt, EditTarget::FenceTitle { fence: i });
            }
            ConsoleZone::FenceLayout(l) => {
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                if l == FenceLayout::List {
                    let w = list_auto_width(rt, i);
                    if let Some(f) = rt.desk.fences.get_mut(i) {
                        f.appearance.layout = l;
                        f.bounds.w = w;
                    }
                } else if l == FenceLayout::Sidebar {
                    if let Some(f) = rt.desk.fences.get_mut(i) {
                        f.appearance.layout = l;
                        f.sidebar_collapsed = false;
                        // 停靠到屏幕边缘（侧 = 当前设置的停靠位置），并夹到屏幕内，
                        // 图标多时高度不超过屏幕（超出滚动），不会跑到屏幕外。
                        let wa = work_area_rect(rt.origin.0, rt.origin.1, rt.vw, rt.vh);
                        let b = sidebar_dock_rect(rt.theme.scale, f, &wa);
                        // 夹在工作区内（任务栏扣除后），下沿不越过任务栏
                        f.bounds = clamp_sidebar_work_rect(b, wa, f.appearance.sidebar_pos);
                    }
                } else if let Some(f) = rt.desk.fences.get_mut(i) {
                    f.appearance.layout = l;
                }
                let _ = rt.store.save(&rt.desk);
            }
            ConsoleZone::FenceCellSize(size) => {
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                let s = rt.theme.scale;
                let vw = rt.vw;
                // 自动列数时：按旧格宽算当前列数，切档后按新格宽回推宽度，
                // 保持"列数不变"——否则切到大档会突然少一列、内容溢出。
                let prev = rt.desk.fences.get(i).map(|f| {
                    (
                        f.appearance.cell,
                        f.appearance.layout,
                        f.appearance.grid_cols,
                        f.bounds.w,
                        f.appearance.padding,
                    )
                });
                if let Some((old_cell, layout, cols, w, pad)) = prev
                    && let Some(f) = rt.desk.fences.get_mut(i)
                {
                    f.appearance.cell = size;
                    if layout == FenceLayout::Grid && cols == 0 && w > 0.0 {
                        let old_w =
                            sylva_core::layout::cell_metrics(old_cell, f.appearance.show_labels, s)
                                .cell_w;
                        let new_w =
                            sylva_core::layout::cell_metrics(size, f.appearance.show_labels, s)
                                .cell_w;
                        let n = (((w - 2.0 * pad * s) / old_w).floor() as usize).max(1);
                        f.bounds.w = (n as f32 * new_w + 2.0 * pad * s)
                            .min(vw.max(MIN_FENCE_W * s))
                            .round();
                    }
                }
                // 档位变了，宽高再按固定行列约束重算一遍。
                resize_fence_to_grid(rt, i);
                // 侧边栏 Dock 的厚度由图标尺寸派生，切档后重新停靠一次。
                if rt
                    .desk
                    .fences
                    .get(i)
                    .is_some_and(|f| f.appearance.layout == FenceLayout::Sidebar)
                {
                    let _ = reanchor_fences(rt);
                }
                let _ = rt.store.save(&rt.desk);
            }
            ConsoleZone::FenceStyle(style) => {
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                if let Some(f) = rt.desk.fences.get_mut(i) {
                    f.appearance.bg_style = style;
                }
                let _ = rt.store.save(&rt.desk);
            }
            ConsoleZone::FenceTint(tint) => {
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                if let Some(f) = rt.desk.fences.get_mut(i) {
                    f.appearance.tint = tint;
                }
                let _ = rt.store.save(&rt.desk);
            }
            ConsoleZone::FenceShowTitle(show) => {
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                if let Some(f) = rt.desk.fences.get_mut(i) {
                    f.appearance.show_title = show;
                }
                resize_fence_to_grid(rt, i);
                let _ = rt.store.save(&rt.desk);
            }
            ConsoleZone::FenceOverflow(mode) => {
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                if let Some(f) = rt.desk.fences.get_mut(i) {
                    f.appearance.overflow = mode;
                }
                // 切换后不再是展开态（换行模式没有溢出格）
                rt.grid_peek = None;
                let _ = rt.store.save(&rt.desk);
            }
            ConsoleZone::FenceShowLabels(show) => {
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                if let Some(f) = rt.desk.fences.get_mut(i) {
                    f.appearance.show_labels = show;
                }
                resize_fence_to_grid(rt, i);
                let _ = rt.store.save(&rt.desk);
            }
            ConsoleZone::FenceIconShape(shape) => {
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                if let Some(f) = rt.desk.fences.get_mut(i) {
                    f.appearance.icon_shape = shape;
                }
                let _ = rt.store.save(&rt.desk);
            }
            ConsoleZone::FenceHoverEffect(effect) => {
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                if let Some(f) = rt.desk.fences.get_mut(i) {
                    f.appearance.hover_effect = effect;
                }
                let _ = rt.store.save(&rt.desk);
            }
            ConsoleZone::FenceHidden(hidden) => {
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                if let Some(f) = rt.desk.fences.get_mut(i) {
                    f.appearance.hidden = hidden;
                }
                let _ = rt.store.save(&rt.desk);
            }
            ConsoleZone::GridPicker(cols, rows) => {
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                if let Some(f) = rt.desk.fences.get_mut(i) {
                    f.appearance.grid_cols = cols.clamp(1, 8);
                    f.appearance.grid_rows = rows.clamp(1, 8);
                }
                resize_fence_to_grid(rt, i);
                let _ = rt.store.save(&rt.desk);
            }
            ConsoleZone::FenceGridCols(delta) => {
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                if let Some(f) = rt.desk.fences.get_mut(i) {
                    f.appearance.grid_cols = step_grid_value(f.appearance.grid_cols, delta);
                }
                resize_fence_to_grid(rt, i);
                let _ = rt.store.save(&rt.desk);
            }
            ConsoleZone::FenceGridRows(delta) => {
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                if let Some(f) = rt.desk.fences.get_mut(i) {
                    f.appearance.grid_rows = step_grid_value(f.appearance.grid_rows, delta);
                }
                // 行数变化同步改栅栏实际高度；0 = 自动高度。
                resize_fence_to_grid(rt, i);
                let _ = rt.store.save(&rt.desk);
            }
            ConsoleZone::FenceOpacity(opacity) => {
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                if let Some(f) = rt.desk.fences.get_mut(i) {
                    f.appearance.fill_opacity = opacity.clamp(0.0, 1.0);
                }
                let _ = rt.store.save(&rt.desk);
            }
            ConsoleZone::FenceCustomColor => {
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                let initial = rt.desk.fences.get(i).and_then(|f| f.appearance.tint);
                if let Some(rgb) = pick_tint(rt.hwnd, initial)
                    && let Some(f) = rt.desk.fences.get_mut(i)
                {
                    f.appearance.tint = Some(rgb);
                }
                let _ = rt.store.save(&rt.desk);
            }
            ConsoleZone::FenceSidebarPos(pos) => {
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                if let Some(f) = rt.desk.fences.get_mut(i) {
                    f.appearance.sidebar_pos = pos;
                    // 位置切换：重新停靠到新的一侧（否则只高亮、不移动）
                    if f.appearance.layout == FenceLayout::Sidebar {
                        let wa = work_area_rect(rt.origin.0, rt.origin.1, rt.vw, rt.vh);
                        let b = sidebar_dock_rect(rt.theme.scale, f, &wa);
                        // 夹在工作区内（任务栏扣除后），下沿不越过任务栏
                        f.bounds = clamp_sidebar_work_rect(b, wa, f.appearance.sidebar_pos);
                    }
                }
                let _ = rt.store.save(&rt.desk);
            }
            ConsoleZone::AddFence => {
                // 新建栅栏：空白栅栏（手动成员）或链接文件夹（门户）。
                match add_fence_menu(rt.hwnd) {
                    Some(AddFenceAction::Blank) => create_blank_fence(rt),
                    Some(AddFenceAction::DesktopInbox) => create_desktop_inbox(rt),
                    Some(AddFenceAction::LinkedFolder) => {
                        if let Some(dir) = pick_folder(rt.hwnd) {
                            if let Err(e) = crate::snapshot::snapshot_before(rt, "新增栅栏") {
                                tracing::warn!("新增栅栏前保存快照失败，取消操作: {e}");
                                return None;
                            }
                            let before = rt.desk.clone();
                            // 用文件夹名作为栅栏标题
                            let title = std::path::Path::new(&dir)
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| format!("栅栏 {}", rt.desk.next_fence_id()));
                            let id = rt.desk.next_fence_id();
                            let s = rt.theme.scale;
                            let w = 360.0 * s;
                            let h = 220.0 * s;
                            let start = Rect::new(80.0 * s, 120.0 * s, w, h);
                            let others = (0..rt.desk.fences.len())
                                .map(|j| fence_collision_rect(rt, j))
                                .collect::<Vec<_>>();
                            let screen = screen_rect(rt);
                            let out = settle_move(&start, &others, &screen, fence_gap(rt));
                            let bounds =
                                Rect::new(out.x.round(), out.y.round(), w.round(), h.round());
                            rt.desk.fences.push(Fence {
                                id,
                                title: Some(title),
                                monitor_id: 0,
                                bounds,
                                state: FenceState::Expanded,
                                icon_ids: Vec::new(),
                                appearance: FenceAppearance::default(),
                                kind: FenceKind::FolderPortal,
                                scroll: 0.0,
                                storage_path: Some(dir),
                                sidebar_collapsed: false,
                                suppressed_paths: Vec::new(),
                            });
                            rt.selected_fence = rt.desk.fences.len() - 1;
                            // 立即同步链接文件夹内容
                            if reconcile_fences(rt) {
                                tracing::info!("新栅栏已链接文件夹并同步内容");
                            }
                            commit_or_rollback(rt, before, "新增栅栏");
                        }
                    }
                    None => {}
                }
            }
            ConsoleZone::RemoveFence => {
                // 移出当前选中的栅栏：成员退回未分组区，栅栏删除。
                // 不删除链接的文件夹（用户数据不受影响）。
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                if i >= rt.desk.fences.len() {
                    return None;
                }
                if let Err(e) = crate::snapshot::snapshot_before(rt, "删除栅栏") {
                    tracing::warn!("删除栅栏前保存快照失败，取消操作: {e}");
                    return None;
                }
                let before = rt.desk.clone();
                if let Some((fence_snapshot, index, items)) = capture_fence(rt, i) {
                    record(
                        rt,
                        "删除栅栏",
                        UndoAction::RestoreFence {
                            fence: Box::new(fence_snapshot),
                            index,
                            items,
                        },
                    );
                }
                let ids: Vec<String> = rt
                    .desk
                    .fences
                    .get(i)
                    .map(|f| f.icon_ids.clone())
                    .unwrap_or_default();
                for id in ids {
                    rt.desk.move_icon(&id, None);
                }
                if i < rt.desk.fences.len() {
                    rt.desk.fences.remove(i);
                }
                rt.selected_fence = rt
                    .selected_fence
                    .saturating_sub(1)
                    .min(rt.desk.fences.len());
                commit_or_rollback(rt, before, "删除栅栏");
            }
            ConsoleZone::ChangeStoragePath => {
                // 更改选中栅栏的存储位置：打开文件夹选择器，移动已有库内项到新路径
                let i = rt
                    .selected_fence
                    .min(rt.desk.fences.len().saturating_sub(1));
                if let Some(paths) = pick_paths(rt.hwnd)
                    && let Some(new_dir) = paths.first()
                {
                    change_fence_storage(rt, i, new_dir);
                }
            }
        },
        OverlayEvent::ConsoleControlDrag { zone } => {
            let i = rt
                .selected_fence
                .min(rt.desk.fences.len().saturating_sub(1));
            match zone {
                ConsoleZone::FenceOpacity(v) => {
                    if let Some(f) = rt.desk.fences.get_mut(i) {
                        f.appearance.fill_opacity = v.clamp(0.0, 1.0);
                    }
                }
                ConsoleZone::GridPicker(cols, rows) => {
                    if let Some(f) = rt.desk.fences.get_mut(i) {
                        f.appearance.grid_cols = cols.clamp(1, 8);
                        f.appearance.grid_rows = rows.clamp(1, 8);
                    }
                    resize_fence_to_grid(rt, i);
                }
                _ => {}
            }
        }
        OverlayEvent::ConsoleControlDragEnd => {
            let _ = rt.store.save(&rt.desk);
        }
        OverlayEvent::ConsoleScroll { delta, pos } => {
            // 光标在左侧栅栏列表上 → 滚列表；否则栅栏页滚右侧设置页。
            let s = rt.theme.scale;
            let in_list = rt.console_list_view.is_some_and(|r| {
                pos.0 >= r.x && pos.0 <= r.x + r.w && pos.1 >= r.y && pos.1 <= r.y + r.h
            });
            if in_list || rt.console_page != ConsolePage::Fence {
                let step = CONSOLE_FENCE_ROW_H * s;
                let max = rt.fence_scroll_max;
                rt.fence_scroll = (rt.fence_scroll - (delta as f32 / 120.0) * step).clamp(0.0, max);
            } else {
                let step = 30.0 * s;
                rt.detail_scroll = (rt.detail_scroll - (delta as f32 / 120.0) * step)
                    .clamp(0.0, rt.detail_scroll_max);
            }
        }
        OverlayEvent::ConsoleHover { zone } => {
            // 控件悬停：存下供下一帧绘制高亮（仅展开面板内上报）
            rt.console_hover = zone;
        }
        OverlayEvent::KeyDown { vk, ctrl } => {
            // Ctrl+Z 撤销删除（编辑框内交给编辑逻辑处理，避免抢文本撤销）。
            if ctrl && vk == b'Z' as u32 && rt.edit.is_none() {
                undo_last(rt);
            } else if vk == VK_ESCAPE.0 as u32 && rt.edit.is_none() && rt.grid_peek.is_some() {
                // Esc 优先收起溢出的九宫格展开态
                rt.grid_peek = None;
            } else if vk == VK_ESCAPE.0 as u32 && rt.edit.is_none() && rt.desk.console_open {
                // Esc 关闭控制中心（避免展开后"困在里面"）
                rt.desk.console_open = false;
                start_panel_tween(rt, 0.0);
                let _ = rt.store.save(&rt.desk);
            } else {
                edit_key(rt, vk, ctrl);
            }
        }
        OverlayEvent::Char { ch } => {
            edit_char(rt, ch);
        }
        OverlayEvent::EditCaret { x } => {
            // 鼠标点编辑框内文本：光标跳到对应字符（不触发「点击别处提交」，
            // 该事件由 overlay 命中模型把框内点击单独路由而来）
            edit_click(rt, x);
        }
        OverlayEvent::ImeStart => {
            if let Some(e) = rt.edit.as_mut() {
                e.composing = true;
                e.comp.clear();
                position_ime_window(rt);
            }
        }
        OverlayEvent::ImeCompose { text, caret } => {
            if let Some(e) = rt.edit.as_mut() {
                e.composing = true;
                e.comp = text;
                let _ = caret;
                position_ime_window(rt);
            }
        }
        OverlayEvent::ImeResult { text } => {
            if let Some(e) = rt.edit.as_mut() {
                e.committing = true;
                e.commit_ime(&text);
                e.committing = false;
                e.composing = false;
            }
        }
        OverlayEvent::ImeEnd => {
            if let Some(e) = rt.edit.as_mut() {
                e.composing = false;
                e.comp.clear();
            }
        }
        OverlayEvent::OverlayFocusLost => {
            // 焦点离开 overlay：内联编辑失焦（待办输入/便签提交文本，重命名提交）
            dismiss_edit(rt);
        }
        OverlayEvent::ConsoleMove { pos } => {
            // 拖动标题栏移动面板：记录左上角（原始坐标 + 增量，避免粘连）
            rt.desk.console_pos = Some(Vec2 { x: pos.0, y: pos.1 });
        }
        OverlayEvent::ConsoleDragEnd => {
            // 拖动结束：持久化面板位置
            if let Err(e) = rt.store.save(&rt.desk) {
                tracing::warn!("控制台位置持久化失败: {e}");
            }
        }
        OverlayEvent::ConsoleResize { rect } => {
            // 拖右/下边缘缩放面板：宽度直接生效；高度换算为「完全展开高度」
            // （可见高度 = 胶囊高 + (展开高 - 胶囊高) × panel 进度，反解展开高）。
            let s = rt.theme.scale;
            let pill_h = CONSOLE_PILL_H * s;
            let panel = rt.console_anim.panel.max(0.05); // 折叠态按最小进度反解，避免除零
            let full_w = rect.2.max(CONSOLE_MIN_W * s);
            let full_h = (pill_h + (rect.3 - pill_h) / panel).max(CONSOLE_MIN_H * s);
            rt.desk.console_size = Some((full_w, full_h));
        }
        OverlayEvent::ConsoleResizeEnd => {
            // 缩放结束：持久化面板尺寸
            if let Err(e) = rt.store.save(&rt.desk) {
                tracing::warn!("控制台尺寸持久化失败: {e}");
            }
        }
        OverlayEvent::ConsoleToggle => {
            // 热键 Ctrl+Alt+T：展开/折叠面板（胶囊始终可见，热键只是切换形态）
            let open = !rt.desk.console_open;
            rt.desk.console_open = open;
            if let Err(e) = rt.store.save(&rt.desk) {
                tracing::warn!("控制台状态持久化失败: {e}");
            }
            if open {
                start_panel_tween(rt, 1.0);
            } else {
                start_panel_tween(rt, 0.0);
            }
        }
        OverlayEvent::QuickHideToggle => {
            toggle_quick_hide(rt);
        }
        OverlayEvent::SystemShutdown => {
            // 系统注销/关机：在进程被终止前恢复真实图标。守卫进程也会兜底，
            // 但系统关机的进程终止顺序不可控，这里必须同步执行。
            rt.hierarchy.restore_icons();
            tracing::warn!("系统注销/关机，已恢复真实桌面图标");
        }
        OverlayEvent::TrayToggle => {
            // 托盘图标双击：切换控制中心开合（与 Ctrl+Alt+T 相同）
            let open = !rt.desk.console_open;
            rt.desk.console_open = open;
            let _ = rt.store.save(&rt.desk);
            start_panel_tween(rt, if open { 1.0 } else { 0.0 });
        }
        OverlayEvent::TrayMenu => {
            handle_tray_menu(rt);
        }
        OverlayEvent::DesktopMenu => {
            handle_desktop_menu(rt);
        }
        OverlayEvent::EditCommitted => {
            // 就地重命名提交后的注入事件：数据已改好，这里只需让尾部重建场景。
        }
        OverlayEvent::AnimTick => {
            // 动画帧：推进面板/行补间；全部结束后停用定时器，回到空闲 0% CPU。
            // 加安全上限：任何状态机异常都不得让 16ms 定时器永久运行（否则持续
            // 全量重绘、日志暴涨，Windows 会以应用无响应终止进程）。
            if rt.anim_started.is_none() {
                rt.anim_started = Some(Instant::now());
            }
            let still = advance_anim(rt);
            let elapsed = rt
                .anim_started
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(0.0);
            if still && elapsed > 2.0 {
                tracing::warn!(elapsed, "动画运行超过 2s，强制停止以避免无响应");
                rt.icon_hover = None;
                rt.desktop_fade = None;
                rt.fence_tweens.clear();
                if let Some(pt) = rt.console_anim.panel_tween.take() {
                    rt.console_anim.panel = pt.to;
                }
                unsafe { (*rt.overlay_ptr).set_anim_active(false) };
                rt.anim_started = None;
            } else if !still {
                unsafe { (*rt.overlay_ptr).set_anim_active(false) };
                rt.anim_started = None;
            }
        }
        OverlayEvent::SyncLibrary => {
            // 双向同步：内部库被外部删除 → 栅栏项移除；链接文件夹增删改名 → 栅栏镜像
            // （文件夹是事实来源，栅栏即文件夹；有变化才持久化）
            // 阶段埋点：这一步会枚举目录 / stat 引用路径，若卡住看门狗会记录下来。
            watchdog::phase(watchdog::SYNC);
            let changed = reconcile_fences(rt);
            watchdog::phase(watchdog::HANDLE);
            if changed {
                let _ = rt.store.save(&rt.desk);
            }
            // 空闲修剪工作集：启动后用户长时间不操作时，把不再活跃的内存页换出 RAM，
            // 保持低常驻（任务管理器「内存」列）。用户交互换入后再空闲同样触发；限频避免频繁调用。
            const IDLE_TRIM_AFTER: std::time::Duration = std::time::Duration::from_secs(30);
            const TRIM_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
            if rt.last_activity.elapsed() >= IDLE_TRIM_AFTER
                && rt.last_trim.elapsed() >= TRIM_MIN_INTERVAL
            {
                rt.last_trim = std::time::Instant::now();
                watchdog::phase(watchdog::TRIM);
                memory::trim();
                watchdog::phase(watchdog::HANDLE);
            }
            // 无变化不重绘（消除 4s 心跳脉冲）
            redraw = changed;
        }
        OverlayEvent::DpiChanged { dpi } => {
            // 所在显示器 DPI 变化（Per-Monitor v2）：重算主题缩放（覆盖旧的缩放
            // 字段），并重排栅栏——侧边栏停靠尺寸随缩放变化，普通栅栏夹回屏内。
            let (primary_w, primary_h) = primary_screen_size();
            let scale = theme_scale_for(dpi, primary_w as u32, primary_h as u32);
            if (scale - rt.theme.scale).abs() < 1e-4 {
                // DPI 实际未变（PerMonitorV2 下窗口跨显示器移动也可能触发）：
                // 无需重画。
                redraw = false;
            } else {
                let mut t = Theme::default();
                apply_theme_scale(&mut t, scale);
                rt.theme = t;
                if let Err(e) = rt.compositor.set_theme(&rt.theme) {
                    tracing::warn!("DPI 变化后重建文本格式失败: {e}");
                }
                if reanchor_fences(rt) {
                    let _ = rt.store.save(&rt.desk);
                }
                tracing::info!(dpi, scale, "DPI 变化：主题已重缩放并重排栅栏");
            }
        }
        OverlayEvent::DisplayChange => {
            // 显示拓扑/分辨率变化：重查虚拟屏，重设 overlay 窗口（移动 + 缩放），
            // 再把超屏栅栏夹回屏内——修复「拔掉显示器/改分辨率后栅栏找不到」。
            let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
            let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
            let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) } as u32;
            let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) } as u32;
            let changed = (vw as f32 - rt.vw).abs() > 0.5
                || (vh as f32 - rt.vh).abs() > 0.5
                || vx as f32 != rt.origin.0
                || vy as f32 != rt.origin.1;
            if changed {
                unsafe { (*rt.overlay_ptr).resize(vx, vy, vw, vh) };
                rt.vw = vw as f32;
                rt.vh = vh as f32;
                rt.origin = (vx as f32, vy as f32);
                // 屏幕尺寸变化后重新计算字体/控件缩放（DPI 不变也要重算）。
                let dpi = unsafe { GetDpiForWindow(rt.hwnd) };
                let (primary_w, primary_h) = primary_screen_size();
                let scale = theme_scale_for(dpi, primary_w as u32, primary_h as u32);
                if (scale - rt.theme.scale).abs() > 1e-4 {
                    let mut t = Theme::default();
                    apply_theme_scale(&mut t, scale);
                    rt.theme = t;
                    if let Err(e) = rt.compositor.set_theme(&rt.theme) {
                        tracing::warn!("显示尺寸变化后重建文本格式失败: {e}");
                    }
                    tracing::info!(dpi, scale, "显示尺寸变化：主题已按新屏幕重新缩放");
                }
                if reanchor_fences(rt) {
                    let _ = rt.store.save(&rt.desk);
                }
                tracing::info!(vx, vy, vw, vh, "显示拓扑变化：overlay 已重设并夹回栅栏");
            }
        }
        OverlayEvent::SidebarReorderDrag {
            fence,
            icon,
            mx,
            my,
        } => {
            // 侧边栏图标拖动中：记录拖动状态，触发重绘
            // 排序计算在 build_scene 中基于实际图标位置完成
            rt.sidebar_reorder = Some((fence, icon, mx, my));
            redraw = true;
        }
        OverlayEvent::SidebarReorderEnd { fence, from, to } => {
            // 侧边栏图标拖动结束：执行重排
            rt.sidebar_reorder = None;
            rt.reorder_slot_order.clear();
            rt.current_icon_positions.clear();
            if let Some(f) = rt.desk.fences.get_mut(fence)
                && from < f.icon_ids.len()
                && to <= f.icon_ids.len()
                && from != to
            {
                let id = f.icon_ids.remove(from);
                let insert_at = if to > from { to - 1 } else { to };
                f.icon_ids.insert(insert_at, id);
                let _ = rt.store.save(&rt.desk);
                tracing::info!(from, to, "侧边栏图标已重排");
            }
            redraw = true;
        }
    }

    // 重绘门控落点：无可见变化的事件直接返回 None（overlay 保持当前命中模型
    // 与区域），省掉一次 build_scene + D2D 全量绘制 + commit。
    if !redraw {
        watchdog::idle();
        return None;
    }

    // 重新排布 + 重绘 + 生成新命中模型（含区域）。
    // 高度策略：`bounds.h == 0` 表示未手动缩放，每帧按内容自适应（增删应用自动长高）；
    // 用户拖角标缩放后 `bounds.h` 变为具体值，高度固定，放不下的图标以 "+N" 提示。
    watchdog::phase(watchdog::SCENE);
    let scene = build_scene(rt, Instant::now());
    let ups = std::mem::take(&mut rt.pending_uploads);
    let upload_refs: Vec<(u64, &IconData)> = ups.iter().map(|(id, d)| (*id, d)).collect();
    watchdog::phase(watchdog::PRESENT);
    watchdog::frame();
    if let Err(e) = rt.compositor.present(&scene, &upload_refs) {
        tracing::warn!("重绘失败: {e}");
    }
    watchdog::phase(watchdog::HIT);
    let model = hit_model_from(&rt.theme, &scene, &rt.desk);
    sync_drop_target(rt, &model);
    watchdog::idle();
    Some(model)
}

/// 把命中模型中的栅栏矩形同步给 OLE 拖放目标（屏幕坐标 = 虚拟坐标 + overlay 原点）。
fn sync_drop_target(rt: &Runtime, model: &HitModel) {
    let Some(target) = &rt.drop_target else {
        return;
    };
    let fences = model
        .fences
        .iter()
        .filter(|f| {
            rt.desk
                .fences
                .get(f.id)
                .map(|fence| !fence.kind.is_read_only())
                .unwrap_or(true)
        })
        .map(|f| {
            (
                f.id,
                Rect::new(
                    f.body.x + rt.origin.0,
                    f.body.y + rt.origin.1,
                    f.body.w,
                    f.body.h,
                ),
            )
        })
        .collect();
    target.set_fences(fences);
}

/// 是否存在侧边栏 Dock 栅栏（决定 `CursorMove` 是否需要驱动重绘）。
fn has_sidebar_dock(rt: &Runtime) -> bool {
    rt.desk
        .fences
        .iter()
        .any(|f| f.appearance.layout == FenceLayout::Sidebar)
}

/// 屏幕尺寸因子：以 1920×1080 为 1.0，按面积比开方，限制在 0.9..=1.35。
/// 高分辨率屏幕字体/控件不会显得过小；拖到不同分辨率工作区后重新计算。
fn resolution_scale(vw: u32, vh: u32) -> f32 {
    if vw == 0 || vh == 0 {
        return 1.0;
    }
    let area = (vw as f32 * vh as f32) / (1920.0 * 1080.0);
    area.sqrt().clamp(0.9, 1.35)
}

/// 实际 UI 缩放 = DPI 缩放 × 屏幕尺寸因子。
fn theme_scale_for(dpi: u32, vw: u32, vh: u32) -> f32 {
    (dpi as f32 / 96.0) * resolution_scale(vw, vh)
}

/// 把默认主题按 DPI/屏幕缩放系数放大（`scale = DPI/96 × 屏幕因子`）。
/// 所有 DIP 度量必须一起缩放，只放文字不放行距/间距正是「行列重叠」的根因。
/// `Theme::default()` 是未缩放基准；DPI 变化时用本函数重新应用，覆盖旧缩放。
fn apply_theme_scale(theme: &mut Theme, scale: f32) {
    theme.scale = scale;
    theme.title.size *= scale;
    theme.fence_title.size *= scale;
    theme.label.size *= scale;
    theme.detail_size *= scale;
    theme.fence_border_width *= scale;
    theme.fence_corner_radius *= scale;
    theme.list_row_gap *= scale;
    theme.list_label_gap *= scale;
}

/// 光标是否按住 Alt（拖动时临时关闭对齐吸附）。
fn is_alt_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_MENU};
    unsafe { GetKeyState(VK_MENU.0 as i32) < 0 }
}

/// 拖动/缩放反馈：对齐参考线 + 尺寸徽标（松手时由 FenceDragEnd 清空）。
fn set_drag_feedback(rt: &mut Runtime, rect: &Rect, others: &[Rect], place: &Rect) {
    rt.guides = alignment_guides(rect, others, place, 0.75)
        .into_iter()
        .map(|g| GuideLine {
            vertical: g.vertical,
            pos: g.pos,
            from: g.from,
            to: g.to,
        })
        .collect();
    let s = rt.theme.scale.max(0.01);
    let text = format!(
        "{:.0} × {:.0}",
        (rect.w / s).round().max(0.0),
        (rect.h / s).round().max(0.0)
    );
    // 徽标贴在栅栏右下角**内侧**：拖动时不会跑到合成表面之外，也不遮挡图标区。
    let font = rt.theme.label.size;
    let w = sylva_core::text::estimate_width(&text, font) + 16.0 * s;
    let h = font * 1.6 + 8.0 * s;
    rt.hud = Some(SceneHud {
        text,
        x: (rect.right() - w - 10.0 * s).max(rect.x + 6.0 * s),
        y: (rect.bottom() - h - 10.0 * s).max(rect.y + 6.0 * s),
        w,
        h,
    });
}

/// 栅栏当前实际高度（物理像素）：`bounds.h > 0` 为固定高度；否则（自动高度）用
/// 最近一次布局渲染高度。碰撞检测与夹屏都必须按真实高度入算——自动高度栅栏
/// `bounds.h == 0`，直接当 0 高会把碰撞检测和夹屏一起带偏。
fn fence_height(rt: &Runtime, i: usize) -> f32 {
    match rt.desk.fences.get(i) {
        // 折叠态：视觉高度恒为「一格标题带」，与 bounds.h 无关
        Some(Fence {
            state: FenceState::Folded,
            ..
        }) => rt.last_layout_h.get(i).copied().unwrap_or(0.0),
        Some(f) if f.bounds.h > 0.0 => f.bounds.h,
        _ => rt.last_layout_h.get(i).copied().unwrap_or(0.0),
    }
}

/// 把矩形左上角夹进给定区域内（宽高不变；区域比矩形小时贴左/上边缘）。
fn clamp_into_rect(x: f32, y: f32, w: f32, h: f32, area: &Rect) -> (f32, f32) {
    (
        x.clamp(area.x, (area.right() - w).max(area.x)),
        y.clamp(area.y, (area.bottom() - h).max(area.y)),
    )
}

/// DPI / 显示拓扑变化后的统一重排：
/// - 侧边栏 Dock：尺寸随主题缩放，重新停靠到屏边（任务栏扣除）。
/// - 普通栅栏：夹回虚拟屏内（真实高度参与，超屏即找回）。
///
/// 返回是否有变化（有变化才需要持久化）。启动恢复与 `WM_DPICHANGED`/
/// `WM_DISPLAYCHANGE` 都走这里，保证同一套语义。
fn reanchor_fences(rt: &mut Runtime) -> bool {
    let mut changed = false;
    for i in 0..rt.desk.fences.len() {
        let bounds = rt.desk.fences[i].bounds;
        if rt.desk.fences[i].appearance.layout == FenceLayout::Sidebar {
            // 侧边栏按所在显示器工作区重钉（多屏时不再被夹回主屏）
            let wa = monitor_work_area(rt, &bounds);
            let b = sidebar_dock_rect(rt.theme.scale, &rt.desk.fences[i], &wa);
            let b = clamp_sidebar_work_rect(b, wa, rt.desk.fences[i].appearance.sidebar_pos);
            if rt.desk.fences[i].bounds != b {
                rt.desk.fences[i].bounds = b;
                changed = true;
            }
        } else {
            // 普通栅栏：夹回所在显示器的"工作区 − 安全区"，与拖动时同一套边界
            let place = fence_placement(rt, &bounds);
            let (nx, ny) =
                clamp_into_rect(bounds.x, bounds.y, bounds.w, fence_height(rt, i), &place);
            if nx != rt.desk.fences[i].bounds.x || ny != rt.desk.fences[i].bounds.y {
                rt.desk.fences[i].bounds.x = nx;
                rt.desk.fences[i].bounds.y = ny;
                changed = true;
            }
        }
    }
    // 夹屏之后再清"行数派生"的旧高度（顺序很重要：先按旧高度夹住位置，
    // 再交给布局收缩，否则高度先归零会让夹屏按 0 高计算，可能把栅栏放到屏幕外）。
    for f in &mut rt.desk.fences {
        if f.appearance.layout == FenceLayout::Grid
            && f.appearance.grid_rows > 0
            && f.bounds.h != 0.0
        {
            f.bounds.h = 0.0;
            changed = true;
        }
    }
    changed
}

/// 把 overlay 窗口设为前台并聚焦（内联编辑接收键盘/IME 的前提）。
fn focus_overlay(rt: &Runtime) {
    // 前台交给隐藏焦点代理，overlay 本体保持 WS_EX_NOACTIVATE（不把桌面壳层提到应用之上）。
    unsafe { (*rt.overlay_ptr).focus_for_input() };
}

/// 内联编辑失焦/点击别处：重命名提交（资源管理器行为）。
fn launch_fence_icon(rt: &mut Runtime, fence: usize, icon: usize) {
    let target = rt
        .desk
        .fences
        .get(fence)
        .and_then(|f| f.icon_ids.get(icon))
        .and_then(|id| rt.item_index.get(id))
        .and_then(|&i| rt.items.get(i));
    if let Some(item) = target {
        tracing::info!(name = %item.display_name, "打开桌面图标");
        item.launch();
    }
}

/// 把栅栏内第 `icon` 个图标移出栅栏：新增项（非桌面枚举）直接删除，桌面图标移回未分组区。
fn remove_fence_icon(rt: &mut Runtime, fence: usize, icon: usize) {
    if rt
        .desk
        .fences
        .get(fence)
        .map(|f| f.kind == FenceKind::DesktopInbox)
        .unwrap_or(false)
    {
        tracing::info!("桌面收件箱为只读镜像，拒绝移出");
        return;
    }
    let before = rt.desk.clone();
    let id = rt
        .desk
        .fences
        .get(fence)
        .and_then(|f| f.icon_ids.get(icon))
        .cloned();
    if let Some(id) = id {
        let action = UndoAction::RestoreItems {
            items: capture_items(rt, std::slice::from_ref(&id)),
        };
        record(rt, "移出栅栏", action);
        remove_fence_icon_by_id(rt, &id);
    }
    commit_or_rollback(rt, before, "移出栅栏");
}

/// 把收集项的链接路径加入所在栅栏的抑制列表，避免文件夹镜像同步又把它加回来。
/// 文件本身保留在磁盘（删除只移除引用）。
fn suppress_linked_references(rt: &mut Runtime, id: &str) {
    let Some(path) = rt.desk.icons.get(id).and_then(|ic| ic.path.clone()) else {
        return;
    };
    if !is_linked_path(rt, &path) {
        return;
    }
    let key = path.to_ascii_lowercase();
    for f in &mut rt.desk.fences {
        if f.icon_ids.iter().any(|x| x == id) && !f.suppressed_paths.iter().any(|p| p == &key) {
            f.suppressed_paths.push(key.clone());
        }
    }
}

/// 按 id 移出栅栏：收集项只移除引用（链接目录项记抑制，文件保留）；
/// 桌面图标移回未分组区。任何路径都不删除磁盘文件（AGENTS §4 / 需求 §2.5）。
fn remove_fence_icon_by_id(rt: &mut Runtime, id: &String) {
    let collected = rt
        .desk
        .icons
        .get(id)
        .map(|i| i.is_collected())
        .unwrap_or(false);
    // 链接目录中的项（无论桌面枚举还是收集项）移出栅栏都要抑制，否则 4s 同步会加回来。
    suppress_linked_references(rt, id);
    if collected {
        remove_icon_entirely(rt, id);
    } else {
        rt.desk.move_icon(id, None);
    }
}

/// 选中集合的 id 快照（先收集后操作，避免移除过程中下标/栅栏顺序变化）。
fn selected_ids(rt: &Runtime) -> Vec<String> {
    rt.selected
        .iter()
        .filter_map(|&(f, i)| {
            rt.desk
                .fences
                .get(f)
                .and_then(|f| f.icon_ids.get(i))
                .cloned()
        })
        .collect()
}

/// 选中集合是否包含只读镜像栅栏（桌面收件箱）的项。
fn selection_has_read_only(rt: &Runtime) -> bool {
    rt.selected.iter().any(|&(f, _)| {
        rt.desk
            .fences
            .get(f)
            .map(|fence| fence.kind == FenceKind::DesktopInbox)
            .unwrap_or(false)
    })
}

/// 选中集合的路径快照（复制到剪贴板用）。
fn selected_paths(rt: &Runtime) -> Vec<String> {
    selected_ids(rt)
        .iter()
        .filter_map(|id| rt.desk.icons.get(id).and_then(|ic| ic.path.clone()))
        .collect()
}

/// 打开全部选中项。
fn open_selected(rt: &mut Runtime) {
    let targets = rt.selected.clone();
    for (f, i) in targets {
        launch_fence_icon(rt, f, i);
    }
}

/// 复制全部选中项到剪贴板（CF_HDROP，与资源管理器「复制」一致，可粘贴到任意文件夹）。
fn copy_selected(rt: &mut Runtime) {
    let paths = selected_paths(rt);
    if paths.is_empty() {
        return;
    }
    set_clipboard_paths(&paths);
}

/// 移出栅栏：选中项全部移出（内部库项删除引用，桌面图标移回未分组区）。
fn remove_selected(rt: &mut Runtime) {
    if selection_has_read_only(rt) {
        tracing::info!("桌面收件箱为只读镜像，拒绝移出");
        return;
    }
    let before = rt.desk.clone();
    let ids = selected_ids(rt);
    if ids.len() > 1
        && let Err(e) = crate::snapshot::snapshot_before(rt, "批量移出栅栏")
    {
        tracing::warn!("批量移出前保存快照失败，取消操作: {e}");
        return;
    }
    if !ids.is_empty() {
        let action = UndoAction::RestoreItems {
            items: capture_items(rt, &ids),
        };
        record(rt, format!("移出 {} 项", ids.len()), action);
    }
    for id in &ids {
        remove_fence_icon_by_id(rt, id);
    }
    commit_or_rollback(rt, before, "批量移出栅栏");
}

/// 删除选中项：只移除引用，绝不删除磁盘文件（AGENTS §4 / 需求 §2.5）。
/// 链接目录项记入抑制列表，防止 4 秒文件夹同步重新加入；桌面图标移回未分组区。
fn delete_selected(rt: &mut Runtime) {
    if selection_has_read_only(rt) {
        tracing::info!("桌面收件箱为只读镜像，拒绝删除引用");
        return;
    }
    let before = rt.desk.clone();
    let ids = selected_ids(rt);
    if ids.len() > 1
        && let Err(e) = crate::snapshot::snapshot_before(rt, "批量删除")
    {
        tracing::warn!("批量删除前保存快照失败，取消操作: {e}");
        return;
    }
    if !ids.is_empty() {
        let action = UndoAction::RestoreItems {
            items: capture_items(rt, &ids),
        };
        record(rt, format!("删除 {} 项", ids.len()), action);
    }
    for id in &ids {
        let collected = rt
            .desk
            .icons
            .get(id)
            .map(|ic| ic.is_collected())
            .unwrap_or(false);
        suppress_linked_references(rt, id);
        if collected {
            remove_icon_entirely(rt, id);
        } else {
            rt.desk.move_icon(id, None);
        }
    }
    commit_or_rollback(rt, before, "删除选中项");
}

/// 让控制中心左侧栅栏列表滚到能看见第 `idx` 行（选中联动时用）。
fn clamp_fence_list_scroll(rt: &Runtime, idx: usize, scale: f32) -> f32 {
    let row_h = CONSOLE_FENCE_ROW_H * scale;
    // 可视行数由面板高度决定（见 build_console），直接用场景回写的滚动上限。
    let max_scroll = rt.fence_scroll_max;
    let view_rows = ((rt.console_list_view.map(|r| r.h).unwrap_or(row_h * 5.0)) / row_h).max(1.0);
    let mut scroll = rt.fence_scroll.clamp(0.0, max_scroll);
    let top = idx as f32 * row_h;
    let bottom = top + row_h;
    if top < scroll {
        scroll = top;
    } else if bottom > scroll + view_rows * row_h {
        scroll = (bottom - view_rows * row_h).min(max_scroll);
    }
    scroll.max(0.0)
}

/// 新栅栏的默认矩形：按当前屏幕做一次碰撞避让，避免叠在已有栅栏上。
fn default_fence_bounds(rt: &Runtime) -> Rect {
    let s = rt.theme.scale;
    let w = 360.0 * s;
    let h = 220.0 * s;
    let start = Rect::new(80.0 * s, 120.0 * s, w, h);
    let others = other_bounds(rt, usize::MAX);
    // 落点限制在"桌面安全区"内：新栅栏不会贴边，也不会压住任务栏。
    let place = fence_placement(rt, &start);
    let out = settle_move(&start, &others, &place, fence_gap(rt));
    Rect::new(out.x.round(), out.y.round(), w.round(), h.round())
}

/// 桌面空白处右键：Windows 原始桌面菜单 + Sylva 子菜单。
fn handle_desktop_menu(rt: &mut Runtime) {
    let (sx, sy) = cursor_screen();
    match shell_menu::show_desktop(rt.hwnd, sx, sy, rt.desk.desktop_mode) {
        shell_menu::DesktopMenuResult::AddBlank => create_blank_fence(rt),
        shell_menu::DesktopMenuResult::AddLinked => {
            if let Some(dir) = pick_folder(rt.hwnd) {
                create_portal_fence(rt, &dir);
            }
        }
        shell_menu::DesktopMenuResult::AddInbox => create_desktop_inbox(rt),
        shell_menu::DesktopMenuResult::OpenConsole => {
            rt.desk.console_open = true;
            let _ = rt.store.save(&rt.desk);
            start_panel_tween(rt, 1.0);
        }
        shell_menu::DesktopMenuResult::ToggleDesktop => toggle_desktop(rt),
        shell_menu::DesktopMenuResult::Invoked | shell_menu::DesktopMenuResult::Canceled => {}
        shell_menu::DesktopMenuResult::Failed => {
            // 极少数环境拿不到桌面 IContextMenu：退回 Sylva 简化菜单。
            tracing::warn!("桌面 Shell 菜单创建失败，退回简化菜单");
            match add_fence_menu(rt.hwnd) {
                Some(AddFenceAction::Blank) => create_blank_fence(rt),
                Some(AddFenceAction::DesktopInbox) => create_desktop_inbox(rt),
                Some(AddFenceAction::LinkedFolder) => {
                    if let Some(dir) = pick_folder(rt.hwnd) {
                        create_portal_fence(rt, &dir);
                    }
                }
                None => {}
            }
        }
    }
}

fn step_grid_value(value: u32, delta: i32) -> u32 {
    if delta < 0 {
        value.saturating_sub(1)
    } else {
        (value + 1).min(8)
    }
}

/// 用户调整固定列/行时同步改变栅栏实际宽高，而不是只改内部排布。
fn resize_fence_to_grid(rt: &mut Runtime, i: usize) {
    let s = rt.theme.scale;
    let vw = rt.vw;
    let Some(f) = rt.desk.fences.get_mut(i) else {
        return;
    };
    if f.appearance.layout != FenceLayout::Grid {
        return;
    }
    let app = &f.appearance;
    let pad = app.padding * s;
    // 格宽/格高来自档位（与布局、绘制同源），不再用图标 + gap 现推。
    let cell = sylva_core::layout::cell_metrics(app.cell, app.show_labels, s);
    if app.grid_cols > 0 {
        let w = (pad * 2.0 + app.grid_cols as f32 * cell.cell_w)
            .max(MIN_FENCE_W * s)
            .min((vw - f.bounds.x).max(MIN_FENCE_W * s));
        f.bounds.w = w.round();
    }
    // 高度一律交给布局派生：行数由 grid_rows 表达（0 = 全部行），
    // bounds.h 只表示"用户手动拉伸过"。这里清掉，避免旧指标写死的值
    // 在标题带/格高变化后留下一条空白。
    f.bounds.h = 0.0;
}

/// Windows 原生颜色选择器；返回 RGB 0..1。取消返回 None。
fn pick_tint(owner: HWND, initial: Option<[f32; 3]>) -> Option<[f32; 3]> {
    use windows::Win32::Foundation::COLORREF;
    use windows::Win32::UI::Controls::Dialogs::{
        CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW,
    };
    const CUSTOM_COLORS: usize = 16;
    let mut custom = [COLORREF(0); CUSTOM_COLORS];
    let init = initial
        .map(|rgb| {
            let r = (rgb[0].clamp(0.0, 1.0) * 255.0).round() as u32;
            let g = (rgb[1].clamp(0.0, 1.0) * 255.0).round() as u32;
            let b = (rgb[2].clamp(0.0, 1.0) * 255.0).round() as u32;
            r | (g << 8) | (b << 16)
        })
        .unwrap_or(0x0020_2020);
    let mut cc = CHOOSECOLORW {
        lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: owner,
        rgbResult: COLORREF(init),
        lpCustColors: custom.as_mut_ptr(),
        Flags: windows::Win32::UI::Controls::Dialogs::CHOOSECOLOR_FLAGS(
            CC_RGBINIT.0 | CC_FULLOPEN.0,
        ),
        ..Default::default()
    };
    unsafe { ChooseColorW(&mut cc) }.as_bool().then(|| {
        let c = cc.rgbResult.0;
        [
            (c & 0xff) as f32 / 255.0,
            ((c >> 8) & 0xff) as f32 / 255.0,
            ((c >> 16) & 0xff) as f32 / 255.0,
        ]
    })
}

/// 新建文件夹门户栅栏。
fn create_portal_fence(rt: &mut Runtime, dir: &str) {
    if let Err(e) = crate::snapshot::snapshot_before(rt, "新增文件夹门户") {
        tracing::warn!("新增文件夹门户前保存快照失败，取消操作: {e}");
        return;
    }
    let before = rt.desk.clone();
    let id = rt.desk.next_fence_id();
    let title = Path::new(dir)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("门户 {id}"));
    rt.desk.fences.push(Fence {
        id,
        title: Some(title),
        monitor_id: 0,
        bounds: default_fence_bounds(rt),
        state: FenceState::Expanded,
        icon_ids: Vec::new(),
        appearance: FenceAppearance::default(),
        kind: FenceKind::FolderPortal,
        scroll: 0.0,
        storage_path: Some(dir.to_string()),
        sidebar_collapsed: false,
        suppressed_paths: Vec::new(),
    });
    rt.selected_fence = rt.desk.fences.len() - 1;
    if reconcile_fences(rt) {
        tracing::info!("新文件夹门户已同步内容");
    }
    commit_or_rollback(rt, before, "新增文件夹门户");
}

/// 把空白普通栅栏转换为文件夹门户；非空栅栏不原地转换，另建门户栅栏。
fn convert_fence_to_portal(rt: &mut Runtime, fence_idx: usize, dir: &str) {
    let Some(fence) = rt.desk.fences.get(fence_idx) else {
        return;
    };
    if fence.kind == FenceKind::DesktopInbox {
        tracing::warn!("桌面收件箱不能转换为文件夹门户");
        return;
    }
    if !fence.icon_ids.is_empty() {
        create_portal_fence(rt, dir);
        return;
    }
    if let Err(e) = crate::snapshot::snapshot_before(rt, "转换文件夹门户") {
        tracing::warn!("转换文件夹门户前保存快照失败，取消操作: {e}");
        return;
    }
    let before = rt.desk.clone();
    if let Some(f) = rt.desk.fences.get_mut(fence_idx) {
        f.kind = FenceKind::FolderPortal;
        f.storage_path = Some(dir.to_string());
        f.suppressed_paths.clear();
        if f.title
            .as_deref()
            .map(|t| t.trim().is_empty() || t.starts_with("新栅栏"))
            .unwrap_or(true)
        {
            f.title = Path::new(dir)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned());
        }
    }
    if reconcile_fences(rt) {
        tracing::info!("栅栏已转换为文件夹门户并同步内容");
    }
    commit_or_rollback(rt, before, "转换文件夹门户");
}

/// 显式创建/恢复桌面收件箱（若已存在则提示，不重复创建）。
fn create_desktop_inbox(rt: &mut Runtime) {
    if rt
        .desk
        .fences
        .iter()
        .any(|f| f.kind == FenceKind::DesktopInbox)
    {
        tracing::info!("桌面收件箱已存在");
        return;
    }
    let Some(desktop) = shell_desktop_path() else {
        tracing::warn!("未找到桌面路径，无法创建桌面收件箱");
        return;
    };
    if let Err(e) = crate::snapshot::snapshot_before(rt, "新建桌面收件箱") {
        tracing::warn!("新建桌面收件箱前保存快照失败，取消操作: {e}");
        return;
    }
    let before = rt.desk.clone();
    if !ensure_desktop_inbox(&mut rt.desk, Some(&desktop)) {
        tracing::warn!("桌面收件箱创建失败");
        return;
    }
    if let Some(idx) = rt
        .desk
        .fences
        .iter()
        .position(|f| f.kind == FenceKind::DesktopInbox)
    {
        rt.selected_fence = idx;
    }
    if reconcile_fences(rt) {
        tracing::info!("桌面收件箱已同步桌面图标");
    }
    commit_or_rollback(rt, before, "新建桌面收件箱");
}

/// 新建空白栅栏：不绑定目录、成员为空，后续由用户拖入分类。
fn create_blank_fence(rt: &mut Runtime) {
    if let Err(e) = crate::snapshot::snapshot_before(rt, "新增空白栅栏") {
        tracing::warn!("新增空白栅栏前保存快照失败，取消操作: {e}");
        return;
    }
    let before = rt.desk.clone();
    let id = rt.desk.next_fence_id();
    let title = format!("新栅栏 {id}");
    rt.desk.fences.push(Fence {
        id,
        title: Some(title),
        monitor_id: 0,
        bounds: default_fence_bounds(rt),
        state: FenceState::Expanded,
        icon_ids: Vec::new(),
        appearance: FenceAppearance::default(),
        kind: FenceKind::Manual,
        scroll: 0.0,
        storage_path: None,
        sidebar_collapsed: false,
        suppressed_paths: Vec::new(),
    });
    rt.selected_fence = rt.desk.fences.len() - 1;
    commit_or_rollback(rt, before, "新增空白栅栏");
}

/// Quick hide：隐藏全部栅栏/控制台视觉（区域置空）并显示真实桌面图标；再按恢复。
fn toggle_quick_hide(rt: &mut Runtime) {
    rt.quick_hide = !rt.quick_hide;
    unsafe {
        (*rt.overlay_ptr).set_desktop_menu_enabled(true);
    }
    if rt.quick_hide {
        rt.hierarchy.restore_icons();
        rt.drop_hover = None;
        tracing::info!("Quick hide 开启：栅栏已隐藏，桌面可正常操作");
    } else {
        if !rt.desk.desktop_mode {
            rt.hierarchy.hide_icons();
        }
        tracing::info!("Quick hide 关闭：栅栏已恢复");
    }
}

/// 一键切换桌面模式：栅栏 ⇄ 原始桌面。
///
/// 切到原始桌面：立即恢复真实图标，栅栏 0.22s 淡出（淡完不再接收命中）；
/// 切回栅栏：隐藏真实图标，栅栏淡入。控制中心本身始终可见，保证随时能切回。
fn toggle_desktop(rt: &mut Runtime) {
    // 桌面模式与 Quick hide 互斥：进入桌面切换前先恢复栅栏视觉，避免两个隐藏状态叠加。
    rt.quick_hide = false;
    rt.desk.desktop_mode = !rt.desk.desktop_mode;
    unsafe {
        (*rt.overlay_ptr).set_desktop_menu_enabled(true);
    }
    if rt.desk.desktop_mode {
        rt.hierarchy.restore_icons();
        rt.desktop_fade = Some(PanelTween {
            t0: Instant::now(),
            dur: 0.22,
            from: 1.0,
            to: 0.0,
        });
    } else {
        rt.hierarchy.hide_icons();
        rt.desktop_fade = Some(PanelTween {
            t0: Instant::now(),
            dur: 0.22,
            from: 0.0,
            to: 1.0,
        });
    }
    arm_anim_timer(rt);
    if let Err(e) = rt.store.save(&rt.desk) {
        tracing::warn!("桌面模式持久化失败: {e}");
    }
}

/// 当前光标在虚拟屏幕的物理坐标（右键菜单弹出位置 / IME 窗口定位用）。
fn cursor_screen() -> (i32, i32) {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    (pt.x, pt.y)
}
