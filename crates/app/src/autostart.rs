//! 开机自启：HKCU 用户级 Run 键（无需管理员权限）。
//!
//! 安装器在勾选时已经写入 `Sylva` 值；App 启动时做对账：
//! - 注册项指向当前 exe 且设置未勾选 → 采纳为 true（保留安装器选择）；
//! - 设置已勾选但注册项缺失/路径变化 → 重写为当前 exe；
//! - 设置未勾选且注册项指向旧路径 → 只记日志，不擅自删除。

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE,
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::core::PCWSTR;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Sylva";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open_run(access: REG_SAM_FLAGS) -> Result<HKEY, String> {
    let mut hkey = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(wide(RUN_KEY).as_ptr()),
            None,
            access,
            &mut hkey,
        )
    };
    if status == ERROR_SUCCESS {
        Ok(hkey)
    } else {
        Err(format!("打开 Run 键失败: {}", status.0))
    }
}

/// 当前 exe 的启动命令行（带引号）。
pub fn command_line_for(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

pub fn current_exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("获取当前 exe 失败: {e}"))
}

/// 读取注册项中的启动命令行。
pub fn registered_command() -> Result<Option<String>, String> {
    let hkey = open_run(KEY_QUERY_VALUE)?;
    let mut size = 0u32;
    let status = unsafe {
        RegQueryValueExW(
            hkey,
            PCWSTR(wide(VALUE_NAME).as_ptr()),
            None,
            None,
            None,
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        return Ok(None);
    }
    if status != ERROR_SUCCESS || size == 0 {
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        return Ok(None);
    }
    let mut buf = vec![0u8; size as usize];
    let mut ty = REG_VALUE_TYPE::default();
    let status = unsafe {
        RegQueryValueExW(
            hkey,
            PCWSTR(wide(VALUE_NAME).as_ptr()),
            None,
            Some(&mut ty),
            Some(buf.as_mut_ptr()),
            Some(&mut size),
        )
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    if status != ERROR_SUCCESS {
        return Err(format!("读取 Run 值失败: {}", status.0));
    }
    let units: Vec<u16> = buf
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&c| c != 0)
        .collect();
    Ok(Some(String::from_utf16_lossy(&units)))
}

/// 设置/清除开机自启。只操作用户级 HKCU，不需要管理员权限。
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let hkey = open_run(KEY_QUERY_VALUE | KEY_SET_VALUE)?;
    let result = if enabled {
        let exe = current_exe()?;
        let command = command_line_for(&exe);
        let data: Vec<u8> = command
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        let status = unsafe {
            RegSetValueExW(
                hkey,
                PCWSTR(wide(VALUE_NAME).as_ptr()),
                None,
                REG_SZ,
                Some(&data),
            )
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("写入 Run 值失败: {}", status.0))
        }
    } else {
        let status = unsafe { RegDeleteValueW(hkey, PCWSTR(wide(VALUE_NAME).as_ptr())) };
        if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!("删除 Run 值失败: {}", status.0))
        }
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    result
}

/// 启动对账：把安装器写入的注册项与设置同步，并确保已勾选时注册项指向当前 exe。
pub fn reconcile(settings: &mut sylva_core::config::AppSettings) -> bool {
    let registered = match registered_command() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("读取开机自启注册项失败: {e}");
            return false;
        }
    };
    let current = match current_exe() {
        Ok(path) => command_line_for(&path),
        Err(e) => {
            tracing::warn!("{e}");
            return false;
        }
    };
    let mut changed = false;
    let matches_current = registered
        .as_deref()
        .map(|cmd| cmd.eq_ignore_ascii_case(&current))
        .unwrap_or(false);
    if !settings.autostart && matches_current {
        // 安装器勾选过：首次启动采纳为已启用状态。
        settings.autostart = true;
        changed = true;
        tracing::info!("已从安装器注册项采纳开机自启设置");
    }
    if settings.autostart && !matches_current {
        match set_enabled(true) {
            Ok(()) => tracing::info!("开机自启注册项已对齐当前 exe"),
            Err(e) => tracing::warn!("开机自启注册失败（设置保持开启）: {e}"),
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_quotes_path() {
        assert_eq!(
            command_line_for(Path::new(r"C:\Program Files\Sylva\sylva.exe")),
            "\"C:\\Program Files\\Sylva\\sylva.exe\""
        );
    }

    #[test]
    fn run_key_is_user_level() {
        assert!(RUN_KEY.starts_with("Software\\Microsoft\\Windows\\CurrentVersion\\Run"));
        assert_eq!(VALUE_NAME, "Sylva");
    }

    #[test]
    fn win32_error_success_is_zero() {
        assert_eq!(ERROR_SUCCESS.0, 0);
        assert_ne!(ERROR_FILE_NOT_FOUND.0, 0);
    }
}
