//! 桌面图标恢复守卫。
//!
//! 需求 §4.3：任何退出路径都必须恢复被隐藏的真实桌面图标，恢复逻辑不能依赖
//! UI 线程存活。主进程的 RAII 只能覆盖正常返回 / panic（unwind），无法覆盖
//! TerminateProcess / 强杀，因此这里再启动一个同二进制的独立守卫进程：
//!
//! - 主进程创建 clean/ready 两个命名事件，spawn `sylva.exe --recovery-guard <pid>`；
//! - 守卫在 ready 事件上握手，然后等待「父进程句柄」或「clean 事件」；
//! - 父进程正常退出：SetEvent(clean)，守卫直接退出；
//! - 父进程崩溃/被强杀：父进程句柄 signaled，守卫重新探测桌面层级并显示
//!   SysListView32，删除恢复标记后退出。
//!
//! 标记文件 `data/recovery.marker` 是最后一道兜底：守卫启动失败或系统在
//! 守卫调度前终止进程时，下次启动会先恢复真实图标再进入正常初始化。

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateEventW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, OpenProcess,
    PROCESS_SYNCHRONIZE, SYNCHRONIZATION_SYNCHRONIZE, SetEvent, WaitForMultipleObjects,
    WaitForSingleObject,
};
use windows::core::PCWSTR;

use sylva_shell::takeover::{self, DesktopHierarchy};

const GUARD_ARG: &str = "--recovery-guard";
const MARKER_FILE: &str = "recovery.marker";

/// 主进程用：若命令行是守卫模式，返回父进程 PID。
pub fn guard_mode() -> Option<u32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some(GUARD_ARG) {
        return None;
    }
    Some(args.next().and_then(|s| s.parse().ok()).unwrap_or(0))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn data_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("data")
}

fn clean_event_name(pid: u32) -> String {
    format!("Local\\Sylva.Recovery.Clean.{pid}")
}

fn ready_event_name(pid: u32) -> String {
    format!("Local\\Sylva.Recovery.Ready.{pid}")
}

fn log_guard(data_dir: &Path, message: &str) {
    let path = data_dir.join("logs").join("guard.log");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "[{}] {message}", now_secs());
    }
}

/// 恢复标记：主进程隐藏图标前写入，正常退出/守卫恢复后删除。
pub struct RecoveryMarker {
    path: PathBuf,
}

impl RecoveryMarker {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(MARKER_FILE),
        }
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    pub fn begin(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("marker.tmp");
        std::fs::write(
            &tmp,
            format!("pid={}\nstarted_at={}\n", std::process::id(), now_secs()),
        )?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn clear(&self) {
        let _ = std::fs::remove_file(&self.path);
    }

    /// 启动时检测上次是否异常退出；存在标记则先恢复真实图标。
    /// 返回是否执行过恢复。
    pub fn recover_if_needed(&self, hierarchy: &DesktopHierarchy) -> bool {
        if !self.exists() {
            return false;
        }
        hierarchy.restore_icons();
        let restored = takeover::restore_desktop_icons() || hierarchy.list_view.is_some();
        self.clear();
        restored
    }
}

/// 主进程持有的守卫句柄；Drop 时 SetEvent(clean) 让守卫正常退出。
pub struct GuardHandle {
    child: Child,
    clean: HANDLE,
    data_dir: PathBuf,
}

impl GuardHandle {
    /// 正常退出：通知守卫不要恢复，给它一小段时间退出，然后关闭句柄。
    pub fn shutdown(mut self) {
        unsafe {
            let _ = SetEvent(self.clean);
        }
        let deadline = Instant::now() + Duration::from_millis(800);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        unsafe {
            let _ = CloseHandle(self.clean);
        }
        log_guard(&self.data_dir, "主进程正常退出，守卫结束");
    }
}

/// 启动独立守卫，并等待它完成 ready 握手后再返回。
/// 返回 None 表示守卫未启动（调用方仍可用 recovery.marker 兜底）。
pub fn start_guard(data_dir: &Path) -> Option<GuardHandle> {
    let pid = std::process::id();
    let clean_name = clean_event_name(pid);
    let ready_name = ready_event_name(pid);

    let clean =
        unsafe { CreateEventW(None, true, false, PCWSTR(wide(&clean_name).as_ptr())) }.ok()?;
    let ready = match unsafe { CreateEventW(None, true, false, PCWSTR(wide(&ready_name).as_ptr())) }
    {
        Ok(h) => h,
        Err(e) => {
            log_guard(data_dir, &format!("创建守卫 ready 事件失败: {e}"));
            unsafe {
                let _ = CloseHandle(clean);
            }
            return None;
        }
    };

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            log_guard(data_dir, &format!("获取当前 exe 失败: {e}"));
            unsafe {
                let _ = CloseHandle(clean);
                let _ = CloseHandle(ready);
            }
            return None;
        }
    };

    let child = match Command::new(exe)
        .arg(GUARD_ARG)
        .arg(pid.to_string())
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log_guard(data_dir, &format!("启动恢复守卫失败: {e}"));
            unsafe {
                let _ = CloseHandle(clean);
                let _ = CloseHandle(ready);
            }
            return None;
        }
    };

    let wait = unsafe { WaitForSingleObject(ready, 3000) };
    if wait.0 != WAIT_OBJECT_0.0 {
        log_guard(
            data_dir,
            &format!("守卫 ready 握手超时/失败（{wait:?}），使用标记文件兜底"),
        );
    }
    unsafe {
        let _ = CloseHandle(ready);
    }

    Some(GuardHandle {
        child,
        clean,
        data_dir: data_dir.to_path_buf(),
    })
}

/// 守卫进程入口：等待父进程退出，若父进程非正常退出则恢复桌面图标。
pub fn run(parent_pid: u32) {
    let data_dir = data_dir();
    let clean_name = clean_event_name(parent_pid);
    let ready_name = ready_event_name(parent_pid);

    let clean = unsafe {
        OpenEventW(
            SYNCHRONIZATION_SYNCHRONIZE,
            false,
            PCWSTR(wide(&clean_name).as_ptr()),
        )
    };
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, parent_pid) };
    // ready 事件需要 SetEvent 权限（EVENT_MODIFY_STATE），只给 SYNCHRONIZE 会
    // 打开成功但 SetEvent 返回拒绝访问，父进程永远等不到握手。
    let ready = unsafe {
        OpenEventW(
            SYNCHRONIZATION_SYNCHRONIZE | EVENT_MODIFY_STATE,
            false,
            PCWSTR(wide(&ready_name).as_ptr()),
        )
    };

    if let Ok(ready) = ready {
        unsafe {
            let _ = SetEvent(ready);
            let _ = CloseHandle(ready);
        }
    }

    match (clean, process) {
        (Ok(clean), Ok(process)) => {
            let wait = unsafe { WaitForMultipleObjects(&[process, clean], false, INFINITE) };
            unsafe {
                let _ = CloseHandle(process);
                let _ = CloseHandle(clean);
            }
            if wait.0 == WAIT_OBJECT_0.0 {
                restore_and_clear(&data_dir, "父进程异常退出，守卫恢复桌面图标");
            } else if wait.0 == WAIT_OBJECT_0.0 + 1 {
                log_guard(&data_dir, "收到正常退出信号，守卫结束");
            } else {
                log_guard(&data_dir, &format!("守卫等待异常（{wait:?}），执行恢复"));
                restore_and_clear(&data_dir, "守卫等待异常兜底恢复");
            }
        }
        _ => {
            restore_and_clear(&data_dir, "无法打开父进程/事件，兜底恢复");
        }
    }
}

fn restore_and_clear(data_dir: &Path, reason: &str) {
    let restored = takeover::restore_desktop_icons();
    let _ = std::fs::remove_file(data_dir.join(MARKER_FILE));
    log_guard(data_dir, &format!("{reason}：restored={restored}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_roundtrip_and_clear() {
        let dir =
            std::env::temp_dir().join(format!("sylva-recovery-marker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let marker = RecoveryMarker::new(&dir);
        assert!(!marker.exists());
        marker.begin().unwrap();
        assert!(marker.exists());
        marker.clear();
        assert!(!marker.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn event_names_are_session_local() {
        assert!(clean_event_name(42).starts_with("Local\\"));
        assert!(clean_event_name(42).ends_with("42"));
        assert!(ready_event_name(42).ends_with("42"));
    }
}
