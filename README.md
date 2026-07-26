# Mino

Mino 是一个面向编码代理的本地计划协议引擎。它把实施计划从一份容易漂移的文档，变成带版本、状态转换、审批边界和执行证据的可恢复工作流。

Mino 不包含大语言模型，也不负责理解用户意图。代理负责判断“应该做什么”，Mino 负责确定“当前允许做什么、做过什么，以及结果是否足以进入下一阶段”。

当前 crate 与插件版本为 `0.1.0`。已实现范围由当前 CLI、领域状态机和可执行契约共同定义，不再维护独立的版本概览文档。

## 为什么需要 Mino

当计划只存在于聊天记录或手写 Markdown 中时，常见问题包括：执行状态无法可靠恢复、旧审批被误用于新版本、检查结果缺少来源、多个工作树选错计划，以及代理绕过约束直接修改状态。

Mino 用一组确定性机制解决这些问题：

- 计划、任务、检查和验收条件都有明确状态，状态只能通过语义命令转换。
- 每次计划修改都绑定当前 revision 和幂等 request ID，过期写入会被拒绝。
- `.mino/` 保存规范 JSON、不可变快照、追加式事件、运行记录和证据。
- `docs/plan/*.md` 由规范状态确定性生成，只用于人工审阅。
- 检查命令以 argv 直接启动，不经过 shell，并受时间、输出和环境边界限制。
- 计划批准、重大修订、最终验收、分支创建等操作拥有彼此独立的审批边界。

## 工作模型

```mermaid
flowchart LR
    U["用户提出目标"] --> A["编码代理解释意图"]
    A --> C["Mino 返回合法操作"]
    C --> P["版本化计划状态"]
    P --> X["受限执行与证据"]
    X --> R["审阅与验收"]
    R --> D["完成或进入返工"]
```

一份计划通常经历以下阶段：

1. 在 `Draft` 中补齐目标、范围、任务、文件责任、验收条件和检查。
2. 校验后进入 `Ready`，生成与当前 revision 和状态哈希绑定的审阅材料。
3. 用户明确批准计划，并选择是否启用计划内声明的 Git Flow。
4. 按依赖顺序执行任务，保存检查结果和验收证据。
5. 全部执行门槛通过后进入 `Review`；审阅反馈可触发证据补充、范围内返工或受保护修订。
6. 用户单独批准最终验收后进入 `Done`。

## 能力概览

### 项目与计划

- 发现、初始化和诊断项目本地状态。
- 通过逐项命令、严格 YAML 或有限交互向导编写计划。
- 校验 schema、语义规则、任务依赖图和策略边界。
- 从保留的历史 revision 分叉独立方案，进行只读语义比较，并在明确选择后归档未采用方案。
- 以保守方式分析旧工作流文件，或把受支持的旧计划导入为新的 `Draft`。

### 执行与证据

- 严格按照任务依赖和单一活动槽执行。
- 运行计划中预先声明的检查，保存不可变命令证据。
- 使用有限次数、间隔和总时限在前台监控一项检查。
- 输出与具体计划 revision 绑定的调度器中立任务说明，但不创建外部定时任务。
- 保存文件、Git diff、提交、URL、日志、截图、人工观察和批准例外等证据。

### Git、审阅与修订

- 读取工作树、HEAD、索引和 porcelain v2 状态，并显式绑定当前计划。
- 在单独批准后创建确定名称的本地分支。
- 仅对已声明 File Map 与 Commit Scope 内的精确路径创建任务提交。
- 记录审阅反馈，区分验收缺陷、范围内返工、重大变更和后续事项。
- 使用类型化 `Minor` 或 `Material` 修订更新已就绪或执行中的计划。
- 可选安装只提供建议的 pre/post commit hooks；hooks 不修改 Git 或计划状态。

### 标准与分发

- 根据项目语言选择内嵌标准，生成计划检查，并显式解决来源冲突。
- 构建可静态托管、摘要校验的团队标准目录。
- 为五个原生目标组装可复现的 Codex 插件 ZIP，并在隔离目录中执行兼容性探测。
- 仓库只负责验证产物，不会自动上传、发布、安装或注册插件。

## 安装

Mino 使用 Rust 2024 edition，要求 Rust `1.96.1`。非 Git 项目可以使用核心计划功能；Git 检查、绑定、分支和提交工作流需要本地 Git。

```text
cargo build --release --locked
cargo install --path . --locked
mino --version
```

Mino 默认离线。生产 CLI 唯一的网络入口是用户显式执行 `standards sync --all`。

## 快速开始

### 1. 初始化项目

```text
mino project init --format json --no-input
```

首次初始化会创建 `.mino` 状态并安装仓库 Skill，但只提出 `AGENTS.md` 和 `.gitignore` 的受管区块变更。审阅返回的 `next_actions` 后，可以显式应用：

```text
mino project init --apply-agents-block --apply-gitignore-block --format json --no-input
mino project doctor --format json --no-input
```

### 2. 创建计划

把原始需求保存到文件，然后创建新的 Draft：

```text
mino plan create \
  --name example-change \
  --trigger durable \
  --request-file request.md \
  --request-id 00000000-0000-0000-0000-000000000001 \
  --actor user \
  --format json --no-input
```

### 3. 驱动代理循环

```text
mino agent context --format json --no-input
```

只执行返回的 `next_actions[].argv`，每次成功修改后重新读取 context；当 `approval_required` 为 `true` 时停止并等待用户决定。不要手工修改 `.mino/**` 或 Mino 管理的 `docs/plan/*.md`。

如果需要接入旧计划，请先阅读[迁移指南](docs/migration.md)，不要直接把旧状态或审批结果复制进新计划。

## 明确的产品边界

Mino 刻意不提供以下能力：

- 不执行 LLM 推理，也不运行自治代理循环。
- 不提供 daemon、后台 worker、无限轮询或内置调度器。
- 不提供云端控制面、遥测、账户系统或 Web UI。
- 不自动更新、搜索或安装插件和标准包。
- 不执行 shell 字符串形式的计划检查。
- 不执行 Git push、merge、rebase、reset、amend、force-push、标签或分支删除。
- 不推断审批，不静默合并冲突，也不允许任意设置状态。

更完整的威胁模型和操作限制见[安全边界](docs/security.md)。

## 文档

完整的阅读路线、专题职责和维护约定见 [Mino 文档入口](docs/README.md)。常用参考包括[架构与状态所有权](docs/architecture.md)、[CLI 与 JSON 契约](docs/command-contract.md)和[安全与操作边界](docs/security.md)。

## 开发与验证

仓库 CI 在 Windows、Linux 和 macOS 上执行格式化、Clippy、依赖排序、离线安装、端到端测试、完整测试和 Rustdoc；另有 Miri 作业覆盖适用的库目标。

```text
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo sort --check
cargo test --locked --all-targets --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --locked --all-features --no-deps
```

原生插件产物使用单独工作流在五个目标上构建和冒烟验证，但仓库不会自动上传或发布这些产物。
