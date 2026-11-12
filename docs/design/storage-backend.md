# 存储后端抽象

> **状态**：已实现（JSON 后端）；redb 后端待接入。
> **关联**：[AGENTS.md](../../AGENTS.md) §2.2/§3 · [需求文档](../requirements/desktop-collection.md) §3 · [待开发清单](../roadmap/backlog.md)

---

## 1. 目标

1. **读写与存储格式解耦**：上层只依赖 `DeskStore`，不关心是 JSON 文件还是 redb 数据库。
2. **可替换**：新增后端不改业务代码，只加一个实现并在 `store::open` 注册。
3. **可迁移/可导出**：任意两个后端之间能整库复制快照，用于 JSON → redb 迁移、redb → JSON 备份导出。
4. **原子性**：崩溃不能产生半写配置。
5. **core 无文件 I/O**：契约在 `core`，实现留在 `app`（符合 AGENTS 分层纪律）。

---

## 2. 分层与接口

`@text
core::storage
├── StoreKind        json | redb（诊断/选择用）
├── DeskStore        load / save / delete / exists / kind / location
├── copy_all         两个后端之间复制快照（迁移/导出入口）
├── normalize        版本迁移 + 校验（所有后端加载后必须调用）
└── MemoryStore      内存实现（测试用，不接触文件系统）

app::store
├── JsonStore        <data_dir>/desk.json，write temp → rename
└── open(data_dir)   后端选择（当前默认 JSON；redb 就绪后在此分发）
`@

`@rust
pub trait DeskStore {
    fn kind(&self) -> StoreKind;
    fn location(&self) -> String;
    fn exists(&self) -> bool;
    fn load(&self) -> Result<Option<Desk>, CoreError>;  // None = 尚无数据
    fn save(&self, desk: &Desk) -> Result<(), CoreError>;
    fn delete(&self) -> Result<(), CoreError>;
}
`@

约定：

- `load` 返回的 `Desk` **必须已归一化**（`normalize` 统一做版本迁移 + 校验），后端不得跳过。
- `load` 返回 `None` 表示"此后端尚无数据"，由调用方决定默认值（首次运行）。
- `save` 必须原子；`delete` 用于"重置所有设置"。

---

## 3. 新增一个后端要做什么

以 redb 为例：

1. 在 `crates/app/src/store.rs`（或 `store/redb.rs`）新增 `RedbStore`，实现 `DeskStore`：
   - `load`：开启读事务，读 meta/version + 各表，组装 `Desk`，调用 `normalize`；
   - `save`：**单个 write transaction** 写入所有表（见需求文档 §3.3）；
   - `delete`：删除数据库文件或清空表。
2. 在 `store::open` 中按 `SYLVA_STORE` 或"文件是否存在"分发。
3. 补测试：roundtrip、空库、版本迁移、与 JSON 的 `copy_all`。
4. 更新本文件与 AGENTS 的依赖约束（引入 `redb` 需要显式确认）。

**redb 草案**

| 项 | 建议 |
|---|---|
| 文件 | `<data_dir>/sylva.redb`（与 `desk.json` 并存，迁移完成后再决定是否删除旧文件） |
| 表 | `meta`（version/schema）、`fences`、`icons`、`todos`、`plugins`、`settings` |
| 键 | 领域 id（如 fence id、图标路径）序列化为字符串 |
| 事务 | 一次 `save` = 一个 write transaction |
| 迁移 | `normalize` 不变；redb 的 schema 版本与 `Desk.version` 分开记录 |

---

## 4. 迁移与导出

`@text
JSON → redb :  JsonStore.load() ──copy_all──▶ RedbStore.save()
redb → JSON :  RedbStore.load() ──copy_all──▶ JsonStore.save()
备份导出    :  当前后端.load()  ──copy_all──▶ JsonStore(导出目录).save()
`@

流程要求：

1. 迁移前对旧文件/旧库做一次 `.bak` 备份；
2. 写入新后端后**回读校验**（`load` 后比较关键字段），校验失败则回滚到旧后端；
3. 迁移只在启动时做一次，失败不阻断启动（降级到旧后端并记录日志）；
4. 导出文件采用 `write temp → rename`，导出目录由用户选择。

> `copy_all` 已在 `core::storage` 实现并有测试覆盖（内存 → 内存、空源清空目标）。

---

## 5. 原子性

| 后端 | 机制 |
|---|---|
| JSON | 写 `desk.json.tmp` → `rename` 覆盖 |
| redb | 单个 write transaction，提交即原子 |
| 内存 | `RefCell` 整体替换（测试用） |

---

## 6. 测试现状

- `core::storage`：内存 roundtrip、`copy_all` 迁移、空源清空目标。
- `app::store`：缺失文件返回 `None`、保存/加载 roundtrip、临时文件不残留、删除、JSON → JSON 导出。
- 全仓库：99 个单测通过，`clippy -D warnings` 与 `fmt --check` 通过。

---

## 7. 与需求文档的关系

- 需求文档 §3 描述的是**目标状态**（redb + AppData + 事务 + 快照）；本抽象是达成该目标的前置改造。
- 迁移到 redb 时，`DeskStore` 契约与上层调用保持不变，只替换后端实现。
- 存储位置（AppData vs exe 同级 `data/`）仍是待确认项，见需求文档附录 A。
