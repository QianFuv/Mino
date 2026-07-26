# 协议与旧工作流迁移

Mino 把“升级运行协议”“分析旧工作流文件”和“导入旧计划”拆成三条互不替代的路径。先根据目标选择路径，再执行对应命令：

| 目标 | 使用的命令 | 是否写入 |
|---|---|---|
| 核对当前项目和内嵌协议是否兼容 | `mino protocol status` | 否 |
| 执行已注册的协议状态转换 | `mino protocol migrate` | 仅存在 transform 时修改计划 |
| 了解旧 AGENTS、模板或执行指南如何映射 | `mino project migrate legacy` | 否 |
| 把一份旧 Markdown 计划变成新的 Draft | `mino project import legacy` | 创建独立计划 |

旧文件中的“已批准”“已完成”“检查通过”等声明都不构成 Mino 信任。迁移只保留可验证的 authored intent，所有运行状态必须重新走当前协议。

## 当前协议身份

Mino `0.1.0` 把规划资源作为惰性字节内嵌，当前 manifest 身份如下：

| 字段 | 值 |
|---|---|
| Protocol version | `2026-05-11` |
| Protocol revision | `review-rework-git-flow-v1` |
| Plan schema | `1` |
| Renderer | `2` |
| `PLAN_TEMPLATE.md` SHA-256 | `73b55c3b64acc7e890464e180a4546b37c288984594f979e49dc117b7b634e9f` |
| `PLAN_EXECUTION.md` SHA-256 | `08076f7ecb2892bdb416c00b170af9d53a835bb5a7e7580f768059d318d1976a` |

初始化时，Mino 会验证 manifest 并写入对应的 `.mino/protocol.lock`。以下命令重新验证内嵌资源，再比较 lock format、protocol version/revision、plan schema 和 renderer：

```text
mino protocol status --format json --no-input
```

该命令只读，兼容性问题通过 `missing` 返回。内嵌 Markdown 只说明来源，不是运行时替代品；CLI 不可用时，不要复制模板到仓库来模拟 Mino。真正的约束来自编译后的 validator 与 state transition。

## 协议迁移

协议迁移必须绑定精确计划 revision 和 request UUID：

```text
mino protocol migrate \
  --plan <id> \
  --expect-revision <n> \
  --request-id <uuid> \
  --to <calendar-version> \
  --format json --no-input
```

当前 registry 没有旧版本 transform。已经使用 `2026-05-11/review-rework-git-flow-v1` 的计划会返回确定性的 `already_current`，且不写 revision、event、snapshot 或 projection。其他 source/target 组合返回 exit 5，并保留全部计划字节。

未来版本只有在显式注册并测试 transform 后才能修改旧计划；直接改写 protocol lock 不能构成升级。

## 版本升级与回滚

<!-- doc-contract: upgrade-and-rollback -->

binary、plugin 与项目本地协议是三个相关但独立的版本边界。升级一份 Mino 安装时按以下顺序处理：

1. 准备完整且经过验证的新 binary 或目标平台 plugin artifact。
2. 在项目中运行 `mino protocol status --format json --no-input`，不要假设安装新版本会自动修改 `.mino`。
3. 只有 status 返回已注册迁移时，才审阅并执行对应的精确 `protocol migrate`。
4. 运行 `mino project doctor`，再读取 `mino agent context`，确认 lock、projection、binding 和 active plan 一致。
5. 保留现有 plan、snapshot、event、evidence、review history、Git journal 与 standards cache；新增能力不会自动激活或授权自身。
6. 如果同时更换 Codex plugin，替换完整 target bundle，并重新运行 launcher 声明的 compatibility probes，不能混合新旧 Skill、launcher 和 binary。

回滚同样以完整 prior binary 或 plugin artifact 为单位。使用旧版本前先确认它仍支持项目当前 protocol lock；如果不兼容，应停止并恢复匹配的完整 artifact，而不是手工降级 `.mino` 文件。失败的 standards sync 会保留旧 cache，失败的 plugin build 不发布 target directory，Blocked 计划则通过记录的状态恢复。

## 旧工作流分析

当仓库存在旧版 `AGENTS.md`、`PLAN_TEMPLATE.md` 或 `PLAN_EXECUTION.md` 时，先运行只读分析：

```text
mino project migrate legacy \
  --agents AGENTS.md \
  --template PLAN_TEMPLATE.md \
  --execution PLAN_EXECUTION.md \
  --format json --no-input
```

三个输入至少提供一个。每个文件必须是非空 UTF-8，大小不超过 1 MiB。报告会保留足够信息供人工判断：

- 精确路径、byte count 与 SHA-256；
- 按源顺序排列的 Markdown headings；
- 每部分的 `mapped`、`ambiguous` 或 `unsupported` disposition；
- duplicate、missing、ambiguous 和 unsupported findings；
- AGENTS 受管区块 diff，或模板/执行文档的惰性迁移建议；
- 固定的 `applied: false` 和空 `deleted_sources`。

分析不会编辑、重命名或删除任何旧文件。

## 旧计划导入

导入适用于一份历史 managed Markdown 计划，并要求当前项目没有活动计划：

```text
mino project import legacy \
  --source legacy-plan.md \
  --name imported-change \
  --request-id 00000000-0000-0000-0000-000000000001 \
  --actor user \
  --format json --no-input
```

### 输入限制

源文件必须是普通文件、非空、UTF-8、无 NUL，最大 1 MiB。parser 识别 code fence，并支持有限的 front matter 与以下 authored 结构：

- Metadata、Summary、Context、Scope、Decisions；
- Approach、File Map、Interfaces、Edge Cases；
- 连续的 `T1..Tn` 任务；
- Git Flow declarations；
- Verification Plan。

报告为每个 mapping 给出源行、fragment、path、byte count 和 SHA-256，并返回稳定 warnings、目标 plan ID 与 revision。

### 保留与舍弃规则

只有能进入严格 `DraftPlanInput` 的定义会被导入。以下内容会被舍弃并产生 warning：

- duplicate、partial、placeholder 或 unknown 内容；
- 不连续或重复的任务 ID；
- absolute、traversal、backslash、`.mino/**` 或 `docs/plan/**` 路径；
- shell control syntax；
- 已知 shell 或 destructive executable 形式的检查。

历史 lifecycle、task/check/criterion status、commit result、review、approval 与 evidence 全部忽略。勾选过的 criterion 和 completed row 只会成为 Pending 定义，不携带证据。

导入通过普通 create/apply API 生成 revision 2 Draft，始终返回 `complete: false`。它不会 finalize、approve、execute 或 commit，也不会改动源文件。必须人工检查所有 mapping、warning、path、command、criterion 和 commit declaration，再走正常校验与批准流程。

相同 source bytes、name、actor 与 request UUID 的精确重试会重放两阶段结果。复用 UUID 但改变源摘要会被识别为 drift。

## 旧概念如何归属

| 旧工作流中的关注点 | 当前所有者 |
|---|---|
| durable plan trigger | 稳定 AGENTS 区块与 Skill description |
| 模板字段 | versioned `Plan` schema 与 deterministic renderer |
| 状态和执行顺序 | domain state machine 与 execution services |
| Ready 条件 | `plan validate` 与 `plan finalize` |
| 计划审阅与批准 | `plan review` 与 `plan approve` |
| 检查结果 | check-run journal 与 immutable evidence store |
| checkpoint、block、resume | 对应的 `exec` semantic commands |
| Git readiness 和 commit declaration | plan fields、File Map 与 Git policy |
| Common/语言规则 | embedded standards 与 resolved checks |
| 固定外部模板 | embedded protocol bundle 与 protocol lock |
| 仓库专用工具路由 | 用户所有的 AGENTS 内容 |
| 发布、部署、通知 | 当前计划之外的手工或外部系统 |

## 推荐接入顺序

1. 按仓库原有策略备份或提交现有指令文件。
2. 运行 legacy analysis，逐条处理 ambiguous 和 unsupported finding。
3. 不带 apply flags 运行 `project init`，先审阅 Skill 与受管区块 proposal。
4. 接受 proposal 后，再显式应用 AGENTS 和 `.gitignore` 区块。
5. 运行 `project doctor` 与 `protocol status`，直到没有 blocking finding。
6. 创建新计划，或导入一份受支持旧计划。
7. 人工复核导入 Draft 后，再 validate、finalize 和 approve。
8. 只有在独立审阅后才删除或简化旧文件；Mino 不执行这一步。

## 冲突与恢复

- 缺少 `<!-- mino-managed-skill:v1 -->` 的 Skill 属于用户，Mino 报告 `mino_skill_conflict` 并保留完整目录。
- marker-owned Skill 可以逐文件更新，目录内未知仓库文件会保留。
- 缺失区块只产生 proposal；valid-but-stale 区块可更新。duplicate、reversed、partial、non-UTF-8、symlink 或 non-file marker target 都会被拒绝。
- protocol migration、legacy analysis 和 import parse/digest 错误不会写计划状态。
- 导入若在 revision 1 创建后中断，可使用完全相同的导入请求补完 authored batch。
- 常规计划加载和 `project doctor` 会恢复 prepared transaction。不要手工删除 `.mino/**` 中的 transaction、snapshot 或 history 文件。
