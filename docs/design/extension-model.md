# 扩展模型（插件 / 扩展包）方案

> **状态**：设计草案（未实现）
> **要回答的问题**：P2 剩余能力能否做成"扩展包"？核心代码是否需要引入扩展框架？
> **关联**：[待开发清单](../roadmap/backlog.md) · [需求文档](../requirements/desktop-collection.md) · [AGENTS.md](../../AGENTS.md)

---

## 1. 结论

**需要，但不是"动态插件系统"。**

建议给核心只加**两个稳定接缝**，其余能力都用数据包或进程外工具承载：

1. **进程内面板接口**（第一方、随源码编译）：把待办/便签/日历/倒数日这类"控制台面板"从硬编码里解耦。
2. **声明式数据包加载器**（只读数据，无代码执行）：主题、规则预设、语言包。

**明确不做**：DLL/WASM/脚本动态加载。理由见 §3。

这样既能把 P2 剩余能力挡在核心之外，又不破坏"原生、轻量、0% CPU、无 WebView"的定位。

---

## 2. 代码现状

| 现状 | 位置 | 说明 |
|---|---|---|
| `PluginEntry` 数据结构 | `crates/core/src/model.rs` | `id/name/kind/enabled/version/desc/note_text`，已随 `desk.json` 持久化 |
| `PluginKind` | 同上 | `Todo / Notes / External` 三种 |
| 默认插件表 | 同上 | 待办启用、便签预留 |
| 运行时接口 | **不存在** | 全仓库除 model 外无 `plugins` 引用 |

也就是说：**"插件"目前只是配置里的一个列表**，没有 trait、没有注册表、没有渲染接入。这是好事——接缝还没被写死，现在补成本最低。

---

## 3. 为什么不做动态插件（DLL / WASM / 脚本）

| 方案 | 否决理由 |
|---|---|
| DLL 动态加载（`libloading`） | Rust 无稳定 ABI，工具链/编译器版本一变就崩；插件崩溃会带走主进程；安全审计成本高 |
| WASM 沙箱 | 需要运行时（wasmtime 等），包体与内存开销与"个位数 MB"纪律冲突 |
| 脚本引擎（Lua/JS） | 引入解释器与 GC，违背"无 WebView / 无 Node"的硬约束 |

结论：**扩展能力靠"数据 + 进程边界"实现，不靠"代码 + 动态链接"**。

---

## 4. 四层扩展模型

```text
L4  进程外工具      CLI / 更新器 / AI 代理       ← 独立二进制，JSON 契约
L3  声明式数据包    主题 / 规则预设 / 语言包      ← 只读数据，schema 校验
L2  进程内面板插件  待办 / 便签 / 日历 / 倒数日    ← 第一方代码，随源码编译
L1  编译期特性      Cargo feature               ← 可选模块，零运行时开销
L0  核心            core / shell / render / app
```

### L0 核心（不扩展）

栅栏模型、布局、磁吸、拖放、桌面接管、渲染管线。这些是产品差异化的根，**不进扩展体系**，避免扩展影响主链路稳定性。

### L1 编译期特性

用 Cargo feature 控制是否编译可选面板/可选能力：

```toml
[features]
default = ["panel-todo", "panel-notes"]
panel-calendar = []
panel-countdown = []
update-check = []
```

适用：体积敏感、依赖较重的第一方模块。**不作为主要扩展手段**，只做裁剪。

### L2 进程内面板插件（重点）

**目标**：让"控制台面板"变成可注册的组件，而不是往 `console` 的 `match` 里加分支。

- **接口位置**：`app` 层定义 trait（渲染类型在 `render`，trait 放在组合层，避免 core 反向依赖）；
- **数据模型**：面板内容抽象为纯数据结构（标题、标签页、行、按钮、输入框），放在 `core`，由 `render` 统一绘制；
- **注册方式**：静态注册表（编译期列表），不做运行时发现；
- **权限**：面板**不得**直接调用 Win32/COM（AGENTS §3 规定 shell 是唯一入口），只能读 `core` 提供的只读快照、通过对齐的 `PanelAction` 提交变更；
- **状态**：面板自身状态持久化到 `PluginEntry` 或独立 table。

草案接口：

```rust
// crates/app/src/panel.rs（草案）
pub trait ConsolePanel {
    fn id(&self) -> &'static str;
    fn title(&self) -> &'static str;
    /// 生成当前帧的面板数据（纯数据，render 负责画）
    fn snapshot(&self, ctx: &PanelCtx) -> PanelSnapshot;
    /// 处理一次交互，返回需要落库的领域命令
    fn on_action(&mut self, action: PanelAction, ctx: &mut PanelCtx) -> Vec<CoreCommand>;
}

pub fn registry() -> &'static [&'static dyn ConsolePanel] {
    &[&TODO_PANEL, &NOTES_PANEL, &CALENDAR_PANEL]
}
```

配套数据结构（`core`）：

```rust
pub struct PanelSnapshot {
    pub title: String,
    pub tabs: Vec<PanelTab>,
    pub rows: Vec<PanelRow>,
    pub actions: Vec<PanelButton>,
}
```

**收益**：新增"日历/倒数日"只加一个 panel 文件 + 注册表一行，不动 core/render/console 的 `match`。

### L3 声明式数据包

**目标**：主题、规则预设、语言包这类"纯数据"能力，通过包目录加载，不重新编译。

目录约定：

```text
extensions/
├── themes/
│   └── nord/
│       ├── manifest.json
│       └── theme.json
├── rules/
│   └── dev-files.json
└── locales/
    └── en-US.json
```

清单草案：

```json
{
  "schema": 1,
  "id": "theme.nord",
  "kind": "theme",
  "version": "1.0.0",
  "min_app": "0.2.0",
  "files": { "theme": "theme.json" }
}
```

加载规则（必须固化到 AGENTS）：

- **绝不执行包内代码**，只解析 JSON；
- 路径必须限定在包目录内，拒绝 `..` 与绝对路径；
- 单包/单文件大小上限（如 1 MB / 5 MB），字段白名单校验；
- 未知 `schema` 或版本过高 → 跳过并记录日志，**不得影响启动**；
- 包损坏/缺失 → 回退默认主题/规则，不弹错；
- 规则包必须支持"预览后再应用"（与 P1-01 对齐）。

### L4 进程外工具

| 工具 | 职责 | 契约 |
|---|---|---|
| `sylva-cli` | 配置读写、`--dry-run`、快照回滚 | 需求文档 §5 的 JSON Schema |
| 更新器 | 版本检查与安装包替换 | 独立进程，主程序不联网 |
| AI 代理 | 自然语言 → 规则/布局 | 复用 CLI，不新增接口 |

进程外工具**不链接进主进程**，通过 JSON 文件/本地 IPC 交互；主进程崩溃时仍可读取配置并触发恢复（需求文档 §5 已要求）。

---

## 5. P2 剩余项的去向

| 能力 | 归属 | 理由 |
|---|---|---|
| 日历 / 倒数日 | **L2 面板插件** | 与待办同构，复用面板接口 |
| 主题 / 皮肤包 | **L3 数据包** | 纯配色/材质参数 |
| 多语言 | **L3 数据包** | 语言资源，无需代码 |
| AI 自然语言整理 | **L4 进程外工具** | 已由需求文档的 CLI 承载 |
| 自动更新 | **L4 进程外工具** | 联网与替换安装包都不该进主进程 |
| 拖放动画 | **L0 核心** | 属核心体验，不做扩展 |
| 无障碍 | **L0 核心** | UI Automation 支持是核心能力 |

> 结论：**P2 剩余项里，只有"日历/倒数日"需要新的代码接缝，"主题/多语言"是纯数据，其余是核心或进程外工具。**

---

## 6. 安全与稳定性约束（建议写入 AGENTS）

1. 扩展包**不得包含可执行代码**，加载器不调用 `dlopen`/`LoadLibrary`/`Command`。
2. 扩展包**不得直接访问 Win32/COM**；需要系统能力时走 `shell` 的既有 API。
3. 扩展加载失败必须**降级**，不得阻断启动、不得影响桌面恢复守卫。
4. 面板插件只能通过 `core` 领域命令改状态，不得直接写持久化。
5. 所有扩展输入视为不可信：路径、大小、字段、编码都要校验。
6. 扩展的启用状态与版本必须持久化，升级后不兼容时禁用并提示。

---

## 7. 对核心代码的改动评估

| 层 | 改动 | 规模 |
|---|---|---|
| `core` | 新增 `panel` 数据结构（`PanelSnapshot` 等）；`PluginEntry` 增加 `manifest` 字段（可选） | 小 |
| `app` | 新增 `panel.rs`（trait + 静态注册表）；控制台改为消费 `PanelSnapshot` | 中 |
| `render` | 把控制台绘制泛化为"按 `PanelSnapshot` 绘制"（现有列表/按钮绘制可复用） | 中 |
| `shell` | 无改动 | 无 |
| 资源 | 新增 `extensions/` 目录与加载器（L3 阶段） | 小 |

**不建议现在做**：L1 feature 拆分、动态发现、插件市场。等 L2/L3 稳定后再评估。

---

## 8. 实施顺序

1. **阶段 1（随 P1 一起）**：控制台面板数据化 + 静态注册表，把待办/便签迁移过去，验证"零行为变化"。
2. **阶段 2（P1-01 之后）**：L3 数据包加载器，先接主题，再接规则预设；同步补 schema 与校验。
3. **阶段 3（与 AI CLI 同步）**：L4 进程外契约，CLI 与主程序共用 core 校验。
4. **阶段 4（可选）**：L1 feature 裁剪、日历/倒数日面板。

---

## 9. 决策点

| 问题 | 建议 |
|---|---|
| 要不要现在给核心加扩展框架？ | **只加 L2 的 trait + 静态注册表**，不加动态加载 |
| 扩展包是否允许代码？ | **不允许**，只允许 JSON + 资源 |
| 第三方能否开发面板？ | 短期不开放；L2 仅第一方编译期注册 |
| 是否需要插件市场？ | 不做；主题/语言包以本地目录或官方分发为主 |
| 与 redb 迁移的关系？ | 扩展状态存储走 `core` 存储抽象，迁移时随主数据一起处理 |
