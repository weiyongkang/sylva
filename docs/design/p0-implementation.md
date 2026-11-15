# P0 实施记录（P0-00 + P0-01～P0-06）

> **状态**：已实现，自动测试通过；Windows 10/11 人工验收待执行。
> **决策**：D1 引用语义按文档 A（只引用、不复制、不删盘）；D2 单 overlay + 统一 `IDropTarget`；
> D3 崩溃直接恢复（独立恢复守卫进程）；D4 本轮范围 P0-00 + P0-01～P0-05。
> **关联**：[AGENTS.md](../../AGENTS.md) · [需求文档](../requirements/desktop-collection.md) · [Backlog](../roadmap/backlog.md)

---

## 1. 本轮结论

| 项 | 实现 |
|---|---|
| 引用语义 | 拖入只登记原路径；删除只移除引用；链接目录用抑制列表防止 4s 同步回加 |
| 失效引用 | `Icon.invalid` 标记、降透明度显示、右键“移除引用”，不自动删引用 |
| 拖入 | 单 overlay 注册 OLE `IDropTarget`，按栅栏命中矩形分发；`DragOver` 按数据/目标返回效果 |
| 拖出 | `IDropSource` + `SHDoDragDrop`，系统 `CF_HDROP`，支持单/多选、网格/列表/侧边栏 |
| 撤销 | `Desk.undo` 持久化队列；删除/移出/删栅栏可撤销，批量删一条记录 |
| 快照 | 独立 JSON 目录 / redb 表；结构性变更前自动保存，保留最近 10 份；启动损坏回退 |
| Quick hide | `Ctrl+Alt+H` / 托盘；空场景 + 空窗口区域，鼠标完全穿透 |
| 开机自启 | HKCU Run 键 + 设置对账；安装器写入的项会被采纳，不误删 |
| 崩溃恢复 | 同二进制 `--recovery-guard <pid>` 独立守卫进程 + `data/recovery.marker` 兜底 |

---

## 2. 关键设计

### 2.1 引用语义与链接目录

- `add_paths_to_fence` 不再复制文件，直接登记原路径。
- 删除/移出只改 `Desk`；绝不调用 `DeleteFile` / `RemoveDirectory`。
- 链接文件夹栅栏的成员来自目录镜像；用户主动移除时把路径写入
  `Fence.suppressed_paths`（小写），镜像同步跳过这些路径；撤销时删除抑制项。
- 旧 `data/library` 中的项保留原路径，不再新增复制、不再自动删盘；作为普通收集项显示。

### 2.2 失效引用

- `Icon.invalid = true`：路径不存在/不可访问；渲染降透明度，双击不打开。
- 同步周期只更新 `invalid`，不删除引用；用户可通过右键“移除引用”清理。
- 路径恢复后 `register_fence_item` / 同步逻辑清除 `invalid`。

### 2.3 OLE 拖放

**拖入**

- 单 overlay 窗口注册一个 `IDropTarget`（`sylva_shell::drag::register`）。
- App 每次命中模型更新后，把栅栏矩形（屏幕坐标 = 虚拟坐标 + overlay 原点）同步给
  `DropHandle::set_fences`。
- `DragEnter` / `Drop` 检查 `CF_HDROP`；不支持的类型返回 `DROPEFFECT_NONE`。
- 虚拟栅栏只加引用，默认返回 `DROPEFFECT_LINK`；**任何情况下都不返回 `MOVE`**，
  避免资源管理器删除源文件。链接目录栅栏也按引用处理。
- 悬停：shell 发出 `DropEvent`，App 通过注入的 `DragHover` 更新
  `Runtime.drop_hover`，绘制强调色描边；`DragLeave` / `Drop` 清除。
- 路径校验：存在、长度、系统保护路径（Windows/Program Files/ProgramData/驱动器根）。

**拖出**

- `start_drag` 用 `SHParseDisplayName` + `SHCreateShellItemArrayFromIDLists` +
  `BHID_DataObject` 生成系统 `IDataObject`，用 `IDropSource` + `SHDoDragDrop`。
- 网格/列表：图标按下后位移超过 6px → `IconDragOut`；不再移动栅栏。
- 侧边栏：仍在 dock 内 → 排序；移出 dock 24px 活动区 → 拖出。
- 拖出前先呈现一帧“源图标降透明度”状态；返回后清除状态并重新校验引用。
- 支持 `Esc` 取消（`IDropSource::QueryContinueDrag`）。

### 2.4 崩溃恢复守卫

主进程启动时在隐藏图标**之前**：

1. 创建 `Local\Sylva.Recovery.Clean.<pid>` / `Ready.<pid>` 命名事件；
2. spawn 同 exe 的 `--recovery-guard <pid>`（`CREATE_NO_WINDOW`）；
3. 等待 ready 握手（最多 3s）；
4. 写 `data/recovery.marker`（temp → rename）；
5. `ShowWindow(SysListView32, SW_HIDE)`。

守卫进程：

- 等待“父进程句柄”或“clean 事件”；
- 父进程正常退出：收到 clean 事件，直接退出；
- 父进程崩溃/强杀：重新探测当前桌面层级并 `SW_SHOW` 真实图标，删除 marker；
- 启动时若 marker 仍存在（守卫未启动/系统先终止了守卫），先恢复再进入本次会话。

注销/关机：overlay 处理 `WM_QUERYENDSESSION`；`SetConsoleCtrlHandler` 处理
`CTRL_LOGOFF_EVENT` / `CTRL_SHUTDOWN_EVENT` 兜底恢复。

> 边界：硬杀后恢复依赖守卫进程被调度；若守卫也被同时杀死，则下次启动恢复。
> 这是“同二进制、无额外安装文件”方案能做到的最强保证。

### 2.5 撤销

- `Desk.undo: UndoStack`（容量 50，随主配置单事务保存）。
- `UndoAction::RestoreItems` 记录 `Icon` 元数据、各栅栏/未分组区原下标、抑制项；
  `RestoreFence` 额外记录栅栏本身与原下标。
- 恢复时先清除该 id 的所有归属，再按记录插回，避免“既在栅栏又在未分组区”。
- App 侧 `ensure_item` 重建 `DesktopItem` / 位图；失效路径只恢复引用。
- UI：托盘“撤销上一步删除”、`Ctrl+Z`（编辑框内不抢文本撤销）。

### 2.6 快照

- `Snapshot` 含 id、unix 时间、原因、完整 `Desk`（保存时清空 undo，避免嵌套）。
- `DeskStore` 新增 `save_snapshot / list_snapshots / load_snapshot / prune_snapshots`。
- JSON：`data/snapshots/<id>.json`，temp → rename；redb：独立 `snapshots` 表，单 write txn。
- 触发：新增/删除栅栏、批量删除/移出、更改链接目录、恢复快照前。
- 保留最近 10 份；启动主配置解析失败 → 最近快照回退并写回。
- 恢复快照前先保存当前状态，并把 `RestoreSnapshot { previous_id }` 压入撤销链。

### 2.7 多栅栏分类（空白栅栏 / 收件箱 / 门户）

- **FenceKind**：`Manual` / `DesktopInbox` / `FolderPortal`，随 `Fence` 持久化；
  `Desk.version=3`，旧 `storage_path` 栅栏迁移为 FolderPortal，app 再把镜像桌面目录的
  栅栏升级为唯一 DesktopInbox。
- **空白栅栏**：控制中心“添加栅栏”菜单选择“空白栅栏”；不绑定目录，成员由拖拽决定。
  栅栏模式下还可直接右键桌面空白处弹出同一菜单；原始桌面模式 / Quick hide 自动停用该钩子。
- **桌面收件箱**：只读镜像；启动时实时扫描，默认侧边栏；镜像“用户桌面 + 公共桌面”两个目录（与实际桌面一致），
  启动时统一 `reanchor_fences` 把越界栅栏夹回虚拟屏，避免旧布局让栅栏左侧被屏幕裁掉；
  用户从收件箱拖到分类栅栏时，
  源栅栏写入 `suppressed_paths`，目标栅栏增加引用，避免同一文件重复显示。
  启动时若已有其它栅栏但没有收件箱，会自动补一个；也可在“添加栅栏 → 桌面收件箱”手动创建。
- **跨栅栏拖放**：拖到另一个栅栏 = 分类；默认 Move，Ctrl = Copy；多选一条撤销记录；
  拖到所有栅栏之外 = 现有 OLE 拖出。门户目标在 OLE 与内部拖拽中都拒绝。
  为穿过栅栏间空隙，离开源栅栏后延迟 400ms 才启动 OLE；期间进入目标栅栏优先分类。
  按住 Shift 可立即拖出到桌面/资源管理器。
- **文件夹门户**：文件夹拖入普通栅栏时弹出“文件夹图标 / 链接为文件夹门户”；门户只读，
  成员由目录镜像决定，拖入被拒绝；`mirror_linked_fence` 只对 mirrored kind 生效。
- **右键菜单**：新建空白栅栏、更改文件夹、解除门户、设为桌面收件箱。

### 2.8 栅栏外观设置与控制中心主从布局

- **FenceAppearance 新增**：`show_title`、`show_labels`、`grid_cols`、`grid_rows`、`fill_opacity`；
  旧配置缺字段时默认显示标题/名称、自动网格、100% 填充。
- **标题/名称**：关闭标题后标题栏高度归零，空白区域拖动移动栅栏（无标题栏时不再框选）；关闭名称同时隐藏网格/列表标签和悬停工具提示。
- **网格行列**：固定列数受栅栏可用宽度钳制；固定可见行数用于自动高度栅栏，超出滚动。
- **透明度/颜色**：`fill_opacity` 使用 0–100 滑轨（支持按住拖拽，默认 0.8），作用于玻璃/纯色填充；
  系统颜色选择器（`ChooseColorW`）写入 `tint`，与预设色板共用同一字段。
- **网格选择器**：外观页的 8×8 个 32px 小格直接选择列/行，选中后 `resize_fence_to_grid`
  同步重算栅栏实际宽高。
- **全局叠加**：`AppSettings.allow_fence_overlap` 开启后移动/缩放跳过 `settle_move/resize`
  的栅栏间推挤，只保留屏幕/最小尺寸约束；系统控制页提供开关。
- **悬停裁剪修复**：内容裁剪按所有图标的最大放大扩张量计算，避免悬停退出动画期间
  `hover_icon` 已清空但 `scale>1` 的图标被左上裁剪。
- **控制中心**：改为主从结构：左侧导航「系统控制 + 栅栏列表 + 新建栅栏」，右侧显示系统功能
  或选中栅栏的布局/样式/色调/透明度/标题与名称/网格行列，避免长条单页。
  默认宽高各为主屏 1/3；系统页可重置；字体/控件按 DPI × 主屏面积缩放，显示变化时重算。
- **选中联动**：控制中心选中栅栏时实体栅栏绘制高对比边框；网格选中框按整列居中。
- **图标形状 / 悬停**：`icon_shape` 支持方角/圆角/圆形（D2D 几何裁剪）；`hover_effect`
  支持关闭/仅高亮/放大。
- **显示开关/占位**：栅栏可独立隐藏（alpha=0、保留成员）；网格/侧边栏的图标选中占位
  为围绕图标中心的正方形。内边距统一按 `pad` 计算，标题隐藏时不再额外多一份上下留白。
- **收件箱只读/复制**：`DesktopInbox` 与 `FolderPortal` 一起被 OLE 目标、内部分类目标、
  粘贴/删除入口拒绝；从收件箱拖出到资源管理器只允许 `DROPEFFECT_COPY`，拖到分类栅栏
  强制 `ClassifyMode::Copy`。
- **右键/重命名**：桌面右键始终保留 Windows 原菜单并在末尾追加常驻 Sylva 子菜单
  （含“切换栅栏/原始桌面”）；图标菜单同样追加 Sylva 子菜单；栅栏右键新增
  “在控制中心管理此栅栏”“选择全部项目”“隐藏/显示此栅栏”。控制中心详情标题右侧新增“重命名”。

### 2.9 Quick hide / 开机自启

- Quick hide：不持久化；`build_scene` 返回空场景，`HitModel` 为空，
  `apply_region` 设置空区域；隐藏时恢复真实桌面图标，恢复时按 `desktop_mode` 重新隐藏。
- 开机自启：HKCU `Software\Microsoft\Windows\CurrentVersion\Run\Sylva`；
  启动时若注册项指向当前 exe 且设置为 false → 采纳为 true（保留安装器选择）；
  设置为 true 但注册项缺失 → 重写；失败只记日志，不阻断启动。

---

## 3. 文件改动

| 层 | 文件 | 内容 |
|---|---|---|
| core | `model.rs` | `ItemOrigin`、`Icon.invalid`、`Fence.suppressed_paths`、`Fence.kind`、`Desk.undo` |
| core | `history.rs` | 撤销栈、恢复动作、容量 |
| core | `snapshot.rs` | Snapshot / SnapshotMeta |
| core | `storage.rs` | DeskStore 快照契约 + MemoryStore |
| core | `drag.rs` | `DropEffect` |
| shell | `drag.rs` | `IDropTarget` / `IDropSource` / `SHDoDragDrop` / 路径校验 |
| shell | `takeover.rs` | `restore_desktop_icons` |
| render | `overlay.rs` | 拖放事件、空区域、Quick hide 热键、IconDragOut |
| render | `scene.rs` / `draw.rs` | invalid/dragging 降透明度 |
| app | `guard.rs` | 恢复守卫进程 + 标记 |
| app | `history.rs` | 捕获/恢复/撤销接线 |
| app | `snapshot.rs` | 快照生成/启动回退/恢复 |
| app | `autostart.rs` | HKCU Run 对账 |
| app | `persist.rs` | save 失败内存回滚 |
| app | `store/json.rs`、`redb_store.rs` | 快照后端 + redb undo key |
| app | `main.rs` | 引用语义、拖放、Quick hide、撤销/快照 UI、启动恢复 |
| app | `shell_menu.rs` | 隐藏 Shell 原生“删除”动词，保留“移除引用” |
| core | `commands.rs` | `classify_move`：跨栅栏移动/复制、收件箱/门户抑制、目标门户拒绝 |
| app | `main.rs` / `context_menu.rs` | 空白栅栏、桌面收件箱、文件夹门户选择、跨栅栏分类拖放 |

---

## 4. 持久化变化

| 后端 | 新增 |
|---|---|
| JSON | `Desk.undo`；`data/snapshots/<id>.json` |
| redb | `undo` 键；`snapshots` 表；schema 2（旧库缺 `undo` 时默认空栈） |
| 兼容 | `Desk.version` 1→3；旧 `added=true` → `origin=Collected`；旧 `storage_path` → `FenceKind::FolderPortal`；旧栅栏缺 `suppressed_paths` 默认空 |

`DeskStore` 仍是唯一存储契约；快照写入原子（JSON temp→rename / redb 单 txn）。

---

## 5. 验证状态

- [x] `cargo test --workspace`：137 passed（core 53 / app 40 / render 26 / shell 18）
- [x] `cargo clippy --workspace --all-targets -- -D warnings`
- [x] `cargo fmt --all -- --check`
- [ ] Windows 10 / 11 人工验收（拖入/拖出/无效类型/强杀恢复/Quick hide/自启/多屏 DPI）
- [ ] Wallpaper Engine 共存回归

> 本机自动 smoke：应用能加载 redb、启动恢复守卫并干净退出；但当前会话在
> `OverlayWindow::create` 被桌面权限拒绝（0x80070005），未进入 GUI 拖放验证。
> 这是执行环境限制，需在交互式桌面会话执行人工验收。

---

### 2.10 长时间运行稳定性修复

- **误报 WM_MOUSELEAVE**：`SetWindowRgn` 更新区域时可能误发离开消息；若光标实际仍在
  栅栏/图标上，不再清空悬停，避免 HoverLeave→HoverEnter 每帧循环导致动画定时器永不停止。
- **动画看门狗**：任何动画批次超过 2 秒强制清空并停止 `ANIM_TIMER`，避免异常状态
  持续 16ms 全量重绘（Windows 会以应用无响应/0xCFFFFFFF 终止进程）。
- **日志降噪**：合成器每帧 `present` 细节从 `debug` 降到 `trace`，避免长时间运行产生
  数十 MB 日志与 UI 线程 I/O 抖动。
- **桌面 Shell 菜单预热**（默认关闭，`SYLVA_PRIME_SHELL=1` 恢复）：启动期预热会把
  上下文菜单扩展在本进程内加载；这些扩展并非线程安全，后台 STA 与 UI 线程的 Shell
  调用可能互相等待，表现为整窗「未响应」。改为按需预热（右键时 `ensure_primed`
  泵送消息等待），代价只是每种文件类型第一次右键稍慢。
- **卡死定位（`data/logs/hang.log`）**：新增看门狗线程，UI 线程按阶段埋点
  （ENTER/HANDLE/SCENE/PRESENT/HIT/SAVE/SYNC/TRIM/MODAL）。同一阶段停留 > 2.5s 记为
  `STALL ... phase=... event=...`；每秒重绘 ≥ 25 帧记为 `STORM fps=... event=...`
  （自激重绘会拖到无响应）。日志直接同步写文件，进程被 WER 结束也能保留。

  > 2026-11-15 两次 `AppHangB1`（19:44、20:52，退出码 0xCFFFFFFF）发生在加入启动期
  > Shell 预热之后；事件日志显示 `sylva-app 未响应`，加载模块含 `ntshrui`/
  > `thumbcache`/`PhotoMetadataHandler` 等上下文菜单扩展。先关预热复测，若仍复现按
  > `hang.log` 的 `phase` 定位。

---

## 6. 已知边界 / 后续

1. **单 overlay + 命中矩形**：需求 §4.2 原写“每个栅栏 overlay”，本轮按 D2 改为
   “单 overlay 按栅栏命中区域分发”，行为等价、成本更低。
2. **未分组图标区未渲染**：`free_icons` 仍是数据字段；本轮以“桌面收件箱”承接
   未分类桌面图标，移除到 free_icons 的项当前不显示。完整 free area 留待后续。
3. **AI CLI**：需求 §5 的独立二进制仍未实现，依赖本轮撤销/快照/存储契约。
4. **硬杀恢复**：依赖独立守卫进程被调度；若守卫同时被杀，只能下次启动恢复。
5. **滤镜参数**：模糊强度 / 饱和度 / 亮度尚未接入合成器效果链；当前只提供
   玻璃/描边/纯色/模糊风格、色调和填充透明度。
5. **拖出后的路径跟踪**：系统 MOVE 到其他目录后旧路径失效，当前标记 invalid，
   不通过 `SHChangeNotify` 追踪新路径。

---

## 7. 后续修复记录

### 侧边栏滚轮不滚动（2026-11-12）

排查到两个叠加原因：

1. `WM_MOUSEWHEEL` 使用屏幕坐标，而命中模型是客户端/虚拟屏幕坐标；多显示器
   （虚拟原点为负）时 `f.body.contains` 失配。已给 `WindowState` 增加 `origin`，
   新增 `screen_to_client` 并补单测。
2. 侧边栏 `bounds.h` 可能为 0（自动高度未落盘）或内容增长后超过工作区，
   导致 `layout_fence` 的 `scroll_max == 0`。`build_scene` 现按工作区重新钉一次
   侧边栏高度（纵向查 h，横向查 w），保证内容超出时存在可滚动范围。
3. 滚轮步距改为与侧边栏实际布局一致：`icon_size * 1.5 + gap`。

验证：`cargo test --workspace` 137 passed，新增
`sidebar_layout_exposes_scroll_range` 与 `wheel_screen_point_subtracts_virtual_origin`。
仍需在 Windows 10/11 实机滚动侧边栏确认。

### 跨栅栏拖拽失效（2026-11-12）

排查到两个交互状态问题：

1. 侧边栏图标移动超过 8px 后先进入 `SidebarReorder`，之后拖到其它栅栏不再切换为分类；
   现已在 `SidebarReorder` 中检查目标栅栏，命中即转 `Classify`。
2. 离开源栅栏瞬间就启动 OLE，用户还没穿过栅栏间空隙到达目标；现改为 400ms 延迟，
   期间进入目标栅栏优先分类，超时才 OLE；按住 Shift 立即拖出。
3. 释放时若已进入目标栅栏但尚未转 Classify，补发 `IconClassifyDrop`；若在外侧则补发
   `IconDragOut`，覆盖快速拖出。

仍需在交互式 Windows 桌面验证侧边栏 → 网格栅栏的实际拖放。
