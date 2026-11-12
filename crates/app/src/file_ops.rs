//! 文件操作：添加路径、库同步、链接文件夹镜像、库内复制、剪贴板文件读取。

use std::collections::HashSet;

use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM, GetFileAttributesW,
};

use crate::*;
pub(crate) fn new_path_for_rename(old_path: &str, new_name: &str) -> Option<PathBuf> {
    let p = Path::new(old_path);
    let parent = p.parent()?;
    let old_file = p.file_name()?.to_string_lossy().into_owned();
    let lower = old_file.to_ascii_lowercase();
    let stripped_ext = ["lnk", "url", "appref-ms"].iter().copied().find(|ext| {
        let dot = format!(".{ext}");
        lower.ends_with(&dot) && old_file.len() > ext.len() + 1
    });
    let final_name = match stripped_ext {
        Some(ext) if !new_name.to_ascii_lowercase().ends_with(&format!(".{ext}")) => {
            format!("{new_name}.{ext}")
        }
        _ => new_name.to_string(),
    };
    if final_name.is_empty() {
        return None;
    }
    Some(parent.join(final_name))
}

/// 把任意文件/文件夹/快捷方式路径加入指定栅栏（拖入 / 粘贴共用）。
///
/// 目标目录 = 栅栏的链接文件夹（`storage_path`，有则用）否则内部库。链接栅栏由此实现
/// 「栅栏 → 文件夹」方向：拖入/粘贴的文件落进文件夹而非内部库。源已在目标目录内
/// （幂等粘贴/跨栅栏移动）直接复用。位图随后在 `handle_event` 末尾随场景上传。
pub(crate) fn add_paths_to_fence(rt: &mut Runtime, fence: usize, paths: &[String]) {
    if rt
        .desk
        .fences
        .get(fence)
        .map(|f| f.kind.is_read_only())
        .unwrap_or(false)
    {
        tracing::warn!(fence, "只读镜像栅栏（收件箱/文件夹门户）拒绝拖入");
        return;
    }
    for src in paths {
        let src_path = Path::new(src);
        // 引用语义（AGENTS §4 / 需求 §2.2）：只登记原路径，不复制、不移动磁盘文件。
        if !src_path.exists() {
            tracing::warn!(src, "拖入路径不存在，跳过");
            continue;
        }
        // 用户主动重新加入曾被抑制的路径：解除抑制，允许镜像同步恢复它。
        let key = src.to_ascii_lowercase();
        if let Some(f) = rt.desk.fences.get_mut(fence) {
            f.suppressed_paths.retain(|p| p != &key);
        }
        register_fence_item(rt, fence, src);
        suppress_in_other_mirrors(rt, fence, src);
    }
}

/// 从资源管理器/桌面拖入普通栅栏时，同一路径若已被桌面收件箱或文件夹门户镜像，
/// 在那些镜像栅栏里抑制它，避免同一文件同时出现在收件箱和分类栅栏。
fn suppress_in_other_mirrors(rt: &mut Runtime, target_fence: usize, path: &str) {
    let key = path.to_ascii_lowercase();
    let ids: Vec<String> = rt
        .desk
        .icons
        .iter()
        .filter(|(_, ic)| {
            ic.path
                .as_deref()
                .map(|p| p.eq_ignore_ascii_case(path))
                .unwrap_or(false)
        })
        .map(|(id, _)| id.clone())
        .collect();
    let public_desktop = crate::scene::shell_public_desktop_path().map(PathBuf::from);
    for (i, fence) in rt.desk.fences.iter_mut().enumerate() {
        if i == target_fence || !fence.kind.is_mirrored() {
            continue;
        }
        let in_dir = fence
            .storage_path
            .as_deref()
            .map(|dir| {
                path.eq_ignore_ascii_case(dir) || path_within(Path::new(dir), Path::new(path))
            })
            .unwrap_or(false)
            || (fence.kind == FenceKind::DesktopInbox
                && public_desktop
                    .as_ref()
                    .map(|dir| path_within(dir, Path::new(path)))
                    .unwrap_or(false));
        if !in_dir {
            continue;
        }
        if !fence.suppressed_paths.iter().any(|p| p == &key) {
            fence.suppressed_paths.push(key.clone());
        }
        fence.icon_ids.retain(|id| !ids.contains(id));
    }
}

/// 把一个已落在目标目录内的路径注册为栅栏图标：建 `DesktopItem`、录入元数据、
/// 分配位图槽、追加成员、提取图标（进 `pending_uploads`）。已在本栅栏则幂等跳过。
/// 拖入/粘贴（`add_paths_to_fence`）与链接文件夹镜像（`mirror_linked_fence`）共用。
pub(crate) fn register_fence_item(rt: &mut Runtime, fence: usize, path: &str) -> bool {
    let item = match sylva_shell::items::item_from_path(path) {
        Ok(it) => it,
        Err(e) => {
            tracing::warn!(path, "无法创建图标项: {e}");
            return false;
        }
    };
    let id = item.id.clone();
    // 已在本栅栏则跳过（防止同栅栏内重复）；在其它栅栏/自由区允许再添加——
    // 「复制→粘贴到另一个栅栏」就是让同一项出现在两个栅栏里（跨栅栏粘贴失效根因）。
    // 除同 id 外再按「同路径（小写）」判重：旧版「更改位置」迁移后 id 可能未随路径
    // 重建（陈旧 id），避免文件夹镜像把同一文件重复加进来。
    let already = rt
        .desk
        .fences
        .get(fence)
        .map(|f| {
            f.icon_ids.iter().any(|existing_id| {
                if *existing_id == id {
                    return true;
                }
                rt.desk
                    .icons
                    .get(existing_id)
                    .and_then(|ic| ic.path.clone())
                    .map(|p| p.eq_ignore_ascii_case(path))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    if already {
        // 同路径项此前可能因文件暂时不可用被标记为失效；路径恢复后清除标记。
        for ic in rt.desk.icons.values_mut() {
            if ic
                .path
                .as_deref()
                .map(|p| p.eq_ignore_ascii_case(path))
                .unwrap_or(false)
            {
                ic.invalid = false;
            }
        }
        return false;
    }
    // 录入元数据：持久化原路径引用（重启恢复图标/打开）；origin=Collected 表示收集项
    let mut ic = Icon::new(id.clone(), item.display_name.clone(), item.kind);
    ic.path = Some(path.to_string());
    ic.mark_collected();
    sylva_core::details::enrich(&mut ic, path);
    rt.desk.icons.insert(id.clone(), ic);
    let new_bitmap = rt.items.len() as u64;
    rt.items.push(item);
    rt.item_index.insert(id.clone(), rt.items.len() - 1);
    rt.bitmap_ids.insert(id.clone(), new_bitmap);
    if let Some(f) = rt.desk.fences.get_mut(fence) {
        f.icon_ids.push(id.clone());
    }
    // 提取图标（阻塞但命中系统图标缓存，通常很快）；失败不阻断添加
    let idx = rt.item_index[&id];
    match sylva_shell::icons::extract_icon(&rt.items[idx], ICON_EXTRACT_SIZE) {
        Ok(data) => rt.pending_uploads.push((new_bitmap, data)),
        Err(e) => tracing::warn!(path, "图标提取失败: {e}"),
    }
    true
}

/// 更改栅栏的链接目录：只更新引用关系，不移动任何磁盘文件（AGENTS §4）。
/// 旧链接目录里的镜像项从栅栏移除引用、文件保留；新目录内容由 reconcile_fences
/// 在后续同步周期镜像进来。
pub(crate) fn change_fence_storage(rt: &mut Runtime, fence_idx: usize, new_dir: &str) {
    let new_path = std::path::Path::new(new_dir);
    if !new_path.is_dir() {
        tracing::warn!(new_dir, "目标路径不是文件夹");
        return;
    }
    let Some(fence) = rt.desk.fences.get(fence_idx) else {
        return;
    };
    if fence.kind == sylva_core::model::FenceKind::DesktopInbox {
        tracing::warn!("桌面收件箱的目录由系统桌面决定，不能更改");
        return;
    }
    if fence.kind == sylva_core::model::FenceKind::Manual && !fence.icon_ids.is_empty() {
        tracing::warn!("普通栅栏已有成员；请先清空，或新建一个文件夹门户栅栏");
        return;
    }
    if let Err(e) = crate::snapshot::snapshot_before(rt, "更改链接目录") {
        tracing::warn!("更改链接目录前保存快照失败，取消操作: {e}");
        return;
    }
    let before = rt.desk.clone();
    let fence_id = fence.id;
    let old_dir = fence.storage_path.clone();
    if let Some(f) = rt.desk.fences.get_mut(fence_idx) {
        f.kind = sylva_core::model::FenceKind::FolderPortal;
        f.storage_path = Some(new_dir.to_string());
        f.suppressed_paths.clear();
    }
    // 旧链接目录的残留镜像项只摘引用，文件保留在磁盘。
    if let Some(old) = &old_dir {
        clear_stale_linked_items(rt, fence_idx, old, new_path);
    }
    if reconcile_fences(rt) {
        tracing::info!(fence = fence_id, "链接后新增了文件夹里的栅栏项");
    }
    crate::persist::commit_or_rollback(rt, before, "更改链接目录");
    tracing::info!(
        fence = fence_id,
        new_dir,
        "栅栏链接目录已更新（引用语义，未移动磁盘文件）"
    );
}
/// 换链接后清理旧链接文件夹里的残留镜像项：`added` 项路径在旧文件夹内、
/// 不在新文件夹内，说明它属于旧链接而非新链接。从栅栏整体摘下（引用删除、
/// 文件保留在旧文件夹磁盘上，用户仍可从资源管理器访问），使栅栏内容与
/// 新链接文件夹保持一致，不再残留旧文件夹的项。
fn clear_stale_linked_items(rt: &mut Runtime, fence_idx: usize, old_dir: &str, new_path: &Path) {
    let stale: Vec<String> = rt
        .desk
        .fences
        .get(fence_idx)
        .map(|f| {
            f.icon_ids
                .iter()
                .filter(|id| {
                    rt.desk
                        .icons
                        .get(*id)
                        .map(|ic| {
                            ic.is_collected()
                                && ic
                                    .path
                                    .as_ref()
                                    .map(|p| {
                                        path_within(Path::new(old_dir), Path::new(p))
                                            && !path_within(new_path, Path::new(p))
                                    })
                                    .unwrap_or(false)
                        })
                        .unwrap_or(false)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    for id in stale {
        tracing::info!(
            fence = fence_idx,
            id,
            "换链接：旧文件夹残留项移出栅栏（文件保留在磁盘）"
        );
        remove_icon_entirely(rt, &id);
    }
}

/// 引用同步：检查收集项路径是否仍可用，失效的标记为 `invalid`（不删除引用）。
/// 需求 §2.4：失效引用降级显示，由用户决定是否移除；不得导致渲染崩溃。
/// 返回是否有项的 invalid 状态发生变化（调用方可据此持久化）。
pub(crate) fn reconcile_invalid_refs(rt: &mut Runtime) -> bool {
    let mut changed = 0usize;
    for ic in rt.desk.icons.values_mut() {
        if !ic.is_collected() {
            continue;
        }
        let Some(path) = ic.path.as_deref() else {
            continue;
        };
        let exists = Path::new(path).exists();
        if ic.invalid == exists {
            ic.invalid = !exists;
            changed += 1;
        }
    }
    if changed > 0 {
        tracing::info!(changed, "收集项失效状态已同步");
    }
    changed > 0
}

/// 双向同步总入口：收集项失效标记 + 链接栅栏文件夹镜像。
/// 返回是否有任何项被增/删（调用方据此持久化；重绘由事件尾部统一完成）。
pub(crate) fn reconcile_fences(rt: &mut Runtime) -> bool {
    let mut changed = reconcile_invalid_refs(rt);
    for idx in 0..rt.desk.fences.len() {
        if mirror_linked_fence(rt, idx) {
            changed = true;
        }
    }
    changed
}

/// 镜像一个链接栅栏的存储文件夹（文件夹 → 栅栏方向）：资源管理器里对文件夹的
/// 新增/删除/改名 ≤ 后台 `SyncLibrary` 周期（4s）反映到栅栏。栅栏即文件夹——
/// 目录内容与栅栏成员互为差集：多出的路径注册进栅栏，消失的路径移除对应图标。
///
/// 廉价快路径：枚举目录名集合与栅栏内「路径在该目录下」的图标路径集合做比较，
/// 集合相同立即返回——平时每周期只做一次 `read_dir`，低 CPU；仅差集非空才做
/// 昂贵的 `DesktopItem` 构建/移除。文件夹不可用（被删/断连）时不动（防御）。
/// 栅栏的镜像根目录：文件夹门户为 storage_path；桌面收件箱额外包含公共桌面。
/// Windows 桌面实际显示“用户桌面 + 公共桌面”的合并内容，只读前者会漏快捷方式。
fn fence_mirror_roots(_rt: &Runtime, fence: &Fence) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(path) = &fence.storage_path {
        roots.push(PathBuf::from(path));
    }
    if fence.kind == FenceKind::DesktopInbox
        && let Some(public) = crate::scene::shell_public_desktop_path()
    {
        let public = PathBuf::from(public);
        let duplicate = roots.iter().any(|r| {
            r.to_string_lossy()
                .eq_ignore_ascii_case(&public.to_string_lossy())
        });
        if !duplicate {
            roots.push(public);
        }
    }
    roots
}

fn path_under_roots(roots: &[PathBuf], path: &str) -> bool {
    let path = Path::new(path);
    roots.iter().any(|root| path_within(root, path))
}

fn mirror_linked_fence(rt: &mut Runtime, idx: usize) -> bool {
    let Some(fence) = rt.desk.fences.get(idx).cloned() else {
        return false;
    };
    if !fence.kind.is_mirrored() {
        return false;
    }
    let roots = fence_mirror_roots(rt, &fence);
    if roots.is_empty() || !roots.iter().any(|r| r.is_dir()) {
        return false;
    }
    // 用户主动移除的路径（抑制列表）：镜像时跳过，避免 4s 同步又加回来。
    let suppressed: HashSet<String> = fence.suppressed_paths.iter().cloned().collect();
    // 栅栏内「路径在该目录下」的图标路径集合（小写，与磁盘大小写无关）
    let existing: HashSet<String> = fence
        .icon_ids
        .iter()
        .filter_map(|id| rt.desk.icons.get(id).and_then(|ic| ic.path.clone()))
        .filter(|p| path_under_roots(&roots, p))
        .map(|p| p.to_ascii_lowercase())
        .collect();
    // 枚举所有存在的镜像根（跳过隐藏/系统文件，与资源管理器默认一致）。
    let entries: Vec<PathBuf> = roots
        .iter()
        .filter(|root| root.is_dir())
        .flat_map(|root| {
            std::fs::read_dir(root)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
        })
        .filter(|p| should_mirror(p))
        .filter(|p| !suppressed.contains(&p.to_string_lossy().to_ascii_lowercase()))
        .collect();
    let entry_set: HashSet<String> = entries
        .iter()
        .map(|p| p.to_string_lossy().to_ascii_lowercase())
        .collect();
    let mut changed = entry_set != existing;
    // 文件夹 → 栅栏：新出现的文件/子文件夹注册进栅栏（含改名产生的新路径）
    for p in &entries {
        let lower = p.to_string_lossy().to_ascii_lowercase();
        if !existing.contains(&lower) && register_fence_item(rt, idx, &p.to_string_lossy()) {
            changed = true;
        }
    }
    // 栅栏 → 文件夹：路径在该目录下但文件已不存在 → 标记失效引用（不删引用）。
    let ids: Vec<String> = rt
        .desk
        .fences
        .get(idx)
        .map(|f| f.icon_ids.clone())
        .unwrap_or_default();
    for id in ids {
        let Some(p) = rt.desk.icons.get(&id).and_then(|ic| ic.path.clone()) else {
            continue;
        };
        if !path_under_roots(&roots, &p) {
            continue;
        }
        if !Path::new(&p).exists()
            && let Some(ic) = rt.desk.icons.get_mut(&id)
            && !ic.invalid
        {
            ic.invalid = true;
            changed = true;
        }
    }
    changed
}

/// 该路径是否应在栅栏镜像中显示：跳过隐藏/系统文件（与资源管理器默认一致，
/// 免 `desktop.ini`、缩略图缓存等混入）。读取失败（被占用/已删）视为不显示。
fn should_mirror(path: &Path) -> bool {
    let w = crate::context_menu::wide(&path.to_string_lossy());
    let attr = unsafe { GetFileAttributesW(PCWSTR(w.as_ptr())) };
    if attr == u32::MAX {
        return false;
    }
    attr & (FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0) == 0
}

/// `path` 是否**严格位于** `root` 目录内（组件级大小写不敏感前缀比较）。
/// 组件边界保证 `C:\lib` 不会匹配 `C:\library\f.txt`；路径即根本身不算「在内」。
pub(crate) fn path_within(root: &Path, path: &Path) -> bool {
    let root_c: Vec<String> = root
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    if root_c.is_empty() {
        return false;
    }
    let comps: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    comps.len() > root_c.len() && root_c.iter().zip(comps.iter()).all(|(a, b)| a == b)
}

/// 路径是否位于某个栅栏的链接存储文件夹内（「移出栅栏 = 删除文件」判定用）。
pub(crate) fn is_linked_path(rt: &Runtime, path: &str) -> bool {
    let p = Path::new(path);
    rt.desk.fences.iter().any(|f| {
        fence_mirror_roots(rt, f)
            .iter()
            .any(|root| path_within(root, p))
    })
}

/// 把一个图标从 Sylva 模型中整体移除（仅删引用，绝不碰磁盘文件）。
/// 调用方负责先把链接路径写入抑制列表，避免文件夹镜像同步重新加回。
pub(crate) fn remove_icon_entirely(rt: &mut Runtime, id: &str) {
    rt.desk.icons.remove(id);
    rt.desk.free_icons.retain(|x| x != id);
    for f in &mut rt.desk.fences {
        f.icon_ids.retain(|x| x != id);
    }
    rt.bitmap_ids.remove(id);
    // 从 items 池移除对应 DesktopItem（持有 PIDL，Drop 时释放），并重建下标
    if let Some(i) = rt.items.iter().position(|it| it.id == *id) {
        rt.items.remove(i);
    }
    rt.item_index = rt
        .items
        .iter()
        .enumerate()
        .map(|(i, it)| (it.id.clone(), i))
        .collect();
}

/// 剪贴板格式：CF_HDROP（拖放文件列表）。windows-rs 0.62 将其定义在
/// `Win32_System_Ole` 里；这里用文档稳定值 15，避免引入整个 Ole 功能集。
pub(crate) const CF_HDROP: u32 = 15;

/// 读剪贴板里的文件列表（CF_HDROP）。
pub(crate) fn clipboard_file_paths() -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        if OpenClipboard(None).is_err() {
            return out;
        }
        if let Ok(handle) = GetClipboardData(CF_HDROP)
            && !handle.is_invalid()
        {
            let hdrop = HDROP(handle.0);
            let n = DragQueryFileW(hdrop, u32::MAX, None);
            for i in 0..n {
                let len = DragQueryFileW(hdrop, i, None);
                let mut buf = vec![0u16; len as usize + 1];
                DragQueryFileW(hdrop, i, Some(&mut buf));
                let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                out.push(String::from_utf16_lossy(&buf[..end]));
            }
        }
        let _ = CloseClipboard();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_within_component_aware_case_insensitive() {
        assert!(path_within(
            Path::new(r"C:\lib"),
            Path::new(r"C:\lib\a.txt")
        ));
        assert!(path_within(
            Path::new(r"C:\Lib"),
            Path::new(r"c:\lib\sub\b.txt")
        ));
        // 组件边界：C:\lib 不匹配 C:\library\f.txt
        assert!(!path_within(
            Path::new(r"C:\lib"),
            Path::new(r"C:\library\f.txt")
        ));
        // 路径即根本身：不算「在内」（严格在内）
        assert!(!path_within(Path::new(r"C:\lib"), Path::new(r"C:\lib")));
        // 父级不是子的前缀
        assert!(!path_within(Path::new(r"C:\lib\a"), Path::new(r"C:\lib")));
    }

    #[test]
    fn path_within_empty_root_is_never_within() {
        assert!(!path_within(Path::new(""), Path::new(r"C:\x\y.txt")));
    }
}
