//! OLE 拖放：`IDropTarget`（拖入）与 `IDropSource` + `SHDoDragDrop`（拖出）。
//!
//! 需求 §4.2：
//! - `DragEnter` / `DragOver` 必须检查 `CF_HDROP`，不支持的类型返回 `DROPEFFECT_NONE`；
//! - 效果按目标栅栏与修饰键决定，虚拟栅栏只加引用，返回 `LINK`，绝不返回 `MOVE`
//!   （否则资源管理器可能删除源文件）；
//! - `Drop` 先校验路径，再把结构化结果交给 App。
//!
//! 单个 overlay 窗口注册一个 `IDropTarget`，按 App 同步过来的栅栏矩形做命中分发。

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, POINTL, S_OK,
};
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE, IDropSource,
    IDropSource_Impl, IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium,
    RevokeDragDrop,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, DragQueryFileW, HDROP, SHCreateShellItemArrayFromIDLists, SHDoDragDrop,
    SHParseDisplayName,
};
use windows::core::{BOOL, HRESULT, PCWSTR, Ref, Result, implement};

use sylva_core::drag::DropEffect;
use sylva_core::model::Rect;

/// `CF_HDROP` 的标准剪贴板格式号。
const CF_HDROP: u16 = 15;

/// 拖放事件（App 回调）。
#[derive(Debug, Clone)]
pub enum DropEvent {
    Enter {
        fence: Option<usize>,
        effect: DropEffect,
    },
    Over {
        fence: Option<usize>,
        effect: DropEffect,
    },
    Leave,
    Drop {
        fence: Option<usize>,
        paths: Vec<String>,
        effect: DropEffect,
    },
}

type DropCallback = Box<dyn FnMut(DropEvent) -> DropEffect>;

#[derive(Default)]
struct DropState {
    /// 栅栏 id 与其屏幕坐标矩形（App 在每次命中模型更新后同步）。
    fences: Vec<(usize, Rect)>,
    callback: Option<DropCallback>,
    /// 当前 OLE 拖放是否为 CF_HDROP（DragOver 没有 IDataObject，需记住 DragEnter 的判定）。
    accepting: bool,
}

/// 注册后返回的句柄：用于同步栅栏矩形、设置回调；Drop 时自动撤销注册。
pub struct DropHandle {
    hwnd: HWND,
    state: Rc<RefCell<DropState>>,
}

impl DropHandle {
    pub fn set_fences(&self, fences: Vec<(usize, Rect)>) {
        self.state.borrow_mut().fences = fences;
    }

    pub fn set_callback(&self, callback: DropCallback) {
        self.state.borrow_mut().callback = Some(callback);
    }
}

impl Drop for DropHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = RevokeDragDrop(self.hwnd);
        }
    }
}

#[implement(IDropTarget)]
struct DropTarget {
    state: Rc<RefCell<DropState>>,
}

impl DropTarget {
    fn normalize_effect(effect: DropEffect) -> DropEffect {
        if effect == DropEffect::Move {
            DropEffect::Link
        } else {
            effect
        }
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        pdataobj: Ref<IDataObject>,
        grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> Result<()> {
        let has_files = pdataobj.as_ref().map(data_has_hdrop).unwrap_or(false);
        let mut state = self.state.borrow_mut();
        state.accepting = has_files;
        let fence = if has_files {
            fence_at(&state.fences, pt)
        } else {
            None
        };
        let effect = if has_files {
            state
                .callback
                .as_mut()
                .map(|callback| {
                    callback(DropEvent::Enter {
                        fence,
                        effect: DropEffect::Link,
                    })
                })
                .unwrap_or(DropEffect::None)
        } else {
            DropEffect::None
        };
        let effect = DropTarget::normalize_effect(effect);
        unsafe {
            *pdweffect = to_ole(effect);
        }
        let _ = grfkeystate;
        Ok(())
    }

    fn DragOver(
        &self,
        grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> Result<()> {
        let mut state = self.state.borrow_mut();
        let has_files = state.accepting;
        let fence = if has_files {
            fence_at(&state.fences, pt)
        } else {
            None
        };
        let effect = if has_files {
            state
                .callback
                .as_mut()
                .map(|callback| {
                    callback(DropEvent::Over {
                        fence,
                        effect: DropEffect::Link,
                    })
                })
                .unwrap_or(DropEffect::None)
        } else {
            DropEffect::None
        };
        let effect = DropTarget::normalize_effect(effect);
        unsafe {
            *pdweffect = to_ole(effect);
        }
        let _ = grfkeystate;
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        let mut state = self.state.borrow_mut();
        state.accepting = false;
        if let Some(callback) = state.callback.as_mut() {
            callback(DropEvent::Leave);
        }
        Ok(())
    }

    fn Drop(
        &self,
        pdataobj: Ref<IDataObject>,
        grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> Result<()> {
        let has_files = pdataobj.as_ref().map(data_has_hdrop).unwrap_or(false);
        let mut state = self.state.borrow_mut();
        state.accepting = false;
        let fence = if has_files {
            fence_at(&state.fences, pt)
        } else {
            None
        };
        let paths = if has_files {
            pdataobj
                .as_ref()
                .map(extract_paths)
                .unwrap_or_default()
                .into_iter()
                .filter(|p| !is_protected_path(p))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let effect = if has_files && !paths.is_empty() {
            state
                .callback
                .as_mut()
                .map(|callback| {
                    callback(DropEvent::Drop {
                        fence,
                        paths,
                        effect: DropEffect::Link,
                    })
                })
                .unwrap_or(DropEffect::None)
        } else {
            DropEffect::None
        };
        let effect = DropTarget::normalize_effect(effect);
        unsafe {
            *pdweffect = to_ole(effect);
        }
        let _ = grfkeystate;
        Ok(())
    }
}
/// 在 overlay 窗口上注册拖放目标。返回句柄需与窗口同生命周期。
pub fn register(hwnd: HWND) -> Result<DropHandle> {
    let state = Rc::new(RefCell::new(DropState::default()));
    let target: IDropTarget = DropTarget {
        state: state.clone(),
    }
    .into();
    unsafe {
        RegisterDragDrop(hwnd, &target)?;
    }
    Ok(DropHandle { hwnd, state })
}

/// 从栅栏把文件拖出到桌面/资源管理器：系统 `CF_HDROP` 数据对象 + 默认拖放源。
pub fn start_drag(hwnd: HWND, paths: &[String]) -> Result<DropEffect> {
    start_drag_with_effect(
        hwnd,
        paths,
        DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK,
    )
}

/// 只允许复制：桌面收件箱是只读镜像，拖到桌面/资源管理器不得移动或删除源文件。
pub fn start_drag_copy(hwnd: HWND, paths: &[String]) -> Result<DropEffect> {
    start_drag_with_effect(hwnd, paths, DROPEFFECT_COPY)
}

fn start_drag_with_effect(hwnd: HWND, paths: &[String], allowed: DROPEFFECT) -> Result<DropEffect> {
    let paths: Vec<String> = paths
        .iter()
        .filter(|p| !p.is_empty() && !is_protected_path(p) && Path::new(p).exists())
        .cloned()
        .collect();
    if paths.is_empty() {
        return Ok(DropEffect::None);
    }
    let data = data_object_for_paths(&paths)?;
    let source: IDropSource = DragSource.into();
    let effect = unsafe { SHDoDragDrop(Some(hwnd), &data, &source, allowed)? };
    Ok(from_ole(effect))
}

fn data_object_for_paths(paths: &[String]) -> Result<IDataObject> {
    let mut pidls: Vec<*const ITEMIDLIST> = Vec::with_capacity(paths.len());
    for path in paths {
        let name = wide(path);
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        unsafe {
            SHParseDisplayName(PCWSTR(name.as_ptr()), None, &mut pidl, 0, None)?;
        }
        if pidl.is_null() {
            continue;
        }
        pidls.push(pidl);
    }
    if pidls.is_empty() {
        return Err(windows::core::Error::from_hresult(HRESULT(
            0x8000_4005_u32 as i32,
        )));
    }
    let array = unsafe { SHCreateShellItemArrayFromIDLists(&pidls)? };
    let data: IDataObject = unsafe { array.BindToHandler(None, &BHID_DataObject)? };
    for pidl in pidls {
        unsafe {
            windows::Win32::System::Com::CoTaskMemFree(Some(pidl as *const core::ffi::c_void));
        }
    }
    Ok(data)
}

#[implement(IDropSource)]
struct DragSource;

impl IDropSource_Impl for DragSource_Impl {
    fn QueryContinueDrag(&self, fescapepressed: BOOL, grfkeystate: MODIFIERKEYS_FLAGS) -> HRESULT {
        if fescapepressed.as_bool() {
            DRAGDROP_S_CANCEL
        } else if grfkeystate.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

fn fence_at(fences: &[(usize, Rect)], pt: &POINTL) -> Option<usize> {
    let p = sylva_core::model::Vec2 {
        x: pt.x as f32,
        y: pt.y as f32,
    };
    fences
        .iter()
        .find(|(_, rect)| rect.contains(p))
        .map(|(id, _)| *id)
}

fn data_has_hdrop(data: &IDataObject) -> bool {
    let format = hdrop_format();
    unsafe { data.QueryGetData(&format).is_ok() }
}

fn extract_paths(data: &IDataObject) -> Vec<String> {
    let format = hdrop_format();
    let Ok(mut medium) = (unsafe { data.GetData(&format) }) else {
        return Vec::new();
    };
    let hdrop = HDROP(unsafe { medium.u.hGlobal }.0);
    let mut out = Vec::new();
    unsafe {
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        for i in 0..count {
            let len = DragQueryFileW(hdrop, i, None);
            let mut buf = vec![0u16; len as usize + 1];
            DragQueryFileW(hdrop, i, Some(&mut buf));
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            out.push(String::from_utf16_lossy(&buf[..end]));
        }
        ReleaseStgMedium(&mut medium);
    }
    out
}

fn hdrop_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn to_ole(effect: DropEffect) -> DROPEFFECT {
    match effect {
        DropEffect::None => DROPEFFECT_NONE,
        DropEffect::Copy => DROPEFFECT_COPY,
        DropEffect::Move => DROPEFFECT_MOVE,
        DropEffect::Link => DROPEFFECT_LINK,
    }
}

fn from_ole(effect: DROPEFFECT) -> DropEffect {
    if effect.0 & DROPEFFECT_COPY.0 != 0 {
        DropEffect::Copy
    } else if effect.0 & DROPEFFECT_MOVE.0 != 0 {
        DropEffect::Move
    } else if effect.0 & DROPEFFECT_LINK.0 != 0 {
        DropEffect::Link
    } else {
        DropEffect::None
    }
}

/// 系统保护路径 / 驱动器根：拒绝作为拖入/拖出目标，避免误操作。
fn is_protected_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.len() > 32767 {
        return true;
    }
    let p = Path::new(&lower);
    if p.parent().is_none() {
        return true;
    }
    let mut roots = vec![
        std::env::var("SystemRoot")
            .ok()
            .map(|v| v.to_ascii_lowercase()),
        std::env::var("ProgramFiles")
            .ok()
            .map(|v| v.to_ascii_lowercase()),
        std::env::var("ProgramFiles(x86)")
            .ok()
            .map(|v| v.to_ascii_lowercase()),
        std::env::var("ProgramData")
            .ok()
            .map(|v| v.to_ascii_lowercase()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    roots.retain(|r| !r.is_empty());
    roots.iter().any(|root| {
        lower == *root
            || lower
                .strip_prefix(root)
                .map(|rest| rest.starts_with('\\') || rest.starts_with('/'))
                .unwrap_or(false)
    })
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_paths_are_rejected() {
        assert!(is_protected_path("C:\\"));
        let windir = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        assert!(is_protected_path(&format!("{windir}\\notepad.exe")));
        assert!(!is_protected_path("C:\\Users\\alice\\Desktop\\a.txt"));
    }

    #[test]
    fn drop_effect_roundtrip() {
        for effect in [
            DropEffect::None,
            DropEffect::Copy,
            DropEffect::Move,
            DropEffect::Link,
        ] {
            assert_eq!(from_ole(to_ole(effect)), effect);
        }
    }
}
