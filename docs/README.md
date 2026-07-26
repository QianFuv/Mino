# Mino 文档

这里保存 Mino 的长期维护文档。文档按“理解系统、调用接口、执行专项操作”组织，不再按发布阶段拆分；当前实现范围始终以 CLI、领域状态机和可执行契约测试为准。

## 从哪里开始

根据你的角色选择最短阅读路径：

| 读者 | 推荐顺序 |
|---|---|
| 第一次使用 Mino | 根 [README](../README.md) → [架构](architecture.md) → [CLI 契约](command-contract.md) |
| 编码代理或集成开发者 | [CLI 契约](command-contract.md) → [安全边界](security.md) → [架构](architecture.md) |
| 仓库维护者 | [迁移](migration.md) → [团队标准目录](team-catalog.md) → [插件分发](distribution.md) |
| 故障排查 | [安全边界的恢复步骤](security.md#恢复步骤) → [迁移的冲突与恢复](migration.md#冲突与恢复) |

## 文档职责

| 文档 | 回答的问题 | 不负责的内容 |
|---|---|---|
| [架构](architecture.md) | 状态由谁拥有，数据如何存储、恢复和投影？ | 单个 CLI 参数的完整拼写 |
| [CLI 与 JSON 契约](command-contract.md) | 有哪些命令、schema、状态和退出码？ | 解释用户需求或授予审批 |
| [安全与操作边界](security.md) | 哪些输入不可信，哪些动作必须停止，如何保留现场？ | 充当操作系统 sandbox |
| [协议与旧工作流迁移](migration.md) | 如何核对协议、导入旧计划、升级或回滚？ | 继承旧审批和执行结果 |
| [团队标准目录](team-catalog.md) | 如何构建、托管、同步和恢复组织标准？ | 自动启用远程规则或执行代码 |
| [原生插件分发](distribution.md) | 如何构建、验证、安装和回滚目标平台插件？ | 自动发布或修改用户安装 |

同一事实只保留一个主要说明位置。其他文档可以给出摘要和链接，但不复制整套流程：

- 状态所有权与恢复机制归架构文档；
- 命令和机器输出归 CLI 契约；
- 权限、拒绝条件和产品边界归安全文档；
- 升级与历史数据接入归迁移文档；
- 标准目录与插件产物分别归各自运维指南。

## 文档与项目内部资源

以下 Markdown 不属于对外指南，仍使用英文并由各自契约管理：

- `assets/protocol/**`：内嵌协议来源；
- `assets/skill/**` 与 `plugins/mino/skills/**`：代理 Skill 与引用资料；
- `tests/fixtures/**`：测试输入和确定性快照；
- `docs/plan/**`：Mino 生成或保留的任务计划与历史记录。

这些文件不能为了统一文档语言而改写。协议资源、Skill 和 fixture 的字节可能直接参与摘要或契约校验。

## 验证体系

<!-- doc-contract: verification-strategy -->

对外文档不是孤立说明，而是由分层测试约束：

1. `documentation_contract` 对照递归 `--help` 和 Agent capabilities，检查命令清单、schema、状态、退出码、路径、导航和关键边界。
2. `plugin_contract` 校验 canonical plugin source、Skill bytes、launcher identity、source digest 和可复现 artifact。
3. E2E 测试覆盖项目初始化、计划生命周期、Git/review/amendment、监控、调度说明、标准目录和插件入口。
4. CI 在 Windows、Linux、macOS 上运行格式、Clippy、依赖排序、测试与 Rustdoc，并用 Miri 检查适用库目标。

新增或调整外部能力时，应更新承担该事实的专题文档和对应契约测试，而不是再创建一次性版本总结。

## 维护约定

- 面向读者的说明使用中文；命令、schema、状态值、路径和代码标识保持原样。
- 先重构信息，再修改措辞；避免逐段翻译内部设计材料。
- 示例必须可复制，并明确区分只读、项目状态写入、进程、Git、网络和外部系统副作用。
- 不根据尚未实现的设计推测行为；以当前代码和测试为准。
- 删除或移动文档时同步更新根 README、本文导航和 `documentation_contract`。
