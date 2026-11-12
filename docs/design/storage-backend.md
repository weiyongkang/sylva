# 存储后端抽象

> **状态**：已实现（JSON + redb）。默认后端 **redb**，数据位于 **exe 同级 `data/`**。
> **关联**：[AGENTS.md](../../AGENTS.md) §2.2/§3 · [需求文档](../requirements/desktop-collection.md) §3 · [待开发清单](../roadmap/backlog.md)

---

## 1. 目标

1. **读写与存储格式解耦**：上层只依赖 `DeskStore`，不关心是 JSON 文件还是 redb 数据库。
2. **可替换**：新增后端不改业务代码，只加实现并在 `store::open` 注册。
3. **可迁移/可导出**：任意两个后端之间能整库复制快照（`copy_all`）。
4. **原子性**：崩溃不能产生半写配置。
5. **core 无文件 I/O**：契约在 `core`，实现留在 `app`。

---

## 2. 分层与接口

`@text
core::storage
├── StoreKind        json | redb
├── DeskStore        load / save / delete / exists / kind / location
├── copy_all         两个后端之间复制快照（迁移/导出入口）
├── normalize        版本迁移 + 校验（所有后端加载后必须调用）
└── MemoryStore      内存实现（测试用）

app::store
├── JsonStore        <data_dir>/desk.json，write temp → rename
├── RedbStore        <data_dir>/sylva.redb，单表多键 + 单 write transaction
└── open(data_dir)   后端选择 + 一次性 JSON 迁移
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
- `save` 必须原子；`delete` 用于"重置所有设置"（redb 实现为清空所有键，不删文件）。

---

## 3. 默认选择与迁移

`store::open` 的顺序：

1. `SYLVA_STORE=json` → 强制 JSON（排查/回退）；
2. 否则打开 `<data_dir>/sylva.redb`：
   - 若 redb 为空且存在 `desk.json` → 迁移一次；
   - 迁移前备份 `desk.json.bak`（已存在不覆盖）；
   - 写入后**回读比对**，不一致只告警、不删除 `desk.json`；
3. redb 打不开 → 回退 JSON 并记录错误，保证应用仍可启动。

迁移只在 redb 为空时触发一次；之后 `desk.json` 不再参与读写，可保留作备份或手动删除。

---

## 4. redb 结构

| 项 | 实现 |
|---|---|
| 文件 | `<data_dir>/sylva.redb` |
| 表 | 单表 `desk`：`TableDefinition<&str, &[u8]>` |
| 键 | `schema` / `version` / `settings` / `fences` / `free_icons` / `icons` / `todos` / `next_todo_id` / `console_open` / `console_pos` / `console_size` / `plugins` / `desktop_mode` |
| 值 | 对应字段的 JSON 字节（`serde_json::to_vec`） |
| schema | 后端结构版本 `SCHEMA: u32 = 1`，与 `Desk.version` 独立 |
| 写入 | 一次 `save` = 一个 write transaction，写全部键 |
| 读取 | 一个 read transaction 读全部键，反序列化后 `normalize` |

键按字段拆分（而不是整库一个 blob）的好处：后续可做局部读写、按字段迁移；代价是每次 `save` 全量写，当前数据量（百级图标）下可忽略。

---

## 5. 迁移与导出

`@text
JSON → redb :  JsonStore.load() ──copy_all──▶ RedbStore.save()      （启动时自动）
redb → JSON :  RedbStore.load() ──copy_all──▶ JsonStore.save()      （备份导出）
备份导出    :  当前后端.load()  ──copy_all──▶ JsonStore(导出目录).save()
`@

`copy_all` 在 `core::storage` 实现，只依赖 `DeskStore`，不关心两端格式；源为空时清空目标。

---

## 6. 原子性

| 后端 | 机制 |
|---|---|
| JSON | 写 `desk.json.tmp` → `rename` 覆盖 |
| redb | 单个 write transaction，提交即原子 |
| 内存 | `RefCell` 整体替换（测试用） |

---

## 7. 测试现状

- `core::storage`：内存 roundtrip、`copy_all` 迁移、空源清空目标。
- `app::store::json`：缺失文件、roundtrip、临时文件不残留、删除、导出。
- `app::store::redb_store`：空库、roundtrip、delete、JSON → redb 导入。
- `app::store`：迁移导入 + 备份、无文件时 no-op。
- 全仓库 **105 个单测**通过；`clippy -D warnings`、`fmt --check` 通过。
- 实机验证：全新目录 redb 冷启动；旧 `desk.json` 自动迁移并备份；二次启动不重复迁移；`SYLVA_STORE=json` 回退成功。

---

## 8. 决策记录

| 日期 | 决策 | 说明 |
|---|---|---|
| 2026-11-12 | 存储位置定为 **exe 同级 `data/`** | 与安装器卸载约定一致，不再迁移到 AppData |
| 2026-11-12 | 默认后端 **redb**，JSON 保留为回退/导入 | `SYLVA_STORE=json` 显式切换 |
| 2026-11-12 | redb 采用"单表多键 + JSON 值" | 先保证事务与可迁移，按字段拆分留待后续 |
