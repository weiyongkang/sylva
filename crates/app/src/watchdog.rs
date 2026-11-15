//! 界面线程卡死（AppHang）诊断。
//!
//! Windows 判定"未响应"需要 UI 线程 5s 不处理消息；这类卡顿通常发生在一次阻塞调用里
//! （Shell/COM、慢盘 I/O、模态菜单）。崩溃后无法回溯，因此在 UI 线程埋点：
//!
//! - 处理事件前后、场景构建、呈现、同步、菜单/对话框处更新"阶段"；
//! - 独立的看门狗线程每秒检查一次：同一阶段停留超过 `STALL_MS` 就**同步**追加一行
//!   到 `data/logs/hang.log`（不经过 tracing 的异步 writer，卡死时也能落盘）。
//!
//! 日志形如：
//! ```text
//! [12345ms] STALL 8.2s phase=SYNC(reconcile) event=FenceClicked
//! ```
//! 下一行就是定位问题的关键：阶段 + 正在处理的事件类型。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// 0 = 空闲（消息循环等待中），不参与卡顿判定。
pub(crate) const IDLE: u32 = 0;
/// 收到事件、进入 `handle_event`。
pub(crate) const ENTER: u32 = 1;
/// 事件分支处理中（尚未重排场景）。
pub(crate) const HANDLE: u32 = 2;
/// 正在 `build_scene`（布局/控制中心几何）。
pub(crate) const SCENE: u32 = 3;
/// 正在 `present`（D2D 绘制 + 合成提交）。
pub(crate) const PRESENT: u32 = 4;
/// 正在生成命中模型 / 更新窗口区域。
pub(crate) const HIT: u32 = 5;
/// 正在持久化（redb 事务 / 快照写盘）。
pub(crate) const SAVE: u32 = 6;
/// 周期同步：枚举目录、stat 引用路径。
pub(crate) const SYNC: u32 = 7;
/// 工作集修剪。
pub(crate) const TRIM: u32 = 8;
/// 模态 Shell 菜单 / 系统对话框（用户不操作就会一直停在这里，属于预期）。
pub(crate) const MODAL: u32 = 9;
/// 启动期 Shell 扩展预热（后台线程）。
pub(crate) const PRIME: u32 = 10;

/// 同一阶段停留超过该时长即记录一次。
const STALL_MS: u64 = 2500;

static START: OnceLock<Instant> = OnceLock::new();
static PHASE: AtomicU32 = AtomicU32::new(IDLE);
static STAMP: AtomicU64 = AtomicU64::new(0);
static EVENT: AtomicU32 = AtomicU32::new(0);
/// 已完成的重绘帧数（看门狗据此识别"重绘风暴"：动画/事件循环导致持续全量重绘）。
static FRAMES: AtomicU64 = AtomicU64::new(0);
static LOG: OnceLock<Option<PathBuf>> = OnceLock::new();

fn elapsed_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn phase_name(p: u32) -> &'static str {
    match p {
        IDLE => "IDLE",
        ENTER => "ENTER",
        HANDLE => "HANDLE",
        SCENE => "SCENE(build_scene)",
        PRESENT => "PRESENT(draw/commit)",
        HIT => "HIT(hit_model/region)",
        SAVE => "SAVE(store)",
        SYNC => "SYNC(reconcile)",
        TRIM => "TRIM(working set)",
        MODAL => "MODAL(menu/dialog)",
        PRIME => "PRIME(shell warmup)",
        _ => "?",
    }
}

/// 事件类型短名（只用于日志；未知类型记 "?"）。
pub(crate) fn event_name(kind: u32) -> &'static str {
    match kind {
        1 => "FenceMove",
        2 => "FenceResize",
        3 => "FenceClicked",
        4 => "FenceDoubleClicked",
        5 => "IconClicked",
        6 => "IconDoubleClicked",
        7 => "IconDragOut",
        8 => "IconClassifyDrag",
        9 => "IconClassifyDrop",
        10 => "SelectDrag",
        11 => "ContextMenu",
        12 => "HoverEnter",
        13 => "HoverLeave",
        14 => "CursorMove",
        15 => "CursorLeave",
        16 => "FilesDropped",
        17 => "DragHover",
        18 => "FenceScroll",
        19 => "ConsoleClick",
        20 => "ConsoleHover",
        21 => "ConsoleScroll",
        22 => "ConsoleMove",
        23 => "ConsoleResize",
        24 => "KeyDown",
        25 => "Char",
        26 => "AnimTick",
        27 => "SyncLibrary",
        28 => "DpiChanged",
        29 => "DisplayChange",
        30 => "FenceDragEnd",
        31 => "ConsoleControlDrag",
        32 => "ConsoleControlDragEnd",
        33 => "ConsoleDragEnd",
        34 => "ConsoleResizeEnd",
        35 => "ConsoleToggle",
        36 => "TrayMenu",
        37 => "TrayToggle",
        38 => "DesktopMenu",
        39 => "EditCommitted",
        40 => "SidebarReorderDrag",
        41 => "SidebarReorderEnd",
        42 => "SelectEnd",
        43 => "EditCaret",
        _ => "?",
    }
}

/// 事件对象 → 日志用的短码。
pub(crate) fn code(ev: &sylva_render::OverlayEvent) -> u32 {
    use sylva_render::OverlayEvent as E;
    match ev {
        E::FenceMove { .. } => 1,
        E::FenceResize { .. } => 2,
        E::FenceClicked { .. } => 3,
        E::FenceDoubleClicked { .. } => 4,
        E::IconClicked { .. } => 5,
        E::IconDoubleClicked { .. } => 6,
        E::IconDragOut { .. } => 7,
        E::IconClassifyDrag { .. } => 8,
        E::IconClassifyDrop { .. } => 9,
        E::SelectDrag { .. } => 10,
        E::ContextMenu { .. } => 11,
        E::HoverEnter { .. } => 12,
        E::HoverLeave => 13,
        E::CursorMove { .. } => 14,
        E::CursorLeave => 15,
        E::FilesDropped { .. } => 16,
        E::DragHover { .. } => 17,
        E::FenceScroll { .. } => 18,
        E::ConsoleClick { .. } => 19,
        E::ConsoleHover { .. } => 20,
        E::ConsoleScroll { .. } => 21,
        E::ConsoleMove { .. } => 22,
        E::ConsoleResize { .. } => 23,
        E::KeyDown { .. } => 24,
        E::Char { .. } => 25,
        E::AnimTick => 26,
        E::SyncLibrary => 27,
        E::DpiChanged { .. } => 28,
        E::DisplayChange => 29,
        E::FenceDragEnd { .. } => 30,
        E::ConsoleControlDrag { .. } => 31,
        E::ConsoleControlDragEnd => 32,
        E::ConsoleDragEnd => 33,
        E::ConsoleResizeEnd => 34,
        E::ConsoleToggle => 35,
        E::TrayMenu => 36,
        E::TrayToggle => 37,
        E::DesktopMenu => 38,
        E::EditCommitted => 39,
        E::SidebarReorderDrag { .. } => 40,
        E::SidebarReorderEnd { .. } => 41,
        E::SelectEnd => 42,
        E::EditCaret { .. } => 43,
        _ => 0,
    }
}

/// 初始化看门狗（写 `<data_dir>/logs/hang.log`）。
pub(crate) fn init(data_dir: &Path) {
    let log_dir = data_dir.join("logs");
    let path = log_dir.join("hang.log");
    let _ = std::fs::create_dir_all(&log_dir);
    let _ = LOG.set(Some(path));
    let _ = START.set(Instant::now());
    let _ = std::thread::Builder::new()
        .name("sylva-hang-watch".into())
        .spawn(watchdog_loop);
}

/// 记录当前阶段（UI 线程调用，极轻：两次原子写）。
pub(crate) fn phase(p: u32) {
    PHASE.store(p, Ordering::Relaxed);
    STAMP.store(elapsed_ms(), Ordering::Relaxed);
}

/// 回到空闲：消息循环等待下一条消息，不算卡顿。
pub(crate) fn idle() {
    phase(IDLE);
}

/// 记录当前事件类型（用于卡顿时说明"在哪个事件里卡住"）。
pub(crate) fn event(kind: u32) {
    EVENT.store(kind, Ordering::Relaxed);
}

/// 计数一次重绘（每帧 present 前调用）。
pub(crate) fn frame() {
    FRAMES.fetch_add(1, Ordering::Relaxed);
}

fn watchdog_loop() {
    let mut last_report = 0u64;
    let mut last_frames = 0u64;
    let mut last_storm = 0u64;
    loop {
        std::thread::sleep(Duration::from_millis(1000));
        // —— 重绘风暴：每秒几十帧说明有动画/事件在自激，UI 会被拖到无响应 ——
        let frames = FRAMES.load(Ordering::Relaxed);
        let now_ms = elapsed_ms();
        if now_ms.saturating_sub(last_storm) >= 5000 {
            let fps = frames.saturating_sub(last_frames);
            if fps >= 25 {
                last_storm = now_ms;
                let ev = EVENT.load(Ordering::Relaxed);
                let line = format!("[{now_ms}ms] STORM fps={fps} event={}\n", event_name(ev));
                if let Some(Some(path)) = LOG.get()
                    && let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                {
                    let _ = f.write_all(line.as_bytes());
                    let _ = f.flush();
                }
            }
        }
        last_frames = frames;
        let p = PHASE.load(Ordering::Relaxed);
        // IDLE：消息循环等待中；MODAL：Shell 菜单/系统对话框，用户不操作本就会停住。
        if p == IDLE || p == MODAL {
            continue;
        }
        let stamp = STAMP.load(Ordering::Relaxed);
        let now = elapsed_ms();
        let stalled = now.saturating_sub(stamp);
        if stalled < STALL_MS {
            continue;
        }
        // 每 5s 报一次，避免日志被刷屏
        if now.saturating_sub(last_report) < 5000 {
            continue;
        }
        last_report = now;
        let ev = EVENT.load(Ordering::Relaxed);
        let line = format!(
            "[{now}ms] STALL {:.1}s phase={} event={}\n",
            stalled as f64 / 1000.0,
            phase_name(p),
            event_name(ev)
        );
        if let Some(Some(path)) = LOG.get()
            && let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
        {
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
        }
    }
}
