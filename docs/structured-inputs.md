# 严格结构化输入

<!-- doc-contract: structured-input-inventory -->

Mino 共有四类需要调用者预先编写的严格 YAML。它们都采用未知字段拒绝、单文档解析和整文件失败语义；输入文件只描述当前操作允许的 authored 内容，不能写入计划状态、审批、执行结果或 evidence。

## 命令与样例

| 命令 | 顶层结构 | 仓库安装后的样例 | 生命周期前置条件 |
|---|---|---|---|
| `mino plan apply --file` | Draft authored 字段：`metadata`、`summary`、`context`、`scope`、`decisions`、`approach`、`interfaces`、`edge_cases`、`tasks`、`verification_plan` | `.agents/skills/mino/references/examples/draft-plan.yaml` | 目标计划必须是 Draft，且 `--expect-revision` 必须匹配当前 revision。 |
| `mino git cleanup propose --file` | `items`，每项包含一个逻辑变化、互不重叠的精确文件集和单行 Conventional Commit message | `.agents/skills/mino/references/examples/git-cleanup-proposal.yaml` | Draft 或 Ready 计划必须保存当前 dirty Git readiness；该命令只记录提案，不执行 Git mutation。 |
| `mino plan amend propose --patch-file` | 非空 `operations`，每项由 `operation` tag 选择一个 typed amendment | `.agents/skills/mino/references/examples/amendment-patch.yaml` | 计划已离开 Draft，且当前状态允许 protected amendment；Material 分类仍需单独明确审批。 |
| `mino review rework --file` | 单个完整 `DraftTaskInput`，使用下一个保留的 `R` task ID | `.agents/skills/mino/references/examples/review-rework-task.yaml` | 计划处于 Review，目标 review item 已分类为允许 in-scope rework，并且依赖、criterion、check 与 commit gate ID 对当前计划有效。 |

## 使用方式

1. 先运行 `mino project doctor --format json --no-input`，再读取 `mino agent context --format json --no-input`。
2. 从当前仓库的 `.agents/skills/mino/references/examples/` 复制对应文件到调用者拥有的工作路径。不要原地编辑 Mino-managed Skill 文件。
3. 把样例中的 task、criterion、check、review、dependency 和文件路径替换为当前计划的精确值。cleanup 文件集还必须完整覆盖当前 dirty paths，并且不同 item 之间不能重叠。
4. 使用当前 context 返回的 plan ID、revision、request ID/actor 形状调用命令。命令会读取 UTF-8 文件、计算 SHA-256，并把 digest 写入规范化 replay argv。
5. 成功 mutation 后丢弃旧 revision 并重新读取 Agent context。只有完全相同的命令、request ID 和输入 bytes 才能作为幂等重试；修改文件后必须使用新的 request ID 和当前 revision。

解析或领域完整性失败时，命令以 `incomplete_or_validation` 返回且不提交 revision。调用者提供的 digest 与当前 bytes 不一致时，以 `revision_conflict` 返回。样例本身没有授权作用：它不批准 cleanup、Material amendment、review disposition、Git commit 或任何执行动作。

## 每类输入的边界

Draft 样例展示一个可执行 authored 结构，包括 metadata、context、完整 scope、decision、approach、interfaces、edge case、task File Map、criterion、task/global verification 和 commit gate。它不包含 `status`、`revision`、approval、evidence 或 execution state。

Cleanup 样例中的顺序决定稳定的 `C1`、`C2` 编号。每项必须代表单一责任，文件路径在项内规范排序且不能跨项重复，planned message 必须是单行 Conventional Commit。记录提案不会 stage、commit、reset 或清理工作树；后续每个 item 仍需用户逐项批准，并由外部 Git 操作创建后再通过 Mino 记录。

Amendment 样例使用现有 task/check ID 替换一个 verification 定义。其他 operation 也必须符合 typed allowlist；Mino 会从完整 operation 集计算最低 Minor/Material 分类，调用者只能提高而不能降低分类。

Review rework 样例是一个完整的新任务，不是计划片段。ID 必须是当前 review 流程要求的下一个 `R` 编号；依赖必须引用已有任务，criterion 和 check ID 必须与该 R task 匹配，File Map、verification 和 commit gate 都不能为空。

## 审计后的排除项

这四项是生产代码中全部严格 YAML 反序列化入口。以下文件不是遗漏的同类样例：standards catalog 的 TOML 由初始化流程生成；legacy Markdown 有独立导入与迁移契约；original request、evidence 内容和 cancel marker 是自由文本或普通文件，不使用预声明 YAML schema。测试目录中的 fixture 只服务回归，不是用户或 Agent 的产品样例来源。
